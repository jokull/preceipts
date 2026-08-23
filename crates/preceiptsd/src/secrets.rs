//! Keychain-backed secret storage. One service namespace per repo.
//!
//! Each (service, account) pair maps to (`procpane:<canonical-repo-path>`, KEY)
//! and stores the UTF-8 secret value. macOS-only for now; Linux returns
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
/// The prefix stays `procpane:` so existing Keychain entries keep working
/// across the dissolution — renaming it would silently orphan every secret a
/// user has already stored, in exchange for tidiness nobody can see
/// through the rename.
pub fn service_name(repo_root: &Path) -> String {
    let root = preceipts_core::workspace::locate(repo_root)
        .map(|workspace| workspace.project_root)
        .unwrap_or_else(|_| repo_root.to_path_buf());
    let canon = root.canonicalize().unwrap_or(root);
    format!("procpane:{}", canon.display())
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
    //! All keychain operations route through `/usr/bin/security` instead of
    //! the security-framework Rust crate.
    //!
    //! Why: the Keychain ACL is evaluated against the *calling binary's*
    //! codesign identity. `cargo install` rewrites procpane constantly during
    //! development; each rebuild is a new identity, and every read prompts.
    //! `/usr/bin/security` is a stable system binary that never moves — once
    //! the user clicks "Always Allow" for it (once, ever), every future
    //! procpane build can read transparently.
    //!
    //! The security-framework fallback is kept only for diagnostic warnings.
    use super::*;
    use security_framework::passwords::set_generic_password;
    use std::collections::BTreeSet;
    use std::process::{Command, Stdio};

    // Magic account name for the per-service index. Listed accounts are
    // tracked here so we don't have to walk every keychain item. The index
    // is hidden from `list_accounts`.
    const INDEX_ACCOUNT: &str = "__procpane_index__";

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
        set_with_open_acl(service, account, value, keychain)?;
        index_add(service, account, keychain)
    }

    /// Write a generic password with an "any-app-may-read" ACL.
    ///
    /// The default `SecItemAdd` ACL ties an item to the codesign hash of the
    /// app that wrote it. `cargo install` rewrites `procpane` constantly during
    /// development, so every rebuild looks like "a new app" to the keychain
    /// and re-prompts the user for permission on every read — extremely
    /// annoying in practice.
    ///
    /// Shelling out to `/usr/bin/security add-generic-password -A` writes the
    /// item with an empty trusted-app list, which the keychain interprets as
    /// "no auth required for any app". `-U` makes it idempotent (update if
    /// exists). The trade-off is documented: anything running as this user
    /// can read these values — the same threat model the rest of procpane
    /// already operates under, since per-task `env_from` injection happens
    /// in-process anyway.
    fn set_with_open_acl(
        service: &str,
        account: &str,
        value: &str,
        keychain: Option<&str>,
    ) -> Result<()> {
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

        // Now add with the open ACL. `-A` writes an empty trusted-app list:
        // "no auth required for any app on this user." The password value
        // briefly appears on argv during the subprocess — local-dev secrets,
        // single-user machine, accepted.
        let mut add_args = vec![
            "add-generic-password",
            "-A",
            "-s",
            service,
            "-a",
            account,
            "-w",
            value,
        ];
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

    /// `security find-generic-password -w` prints the password (and only the
    /// password) on stdout. Captures it without ever passing through the
    /// security-framework crate, so the ACL check is against `/usr/bin/security`
    /// and the user's one-time "Always Allow" grant covers every procpane
    /// build forever.
    fn sec_get(service: &str, account: &str, keychain: Option<&str>) -> Result<Option<String>> {
        let mut args = vec!["find-generic-password", "-s", service, "-a", account, "-w"];
        args.extend(kc_args(keychain));
        let output = sec_run(&args)?;
        if output.status.success() {
            let mut s = String::from_utf8(output.stdout)
                .map_err(|e| anyhow!("keychain value for {account} is not utf-8: {e}"))?;
            // `-w` appends a newline; trim it without losing internal NLs.
            if s.ends_with('\n') {
                s.pop();
                if s.ends_with('\r') {
                    s.pop();
                }
            }
            return Ok(Some(s));
        }
        // exit 44 + "could not be found" → errSecItemNotFound. Anything
        // else is a real error worth surfacing.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("could not be found") || stderr.contains("-25300") {
            return Ok(None);
        }
        Err(anyhow!(
            "security find failed for {account}: {}",
            stderr.trim()
        ))
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
        match sec_get(service, INDEX_ACCOUNT, keychain)? {
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
        // Route through the CLI like the user-facing items, so the index
        // inherits the same "Always Allow /usr/bin/security" trust grant
        // and never re-prompts on rebuild. Delete+add to force a fresh ACL.
        let _ = sec_delete(service, INDEX_ACCOUNT, keychain);
        let mut add_args = vec![
            "add-generic-password",
            "-A",
            "-s",
            service,
            "-a",
            INDEX_ACCOUNT,
            "-w",
            &payload,
        ];
        add_args.extend(kc_args(keychain));
        let output = sec_run(&add_args)?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Fallback to the Rust crate so a borked /usr/bin/security
            // doesn't lose the index entirely.
            tracing::warn!("security index write failed ({stderr}); falling back");
            return set_generic_password(service, INDEX_ACCOUNT, payload.as_bytes())
                .map_err(|e| anyhow!("index write failed: {e}"));
        }
        Ok(())
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
}
