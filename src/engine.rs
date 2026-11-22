//! High-level operations used by both the GUI and the one-command CLI.

use crate::brain::{self, Brain, Insight, ProjectObs};
use crate::config::Config;
use crate::i18n::{t, tf};
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
    /// AI: chance the project is built again within the horizon
    pub reuse: Option<f64>,
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
    // --- AI ---
    pub insights: Vec<Insight>,
    pub days_to_full: Option<f64>,
    pub pressure: u8,
    pub learned: u64,
    pub accuracy: f64,
    pub stale_artifacts: u64,
    /// set when the saved schedule time differs from what the caller had (smart schedule moved it)
    pub auto_at: Option<String>,
}

impl Snapshot {
    pub fn leftover(&self) -> u64 {
        self.targets.iter().filter(|t| !t.keep).map(|t| t.size).sum()
    }
    /// What "Fix everything" could roughly give back.
    pub fn reclaimable(&self) -> u64 {
        self.leftover() + self.shared_incremental + self.cache_size / 2 + self.stale_artifacts
    }
    pub fn issues(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.shared.is_none() {
            v.push(t("Each project keeps its own target/ folder").into());
        }
        let n = self.targets.iter().filter(|t| !t.keep).count();
        if n > 0 {
            v.push(tf("{} leftover target/ folders ({})", &[&n.to_string(), &human(self.leftover())]));
        }
        if !self.lean {
            v.push(t("Dependencies are built with full debug info").into());
        }
        if !self.scheduled {
            v.push(t("Automatic daily clean-up is off").into());
        }
        for o in &self.overrides {
            v.push(tf("{} sets its own target-dir", &[&o.display().to_string()]));
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

fn project_key(target: &Path) -> String {
    target.parent().unwrap_or(target).display().to_string()
}

fn project_obs(ts: &[targets::Target], now: i64) -> Vec<ProjectObs> {
    ts.iter()
        .map(|t| ProjectObs {
            key: project_key(&t.path),
            last_build: brain::unix(t.last_build),
            size: t.size,
            git_days: brain::git_days(t.path.parent().unwrap_or(&t.path), now),
        })
        .collect()
}

pub fn snapshot(cfg: &Config) -> Snapshot {
    let roots = &cfg.roots;
    let now = brain::now();
    let found = targets::find_targets(roots);
    let obs = project_obs(&found, now);
    let mut overrides = Vec::new();
    targets::walk_projects(roots, |d| overrides.extend(project_override(d)));

    let shared = shared::shared_target_dir().ok().flatten();
    let (shared_size, shared_incremental) = match &shared {
        Some(d) => (dir_size(d), shared::profiles(d).iter().map(|p| dir_size(&p.join("incremental"))).sum()),
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

    let mut snap = Snapshot {
        targets: found
            .iter()
            .map(|t| TargetRow {
                age_days: age_days(t.last_build),
                path: t.path.clone(),
                size: t.size,
                keep: t.keep,
                reuse: None,
            })
            .collect(),
        shared: shared.clone(),
        shared_size,
        shared_incremental,
        lean,
        cache_size,
        toolchains,
        free,
        overrides,
        scheduled: schedule::is_installed(),
        last_run,
        ..Default::default()
    };

    if cfg.smart {
        let mut b = Brain::load();
        let groups = shared.as_deref().map(brain::scan_artifacts).unwrap_or_default();
        b.observe(now, &obs, &groups, free, shared_size);
        let _ = b.save();
        let mut rows = Vec::new();
        for (row, o) in snap.targets.iter_mut().zip(&obs) {
            let p = b.project_reuse(&o.key, o.last_build, o.size, o.git_days, now);
            row.reuse = Some(p);
            if !row.keep {
                let name = row
                    .path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                rows.push((name, row.size, p));
            }
        }
        let extra: Vec<i64> = obs.iter().map(|o| o.last_build).collect();
        let a = brain::analyze(&b, &groups, &rows, free, &extra, &cfg.auto_at, now);
        snap.insights = a.insights;
        snap.days_to_full = a.days_to_full;
        snap.pressure = a.pressure;
        snap.learned = a.learned;
        snap.accuracy = a.accuracy;
        snap.stale_artifacts = a.stale_artifacts;
    }
    let saved_at = Config::load().auto_at;
    if saved_at != cfg.auto_at {
        snap.auto_at = Some(saved_at);
    }
    snap
}

pub struct FixReport {
    pub freed: u64,
    pub warnings: Vec<String>,
}

/// Everything in one go: shared target dir, lean deps, delete leftovers, shrink caches,
/// daily automation. Safe to run again and again.
pub fn fix_all(cfg: &Config) -> Result<FixReport> {
    let mut warnings = Vec::new();
    say!("== fix everything ==");
    shared::ensure_setup(cfg.lean)?;
    let now = brain::now();
    let mut b = cfg.smart.then(Brain::load);
    if let Some(b) = b.as_mut() {
        // record leftovers before they go, so the model knows these projects
        let found = targets::find_targets(&cfg.roots);
        b.observe(now, &project_obs(&found, now), &[], None, 0);
    }
    // With a shared dir, per-project target/ folders are dead weight: remove them all
    // (projects marked with .rtidy-keep are left alone).
    let mut freed = targets::clean(&cfg.roots, 0, false);
    freed += match &b {
        Some(b) => shared::smart_gc(cfg, b, now, false)?,
        None => shared::gc(cfg.gc_days, cfg.max_size(), false)?,
    };
    if cfg.cache_days > 0 {
        freed += cache::clean(cfg.cache_days, false)?;
    }
    if let Some(b) = &b {
        let _ = b.save();
    }
    // Cleaning already happened; a scheduler problem shouldn't turn that into a failure.
    if let Err(e) = apply_schedule(cfg) {
        say!("warning: couldn't set up daily clean-up: {e}");
        warnings.push(tf("couldn't set up daily clean-up ({})", &[&e.to_string()]));
    }
    cfg.save()?;
    say!("done: freed {}", human(freed));
    Ok(FixReport { freed, warnings })
}

/// What the scheduled task runs (no arguments: saved settings).
pub fn auto_from_config(cfg: &Config) -> Result<u64> {
    if !cfg.smart {
        let mut freed = targets::clean(&cfg.roots, cfg.days, false);
        freed += shared::gc(cfg.gc_days, cfg.max_size(), false)?;
        if cfg.cache_days > 0 {
            freed += cache::clean(cfg.cache_days, false)?;
        }
        say!("auto: freed {} total", human(freed));
        return Ok(freed);
    }

    let now = brain::now();
    let mut b = Brain::load();
    let found = targets::find_targets(&cfg.roots);
    let obs = project_obs(&found, now);
    let shared_dir = shared::shared_target_dir()?;
    let groups = shared_dir.as_deref().map(brain::scan_artifacts).unwrap_or_default();
    let free = cfg.roots.first().and_then(|r| free_space(r));
    let shared_size = shared_dir.as_deref().map(dir_size).unwrap_or(0);
    b.observe(now, &obs, &groups, free, shared_size);
    let pressure = b.pressure(free);
    let (_, tau_p, _) = brain::policy(pressure);
    say!("ai: disk pressure {pressure}, {} outcomes learned", b.accuracy().0);

    // leftover project targets: remove the ones the model says won't come back
    let mut freed = 0;
    for (t, o) in found.iter().zip(&obs) {
        if t.keep {
            continue;
        }
        let age = age_days(t.last_build);
        let p = b.project_reuse(&o.key, o.last_build, o.size, o.git_days, now);
        if age >= 3 && (p < tau_p || age >= cfg.days * 2) {
            let got = remove(&t.path, false);
            if got > 0 {
                say!("ai: deleted {} ({}, {:.0}% reuse chance)", t.path.display(), human(got), p * 100.0);
                freed += got;
            }
        }
    }
    freed += shared::smart_gc(cfg, &b, now, false)?;
    if cfg.cache_days > 0 {
        freed += cache::clean(cfg.cache_days, false)?;
    }

    // smart schedule: move the daily run to the hour you build least
    if cfg.smart_schedule && cfg.auto_enabled {
        let extra: Vec<i64> = obs.iter().map(|o| o.last_build).collect();
        let hours = brain::build_hours(&groups, &extra, now);
        if let Some(h) = brain::stats::quietest_hour(&hours, 40) {
            let cur = cfg.auto_at.split(':').next().and_then(|x| x.parse::<i32>().ok()).unwrap_or(-10);
            let dist = (cur - h as i32).rem_euclid(24).min((h as i32 - cur).rem_euclid(24));
            if dist >= 2 {
                let mut c = cfg.clone();
                c.auto_at = format!("{h:02}:17");
                if schedule::install(&c.auto_at, &[]).is_ok() {
                    let _ = c.save();
                    say!("ai: moved daily clean-up to {} (quietest hour)", c.auto_at);
                }
            }
        }
    }
    b.save()?;
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
