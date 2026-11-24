//! High-level operations used by both the GUI and the one-command CLI.
//!
//! Everything destructive goes through one shape now: build a `crate::plan::Plan`, then
//! hand it to `crate::plan::apply`. That is what lets the app show a preview, the CLI
//! print `--json`, a CI job read an exit code, and `rtidy undo` exist at all — before,
//! each step chose and deleted and printed in one breath, and there was no moment where
//! rtidy knew what it was about to do.

use crate::brain::{self, Brain, Insight, ProjectObs};
use crate::config::Config;
use crate::eco::Eco;
use crate::i18n::{t, tf};
use crate::ledger::Ledger;
use crate::plan::{Kind, Mode, Plan};
use crate::util::*;
use crate::{cache, say, schedule, shared, stores, targets, trash};
use anyhow::Result;
use rayon::prelude::*;
use std::path::{Path, PathBuf};

#[derive(Clone)]
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub struct TargetRow {
    pub path: PathBuf,
    pub size: u64,
    pub age_days: u64,
    pub keep: bool,
    /// AI: chance the project is built again within the horizon
    pub reuse: Option<f64>,
    /// Which ecosystem's output this is (`crate::eco` id).
    pub eco: &'static str,
    /// Human label for that ecosystem.
    pub eco_label: &'static str,
    /// Estimated seconds to build it again.
    pub rebuild_secs: f64,
    /// The project said so itself, in `.rtidy.toml`.
    pub rules_from: Option<PathBuf>,
}

#[derive(Clone, Default)]
// Most of this is the dashboard's view of the machine; a CLI-only build fills it in
// but only reads a few fields back.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
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
    /// The installed task starts a program that no longer exists (rtidy was moved).
    pub schedule_broken: Option<PathBuf>,
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
    // --- other languages, trash, history ---
    /// Per-user package stores of the enabled ecosystems: label, path, size.
    pub stores: Vec<(String, PathBuf, u64)>,
    /// Bytes sitting in the trash, recoverable.
    pub trash_size: u64,
    /// `"3 items, 1.2 GB — recoverable"`, when there is anything to say.
    pub trash_summary: Option<String>,
    /// Leftover bytes per ecosystem id, largest first.
    pub by_eco: Vec<(String, u64)>,
    /// What rtidy has freed on this machine over time.
    pub ledger: Ledger,
}

impl Snapshot {
    // `leftover`, `reclaimable` and `issues` are what the dashboard summarises.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn leftover(&self) -> u64 {
        self.targets.iter().filter(|t| !t.keep).map(|t| t.size).sum()
    }
    /// What "Fix everything" could roughly give back.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub fn reclaimable(&self) -> u64 {
        self.leftover() + self.shared_incremental + self.cache_size / 2 + self.stale_artifacts
    }
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
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

/// The observation for the project an artifact dir belongs to.
fn obs_for<'a>(obs: &'a [ProjectObs], target: &Path) -> Option<&'a ProjectObs> {
    let dir = target.parent()?;
    let key = project_key(dir);
    obs.iter().find(|o| o.key == key)
}

