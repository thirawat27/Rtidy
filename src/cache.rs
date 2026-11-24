//! ~/.cargo download caches: extracted sources, .crate archives, git checkouts.
//! Everything here is re-downloaded or re-extracted on demand, so it is safe to drop.

use crate::plan::{Action, Kind, Plan};
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

/// Turn candidates into plan actions, measuring the directories among them in parallel
/// (a `.crate` archive already reported its size; an extracted source tree has to be
/// walked, and there are thousands of them).
fn actions(victims: Vec<(PathBuf, Option<u64>)>, days: u64) -> Vec<Action> {
    let why = crate::i18n::tf("downloaded, unused for {} days", &[&days.to_string()]);
    victims
        .into_par_iter()
        .map(|(p, size)| {
            let size = size.unwrap_or_else(|| dir_size(&p));
            let age = fs::metadata(&p).map(|m| age_days(mtime_of(&m))).unwrap_or(days);
            Action::new(Kind::Cache, p, size, crate::eco::cargo(), age, why.clone())
        })
        .collect()
}

/// Candidates two levels down (`<dir>/<index>/<entry>`) older than `days`.
fn stale_level2(dir: &Path, days: u64) -> Vec<(PathBuf, Option<u64>)> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|idx| stale(fs::read_dir(idx.path()).into_iter().flatten().flatten(), days))
        .collect()
}

fn stale_level1(dir: &Path, days: u64) -> Vec<(PathBuf, Option<u64>)> {
    stale(fs::read_dir(dir).into_iter().flatten().flatten(), days)
}

/// What a cache clean-up would remove, without removing it.
pub fn clean_plan(days: u64) -> Result<Plan> {
    let h = cargo_home()?;
    let reg = h.join("registry");
    let git = h.join("git");
    let ((src, crates), (checkouts, db)) = rayon::join(
        || {
            rayon::join(
                || stale_level2(&reg.join("src"), days),
                || stale_level2(&reg.join("cache"), days),
            )
        },
        || {
            rayon::join(
                || stale_level2(&git.join("checkouts"), days),
                || stale_level1(&git.join("db"), days),
            )
        },
    );
    let mut all = src;
    all.extend(crates);
    all.extend(checkouts);
    all.extend(db);
    Ok(Plan { actions: actions(all, days), kept: Vec::new() })
}

pub fn clean(days: u64, dry_run: bool) -> Result<u64> {
    let plan = clean_plan(days)?;
    let mode = if dry_run { crate::plan::Mode::Preview } else { crate::plan::Mode::Delete };
    let out = crate::plan::apply(&plan, mode, "cache");
    say!(
        "cargo cache: {} {} ({} entries)",
        if dry_run { "would free" } else { "freed" },
        human(out.freed),
        out.done
    );
    Ok(out.freed)
}
