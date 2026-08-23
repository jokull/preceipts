//! The one privileged thing in the whole system: a root-owned TCP forwarder
//! that binds `127.0.0.1:443` and copies bytes to the daemon's TLS proxy on
//! `:8443`, so `https://web.proj.localhost` works without typing a port.
//!
//! ## Why this is all that is left
//!
//! This module used to also manage a block in `/etc/hosts`, because hostnames
//! had to be made to resolve. Decision 13 deleted that half: macOS resolves
//! `*.localhost` to loopback at any depth on its own, so there is no name to
//! install and none to clean up. What remains is the one thing the OS will not
//! give an unprivileged process — a bind below port 1024.
//!
//! The helper is deliberately stupid. It knows nothing about repositories,
//! tasks, certificates, or hostnames; it accepts and it copies. Everything that
//! could be wrong lives in the unprivileged daemon, and `lsof -i :443` shows an
//! inspectable process rather than a hidden `pf` redirect.
//!
//! Loopback only (`127.0.0.1`), so this cannot expose anything to the network.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Command;
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};

use crate::proxy::PROXY_PORT;

pub const PROXY_PLIST_PATH: &str = "/Library/LaunchDaemons/com.procpane.pretty-url-proxy.plist";

const LISTEN_ADDR: &str = "127.0.0.1:443";
const TARGET_ADDR: &str = "127.0.0.1";

/// Where a previous version wrote hostnames. Kept only so `uninstall` can
/// remove a block it is no longer capable of creating — a tool that stops
/// managing a root-owned file still owes the user its cleanup.
const HOSTS_PATH: &str = "/etc/hosts";
const HOSTS_BEGIN: &str =
    "# BEGIN procpane hostnames (managed by `procpane trust install --pretty-urls`)";
const HOSTS_END: &str = "# END procpane hostnames";

pub fn is_installed() -> bool {
    Path::new(PROXY_PLIST_PATH).is_file()
}

pub fn install() -> Result<()> {
    println!("\nInstalling the :443 forwarder.");
    println!("  launchd: {PROXY_PLIST_PATH}");
    println!("  Hostnames need no install — macOS resolves *.localhost itself.");

    let procpane_path = std::env::current_exe().map_err(|e| anyhow!("current executable: {e}"))?;
    sudo_write(
        PROXY_PLIST_PATH,
        &proxy_plist_contents(&procpane_path),
        0o644,
    )?;

    // Re-register so repeat installs pick up a newly-installed binary path.
    run_sudo_quiet(&["launchctl", "bootout", "system", PROXY_PLIST_PATH]);
    run_sudo(&["launchctl", "bootstrap", "system", PROXY_PLIST_PATH])?;

    println!("✓ portless URLs enabled: https://<service>.<workspace>.<project>.localhost");
    Ok(())
}

pub fn uninstall() -> Result<()> {
    println!("\nRemoving the :443 forwarder.");

    // Ignore errors because the helper may not be loaded.
    run_sudo_quiet(&["launchctl", "bootout", "system", PROXY_PLIST_PATH]);
    let _ = run_sudo(&["/bin/rm", "-f", PROXY_PLIST_PATH]);
    remove_legacy_hosts_block();

    println!("✓ forwarder removed.");
    Ok(())
}

/// Take out the `/etc/hosts` block an older procpane may have left behind.
///
/// Silent when there is nothing to do, which is the common case now — the only
/// machines that have one are those that ran `trust install --pretty-urls`
/// before decision 13.
pub fn remove_legacy_hosts_block() {
    let Ok(current) = std::fs::read_to_string(HOSTS_PATH) else {
        return;
    };
    if !current.contains(HOSTS_BEGIN) {
        return;
    }
    println!("  removing the hostname block an earlier version left in {HOSTS_PATH}");
    let cleaned = strip_block(&current, HOSTS_BEGIN, HOSTS_END);
    let _ = sudo_write(HOSTS_PATH, &cleaned, 0o644);
}

/// True when a stale managed block is still sitting in `/etc/hosts`.
pub fn has_legacy_hosts_block() -> bool {
    std::fs::read_to_string(HOSTS_PATH)
        .map(|text| text.contains(HOSTS_BEGIN))
        .unwrap_or(false)
}

/// The forwarder itself, run as root by launchd.
pub fn run() -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_inner())
}

