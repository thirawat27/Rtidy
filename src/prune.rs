//! Remembering which folders hold no Rust, so a rescan does not walk them again.
//!
//! Scanning is bound by the filesystem, not by rtidy. Measured on a 112,000-directory
//! drive, a walk that does nothing at all costs the same as rtidy's real one, and
//! neither batching the Windows directory API nor adding threads moves it much. The
//! only way to make a rescan meaningfully faster is to walk less ground.
//!
//! Most of a drive is usually not code. One game's asset tree on the author's machine
//! held 72,000 of the drive's 112,000 directories and could never contain a Cargo
//! project. This records subtrees that turned out to hold nothing rtidy cares about,
//! so the next scan can skip them after a single `stat`.
//!
//! The trade-off, stated plainly: a subtree is trusted only while its own modification
//! time is unchanged *and* the note is fresh. A project created deep inside a folder
//! that was empty of Rust before may therefore be missed until the note expires. For a
//! clean-up tool that means a folder goes uncleaned for a few hours longer, and the
//! next full walk picks it up. `scan_all` / `--deep` ignores the notes entirely.

use crate::util::mtime_of;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How long a "nothing here" note is trusted before the folder is walked again.
const TTL_SECS: i64 = 12 * 3600;

/// Don't bother remembering trivially small subtrees; the note would cost more than
/// the walk it saves.
const MIN_DIRS_TO_REMEMBER: u32 = 64;

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
struct Note {
    /// The folder's own modification time when it was walked.
    mtime: i64,
    /// When the walk happened.
    checked: i64,
    /// How big the subtree was, so the saving is visible in the log.
    dirs: u32,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Prune {
    /// Folder path → what the last walk of it found.
    empty: HashMap<String, Note>,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn dir_mtime(p: &Path) -> i64 {
    fs::metadata(p)
        .map(|m| mtime_of(&m))
        .and_then(|t| t.duration_since(UNIX_EPOCH).map_err(|_| std::io::Error::other("pre-epoch")))
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Prune {
    fn path() -> Option<PathBuf> {
        Some(dirs::cache_dir()?.join("rtidy").join("prune.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let Some(p) = Self::path() else { return Ok(()) };
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string(self).unwrap_or_default())?;
        fs::rename(tmp, p)
    }

    /// May this folder be skipped? True only when it was walked recently, found to
    /// hold nothing, and has not been touched since.
    pub fn can_skip(&self, dir: &Path) -> bool {
        let Some(note) = self.empty.get(&dir.to_string_lossy().to_string()) else { return false };
        let now = now();
        note.checked <= now && now - note.checked < TTL_SECS && note.mtime == dir_mtime(dir)
    }

    /// Record that walking `dir` found no target dirs and no overrides.
    pub fn remember_empty(&mut self, dir: &Path, dirs: u32) {
        if dirs < MIN_DIRS_TO_REMEMBER {
            return;
        }
        self.empty.insert(
            dir.to_string_lossy().to_string(),
            Note { mtime: dir_mtime(dir), checked: now(), dirs },
        );
    }

    /// Forget a folder: it has something in it after all.
    pub fn forget(&mut self, dir: &Path) {
        self.empty.remove(&dir.to_string_lossy().to_string());
    }

    /// Drop notes that have expired or whose folder is gone, so the file cannot grow
    /// without bound as projects come and go.
    pub fn tidy(&mut self) {
        let now = now();
        self.empty.retain(|k, n| now - n.checked < TTL_SECS && Path::new(k).is_dir());
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rtidy-prune-{}-{name}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn skips_only_what_is_unchanged_and_fresh() {
        let dir = temp("a");
        let mut p = Prune::default();
        assert!(!p.can_skip(&dir), "nothing recorded yet");

        p.remember_empty(&dir, 1000);
        assert!(p.can_skip(&dir), "recorded, unchanged and fresh");

        // Touching the folder (adding an entry) changes its mtime, so the note lapses.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        fs::create_dir_all(dir.join("new-project")).unwrap();
        assert!(!p.can_skip(&dir), "folder changed since it was walked");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tiny_subtrees_are_not_worth_remembering() {
        let dir = temp("b");
        let mut p = Prune::default();
        p.remember_empty(&dir, MIN_DIRS_TO_REMEMBER - 1);
        assert!(!p.can_skip(&dir), "too small to be worth a note");
        p.remember_empty(&dir, MIN_DIRS_TO_REMEMBER);
        assert!(p.can_skip(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_expired_note_is_not_trusted() {
        let dir = temp("c");
        let mut p = Prune::default();
        p.remember_empty(&dir, 500);
        // Backdate it past the TTL.
        let key = dir.to_string_lossy().to_string();
        p.empty.get_mut(&key).unwrap().checked -= TTL_SECS + 1;
        assert!(!p.can_skip(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tidy_drops_expired_and_missing_folders() {
        let dir = temp("d");
        let mut p = Prune::default();
        p.remember_empty(&dir, 500);
        p.empty.insert("/definitely/not/here".into(), Note { mtime: 0, checked: now(), dirs: 900 });
        p.tidy();
        assert!(p.empty.contains_key(&dir.to_string_lossy().to_string()));
        assert!(!p.empty.contains_key("/definitely/not/here"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn forget_removes_a_note() {
        let dir = temp("e");
        let mut p = Prune::default();
        p.remember_empty(&dir, 500);
        p.forget(&dir);
        assert!(!p.can_skip(&dir));
        fs::remove_dir_all(&dir).unwrap();
    }
}
