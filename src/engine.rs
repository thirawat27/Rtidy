//! High-level operations used by both the GUI and the one-command CLI.

use crate::config::Config;
use crate::util::*;
use crate::{cache, say, schedule, shared, targets};
use anyhow::Result;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct TargetRow {
    pub path: PathBuf,
    pub size: u64,
    pub age_days: u64,
    pub keep: bool,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub targets: Vec<TargetRow>,
    pub shared: Option<PathBuf>,
    pub shared_size: u64,
    pub shared_incremental: u64,
    pub lean: bool,
    pub cache_size: u64,
    pub toolchains: Vec<(String, u64)>,
    pub free: Option<u64>,
    pub overrides: Vec<PathBuf>,
    pub scheduled: bool,
    pub last_run: Option<String>,
}

impl Snapshot {
    pub fn leftover(&self) -> u64 {
        self.targets.iter().filter(|t| !t.keep).map(|t| t.size).sum()
    }
    /// What "Fix everything" could roughly give back.
    pub fn reclaimable(&self) -> u64 {
        self.leftover() + self.shared_incremental + self.cache_size / 2
    }
    pub fn issues(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.shared.is_none() {
            v.push("Each project keeps its own target/ folder".into());
        }
        let n = self.targets.iter().filter(|t| !t.keep).count();
        if n > 0 {
            v.push(format!("{n} leftover target/ folder{} ({})", if n == 1 { "" } else { "s" }, human(self.leftover())));
        }
        if !self.lean {
            v.push("Dependencies are built with full debug info".into());
        }
        if !self.scheduled {
            v.push("Automatic daily clean-up is off".into());
        }
        for o in &self.overrides {
            v.push(format!("{} sets its own target-dir", o.display()));
        }
        v
    }
}

fn project_override(dir: &Path) -> Option<PathBuf> {
    for name in ["config.toml", "config"] {
        let p = dir.join(".cargo").join(name);
        let Ok(text) = fs::read_to_string(&p) else { continue };
        let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { continue };
        if doc.get("build").and_then(|b| b.get("target-dir")).is_some() {
            return Some(p);
        }
    }
    None
}

pub fn snapshot(cfg: &Config) -> Snapshot {
    let roots = &cfg.roots;
    let targets = targets::find_targets(roots)
        .into_iter()
        .map(|t| TargetRow { age_days: age_days(t.last_build), path: t.path, size: t.size, keep: t.keep })
        .collect();
    let mut overrides = Vec::new();
    targets::walk_projects(roots, |d| overrides.extend(project_override(d)));

    let shared = shared::shared_target_dir().ok().flatten();
    let (shared_size, shared_incremental) = match &shared {
        Some(d) => (
            dir_size(d),
            shared::profiles(d).iter().map(|p| dir_size(&p.join("incremental"))).sum(),
        ),
        None => (0, 0),
    };
    let lean = read_cargo_config().map(|(_, _, d)| shared::is_lean(&d)).unwrap_or(false);
    let cache_size = cache::sizes().map(|c| c.registry_src + c.registry_cache + c.git).unwrap_or(0);

    let tc_dir = rustup_home().map(|h| h.join("toolchains")).unwrap_or_default();
    let tc_paths: Vec<PathBuf> = fs::read_dir(&tc_dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    let mut toolchains: Vec<(String, u64)> = tc_paths
        .into_par_iter()
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), dir_size(&p)))
        .collect();
    toolchains.sort_by(|a, b| b.1.cmp(&a.1));

    let free = roots.first().and_then(|r| free_space(r)).or_else(|| dirs::home_dir().and_then(|h| free_space(&h)));
    let last_run = log_path()
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|t| t.lines().rev().find(|l| l.contains("auto:")).map(str::to_string));

    Snapshot {
        targets,
        shared,
        shared_size,
        shared_incremental,
        lean,
        cache_size,
        toolchains,
        free,
        overrides,
        scheduled: schedule::is_installed(),
        last_run,
    }
}

/// Everything in one go: shared target dir, lean deps, delete leftovers, shrink caches,
/// daily automation. Safe to run again and again.
pub struct FixReport {
    pub freed: u64,
    pub warnings: Vec<String>,
}

pub fn fix_all(cfg: &Config) -> Result<FixReport> {
    let mut warnings = Vec::new();
    say!("== fix everything ==");
    shared::ensure_setup(cfg.lean)?;
    // With a shared dir, per-project target/ folders are dead weight: remove them all
    // (projects marked with .rtidy-keep are left alone).
    let mut freed = targets::clean(&cfg.roots, 0, false);
    freed += shared::gc(cfg.gc_days, cfg.max_size(), false)?;
    if cfg.cache_days > 0 {
        freed += cache::clean(cfg.cache_days, false)?;
    }
    // Cleaning already happened; a scheduler problem shouldn't turn that into a failure.
    if let Err(e) = apply_schedule(cfg) {
        say!("warning: couldn't set up daily clean-up: {e}");
        warnings.push(format!("couldn't set up daily clean-up ({e})"));
    }
    cfg.save()?;
    say!("done: freed {}", human(freed));
    Ok(FixReport { freed, warnings })
}

/// What the scheduled task runs (no arguments: saved settings).
pub fn auto_from_config(cfg: &Config) -> Result<u64> {
    let mut freed = targets::clean(&cfg.roots, cfg.days, false);
    freed += shared::gc(cfg.gc_days, cfg.max_size(), false)?;
    if cfg.cache_days > 0 {
        freed += cache::clean(cfg.cache_days, false)?;
    }
    say!("auto: freed {} total", human(freed));
    Ok(freed)
}

pub fn apply_schedule(cfg: &Config) -> Result<()> {
    if cfg.auto_enabled {
        schedule::install(&cfg.auto_at, &[])?;
        say!("automatic clean-up: every day at {}", cfg.auto_at);
    } else {
        schedule::remove()?;
        say!("automatic clean-up: off");
    }
    Ok(())
}