async fn run_inner() -> Result<()> {
    let listener = TcpListener::bind(LISTEN_ADDR)
        .await
        .with_context(|| format!("bind {LISTEN_ADDR}"))?;
    eprintln!("procpane :443 forwarder listening on {LISTEN_ADDR}");

    loop {
        let (stream, peer) = listener.accept().await.context("accept on :443")?;
        tokio::spawn(async move {
            if let Err(error) = forward(stream).await {
                tracing::debug!(?peer, ?error, ":443 forwarder connection ended");
            }
        });
    }
}

async fn forward(mut inbound: TcpStream) -> Result<()> {
    let mut outbound = TcpStream::connect((TARGET_ADDR, PROXY_PORT))
        .await
        .with_context(|| format!("connect {TARGET_ADDR}:{PROXY_PORT}"))?;
    let _ = copy_bidirectional(&mut inbound, &mut outbound).await?;
    Ok(())
}

fn proxy_plist_contents(procpane_path: &Path) -> String {
    let procpane_path = xml_escape(&procpane_path.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.procpane.pretty-url-proxy</string>
  <key>ProgramArguments</key>
  <array>
    <string>{procpane_path}</string>
    <string>pretty-url-proxy</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/var/log/procpane-pretty-url-proxy.log</string>
  <key>StandardErrorPath</key><string>/var/log/procpane-pretty-url-proxy.log</string>
</dict>
</plist>
"#
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Strip the lines between `begin` and `end` markers (inclusive). Trims one
/// trailing newline so we don't grow the file by one empty line per cycle.
fn strip_block(text: &str, begin: &str, end: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut skipping = false;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if !skipping && trimmed == begin {
            skipping = true;
            continue;
        }
        if skipping {
            if trimmed == end {
                skipping = false;
            }
            continue;
        }
        out.push_str(line);
    }
    // Avoid leaving a leading blank line if our block sat at the bottom.
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Write `contents` to `path` as root, using `sudo tee` + `sudo chmod`.
fn sudo_write(path: &str, contents: &str, mode: u32) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = Command::new("sudo")
        .args(["tee", path])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| anyhow!("spawn sudo tee {path}: {e}"))?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("sudo tee stdin"))?;
        stdin
            .write_all(contents.as_bytes())
            .map_err(|e| anyhow!("write to sudo tee: {e}"))?;
    }
    let status = child
        .wait()
        .map_err(|e| anyhow!("wait sudo tee {path}: {e}"))?;
    if !status.success() {
        return Err(anyhow!("sudo tee {path} failed"));
    }
    let mode_str = format!("{mode:o}");
    run_sudo(&["chmod", &mode_str, path])
}

fn run_sudo(args: &[&str]) -> Result<()> {
    let status = Command::new("sudo")
        .args(args)
        .status()
        .map_err(|e| anyhow!("spawn sudo {args:?}: {e}"))?;
    if !status.success() {
        return Err(anyhow!("`sudo {}` failed", args.join(" ")));
    }
    Ok(())
}

fn run_sudo_quiet(args: &[&str]) {
    use std::process::Stdio;

    let _ = Command::new("sudo")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_plist_runs_current_binary_helper() {
        let plist = proxy_plist_contents(Path::new("/tmp/procpane & friends/procpane"));
        assert!(plist.contains("com.procpane.pretty-url-proxy"));
        assert!(plist.contains("<string>/tmp/procpane &amp; friends/procpane</string>"));
        assert!(plist.contains("<string>pretty-url-proxy</string>"));
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
    }

    /// The cleanup path has to survive a hosts file that has been edited by
    /// hand around our block, which is the normal state of `/etc/hosts`.
    #[test]
    fn stripping_the_legacy_block_leaves_the_rest_of_the_file_alone() {
        let text = format!(
            "127.0.0.1 localhost\n\n{HOSTS_BEGIN}\n127.0.0.1 web.test api.test\n{HOSTS_END}\n\n255.255.255.255 broadcasthost\n"
        );
        let cleaned = strip_block(&text, HOSTS_BEGIN, HOSTS_END);
        assert!(cleaned.contains("127.0.0.1 localhost"));
        assert!(cleaned.contains("255.255.255.255 broadcasthost"));
        assert!(!cleaned.contains("web.test"));
        assert!(!cleaned.contains(HOSTS_BEGIN));
    }

    #[test]
    fn stripping_a_file_without_our_block_changes_nothing_but_whitespace() {
        let text = "127.0.0.1 localhost\n";
        assert_eq!(strip_block(text, HOSTS_BEGIN, HOSTS_END), text);
    }
}
