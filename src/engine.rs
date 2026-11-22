//! High-level operations used by both the GUI and the one-command CLI.

use crate::brain::{self, Brain, Insight, ProjectObs};
use crate::config::Config;
use crate::i18n::{t, tf};
use crate::util::*;
use crate::{cache, say, schedule, shared, targets};
use anyhow::Result;
use rayon::prelude::*;
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

/// The key a project is known by: its directory.
fn project_key(dir: &Path) -> String {
    dir.display().to_string()
}

/// The observation for the project a target dir belongs to.
///
/// Observations are per project now, and a project need not have a target dir, so
/// the two lists cannot be walked side by side any more.
fn obs_for<'a>(obs: &'a [ProjectObs], target: &Path) -> Option<&'a ProjectObs> {
    let dir = target.parent()?;
    let key = project_key(dir);
    obs.iter().find(|o| o.key == key)
}

/// What the model gets to see about each project.
///
/// Every directory holding a `Cargo.toml` is reported, whether or not it still has a
/// `target/` beside it. Deriving this from leftover target dirs alone meant that once
/// rtidy had done its job there was nothing left to observe, and the model stayed at
/// zero outcomes forever.
fn project_obs(projects: &[PathBuf], ts: &[targets::Target], now: i64) -> Vec<ProjectObs> {
    // Size and last build come from the project's own target dir when one survives.
    let by_dir: std::collections::HashMap<&Path, &targets::Target> =
        ts.iter().filter_map(|t| t.path.parent().map(|d| (d, t))).collect();

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for dir in projects {
        let key = project_key(dir);
        if !seen.insert(key.clone()) {
            continue; // a workspace can list the same directory twice
        }
        let found = by_dir.get(dir.as_path());
        let git_days = brain::git_days(dir, now);
        // With no target dir left, the manifest's own timestamp is the best stand-in
        // for "when was this last worked on"; `git_days` carries the rest.
        let last_build = found.map(|t| brain::unix(t.last_build)).unwrap_or_else(|| {
            std::fs::metadata(dir.join("Cargo.toml"))
                .and_then(|m| m.modified())
                .map(brain::unix)
                .unwrap_or(0)
        });
        out.push(ProjectObs {
            key,
            last_build,
            size: found.map(|t| t.size).unwrap_or(0),
            git_days,
        });
    }
    out
}

pub fn snapshot(cfg: &Config) -> Snapshot {
    let roots = &cfg.roots;
    let now = brain::now();
    let shared = shared::shared_target_dir().ok().flatten();

    // Five independent scans over different parts of the disk, one of them a
    // subprocess (`schtasks` on Windows). Run together, the dashboard waits for the
    // slowest instead of for their sum.
    #[allow(clippy::type_complexity)]
    let (found, overrides, project_dirs, (shared_size, shared_incremental), cache_size, toolchains, scheduled, groups) =
        std::thread::scope(|s| {
            // One pass over the project tree yields both the target dirs and the
            // per-project target-dir overrides.
            let projects = s.spawn(|| {
                let scan = targets::scan_tree(roots);
                (targets::size_targets(scan.targets), scan.overrides, scan.projects)
            });
            let shared_sizes = s.spawn(|| match &shared {
                Some(d) => {
                    let profs = shared::profiles(d);
                    rayon::join(
                        || dir_size(d),
                        || profs.par_iter().map(|p| dir_size(&p.join("incremental"))).sum::<u64>(),
                    )
                }
                None => (0, 0),
            });
            let cache = s.spawn(|| cache::sizes().map(|c| c.registry_src + c.registry_cache + c.git).unwrap_or(0));
            let tcs = s.spawn(shared::toolchain_sizes);
            let sched = s.spawn(schedule::is_installed);
            // Smart mode needs the build units too, and that is another pass over the
            // shared dir: run it alongside the rest rather than after them.
            let units = s.spawn(|| match (cfg.smart, shared.as_deref()) {
                (true, Some(d)) => brain::scan_artifacts(d),
                _ => Vec::new(),
            });

            let (found, overrides, project_dirs) = projects.join().unwrap_or_default();
            (
                found,
                overrides,
                project_dirs,
                shared_sizes.join().unwrap_or((0, 0)),
                cache.join().unwrap_or(0),
                tcs.join().unwrap_or_default(),
                sched.join().unwrap_or(false),
                units.join().unwrap_or_default(),
            )
        });

    let obs = project_obs(&project_dirs, &found, now);
    let lean = read_cargo_config().map(|(_, _, d)| shared::is_lean(&d)).unwrap_or(false);
    let free = roots.first().and_then(|r| free_space(r)).or_else(|| dirs::home_dir().and_then(|h| free_space(&h)));
    // Only the tail matters, and the log outlives many runs.
    let last_run = log_path()
        .ok()
        .map(|p| read_tail(&p, 64 << 10))
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
        scheduled,
        last_run,
        ..Default::default()
    };

    if cfg.smart {
        let mut b = Brain::load();
        b.observe(now, &obs, &groups, free, shared_size);
        let _ = b.save();
        let mut rows = Vec::new();
        for row in snap.targets.iter_mut() {
            let Some(o) = obs_for(&obs, &row.path) else { continue };
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
    // One scan feeds both the model and the clean-up; it used to be walked and sized
    // twice whenever smart mode was on.
    let scan = targets::scan_tree(&cfg.roots);
    let project_dirs = scan.projects;
    let found = targets::size_targets(scan.targets);
    if let Some(b) = b.as_mut() {
        // Record the projects before their leftovers go, so the model keeps knowing
        // about them once the target dirs are gone.
        b.observe(now, &project_obs(&project_dirs, &found, now), &[], None, 0);
    }
    // With a shared dir, per-project target/ folders are dead weight: remove them all
    // (projects marked with .rtidy-keep are left alone).
    let mut freed = targets::clean_found(found, 0, false);
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
    let scan = targets::scan_tree(&cfg.roots);
    let project_dirs = scan.projects;
    let found = targets::size_targets(scan.targets);
    let obs = project_obs(&project_dirs, &found, now);
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
    for t in found.iter() {
        if t.keep {
            continue;
        }
        let Some(o) = obs_for(&obs, &t.path) else { continue };
        let age = age_days(t.last_build);
        let p = b.project_reuse(&o.key, o.last_build, o.size, o.git_days, now);
        if age >= 3 && (p < tau_p || age >= cfg.days * 2) {
            let got = remove_known(&t.path, t.size, false);
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
