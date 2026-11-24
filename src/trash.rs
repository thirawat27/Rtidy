//! A way back from a delete.
//!
//! Everything rtidy removes is reproducible — that is the whole premise — but
//! "reproducible" is not the same as "free", and it is no comfort at all when the thing
//! that went was the one artifact you were about to use. Kondo is honest that it is
//! `rm -rf`; rtidy does not have to be.
//!
//! A clean-up renames each artifact instead of unlinking it, into `.rtidy-trash` at the
//! top of the same volume (a rename inside one filesystem is instant whatever the size,
//! which is why the trash cannot simply live in the cache directory), and writes down
//! where it came from. `rtidy undo` puts the last run back. Entries older than
//! `trash_days` are purged at the start of every clean-up, so the trash is a few days
//! of insurance and never the thing filling the disk.
//!
//! Two cases deliberately skip it: a disk already critically full, where holding bytes
//! back would defeat the point, and `trash_days = 0`.

use crate::util::{human, remove_known, say};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Directory name used both for the volume-level trash and, as a prefix, for the
/// in-place fallback. The walk skips anything starting with this, so quarantined
/// artifacts are never re-reported as leftovers.
pub const DIR: &str = ".rtidy-trash";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    /// Where it was.
    pub from: PathBuf,
    /// Where it is now.
    pub at: PathBuf,
    pub size: u64,
    /// Unix seconds when it was stashed.
    pub when: i64,
    /// Which clean-up run stashed it, so `undo` can take a whole run back.
    pub run: i64,
    /// Ecosystem id, for the report.
    pub eco: String,
}

#[derive(Serialize, Deserialize, Default)]
struct Index {
    entries: Vec<Entry>,
}

fn index_path() -> Result<PathBuf> {
    Ok(crate::config::Config::path()?.with_file_name("trash.json"))
}

