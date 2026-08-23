//! Keychain-backed secret storage. One service namespace per project.
//!
//! **Which mechanism is in use depends on how this build is signed**, and
//! `signing::KeychainStrategy` is where that is decided. A build with a Team
//! Identifier can use a shared access group: real ACLs, one item the app and
//! the daemon both read, no prompts. Without one there is no group to share —
//! the entitlement is prefixed by the team — so an unsigned build falls back
//! to the open-ACL workaround below and `preceipts trust status` says so out
//! loud rather than leaving it to be discovered.
//!
//! Each (service, account) pair maps to (`preceipts:<canonical-project-path>`,
//! KEY) and stores the UTF-8 secret value. macOS-only for now; Linux returns
//! "unsupported" until libsecret integration lands.

use anyhow::{anyhow, Result};
use std::path::Path;

/// Compute the per-repo service name used as the Keychain namespace.
/// Keychain service name for a repository's secrets.
///
/// Scoped to the **project**, not the worktree. Every workspace of a project
/// is a different directory, so keying on the working directory would mean a
/// new worktree cannot see the secrets you already stored — you would re-enter
/// your Stripe key per branch. Secrets belong to the repository; workspaces
/// borrow them.
///
/// The namespace secrets are stored under.
pub const PREFIX: &str = "preceipts:";

/// What procpane stored them under. Only `migrate` looks here — see
/// [`legacy_service_name`].
pub const LEGACY_PREFIX: &str = "procpane:";

pub fn service_name(repo_root: &Path) -> String {
    format!("{PREFIX}{}", project_path(repo_root))
}

/// The same project's old namespace.
///
/// Secrets are the one thing in this dissolution that is *data* rather than
/// configuration: a renamed constant orphans values a person typed in and
/// cannot get back from anywhere else. So the old name survives here, used by
/// exactly one thing — `preceipts migrate`, which copies items across and
/// deletes the originals. After that runs, nothing reads it.
pub fn legacy_service_name(repo_root: &Path) -> String {
    format!("{LEGACY_PREFIX}{}", project_path(repo_root))
}

fn project_path(repo_root: &Path) -> String {
    let root = preceipts_core::workspace::locate(repo_root)
        .map(|workspace| workspace.project_root)
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let canon = root.canonicalize().unwrap_or(root);
    canon.display().to_string()
}

/// Move every secret from the old namespace into the new one.
///
/// Copy-then-delete, in that order and per item: a crash between the two
/// leaves a duplicate, which is recoverable, rather than a hole, which is not.
/// An item already present in the new namespace is left alone — re-running the
/// migration must not overwrite a value someone has since changed.
pub fn migrate_namespace(repo_root: &Path, keychain: Option<&str>) -> Result<Vec<String>> {
    let legacy = legacy_service_name(repo_root);
    let current = service_name(repo_root);
    let mut moved = Vec::new();
    for account in list_accounts(&legacy, keychain)? {
        let Some(value) = get(&legacy, &account, keychain)? else {
            continue;
        };
        if get(&current, &account, keychain)?.is_none() {
            set(&current, &account, &value, keychain)?;
            moved.push(account.clone());
        }
        delete(&legacy, &account, keychain)?;
    }
    Ok(moved)
}

#[cfg(test)]
mod scope_tests {
    use super::service_name;
    use std::process::Command;

    fn sh(dir: &std::path::Path, args: &[&str]) {
        assert!(Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
    }

    /// The bug the workspace dimension exposes: a worktree is its own
    /// directory, so a path-keyed secret scope would hide the project's
    /// secrets from every branch.
    #[test]
    fn every_workspace_of_a_project_shares_one_secret_scope() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path().join("proj");
        std::fs::create_dir_all(&project).unwrap();
        sh(&project, &["git", "init", "-q", "-b", "main"]);
        sh(&project, &["git", "config", "user.name", "t"]);
        sh(&project, &["git", "config", "user.email", "t@t.local"]);
        sh(&project, &["git", "config", "commit.gpgsign", "false"]);
        std::fs::write(project.join("a.txt"), "one\n").unwrap();
        sh(&project, &["git", "add", "-A"]);
        sh(&project, &["git", "commit", "-qm", "base"]);

        let workspace = preceipts_core::workspace::create(&project, "feature", None, None).unwrap();

        assert_eq!(
            service_name(&project),
            service_name(&workspace.path),
            "a workspace must see the project's secrets"
        );
    }

    /// Two unrelated projects must not share a scope.
    #[test]
    fn different_projects_keep_different_scopes() {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert_ne!(service_name(&a), service_name(&b));
    }
}

#[cfg(target_os = "macos")]
pub use mac::*;

