//! The daemon behind the workbench: process supervision, healthchecks, the
//! TLS proxy and local CA, Keychain secrets, and the URL registry.
//!
//! This was `procpane`, a separate product with its own binary. Decision 13
//! dissolved it rather than vendoring it: the jobs moved here and the crate
//! that used to hold them is gone, so there is no dependency edge left between
//! two halves of one system. `preceipts-core` stays on the frame path — git,
//! diff, highlight, watch, schema — and never grows a process supervisor.

#[cfg(target_os = "macos")]
pub mod acl;
pub mod bridge;
pub mod buffer;
pub mod ca;
pub mod checkrun;
pub mod cli;
pub mod client;
pub mod commands;
pub mod config;
pub mod container;
pub mod daemon;
pub mod forwarder;
pub mod graph;
pub mod healthcheck;
pub mod lock;
pub mod process;
pub mod project;
pub mod proto;
pub mod proxy;
pub mod router;
pub mod routes;
pub mod secrets;
pub mod services;
pub mod share;
pub mod signing;
pub mod transcript;

use std::path::PathBuf;

/// Where the `preceiptsd` binary is.
///
/// Needed because two things re-exec it by path — `up` detaching the daemon,
/// and the launchd plist for the `:443` forwarder — and both may be invoked
/// from the `preceipts` CLI rather than from the daemon itself. Taking
/// `current_exe()` on faith was correct exactly while there was one binary.
///
/// The sibling lookup is the case that matters: `preceipts` and `preceiptsd`
/// are built and installed together, so a `preceiptsd` next to the running
/// binary is the right one, and preferring it over `PATH` means a locally
/// built pair does not silently drive an installed daemon.
pub fn daemon_exe() -> anyhow::Result<PathBuf> {
    let current = std::env::current_exe().map_err(|e| anyhow::anyhow!("current_exe: {e}"))?;
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    resolve_daemon_exe(&current, &path_dirs, |p| p.is_file()).ok_or_else(|| {
        anyhow::anyhow!(
            "cannot find the `{DAEMON_NAME}` binary — it is installed alongside `preceipts`, \
             so a `preceipts` that was copied on its own will not find it"
        )
    })
}

const DAEMON_NAME: &str = "preceiptsd";

/// The lookup, with the filesystem passed in so it can be tested.
///
/// `exists` is a predicate rather than a direct `is_file` call because the
/// only interesting cases here are about *which* candidate wins, and staging
/// three real binaries on disk to assert an ordering would test the operating
/// system rather than the rule.
fn resolve_daemon_exe(
    current: &std::path::Path,
    path_dirs: &[PathBuf],
    exists: impl Fn(&std::path::Path) -> bool,
) -> Option<PathBuf> {
    if current.file_name().is_some_and(|n| n == DAEMON_NAME) {
        return Some(current.to_path_buf());
    }
    if let Some(sibling) = current.parent().map(|dir| dir.join(DAEMON_NAME)) {
        if exists(&sibling) {
            return Some(sibling);
        }
    }
    path_dirs
        .iter()
        .map(|dir| dir.join(DAEMON_NAME))
        .find(|candidate| exists(candidate))
}

#[cfg(test)]
mod daemon_exe_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_daemon_running_itself_needs_no_lookup() {
        let me = Path::new("/opt/bin/preceiptsd");
        assert_eq!(
            resolve_daemon_exe(me, &[], |_| false).as_deref(),
            Some(me),
            "no filesystem access at all when we already are the daemon"
        );
    }

    /// The case that matters: a locally built `preceipts` must drive the
    /// daemon it was built with, not one that happens to be installed.
    #[test]
    fn a_sibling_daemon_beats_anything_on_path() {
        let cli = Path::new("/repo/target/debug/preceipts");
        let path = vec![PathBuf::from("/usr/local/bin")];
        let found = resolve_daemon_exe(cli, &path, |p| {
            p == Path::new("/repo/target/debug/preceiptsd")
                || p == Path::new("/usr/local/bin/preceiptsd")
        });
        assert_eq!(
            found.as_deref(),
            Some(Path::new("/repo/target/debug/preceiptsd")),
            "the sibling wins even when an installed one exists"
        );
    }

    #[test]
    fn path_is_the_fallback_when_there_is_no_sibling() {
        let cli = Path::new("/somewhere/else/preceipts");
        let path = vec![PathBuf::from("/a"), PathBuf::from("/usr/local/bin")];
        let found = resolve_daemon_exe(cli, &path, |p| p == Path::new("/usr/local/bin/preceiptsd"));
        assert_eq!(
            found.as_deref(),
            Some(Path::new("/usr/local/bin/preceiptsd"))
        );
    }

    #[test]
    fn nothing_found_is_a_reported_failure_rather_than_a_guess() {
        let cli = Path::new("/tmp/preceipts");
        assert!(resolve_daemon_exe(cli, &[PathBuf::from("/a")], |_| false).is_none());
    }
}
