//! Per-project `target/` folders: find, list, delete.

use crate::say;
use crate::util::*;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

/// Put this empty file next to Cargo.toml to protect a project's target/ from cleaning.
pub const KEEP_MARKER: &str = ".rtidy-keep";

pub struct Target {
    pub path: PathBuf,
    pub size: u64,
    pub last_build: SystemTime,
    pub keep: bool,
}

/// A Cargo target dir: named `target`, sibling of Cargo.toml, with a Cargo marker inside.
fn is_cargo_target(dir: &Path) -> bool {
    dir.file_name().is_some_and(|n| n == "target")
        && dir.parent().is_some_and(|p| p.join("Cargo.toml").is_file())
        && (dir.join("CACHEDIR.TAG").exists()
            || dir.join(".rustc_info.json").exists()
            || dir.join("debug").is_dir()
            || dir.join("release").is_dir())
}

fn skip_dir(name: &str) -> bool {
    matches!(name, ".git" | "node_modules" | ".cargo" | ".rustup" | "$RECYCLE.BIN" | "AppData")
}

/// Walk roots, calling `f` on every directory (not descending into target/ or junk dirs).
pub fn walk_projects(roots: &[PathBuf], mut on_dir: impl FnMut(&Path)) {
    for root in roots {
        let mut it = WalkDir::new(root).follow_links(false).into_iter();
        while let Some(entry) = it.next() {
            let Ok(e) = entry else { continue };
            if !e.file_type().is_dir() {
                continue;
            }
            if e.depth() > 0 && skip_dir(&e.file_name().to_string_lossy()) {
                it.skip_current_dir();
                continue;
            }
            on_dir(e.path());
            if is_cargo_target(e.path()) {
                it.skip_current_dir();
            }
        }
    }
}

pub fn find_targets(roots: &[PathBuf]) -> Vec<Target> {
    let mut paths = Vec::new();
    walk_projects(roots, |d| {
        if is_cargo_target(d) {
            paths.push(d.to_path_buf());
        }
    });
    // Sizing is the slow part: do it in parallel.
    let mut out: Vec<Target> = paths
        .into_par_iter()
        .map(|p| Target {
            size: dir_size(&p),
            last_build: last_build(&p),
            keep: p.parent().is_some_and(|proj| proj.join(KEEP_MARKER).exists()),
            path: p,
        })
        .collect();
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

/// Newest mtime among the target dir's top-level entries (profiles, .rustc_info.json).
fn last_build(p: &Path) -> SystemTime {
    fs::read_dir(p)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.metadata().ok()?.modified().ok())
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

pub fn scan(roots: &[PathBuf]) {
    let ts = find_targets(roots);
    for t in &ts {
        let tag = if t.keep { " [keep]" } else { "" };
        println!("{:>10}  {:>4}d ago  {}{tag}", human(t.size), age_days(t.last_build), t.path.display());
    }
    let total: u64 = ts.iter().map(|t| t.size).sum();
    println!("{} target dirs, {} total", ts.len(), human(total));
}

pub fn clean(roots: &[PathBuf], days: u64, dry_run: bool) -> u64 {
    let mut freed = 0;
    for t in find_targets(roots) {
        if t.keep || age_days(t.last_build) < days {
            continue;
        }
        let got = remove(&t.path, dry_run);
        if got == 0 && t.size > 0 {
            say!("skip (in use?) {}", t.path.display());
            continue;
        }
        say!("{} {} ({})", if dry_run { "would delete" } else { "deleted" }, t.path.display(), human(got));
        freed += got;
    }
    say!("targets: {} {}", if dry_run { "would free" } else { "freed" }, human(freed));
    freed
}