fn load() -> Index {
    index_path()
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn store(i: &Index) -> Result<()> {
    let p = index_path()?;
    if let Some(d) = p.parent() {
        fs::create_dir_all(d)?;
    }
    fs::write(p, serde_json::to_string_pretty(i)?)?;
    Ok(())
}

/// Is this path inside a trash directory (or is one)?
pub fn is_trash(name: &str) -> bool {
    name.starts_with(DIR) || name.starts_with("rtidy-trash-")
}

/// The volume `p` lives on: `D:\` on Windows, `/` elsewhere. A rename is only instant
/// within one filesystem, so the trash has to sit on the same one as the artifact.
fn volume_root(p: &Path) -> Option<PathBuf> {
    let mut it = p.components();
    match it.next()? {
        // `C:` plus the following `\`
        Component::Prefix(prefix) => {
            let mut root = PathBuf::from(prefix.as_os_str());
            root.push(std::path::MAIN_SEPARATOR_STR);
            Some(root)
        }
        Component::RootDir => Some(PathBuf::from(std::path::MAIN_SEPARATOR_STR)),
        _ => None,
    }
}

/// Move `path` out of the way, recoverably. Returns where it went.
///
/// Falls back to a rename beside the original when the volume root cannot be written
/// to (a read-only `/`, a locked-down machine), because a rename in the same directory
/// is always possible if the delete would have been.
pub fn stash(path: &Path, size: u64, run: i64, eco: &str) -> Result<PathBuf> {
    let name = path.file_name().context("no file name")?.to_string_lossy().into_owned();
    let stamp = crate::brain::now();
    let unique = format!("rtidy-trash-{stamp}-{}-{name}", std::process::id());

    let mut dest = None;
    if let Some(root) = volume_root(path) {
        let bin = root.join(DIR);
        if fs::create_dir_all(&bin).is_ok() {
            dest = Some(bin.join(&unique));
        }
    }
    let dest = dest.unwrap_or_else(|| path.with_file_name(&unique));
    fs::rename(path, &dest).with_context(|| format!("couldn't move {} aside", path.display()))?;

    let mut i = load();
    i.entries.push(Entry { from: path.to_path_buf(), at: dest.clone(), size, when: stamp, run, eco: eco.to_string() });
    // The index is what makes this recoverable. Without it the artifact is renamed,
    // invisible to `undo`, and still taking up the space — strictly worse than having
    // deleted it. So an index that cannot be written undoes the move and reports it.
    if let Err(e) = store(&i) {
        let _ = fs::rename(&dest, path);
        return Err(e.context("couldn't record the move, so nothing was moved"));
    }
    Ok(dest)
}

/// Everything recoverable right now, newest run first.
pub fn list() -> Vec<Entry> {
    let mut e = load().entries;
    e.retain(|e| e.at.exists());
    e.sort_unstable_by_key(|e| std::cmp::Reverse(e.when));
    e
}

/// Bytes the trash is holding.
pub fn size() -> u64 {
    list().iter().map(|e| e.size).sum()
}

/// Put back the most recent run, or everything.
///
/// Returns how many items came back and how many bytes they were.
pub fn restore(all: bool) -> Result<(usize, u64)> {
    let mut i = load();
    let Some(latest) = i.entries.iter().map(|e| e.run).max() else { return Ok((0, 0)) };
    let mut back = 0;
    let mut bytes = 0;
    let mut keep = Vec::new();
    for e in std::mem::take(&mut i.entries) {
        if !e.at.exists() {
            continue; // purged, or restored by hand
        }
        if !all && e.run != latest {
            keep.push(e);
            continue;
        }
        // Something is there again — a rebuild happened since. Leave both alone rather
        // than overwrite the newer artifact with the older one.
        if e.from.exists() {
            say(&format!("skip: {} exists again", e.from.display()));
            keep.push(e);
            continue;
        }
        if let Some(parent) = e.from.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match fs::rename(&e.at, &e.from) {
            Ok(()) => {
                back += 1;
                bytes += e.size;
                say(&format!("restored {} ({})", e.from.display(), human(e.size)));
            }
            Err(err) => {
                say(&format!("couldn't restore {}: {err}", e.from.display()));
                keep.push(e);
            }
        }
    }
    i.entries = keep;
    store(&i)?;
    Ok((back, bytes))
}

/// Delete trash entries older than `days`, for good. Returns bytes actually freed.
///
/// This is where the space from a clean-up is finally handed back, so a clean-up runs
/// it first: a machine that cleans every day keeps roughly `trash_days` of history and
/// frees the rest without anyone having to think about it.
pub fn purge(days: u64) -> u64 {
    let now = crate::brain::now();
    let cutoff = now - (days as i64) * 86_400;
    let mut i = load();
    let mut freed = 0;
    let mut keep = Vec::new();
    for e in std::mem::take(&mut i.entries) {
        if !e.at.exists() {
            continue;
        }
        if e.when > cutoff {
            keep.push(e);
            continue;
        }
        freed += remove_known(&e.at, e.size, false);
    }
    i.entries = keep;
    let _ = store(&i);
    // Tidy up the now-empty volume bins so a drive root is not left with a stray dir.
    for root in roots_in_use() {
        let _ = fs::remove_dir(root);
    }
    if freed > 0 {
        say(&format!("trash: freed {} from items older than {days}d", human(freed)));
    }
    freed
}

/// Volume-level trash directories rtidy has used, for tidying up.
fn roots_in_use() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = load()
        .entries
        .iter()
        .filter_map(|e| e.at.parent().map(Path::to_path_buf))
        .filter(|p| p.file_name().is_some_and(|n| n == DIR))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// What `rtidy undo` prints, and what the app shows on its restore button.
pub fn summary() -> Option<String> {
    let e = list();
    if e.is_empty() {
        return None;
    }
    let latest = e.iter().map(|x| x.run).max().unwrap_or(0);
    let n = e.iter().filter(|x| x.run == latest).count();
    let bytes: u64 = e.iter().filter(|x| x.run == latest).map(|x| x.size).sum();
    Some(crate::i18n::tf(
        "{} item(s), {} — recoverable",
        &[&n.to_string(), &human(bytes)],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_root_is_the_filesystem_the_path_is_on() {
        if cfg!(windows) {
            assert_eq!(volume_root(Path::new(r"D:\code\thing\target")), Some(PathBuf::from(r"D:\")));
        } else {
            assert_eq!(volume_root(Path::new("/home/me/code/target")), Some(PathBuf::from("/")));
        }
        assert_eq!(volume_root(Path::new("relative/path")), None);
    }

    #[test]
    fn trash_names_are_recognised() {
        assert!(is_trash(".rtidy-trash"));
        assert!(is_trash("rtidy-trash-123-4-target"));
        assert!(!is_trash("target"));
        assert!(!is_trash(".rtidy-keep"));
    }
}
