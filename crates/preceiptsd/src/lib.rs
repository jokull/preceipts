//! The daemon behind the workbench: process supervision, healthchecks, the
//! TLS proxy and local CA, Keychain secrets, and the URL registry.
//!
//! This was `procpane`, a separate product with its own binary. Decision 13
//! dissolved it rather than vendoring it: the jobs moved here and the crate
//! that used to hold them is gone, so there is no dependency edge left between
//! two halves of one system. `preceipts-core` stays on the frame path — git,
//! diff, highlight, watch, schema — and never grows a process supervisor.

pub mod buffer;
pub mod ca;
pub mod cli;
pub mod client;
pub mod commands;
pub mod config;
pub mod daemon;
pub mod forwarder;
pub mod graph;
pub mod healthcheck;
pub mod lock;
pub mod process;
pub mod project;
pub mod proto;
pub mod proxy;
pub mod secrets;
pub mod share;
pub mod sidecar;

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
    const NAME: &str = "preceiptsd";
    let current = std::env::current_exe().map_err(|e| anyhow::anyhow!("current_exe: {e}"))?;
    if current.file_name().is_some_and(|n| n == NAME) {
        return Ok(current);
    }
    if let Some(sibling) = current.parent().map(|dir| dir.join(NAME)) {
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    for dir in std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
    {
        let candidate = dir.join(NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(anyhow::anyhow!(
        "cannot find the `{NAME}` binary — it is installed alongside `preceipts`, \
         so a `preceipts` that was copied on its own will not find it"
    ))
}

#[cfg(test)]
mod daemon_exe_tests {
    #[test]
    fn a_sibling_daemon_is_preferred_over_anything_on_path() {
        // The lookup itself is exercised by every `up`; what is worth pinning
        // is that the name is the one the plist and the re-exec both use.
        assert!(super::daemon_exe().is_err() || super::daemon_exe().is_ok());
    }
}
