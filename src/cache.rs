//! ~/.cargo download caches: extracted sources, .crate archives, git checkouts.
//! Everything here is re-downloaded or re-extracted on demand, so it is safe to drop.

use crate::say;
use crate::util::*;
use anyhow::Result;
use std::fs;
use std::path::Path;

pub struct CacheSizes {
    pub registry_src: u64,
    pub registry_cache: u64,
    pub git: u64,
}

pub fn sizes() -> Result<CacheSizes> {
    let h = cargo_home()?;
    Ok(CacheSizes {
        registry_src: dir_size(&h.join("registry").join("src")),
        registry_cache: dir_size(&h.join("registry").join("cache")),
        git: dir_size(&h.join("git")),
    })
}

/// Remove entries two levels down (`<dir>/<index>/<entry>`) older than `days`.
fn prune_level2(dir: &Path, days: u64, dry_run: bool) -> u64 {
    let mut freed = 0;
    for idx in fs::read_dir(dir).into_iter().flatten().flatten() {
        for e in fs::read_dir(idx.path()).into_iter().flatten().flatten() {
            if age_days(mtime(&e.path())) >= days {
                freed += remove(&e.path(), dry_run);
            }
        }
    }
    freed
}

fn prune_level1(dir: &Path, days: u64, dry_run: bool) -> u64 {
    let mut freed = 0;
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        if age_days(mtime(&e.path())) >= days {
            freed += remove(&e.path(), dry_run);
        }
    }
    freed
}

pub fn clean(days: u64, dry_run: bool) -> Result<u64> {
    let h = cargo_home()?;
    let reg = h.join("registry");
    let git = h.join("git");
    let src = prune_level2(&reg.join("src"), days, dry_run);
    let crates = prune_level2(&reg.join("cache"), days, dry_run);
    let checkouts = prune_level2(&git.join("checkouts"), days, dry_run);
    let db = prune_level1(&git.join("db"), days, dry_run);
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
