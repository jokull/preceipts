//! What this build is allowed to do, asked of the binary rather than assumed.
//!
//! Two capabilities in this project wait on a real Apple signing identity, and
//! both fail in ways that are hard to read if you guess wrong:
//!
//! - **A real ACL on each secret.** A keychain item's ACL names the code
//!   allowed to read it, and code is named by its *designated requirement* —
//!   which, for a Developer ID signature, is an identifier plus a team anchor
//!   and therefore survives every rebuild. An ad-hoc signature has no such
//!   stable name: each `cargo build` is a different application to the
//!   keychain, which is exactly why the open-ACL workaround exists. It is a
//!   real compromise taken for dev convenience, not a design.
//!
//!   (An earlier plan here was the `keychain-access-groups` entitlement. It
//!   does not work: it is a restricted entitlement, so outside the App Store
//!   it needs an embedded provisioning profile and AMFI SIGKILLs a binary
//!   that claims one without — and it governs the data-protection keychain,
//!   not the file-based one these items live in. Measured, not assumed.)
//! - **`SMAppService` eligibility.** An unsigned or ad-hoc daemon cannot be
//!   registered at all, so a build that is not signed can stop asking. A
//!   signed one is *eligible*; calling the API is a separate piece of work
//!   that is not written yet, and the `:443` forwarder still goes in through
//!   `sudo` and a hand-written plist in both cases. `can_register_daemon`
//!   answers "would this be allowed", not "does this happen".
//!
//! So the strategy is read from the running binary at startup rather than
//! compiled in. A developer running `cargo run` and a user running a notarized
//! `.app` are the same code taking different branches, and each can say which
//! branch it took.

use std::process::Command;

/// The Team Identifier this binary is signed with, if any.
///
/// Ad-hoc signatures — what `cargo build` produces on Apple silicon, since the
/// linker signs every binary — report `TeamIdentifier=not set`. That is the
/// distinction that matters: not "is there a signature" but "is there a team",
/// because a team anchor is what makes a designated requirement stable across
/// rebuilds — and a stable requirement is what an ACL can be written against.
pub fn team_identifier() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2"])
        .arg(&exe)
        .output()
        .ok()?;
    // codesign writes its report to stderr, which is not an error here.
    parse_team_identifier(&String::from_utf8_lossy(&output.stderr))
}

fn parse_team_identifier(report: &str) -> Option<String> {
    report
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|team| !team.is_empty() && *team != "not set")
        .map(str::to_string)
}

/// How this build stores secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainStrategy {
    /// Items carry an ACL naming this project's own binaries, identified by
    /// their designated requirement — the Team Identifier held here plus each
    /// binary's identifier. Nothing else on the machine can read them, and
    /// because the requirement does not change when the binary is rebuilt,
    /// there are no prompts either.
    TeamAcl(String),
    /// `/usr/bin/security` with an empty trusted-app list.
    ///
    /// The compromise an unsigned build has to make. `SecItemAdd`'s default
    /// ACL binds an item to the creating binary's codesign hash, so every
    /// rebuild reads as a new application and the user drowns in prompts;
    /// an open ACL trades that away. Any process running as this user can
    /// read the values, which is the same threat model as a dotenv file in
    /// the repository — worth stating rather than hiding.
    OpenAcl,
}

impl KeychainStrategy {
    pub fn detect() -> Self {
        match team_identifier() {
            Some(team) => KeychainStrategy::TeamAcl(team),
            None => KeychainStrategy::OpenAcl,
        }
    }

    /// One line for `trust status`, so which branch is live is never a guess.
    pub fn explain(&self) -> String {
        match self {
            KeychainStrategy::TeamAcl(team) => {
                format!("secrets are readable only by code signed by team {team}")
            }
            KeychainStrategy::OpenAcl => "secrets use an open ACL — this build is not signed \
                 with a Team Identifier, so it has no stable identity to write an ACL against. \
                 Any process running as you can read them."
                .to_string(),
        }
    }
}

/// Can this build register a daemon with `SMAppService`?
///
/// Registration requires the daemon to live inside a signed bundle whose
/// signature the system trusts. Unsigned, the call fails at a point where the
/// error says little, so the honest thing is to check first and route through
/// the `sudo` path with an explanation.
pub fn can_register_daemon() -> bool {
    team_identifier().is_some() && bundled().is_some()
}

