//! ~/.cargo download caches: extracted sources, .crate archives, git checkouts.
//! Everything here is re-downloaded or re-extracted on demand, so it is safe to drop.

use crate::say;
use crate::util::*;
use anyhow::Result;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

pub struct CacheSizes {
    pub registry_src: u64,
    pub registry_cache: u64,
    pub git: u64,
}

pub fn sizes() -> Result<CacheSizes> {
    let h = cargo_home()?;
    // Three independent trees, each large enough to be worth its own thread.
    let (registry_src, (registry_cache, git)) = rayon::join(
        || dir_size(&h.join("registry").join("src")),
        || {
            rayon::join(
                || dir_size(&h.join("registry").join("cache")),
                || dir_size(&h.join("git")),
            )
        },
    );
    Ok(CacheSizes { registry_src, registry_cache, git })
}

/// Candidates to drop: path plus, for plain files such as `.crate` archives, the size
/// the directory read already told us. Reading the entry's metadata once covers both the
/// age test and the size, where the old code paid a `stat` and then a full walk.
fn stale(entries: impl Iterator<Item = fs::DirEntry>, days: u64) -> Vec<(PathBuf, Option<u64>)> {
    entries
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            let old = age_days(mtime_of(&md)) >= days;
            old.then(|| (e.path(), md.is_file().then_some(md.len())))
        })
        .collect()
}

/// Deleting is I/O-bound and the entries are independent, so run them in parallel.
fn drop_all(victims: Vec<(PathBuf, Option<u64>)>, dry_run: bool) -> u64 {
    victims
        .into_par_iter()
        .map(|(p, size)| match size {
            Some(s) => remove_known(&p, s, dry_run),
            None => remove(&p, dry_run),
        })
        .sum()
}

/// Remove entries two levels down (`<dir>/<index>/<entry>`) older than `days`.
fn prune_level2(dir: &Path, days: u64, dry_run: bool) -> u64 {
    let victims: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|idx| stale(fs::read_dir(idx.path()).into_iter().flatten().flatten(), days))
        .collect();
    drop_all(victims, dry_run)
}

fn prune_level1(dir: &Path, days: u64, dry_run: bool) -> u64 {
    drop_all(stale(fs::read_dir(dir).into_iter().flatten().flatten(), days), dry_run)
}

pub fn clean(days: u64, dry_run: bool) -> Result<u64> {
    let h = cargo_home()?;
    let reg = h.join("registry");
    let git = h.join("git");
    let ((src, crates), (checkouts, db)) = rayon::join(
        || {
            rayon::join(
                || prune_level2(&reg.join("src"), days, dry_run),
                || prune_level2(&reg.join("cache"), days, dry_run),
            )
        },
        || {
            rayon::join(
                || prune_level2(&git.join("checkouts"), days, dry_run),
                || prune_level1(&git.join("db"), days, dry_run),
            )
        },
    );
    let total = src + crates + checkouts + db;
    say!(
        "cargo cache: {} {} (sources {}, .crate {}, git {})",
        if dry_run { "would free" } else { "freed" },
        human(total),
        human(src),
        human(crates),
        human(checkouts + db)
    );
    Ok(total)
}
