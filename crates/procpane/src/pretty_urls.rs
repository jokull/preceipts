//! hostname setup plus a root-owned loopback proxy on `:443`, so that
//! `https://web.test` works in the browser without typing `:8443`.
//!
//! ## Why this exists at all
//!
//! macOS doesn't let user-land processes bind to ports below 1024. The main
//! procpane daemon stays unprivileged and keeps the TLS/SNI proxy on :8443.
//! A tiny LaunchDaemon-owned helper binds 127.0.0.1:443 and TCP-forwards to
//! :8443. The helper does not know about repos, tasks, certs, or hostnames.
//!
//! ## Layout — two system files, both tagged for `trust uninstall`
//!
//! 1. `/etc/hosts` — explicit loopback entries for hostnames declared in
//!    the current repo's `procpane.toml`. `/etc/hosts` has no wildcard
//!    support, so we install the concrete names the repo uses.
//! 2. `/Library/LaunchDaemons/com.procpane.pretty-url-proxy.plist` — runs
//!    `procpane pretty-url-proxy` as root so only the privileged bind lives
//!    outside the user daemon.
//!
//! Loopback-only (`on lo0`) — we don't touch external interfaces, so this
//! cannot accidentally expose anything to the network.

use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

pub const HOSTS_PATH: &str = "/etc/hosts";
pub const PROXY_PLIST_PATH: &str = "/Library/LaunchDaemons/com.procpane.pretty-url-proxy.plist";
pub const HOSTS_BEGIN: &str =
    "# BEGIN procpane hostnames (managed by `procpane trust install --pretty-urls`)";
pub const HOSTS_END: &str = "# END procpane hostnames";

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

pub fn is_installed() -> bool {
    Path::new(PROXY_PLIST_PATH).is_file()
}

pub fn install(hostnames: &[String]) -> Result<()> {
    println!("\nInstalling pretty-urls support.");
    println!("  hosts: {HOSTS_PATH} (adds declared procpane.toml hostnames)");
    println!("  launchd: {PROXY_PLIST_PATH}");

    install_hosts(hostnames)?;

    let procpane_path = std::env::current_exe().map_err(|e| anyhow!("current executable: {e}"))?;
    sudo_write(
        PROXY_PLIST_PATH,
        &proxy_plist_contents(&procpane_path),
        0o644,
    )?;

    // Re-register so repeat installs pick up a newly-installed binary path.
    run_sudo_quiet(&["launchctl", "bootout", "system", PROXY_PLIST_PATH]);
    run_sudo(&["launchctl", "bootstrap", "system", PROXY_PLIST_PATH])?;

    if hostnames.is_empty() {
        println!(
            "✓ pretty-urls enabled. No procpane.toml hostnames were found to add to /etc/hosts."
        );
    } else {
        println!(
            "✓ pretty-urls enabled for: {}",
            normalized_hostnames(hostnames)?.join(", ")
        );
    }
    Ok(())
}

pub fn uninstall() -> Result<()> {
    println!("\nReverting pretty-urls support.");

    // Ignore errors because the helper may not be loaded.
    run_sudo_quiet(&["launchctl", "bootout", "system", PROXY_PLIST_PATH]);

    // Delete current plist.
    let _ = run_sudo(&["/bin/rm", "-f", PROXY_PLIST_PATH]);

    // Remove the managed hosts block.
    if let Ok(current) = std::fs::read_to_string(HOSTS_PATH) {
        if current.contains(HOSTS_BEGIN) {
            let cleaned = strip_block(&current, HOSTS_BEGIN, HOSTS_END);
            let _ = sudo_write(HOSTS_PATH, &cleaned, 0o644);
        }
    }

    println!("✓ pretty-urls removed.");
    Ok(())
}

pub fn missing_hostnames(hostnames: &[String]) -> Vec<String> {
    let current = std::fs::read_to_string(HOSTS_PATH).unwrap_or_default();
    hostnames
        .iter()
        .filter(|host| {
            !current
                .split_whitespace()
                .any(|token| token.eq_ignore_ascii_case(host))
        })
        .cloned()
        .collect()
}

fn install_hosts(hostnames: &[String]) -> Result<()> {
    let hostnames = normalized_hostnames(hostnames)?;
    if hostnames.is_empty() {
        println!("  (no procpane.toml hostnames found; skipping /etc/hosts)");
        return Ok(());
    }

    let current =
        std::fs::read_to_string(HOSTS_PATH).map_err(|e| anyhow!("read {HOSTS_PATH}: {e}"))?;
    let cleaned = if current.contains(HOSTS_BEGIN) {
        strip_block(&current, HOSTS_BEGIN, HOSTS_END)
    } else {
        current
    };
    let mut next = cleaned.trim_end().to_string();
    next.push_str("\n\n");
    next.push_str(&hosts_block(&hostnames));
    sudo_write(HOSTS_PATH, &next, 0o644)
}

fn hosts_block(hostnames: &[String]) -> String {
    format!(
        "{HOSTS_BEGIN}\n127.0.0.1 {}\n{HOSTS_END}\n",
        hostnames.join(" ")
    )
}

fn normalized_hostnames(hostnames: &[String]) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for host in hostnames {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if host.is_empty() {
            continue;
        }
        validate_hostname(&host)?;
        out.push(host);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn validate_hostname(host: &str) -> Result<()> {
    if host.len() > 253 {
        return Err(anyhow!("hostname too long: {host}"));
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(anyhow!("invalid hostname label in {host}"));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(anyhow!("invalid hostname label in {host}"));
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(anyhow!("invalid hostname: {host}"));
        }
    }
    Ok(())
}

/// Strip the lines between `begin` and `end` markers (inclusive). Used to
/// reverse managed file edits. Trims one trailing newline so we don't grow the
/// file by one empty line every install/uninstall cycle.
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

/// Write `contents` to `path` as root, using `sudo tee` + `sudo chmod`. The
/// helper exists so `install()` reads top-to-bottom; calling it does prompt
/// for sudo if the credential cache is cold.
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

    #[test]
    fn hosts_block_uses_explicit_ipv4_loopback_names() {
        let names = normalized_hostnames(&[
            "web.test".to_string(),
            "API.TEST.".to_string(),
            "web.test".to_string(),
        ])
        .unwrap();
        assert_eq!(names, vec!["api.test".to_string(), "web.test".to_string()]);
        let block = hosts_block(&names);
        assert!(block.contains(HOSTS_BEGIN));
        assert!(block.contains("127.0.0.1 api.test web.test"));
        assert!(!block.contains("::1"));
    }

    #[test]
    fn invalid_hostnames_are_rejected() {
        assert!(normalized_hostnames(&["api test".to_string()]).is_err());
        assert!(normalized_hostnames(&["-api.test".to_string()]).is_err());
    }
}