#[cfg(not(target_os = "macos"))]
pub use unsupported::*;

#[cfg(target_os = "macos")]
mod mac {
    //! Writes go out through `/usr/bin/security`; reads happen in-process.
    //!
    //! The split is the whole design, because a keychain ACL is evaluated
    //! against the *calling* binary. Shelling out to read would put
    //! `/usr/bin/security` — a binary every process on the machine can run —
    //! on the wrong side of that check, and any ACL we wrote would be
    //! decoration. Reading through `SecKeychainFindGenericPassword` in this
    //! process is what makes "only our code may read this" a claim the
    //! system enforces rather than one we make.
    //!
    //! Writing is different: `-T` tells `security` which applications to
    //! trust, and that is independent of who does the writing. The CLI is
    //! kept there because it is the only ACL-setting interface that needs no
    //! raw Security.framework FFI.
    //!
    //! Which ACL gets written depends on how this build is signed —
    //! `signing::KeychainStrategy` decides, per binary, at runtime.
    use super::*;
    use security_framework::os::macos::keychain::{KeychainUserInteractionLock, SecKeychain};
    use security_framework::os::macos::passwords::find_generic_password;
    use security_framework::passwords::set_generic_password;
    use std::collections::BTreeSet;
    use std::process::{Command, Stdio};

    // Magic account name for the per-service index. Listed accounts are
    // tracked here so we don't have to walk every keychain item. The index
    // is hidden from `list_accounts`.
    const INDEX_ACCOUNT: &str = "__preceipts_index__";

    /// procpane's name for the same index. Read when the current one is
    /// absent, so `migrate` can enumerate what the old tool stored — and so a
    /// namespace written by procpane is not simply invisible.
    const LEGACY_INDEX_ACCOUNT: &str = "__procpane_index__";

    /// Keychain database path(s) appended as trailing positional args to
    /// `/usr/bin/security` (add/find/delete all take `[keychain...]` at the
    /// end of argv). Empty when no specific keychain is targeted, so the
    /// default search list is used.
    fn kc_args(keychain: Option<&str>) -> Vec<&str> {
        keychain.map(|k| vec![k]).unwrap_or_default()
    }

    pub fn set(service: &str, account: &str, value: &str, keychain: Option<&str>) -> Result<()> {
        if account == INDEX_ACCOUNT {
            return Err(anyhow!("reserved key name"));
        }
        write_item(service, account, value, keychain)?;
        index_add(service, account, keychain)
    }