/// The `.app` this binary is inside, if it is inside one.
///
/// `…/Preceipts.app/Contents/MacOS/preceiptsd` — three parents up is the
/// bundle. Checked by name rather than by asking the system, because the
/// question is "did our packaging put me here", not "is some bundle running".
pub fn bundled() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let bundle = exe.parent()?.parent()?.parent()?;
    bundle
        .extension()
        .is_some_and(|extension| extension == "app")
        .then(|| bundle.to_path_buf())
}

/// The binaries an ACL should trust: this one and its siblings.
///
/// All three ship in one directory — inside `Contents/MacOS` for the bundle,
/// or wherever the CLI was installed — and all three read the same secrets.
/// Paths are how `security -T` is told about an application, but what it
/// stores is the *requirement* it reads off that path, so moving a binary
/// afterwards does not invalidate the grant.
///
/// Only paths that exist are returned: naming an absent one makes `security`
/// fail the whole write, which would turn a partial install into no secrets
/// at all.
pub fn trusted_binaries() -> Vec<std::path::PathBuf> {
    const SIBLINGS: [&str; 3] = ["preceipts", "preceiptsd", "preceipts-app"];
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    let Some(dir) = exe.parent() else {
        return Vec::new();
    };
    SIBLINGS
        .iter()
        .map(|name| dir.join(name))
        .filter(|path| path.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `cargo build` produces on Apple silicon: signed, but ad-hoc. The
    /// linker signs every binary now, so "has a signature" stopped being a
    /// useful question and "has a team" replaced it.
    #[test]
    fn an_adhoc_signature_has_no_team() {
        let report = "Executable=/path/preceiptsd\n\
                      CodeDirectory v=20400 flags=0x20002(adhoc,linker-signed)\n\
                      Signature=adhoc\n\
                      TeamIdentifier=not set\n";
        assert_eq!(parse_team_identifier(report), None);
    }

    #[test]
    fn a_developer_id_signature_yields_its_team() {
        let report = "Authority=Developer ID Application: Someone (AB12CD34EF)\n\
                      TeamIdentifier=AB12CD34EF\n\
                      Sealed Resources=none\n";
        assert_eq!(parse_team_identifier(report).as_deref(), Some("AB12CD34EF"));
    }

    #[test]
    fn an_unsigned_binary_yields_nothing() {
        assert_eq!(
            parse_team_identifier("code object is not signed at all"),
            None
        );
        assert_eq!(parse_team_identifier(""), None);
    }

    /// The team is what the ACL is written against, so a build that has one
    /// has to name it. The previous shape of this type held a pre-formatted
    /// string and the test built its own copy of that string — which meant a
    /// missing separator in the real code passed the test for as long as it
    /// existed. Hold the team itself, and there is nothing left to format
    /// wrongly.
    #[test]
    fn a_signed_build_names_the_team_its_secrets_are_locked_to() {
        let strategy = KeychainStrategy::TeamAcl("AB12CD34EF".into());
        assert!(
            strategy.explain().contains("AB12CD34EF"),
            "the team is the whole claim: {}",
            strategy.explain()
        );
    }

    /// Naming a path that is not there makes `security` reject the entire
    /// write, so a half-installed tree would mean no secrets rather than
    /// fewer trusted readers.
    #[test]
    fn only_binaries_that_exist_are_offered_to_the_acl() {
        for path in trusted_binaries() {
            assert!(path.is_file(), "{} does not exist", path.display());
        }
    }

    /// An unsigned build must say what it is doing instead. The compromise is
    /// defensible; hiding it is not.
    #[test]
    fn an_unsigned_build_states_the_compromise_it_is_making() {
        let explanation = KeychainStrategy::OpenAcl.explain();
        assert!(explanation.contains("not signed"));
        assert!(
            explanation.contains("read them"),
            "who can read the secrets is the part worth saying: {explanation}"
        );
    }

    /// Whatever this build is, asking must not panic or hang — `trust status`
    /// calls it and a person is waiting.
    #[test]
    fn detecting_the_strategy_works_on_whatever_this_build_is() {
        let started = std::time::Instant::now();
        let strategy = KeychainStrategy::detect();
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        assert!(!strategy.explain().is_empty());
        // In a test binary this is always the unsigned branch, and saying so
        // is the point: the signed branch cannot be exercised here.
        assert_eq!(strategy, KeychainStrategy::OpenAcl);
    }

    #[test]
    fn a_binary_outside_a_bundle_is_not_bundled() {
        assert!(bundled().is_none(), "a test binary is not in an .app");
        assert!(!can_register_daemon());
    }
}
