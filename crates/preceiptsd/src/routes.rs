//! Which workspace answers for a hostname, machine-wide.
//!
//! Every workspace daemon runs its own TLS proxy on its own port, which is
//! what lets two worktrees of one project serve at the same time. The cost was
//! that only one of them could hold the well-known port, so every other
//! workspace's URL had to name a port — and the `:443` forwarder, which points
//! at exactly one listener, could only ever serve one of them.
//!
//! This is the missing piece: a file every daemon writes its hostnames into,
//! and one router reads. A plain text file rather than a socket, for the same
//! reason the port reservations are one — a human can read it, and a daemon
//! that died in a way nobody predicted leaves something inspectable rather
//! than a lost connection.

use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Where the router listens, and what the `:443` forwarder points at.
pub const ROUTER_PORT: u16 = 8443;

pub fn routes_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("could not determine $HOME"))?;
    Ok(home.join(".preceipts").join("routes.toml"))
}

/// Read the table. A missing or unreadable file is an empty table, never an
/// error: the router must keep serving what it already knows rather than fall
/// over because someone was editing the file at the wrong moment.
pub fn load() -> BTreeMap<String, u16> {
    let Ok(path) = routes_path() else {
        return BTreeMap::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    parse(&text)
}

pub fn parse(text: &str) -> BTreeMap<String, u16> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (host, port) = line.split_once('=')?;
            let port = port.trim().parse().ok()?;
            Some((host.trim().to_string(), port))
        })
        .collect()
}

pub fn render(routes: &BTreeMap<String, u16>) -> String {
    let mut out = String::from("# preceipts host routes — <hostname> = <workspace proxy port>\n");
    for (host, port) in routes {
        out.push_str(&format!("{host} = {port}\n"));
    }
    out
}

/// Point `hosts` at `port`, leaving every other workspace's routes alone.
///
/// Read-modify-write rather than append: a workspace that restarts on a
/// different port must replace its old entries, and a stale route is worse
/// than a missing one — it sends traffic to a port that may since have been
/// taken by something else entirely.
pub fn register(hosts: &[String], port: u16) -> Result<()> {
    update(|routes| {
        for host in hosts {
            routes.insert(host.clone(), port);
        }
    })
}

/// Drop these hostnames, whatever they pointed at.
pub fn unregister(hosts: &[String]) -> Result<()> {
    update(|routes| {
        for host in hosts {
            routes.remove(host);
        }
    })
}

/// Drop every route pointing at `port`.
///
/// The cleanup path for a daemon that stopped without saying so: its port is
/// the one thing another process can identify its routes by.
pub fn unregister_port(port: u16) -> Result<()> {
    update(|routes| routes.retain(|_, p| *p != port))
}

fn update(mutate: impl FnOnce(&mut BTreeMap<String, u16>)) -> Result<()> {
    let path = routes_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut routes = load();
    mutate(&mut routes);

    // Write beside, then rename. The router reads this file on every miss, and
    // a partially written table would look like a set of hostnames that do not
    // exist — a 503 for a service that is running perfectly well.
    let temporary = path.with_extension("toml.new");
    std::fs::write(&temporary, render(&routes))?;
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_round_trips() {
        let mut routes = BTreeMap::new();
        routes.insert("api.proj.localhost".to_string(), 21001);
        routes.insert("web.feat.proj.localhost".to_string(), 27801);
        assert_eq!(parse(&render(&routes)), routes);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let routes = parse("# a comment\n\n  api.localhost = 21001  \n");
        assert_eq!(routes.get("api.localhost"), Some(&21001));
        assert_eq!(routes.len(), 1);
    }

    /// A file someone hand-edited badly must cost only the broken line. The
    /// router reads this on every request it has not seen before.
    #[test]
    fn a_malformed_line_costs_only_itself() {
        let routes = parse("api.localhost = 21001\nnonsense\nweb.localhost = notaport\n");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes.get("api.localhost"), Some(&21001));
    }
}