    /// Write a generic password with the strongest ACL this build can back.
    ///
    /// Signed with a Team Identifier, that is a list of trusted applications —
    /// our own binaries, named by the designated requirement `security` reads
    /// off each path. Only they can read the item, and since the requirement
    /// is an identifier plus a team anchor rather than a hash of the bytes, a
    /// rebuild is still the same application and the grant holds.
    ///
    /// Unsigned, there is no stable identity to name. The default ACL binds an
    /// item to the codesign hash of whatever wrote it, so every `cargo build`
    /// would read as a new application and re-prompt on every read. `-A`
    /// writes an empty trusted-app list instead — "no auth required for any
    /// app" — which is a real compromise for dev convenience: anything running
    /// as this user can read the values. `trust status` says so out loud.
    fn write_item(service: &str, account: &str, value: &str, keychain: Option<&str>) -> Result<()> {
        // Idempotent open-ACL write via `/usr/bin/security`.
        //
        // We can't just use `add-generic-password -U` (update if exists),
        // because `-U` updates the *password* but leaves the existing ACL
        // intact — and the original ACL was codesign-bound to whichever
        // procpane build first wrote it. Drop the item and re-add cleanly.

        // Best-effort delete. errSecItemNotFound (-25300) is fine.
        let mut delete_args = vec!["delete-generic-password", "-s", service, "-a", account];
        delete_args.extend(kc_args(keychain));
        let _ = sec_run(&delete_args);

        // Signed: create the item ourselves, with its ACL, in one call.
        //
        // Not because FFI is nicer, but because a login-keychain item's
        // partition list is taken from whichever application created it. An
        // item created by `/usr/bin/security` is partitioned to Apple's own
        // tools, and our binaries can then be refused an item their ACL
        // names — the ACL and the partition are two separate gates and both
        // have to open. Setting the partition afterwards would need the
        // keychain password. Creating the item does not.
        let trusted = match crate::signing::KeychainStrategy::detect() {
            crate::signing::KeychainStrategy::TeamAcl(_) => crate::signing::trusted_binaries(),
            crate::signing::KeychainStrategy::OpenAcl => Vec::new(),
        };
        if !trusted.is_empty() {
            // No fallback to `-A` on failure: a secret that is quietly less
            // protected than the caller believes is worse than one that
            // failed to store.
            return crate::acl::add_generic_password_trusting(
                service, account, value, keychain, &trusted,
            );
        }

        // Unsigned: no stable identity to name, so no ACL worth writing. The
        // password value briefly appears on argv during the subprocess —
        // local-dev secrets, single-user machine, accepted.
        let mut add_args = vec!["add-generic-password", "-A"];
        add_args.extend(["-s", service, "-a", account, "-w", value]);
        add_args.extend(kc_args(keychain));
        let output = sec_run(&add_args)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("security add failed ({stderr}); falling back to default keychain ACL");
            return set_generic_password(service, account, value.as_bytes())
                .map_err(|e| anyhow!("keychain set failed for {account}: {e}"));
        }
        Ok(())
    }

    /// Wrapper that runs `/usr/bin/security <args>` and captures stdio.
    /// Centralizing this lets every keychain op share the same code path
    /// (and same "Always Allow" trust grant) without sprinkling `Command`
    /// builders all over the module.
    fn sec_run(args: &[&str]) -> Result<std::process::Output> {
        Command::new("/usr/bin/security")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| anyhow!("spawn /usr/bin/security: {e}"))
    }

    pub fn get(service: &str, account: &str, keychain: Option<&str>) -> Result<Option<String>> {
        if account == INDEX_ACCOUNT {
            return Err(anyhow!("reserved key name"));
        }
        sec_get(service, account, keychain)
    }

    pub fn delete(service: &str, account: &str, keychain: Option<&str>) -> Result<bool> {
        if account == INDEX_ACCOUNT {
            return Err(anyhow!("reserved key name"));
        }
        let removed = sec_delete(service, account, keychain)?;
        if removed {
            index_remove(service, account, keychain)?;
        }
        Ok(removed)
    }

    pub fn list_accounts(service: &str, keychain: Option<&str>) -> Result<Vec<String>> {
        let mut accts = index_read(service, keychain)?;
        // Filter to entries that still resolve — drift-tolerant.
        accts.retain(|a| matches!(sec_get(service, a, keychain), Ok(Some(_))));
        accts.sort();
        accts.dedup();
        Ok(accts)
    }

    /// Refuse to put a dialog on screen for the rest of this process.
    ///
    /// For the daemon, which reads secrets while launching tasks with nobody
    /// watching. A keychain read the ACL does not allow otherwise raises a GUI
    /// authorization dialog, and a background process waiting on one is a
    /// `preceipts up` that hangs with no visible cause. Suppressed, the same
    /// read fails immediately and the error reaches the log, where a person
    /// can act on it.
    ///
    /// The returned guard restores prompting when dropped, so it is held for
    /// the process's life by binding it in `daemon_inner`. `None` if the
    /// system refuses, which is not worth failing startup over — the
    /// hang it prevents is a bad outcome, not a corrupt one.
    pub fn hush_prompts() -> Option<KeychainUserInteractionLock> {
        SecKeychain::disable_user_interaction().ok()
    }

    /// `errSecItemNotFound` — an absent secret, not a failure.
    const NOT_FOUND: i32 = -25300;

    /// Read a secret in this process, so the ACL is checked against *us*.
    ///
    /// This is the half that cannot be a subprocess. `security
    /// find-generic-password` would present `/usr/bin/security` as the reader,
    /// and an ACL that trusts a binary every process can exec protects
    /// nothing. `SecKeychainFindGenericPassword` asks on behalf of the running
    /// binary, whose designated requirement is what the item was written to
    /// trust.
    ///
    /// A read the ACL denies surfaces as an error rather than as "no such
    /// secret", because those two want opposite responses from the reader.
    fn sec_get(service: &str, account: &str, keychain: Option<&str>) -> Result<Option<String>> {
        // `security` takes a keychain by path; the API takes an opened handle.
        let opened = match keychain {
            Some(path) => Some(
                SecKeychain::open(path).map_err(|e| anyhow!("cannot open keychain {path}: {e}"))?,
            ),
            None => None,
        };
        let search = opened.as_ref().map(std::slice::from_ref);
        match find_generic_password(search, service, account) {
            Ok((password, _item)) => {
                let text = String::from_utf8(password.to_vec())
                    .map_err(|e| anyhow!("keychain value for {account} is not utf-8: {e}"))?;
                Ok(Some(text))
            }
            Err(e) if e.code() == NOT_FOUND => Ok(None),
            Err(e) => Err(anyhow!("keychain read failed for {account}: {e}")),
        }
    }

    fn sec_delete(service: &str, account: &str, keychain: Option<&str>) -> Result<bool> {
        let mut args = vec!["delete-generic-password", "-s", service, "-a", account];
        args.extend(kc_args(keychain));
        let output = sec_run(&args)?;
        if output.status.success() {
            return Ok(true);
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("could not be found") || stderr.contains("-25300") {
            return Ok(false);
        }
        Err(anyhow!(
            "security delete failed for {account}: {}",
            stderr.trim()
        ))
    }

    fn index_read(service: &str, keychain: Option<&str>) -> Result<Vec<String>> {
        if let Some(s) = sec_get(service, INDEX_ACCOUNT, keychain)? {
            return Ok(index_payload_lines(&s));
        }
        match sec_get(service, LEGACY_INDEX_ACCOUNT, keychain)? {
            Some(s) => Ok(index_payload_lines(&s)),
            None => Ok(Vec::new()),
        }
    }

    fn index_payload_lines(payload: &str) -> Vec<String> {
        let payload = decode_hex_index_payload(payload).unwrap_or_else(|| payload.to_string());
        payload
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    fn decode_hex_index_payload(payload: &str) -> Option<String> {
        let s = payload.trim();
        if s.is_empty() || !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }

        let mut bytes = Vec::with_capacity(s.len() / 2);
        for pair in s.as_bytes().chunks_exact(2) {
            let hi = hex_nibble(pair[0])?;
            let lo = hex_nibble(pair[1])?;
            bytes.push((hi << 4) | lo);
        }

        let decoded = String::from_utf8(bytes).ok()?;
        // A single env var name can itself be hex-looking. Only treat the
        // payload as a legacy hex-encoded index when it decodes to the
        // newline-delimited shape the index writer produces for multiple keys.
        if decoded.contains('\n') {
            Some(decoded)
        } else {
            None
        }
    }

    fn hex_nibble(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }

    fn index_write(service: &str, accounts: &[String], keychain: Option<&str>) -> Result<()> {
        let mut set: BTreeSet<String> = accounts.iter().cloned().collect();
        set.remove(INDEX_ACCOUNT);
        let payload = set.into_iter().collect::<Vec<_>>().join("\n");
        // The index is written exactly like the secrets it indexes — same
        // ACL, same rules. It holds only key *names*, but a list of what a
        // project keeps in the keychain is worth no less protection than the
        // values, and two write paths that could drift apart is one too many.
        // `write_item` drops the old item before adding, so the ACL is
        // rewritten rather than inherited from whichever build wrote it first.
        write_item(service, INDEX_ACCOUNT, &payload, keychain)
    }

    fn index_add(service: &str, account: &str, keychain: Option<&str>) -> Result<()> {
        let mut accts = index_read(service, keychain)?;
        if !accts.contains(&account.to_string()) {
            accts.push(account.to_string());
            index_write(service, &accts, keychain)?;
        }
        Ok(())
    }

    fn index_remove(service: &str, account: &str, keychain: Option<&str>) -> Result<()> {
        let mut accts = index_read(service, keychain)?;
        accts.retain(|a| a != account);
        index_write(service, &accts, keychain)
    }

    #[cfg(test)]
    mod tests {
        use super::index_payload_lines;

        #[test]
        fn reads_plain_newline_index() {
            assert_eq!(
                index_payload_lines("SECRET\nDATOCMS_API_TOKEN\n"),
                vec!["SECRET", "DATOCMS_API_TOKEN"]
            );
        }

        #[test]
        fn reads_hex_encoded_newline_index() {
            assert_eq!(
                index_payload_lines("5345435245540a4441544f434d535f4150495f544f4b454e0a"),
                vec!["SECRET", "DATOCMS_API_TOKEN"]
            );
        }

        #[test]
        fn does_not_decode_single_hex_looking_key() {
            assert_eq!(index_payload_lines("ABCDEF"), vec!["ABCDEF"]);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod unsupported {
    use super::*;
    fn err() -> anyhow::Error {
        anyhow!("preceipts secrets: storage requires macOS; Linux libsecret support is not yet implemented")
    }
    pub fn set(
        _service: &str,
        _account: &str,
        _value: &str,
        _keychain: Option<&str>,
    ) -> Result<()> {
        Err(err())
    }
    pub fn get(_service: &str, _account: &str, _keychain: Option<&str>) -> Result<Option<String>> {
        Err(err())
    }
    pub fn delete(_service: &str, _account: &str, _keychain: Option<&str>) -> Result<bool> {
        Err(err())
    }
    pub fn list_accounts(_service: &str, _keychain: Option<&str>) -> Result<Vec<String>> {
        Err(err())
    }
    /// Nothing prompts here, because nothing stores anything here.
    pub fn hush_prompts() -> Option<()> {
        None
    }
}
