//! Per-workspace port blocks.
//!
//! Every workspace gets a contiguous, reserved block. Two properties matter
//! more than they sound, and both were learned the hard way in trip's sandbox:
//!
//! 1. **Stable across restarts.** A worktree keeps its addresses, so a bookmark
//!    still works and a printed URL does not rot between boots.
//! 2. **Preserved across teardown.** Stopping a workspace does not surrender
//!    its block; only an explicit release does. Otherwise the addresses shuffle
//!    every time you stop for lunch.
//!
//! Allocation is deterministic — derived from the workspace id — so the same
//! workspace lands on the same block on any machine, with linear probing to
//! settle the collisions that a hash inevitably produces.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Ports per workspace. Enough for a database, a handful of services, and
/// room to add one without re-allocating.
pub const BLOCK_SIZE: u16 = 16;

/// Start of the allocatable range. Above the ephemeral ports macOS hands out
/// (49152+) would collide with them; below 1024 needs privilege. This sits in
/// the registered range, clear of the usual dev-server suspects.
pub const RANGE_START: u16 = 21000;
pub const RANGE_END: u16 = 29000;

const BLOCK_COUNT: u16 = (RANGE_END - RANGE_START) / BLOCK_SIZE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub start: u16,
}

impl Block {
    pub fn end(self) -> u16 {
        self.start + BLOCK_SIZE
    }

    /// The nth port in the block. Services are assigned by declaration order,
    /// so a service keeps its offset as long as the manifest does.
    pub fn port(self, offset: u16) -> Option<u16> {
        (offset < BLOCK_SIZE).then_some(self.start + offset)
    }

    pub fn contains(self, port: u16) -> bool {
        (self.start..self.end()).contains(&port)
    }
}

/// Reservations, keyed by workspace id.
///
/// Persisted as a plain text file so a human can read it, and so a stale entry
/// can be deleted with an editor when something has gone wrong at 2am.
#[derive(Debug, Default, Clone)]
pub struct Reservations {
    entries: BTreeMap<String, u16>,
}

impl Reservations {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut entries = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((id, port)) = line.split_once('=') {
                if let Ok(port) = port.trim().parse::<u16>() {
                    entries.insert(id.trim().to_string(), port);
                }
            }
        }
        Self { entries }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::from("# preceipts port reservations — <workspace> = <block start>\n");
        for (id, port) in &self.entries {
            out.push_str(&format!("{id} = {port}\n"));
        }
        std::fs::write(path, out)
    }

    pub fn get(&self, workspace: &str) -> Option<Block> {
        self.entries.get(workspace).map(|&start| Block { start })
    }

    /// The workspace's block, allocating one if it has none.
    ///
    /// Deterministic from the id, then linear probing. Probing wraps, so a
    /// full range is detected rather than looping forever.
    pub fn reserve(&mut self, workspace: &str) -> Option<Block> {
        if let Some(block) = self.get(workspace) {
            return Some(block);
        }
        let taken: std::collections::HashSet<u16> = self.entries.values().copied().collect();
        let first = hash_index(workspace);
        for step in 0..BLOCK_COUNT {
            let index = (first + step) % BLOCK_COUNT;
            let start = RANGE_START + index * BLOCK_SIZE;
            if !taken.contains(&start) {
                self.entries.insert(workspace.to_string(), start);
                return Some(Block { start });
            }
        }
        None
    }

    /// Give a block back. Teardown must *not* call this — only an explicit
    /// release should, which is why it is a separate verb rather than part of
    /// stopping a workspace.
    pub fn release(&mut self, workspace: &str) -> bool {
        self.entries.remove(workspace).is_some()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Block)> {
        self.entries
            .iter()
            .map(|(id, &start)| (id.as_str(), Block { start }))
    }
}

/// FNV-1a, chosen for being stable across machines and releases. `DefaultHasher`
/// is explicitly not: a port block that moved when the toolchain changed would
/// be a genuinely baffling bug.
fn hash_index(value: &str) -> u16 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    (hash % BLOCK_COUNT as u64) as u16
}

/// Default location for the reservation file.
pub fn default_path(state_dir: &Path) -> PathBuf {
    state_dir.join("ports.toml")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_is_deterministic() {
        let mut a = Reservations::default();
        let mut b = Reservations::default();
        assert_eq!(a.reserve("fix-checkout"), b.reserve("fix-checkout"));
    }

    #[test]
    fn reserving_twice_returns_the_same_block() {
        let mut r = Reservations::default();
        let first = r.reserve("alpha").unwrap();
        let second = r.reserve("alpha").unwrap();
        assert_eq!(first, second);
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn blocks_never_overlap() {
        let mut r = Reservations::default();
        let mut seen: Vec<Block> = Vec::new();
        for i in 0..200 {
            let block = r.reserve(&format!("ws-{i}")).unwrap();
            assert!(
                !seen.iter().any(|b| b.contains(block.start)),
                "block {} overlaps an existing one",
                block.start
            );
            seen.push(block);
        }
    }

    #[test]
    fn blocks_stay_inside_the_range() {
        let mut r = Reservations::default();
        for i in 0..500 {
            let block = r.reserve(&format!("ws-{i}")).unwrap();
            assert!(block.start >= RANGE_START);
            assert!(block.end() <= RANGE_END);
        }
    }

    #[test]
    fn ports_are_offsets_within_the_block() {
        let mut r = Reservations::default();
        let block = r.reserve("alpha").unwrap();
        assert_eq!(block.port(0), Some(block.start));
        assert_eq!(block.port(BLOCK_SIZE - 1), Some(block.end() - 1));
        assert_eq!(block.port(BLOCK_SIZE), None);
    }

    #[test]
    fn reservations_survive_a_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let path = default_path(temp.path());

        let mut r = Reservations::default();
        let alpha = r.reserve("alpha").unwrap();
        let beta = r.reserve("beta").unwrap();
        r.save(&path).unwrap();

        let loaded = Reservations::load(&path);
        assert_eq!(loaded.get("alpha"), Some(alpha));
        assert_eq!(loaded.get("beta"), Some(beta));
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn a_missing_file_is_an_empty_set_not_an_error() {
        let loaded = Reservations::load(Path::new("/nonexistent/ports.toml"));
        assert!(loaded.is_empty());
    }

    #[test]
    fn release_frees_the_block_for_reuse() {
        let mut r = Reservations::default();
        let block = r.reserve("alpha").unwrap();
        assert!(r.release("alpha"));
        assert!(
            !r.release("alpha"),
            "releasing twice is not an error, just false"
        );
        // The same id is deterministic, so it lands back where it was.
        assert_eq!(r.reserve("alpha"), Some(block));
    }

    #[test]
    fn a_full_range_reports_exhaustion_rather_than_hanging() {
        let mut r = Reservations::default();
        for i in 0..BLOCK_COUNT {
            assert!(r.reserve(&format!("ws-{i}")).is_some(), "block {i}");
        }
        assert_eq!(r.reserve("one-too-many"), None);
    }
}