/// What the model gets to see about each project.
///
/// Every directory holding a manifest is reported, whether or not it still has build
/// output beside it. Deriving this from leftovers alone meant that once rtidy had done
/// its job there was nothing left to observe, and the model stayed at zero outcomes
/// forever.
fn project_obs(projects: &[PathBuf], ts: &[targets::Target], now: i64) -> Vec<ProjectObs> {
    // Size and last build come from the project's own artifact dir when one survives.
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
        // With no artifact dir left, the manifest's own timestamp is the best stand-in
        // for "when was this last worked on"; `git_days` carries the rest.
        let last_build = found.map(|t| brain::unix(t.last_build)).unwrap_or_else(|| {
            manifest_mtime(dir).unwrap_or(0)
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

/// When any of this project's manifests was last written.
fn manifest_mtime(dir: &Path) -> Option<i64> {
    crate::eco::ALL
        .iter()
        .flat_map(|e| e.manifests.iter())
        .filter(|m| !m.starts_with("*.") && !m.contains('/'))
        .filter_map(|m| std::fs::metadata(dir.join(m)).and_then(|md| md.modified()).ok())
        .map(brain::unix)
        .max()
}

pub fn snapshot(cfg: &Config) -> Snapshot {
    let roots = &cfg.roots;
    let now = brain::now();
    let shared = shared::shared_target_dir().ok().flatten();
    let ecos = cfg.ecos();

    // Independent scans over different parts of the disk, one of them a subprocess
    // (`schtasks` on Windows). Run together, the dashboard waits for the slowest
    // instead of for their sum.
    #[allow(clippy::type_complexity)]
    let (
        found,
        overrides,
        project_dirs,
        (shared_size, shared_incremental),
        cache_size,
        toolchains,
        (scheduled, schedule_broken),
        groups,
        store_list,
    ) = std::thread::scope(|s| {
        // One pass over the project tree yields the artifact dirs, the projects and the
        // per-project target-dir overrides.
        let projects = s.spawn(|| {
            let scan = targets::scan_tree(roots, &ecos);
            (targets::size_targets(scan.artifacts), scan.overrides, scan.projects)
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
        let sched = s.spawn(|| {
            let on = schedule::is_installed();
            (on, if on { schedule::installed_program().filter(|p| !p.exists()) } else { None })
        });
        // Smart mode needs the build units too, and that is another pass over the shared
        // dir: run it alongside the rest rather than after them.
        let units = s.spawn(|| match (cfg.smart, shared.as_deref()) {
            (true, Some(d)) => brain::scan_artifacts(d),
            _ => Vec::new(),
        });
        // Only worth walking when other languages are on: with Rust alone the one store
        // is `~/.cargo`, which `cache_size` already covers.
        let st = s.spawn(|| match cfg.all_langs {
            true => stores::survey(&cfg.ecos())
                .into_iter()
                .map(|f| (f.store.label.to_string(), f.path, f.size))
                .collect(),
            false => Vec::new(),
        });

        let (found, overrides, project_dirs) = projects.join().unwrap_or_default();
        (
            found,
            overrides,
            project_dirs,
            shared_sizes.join().unwrap_or((0, 0)),
            cache.join().unwrap_or(0),
            tcs.join().unwrap_or_default(),
            sched.join().unwrap_or((false, None)),
            units.join().unwrap_or_default(),
            st.join().unwrap_or_default(),
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

    let mut by_eco: std::collections::BTreeMap<&'static str, u64> = Default::default();
    for t in found.iter().filter(|t| !t.keep) {
        *by_eco.entry(t.eco.id).or_default() += t.size;
    }
    let mut by_eco: Vec<(String, u64)> = by_eco.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    by_eco.sort_unstable_by_key(|(_, b)| std::cmp::Reverse(*b));

    let mut snap = Snapshot {
        targets: found
            .iter()
            .map(|t| TargetRow {
                age_days: age_days(t.last_build),
                path: t.path.clone(),
                size: t.size,
                keep: t.keep,
                reuse: None,
                eco: t.eco.id,
                eco_label: t.eco.label,
                rebuild_secs: t.eco.rebuild_secs(t.size),
                rules_from: t.rules.from.clone(),
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
        schedule_broken,
        last_run,
        stores: store_list,
        trash_size: trash::size(),
        trash_summary: trash::summary(),
        by_eco,
        ledger: Ledger::load(),
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

// The app shows these; the CLI prints its own progress as it goes.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub struct FixReport {
    pub freed: u64,
    /// Moved aside and recoverable until the trash is purged.
    pub quarantined: u64,
    pub warnings: Vec<String>,
}

/// Everything that would be removed, with nothing removed.
///
/// This is the preview the app shows, what `--dry-run` prints and what `--json` hands a
/// CI job. `days` of `Some(0)` means "every leftover", which is what `fix` does once the
/// shared dir makes per-project artifacts dead weight.
pub fn plan_all(cfg: &Config, days: u64, ecos: &[&'static Eco]) -> Plan {
    let mut plan = Plan::default();
    let now = brain::now();
    let scan = targets::scan_tree(&cfg.roots, ecos);
    let found = targets::size_targets(scan.artifacts);

    let mut artifacts = targets::plan(&found, days);
    // Smart mode has an opinion about each project; attach it so the preview can show
    // the reuse chance and the ranking can use it.
    if cfg.smart {
        let b = Brain::load();
        let obs = project_obs(&scan.projects, &found, now);
        for a in artifacts.actions.iter_mut() {
            if let Some(o) = obs_for(&obs, &a.path) {
                a.reuse = Some(b.project_reuse(&o.key, o.last_build, o.size, o.git_days, now));
            }
        }
    }
    plan.extend(artifacts);

    let shared_plan = match cfg.smart {
        true => shared::smart_gc_plan(cfg, &Brain::load(), now),
        false => shared::gc_plan(cfg.gc_days, cfg.max_size()),
    };
    if let Ok(p) = shared_plan {
        plan.extend(p);
    }
    if cfg.cache_days > 0
        && let Ok(p) = cache::clean_plan(cfg.cache_days)
    {
        plan.extend(p);
    }
    if cfg.all_langs && cfg.store_days > 0 {
        plan.extend(stores::plan(ecos, cfg.store_days));
    }
    // A child of something already going would be deleted twice and counted twice.
    let actions = plan.actions.clone();
    plan.actions.retain(|a| !crate::plan::covered(&a.path, &actions));
    plan.rank();
    plan
}

/// Everything in one go: shared target dir, lean deps, delete leftovers, shrink caches,
/// daily automation. Safe to run again and again.
pub fn fix_all(cfg: &Config) -> Result<FixReport> {
    let mut warnings = Vec::new();
    say!("== fix everything ==");
    shared::ensure_setup(cfg.lean)?;
    // Expired quarantine first: this is where the space from the last run is handed
    // back, and doing it before the plan keeps the free-space figures honest.
    trash::purge(cfg.trash_days);

    let now = brain::now();
    let ecos = cfg.ecos();
    if cfg.all_langs {
        let names: Vec<&str> = ecos.iter().map(|e| e.label).collect();
        say!("languages: {}", names.join(", "));
    }
    let mut b = cfg.smart.then(Brain::load);
    if let Some(b) = b.as_mut() {
        // Record the projects before their leftovers go, so the model keeps knowing
        // about them once the artifact dirs are gone.
        let scan = targets::scan_tree(&cfg.roots, &ecos);
        let found = targets::size_targets(scan.artifacts);
        b.observe(now, &project_obs(&scan.projects, &found, now), &[], None, 0);
    }

    // With a shared dir, per-project `target/` folders are dead weight: age floor 0.
    let plan = plan_all(cfg, 0, &ecos);
    let mode = if cfg.trash_days == 0 { Mode::Delete } else { Mode::Trash };
    let out = crate::plan::apply(&plan, mode, "fix");
    for (eco, bytes) in plan.by_eco() {
        let label = crate::eco::by_id(&eco).map(|e| e.label).unwrap_or("other");
        say!("  {label}: {}", human(bytes));
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
    if out.quarantined > 0 {
        say!(
            "done: freed {}, {} recoverable for {} day(s) (`rtidy undo` puts it back)",
            human(out.freed),
            human(out.quarantined),
            cfg.trash_days
        );
    } else {
        say!("done: freed {}", human(out.freed));
    }
    Ok(FixReport { freed: out.freed, quarantined: out.quarantined, warnings })
}

/// What the scheduled task runs (no arguments: saved settings).
pub fn auto_from_config(cfg: &Config) -> Result<u64> {
    trash::purge(cfg.trash_days);
    let ecos = cfg.ecos();

    if !cfg.smart {
        let plan = plan_all(cfg, cfg.days, &ecos);
        let mode = if cfg.trash_days == 0 { Mode::Delete } else { Mode::Trash };
        let out = crate::plan::apply(&plan, mode, "auto");
        say!("auto: freed {} total", human(out.freed + out.quarantined));
        return Ok(out.freed + out.quarantined);
    }

    let now = brain::now();
    let mut b = Brain::load();
    let scan = targets::scan_tree(&cfg.roots, &ecos);
    let found = targets::size_targets(scan.artifacts);
    let obs = project_obs(&scan.projects, &found, now);
    let shared_dir = shared::shared_target_dir()?;
    let groups = shared_dir.as_deref().map(brain::scan_artifacts).unwrap_or_default();
    let free = cfg.roots.first().and_then(|r| free_space(r));
    let shared_size = shared_dir.as_deref().map(dir_size).unwrap_or(0);
    b.observe(now, &obs, &groups, free, shared_size);
    let pressure = b.pressure(free);
    let (_, tau_p, _) = brain::policy(pressure);
    say!("ai: disk pressure {pressure}, {} outcomes learned", b.accuracy().0);

    // Leftover project artifacts: the ones the model says won't come back.
    let mut plan = Plan::default();
    for t in found.iter() {
        if t.keep {
            continue;
        }
        let Some(o) = obs_for(&obs, &t.path) else { continue };
        let age = age_days(t.last_build);
        let floor = t.rules.days.unwrap_or(cfg.days);
        let p = b.project_reuse(&o.key, o.last_build, o.size, o.git_days, now);
        if age >= 3 && (p < tau_p || age >= floor * 2) {
            let why = tf("{}% chance you build it again", &[&format!("{:.0}", p * 100.0)]);
            let mut a = crate::plan::Action::new(Kind::Artifact, t.path.clone(), t.size, t.eco, age, why);
            a.reuse = Some(p);
            plan.push(a);
        }
    }
    plan.extend(shared::smart_gc_plan(cfg, &b, now)?);
    if cfg.cache_days > 0 {
        plan.extend(cache::clean_plan(cfg.cache_days)?);
    }
    if cfg.all_langs && cfg.store_days > 0 {
        plan.extend(stores::plan(&ecos, cfg.store_days));
    }
    plan.rank();
    // A disk this tight cannot afford a few days of quarantine; say so and delete.
    let mode = if cfg.trash_days == 0 || pressure >= 3 {
        if pressure >= 3 && cfg.trash_days > 0 {
            say!("ai: disk critical, deleting outright instead of keeping a recoverable copy");
        }
        Mode::Delete
    } else {
        Mode::Trash
    };
    let out = crate::plan::apply(&plan, mode, "auto");

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
    let total = out.freed + out.quarantined;
    say!("auto: freed {} total", human(total));
    Ok(total)
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
