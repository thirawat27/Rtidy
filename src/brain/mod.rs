//! rtidy's "brain": lightweight, on-device machine learning for disk management.
//!
//! - Reuse prediction: online logistic regression estimates, for every build artifact in
//!   the shared folder (and every leftover project target/), the chance it will be used
//!   again within 14 days. It learns from what actually happens: an artifact that gets
//!   rebuilt after rtidy removed it teaches the model it was wrong.
//! - Disk forecast: exponentially-weighted regression on free space → days until full.
//!   Cleaning gets stricter automatically as the disk gets tight.
//! - Growth anomalies: median/MAD test on daily growth of the shared folder.
//! - Quiet hours: when you never build, so the daily clean-up runs then.
//!
//! Everything is a few hundred floats in a JSON file; analysis takes milliseconds.

pub mod model;
pub mod stats;

use crate::i18n::{t, tf};
use crate::util::{dir_size, human};
use anyhow::{Context, Result};
use chrono::{Local, TimeZone, Timelike};
use model::Logit;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DAY: f64 = 86_400.0;
/// "Will it be used again within this many days?"
pub const HORIZON_DAYS: f64 = 14.0;

pub fn now() -> i64 {
    unix(SystemTime::now())
}
pub fn unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
fn days(a: i64, b: i64) -> f64 {
    ((b - a) as f64 / DAY).max(0.0)
}

// ---------------- state ----------------

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Item {
    pub last_use: i64,
    pub uses: u32,
    pub first_seen: i64,
    /// EWMA of days between uses
    pub interval: f64,
    /// last time it was present on disk
    pub seen: i64,
    pub pending: Option<Pending>,
}

/// A prediction-time snapshot waiting for its outcome (used again within the horizon?).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Pending {
    pub t0: i64,
    pub last_use: i64,
    pub x: Vec<f64>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct DiskPoint {
    pub t: i64,
    pub free: u64,
    pub shared: u64,
}

#[derive(Serialize, Deserialize)]
pub struct Brain {
    pub projects: HashMap<String, Item>,
    pub artifacts: HashMap<String, Item>,
    pub project_model: Logit,
    pub artifact_model: Logit,
    pub disk: Vec<DiskPoint>,
}

// Prior weights (used until the models have learned your habits).
// project:  [bias, ln(1+days since build), ln(1+builds), ln(1+typical gap), ln(1+days since git activity), ln(1+GB)]
const PROJECT_PRIOR: [f64; 6] = [2.0, -1.1, 0.4, -0.3, -0.5, 0.0];
// artifact: [bias, ln(1+days since use), ln(1+uses), ln(1+age days), ln(1+MB)]
const ARTIFACT_PRIOR: [f64; 5] = [1.2, -1.2, 0.6, -0.15, 0.0];

impl Default for Brain {
    fn default() -> Self {
        Self {
            projects: HashMap::new(),
            artifacts: HashMap::new(),
            project_model: Logit::with_prior(&PROJECT_PRIOR),
            artifact_model: Logit::with_prior(&ARTIFACT_PRIOR),
            disk: Vec::new(),
        }
    }
}

// ---------------- observations ----------------

pub struct ProjectObs {
    pub key: String,
    pub last_build: i64,
    pub size: u64,
    pub git_days: f64,
}

/// One build unit in the shared folder (its .fingerprint, deps and build files together).
pub struct Group {
    pub key: String,
    pub paths: Vec<PathBuf>,
    pub size: u64,
    pub last_use: i64,
}

fn used_time_file(md: &fs::Metadata) -> i64 {
    // atime catches "used but already up to date" builds where the OS records it
    let m = md.modified().map(unix).unwrap_or(0);
    let a = md.accessed().map(unix).unwrap_or(0);
    m.max(a)
}

/// Group shared-folder artifacts by build unit. Directories contribute only their mtime
/// (listing them ourselves would otherwise bump their access time).
pub fn scan_artifacts(shared: &Path) -> Vec<Group> {
    let mut groups: HashMap<String, Group> = HashMap::new();
    for prof in crate::shared::profiles(shared) {
        let pname = prof.strip_prefix(shared).unwrap_or(&prof).to_string_lossy().replace('\\', "/");
        let fp: HashSet<String> = fs::read_dir(prof.join(".fingerprint"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        for sub in [".fingerprint", "deps", "build"] {
            for e in fs::read_dir(prof.join(sub)).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let stem = name.split('.').next().unwrap_or(&name).to_string();
                let unit = if fp.contains(&stem) {
                    stem
                } else if stem.starts_with("lib") && fp.contains(&stem[3..]) {
                    stem[3..].to_string()
                } else {
                    stem
                };
                let Ok(md) = e.metadata() else { continue };
                let path = e.path();
                let (size, last) = if md.is_dir() {
                    let mut t = md.modified().map(unix).unwrap_or(0);
                    for c in fs::read_dir(&path).into_iter().flatten().flatten() {
                        if let Ok(cm) = c.metadata() {
                            t = t.max(if cm.is_dir() { cm.modified().map(unix).unwrap_or(0) } else { used_time_file(&cm) });
                        }
                    }
                    (dir_size(&path), t)
                } else {
                    (md.len(), used_time_file(&md))
                };
                let key = format!("{pname}/{unit}");
                let g = groups.entry(key.clone()).or_insert_with(|| Group { key, paths: vec![], size: 0, last_use: 0 });
                g.paths.push(path);
                g.size += size;
                g.last_use = g.last_use.max(last);
            }
        }
    }
    groups.into_values().collect()
}

/// Days since the project's git repo last changed (commit, checkout, staging).
pub fn git_days(project: &Path, now: i64) -> f64 {
    let git = project.join(".git");
    let t = ["index", "HEAD", "FETCH_HEAD"]
        .iter()
        .filter_map(|f| fs::metadata(git.join(f)).ok()?.modified().ok())
        .map(unix)
        .max();
    match t {
        Some(t) => days(t, now),
        None => fs::metadata(project.join("Cargo.toml"))
            .and_then(|m| m.modified())
            .map(|m| days(unix(m), now))
            .unwrap_or(365.0),
    }
}

fn ln1p(x: f64) -> f64 {
    x.max(0.0).ln_1p()
}

fn project_x(it: &Item, now: i64, size: u64, git_days: f64) -> Vec<f64> {
    let gap = if it.uses >= 2 { it.interval } else { 30.0 };
    vec![
        1.0,
        ln1p(days(it.last_use, now)),
        ln1p(it.uses as f64),
        ln1p(gap),
        ln1p(git_days),
        ln1p(size as f64 / 1e9),
    ]
}

fn artifact_x(it: &Item, now: i64, size: u64) -> Vec<f64> {
    vec![
        1.0,
        ln1p(days(it.last_use, now)),
        ln1p(it.uses as f64),
        ln1p(days(it.first_seen, now)),
        ln1p(size as f64 / 1e6),
    ]
}

/// Update one item with a fresh observation; learn from any matured prediction.
fn update(map: &mut HashMap<String, Item>, model: &mut Logit, key: &str, last_use: i64, now: i64, x_of: impl Fn(&Item) -> Vec<f64>) {
    let it = map.entry(key.to_string()).or_insert_with(|| Item {
        last_use,
        uses: 1,
        first_seen: now.min(last_use.max(1)),
        interval: 0.0,
        seen: now,
        pending: None,
    });
    if last_use > it.last_use + 3600 {
        let gap = days(it.last_use, last_use);
        it.interval = if it.uses <= 1 { gap } else { 0.7 * it.interval + 0.3 * gap };
        it.uses += 1;
        it.last_use = last_use;
    }
    it.seen = now;
    if let Some(p) = &it.pending {
        let reused = it.last_use > p.last_use + 3600;
        let matured = days(p.t0, now) >= HORIZON_DAYS;
        if reused || matured {
            let within = reused && days(p.t0, it.last_use) <= HORIZON_DAYS + 1.0;
            model.learn(&p.x.clone(), within);
            it.pending = None;
        }
    }
    if it.pending.is_none() {
        it.pending = Some(Pending { t0: now, last_use: it.last_use, x: x_of(it) });
    }
}

impl Brain {
    fn path() -> Result<PathBuf> {
        Ok(dirs::data_dir().context("no data dir")?.join("rtidy").join("brain.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .ok()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let p = Self::path()?;
        fs::create_dir_all(p.parent().unwrap())?;
        let tmp = p.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string(self)?)?;
        fs::rename(tmp, p)?;
        Ok(())
    }

    /// Feed today's picture of the disk. Cheap; safe to call often.
    pub fn observe(&mut self, now: i64, projects: &[ProjectObs], artifacts: &[Group], free: Option<u64>, shared_size: u64) {
        for p in projects {
            let (size, gd) = (p.size, p.git_days);
            update(&mut self.projects, &mut self.project_model, &p.key, p.last_build, now, |it| project_x(it, now, size, gd));
        }
        for g in artifacts {
            let size = g.size;
            update(&mut self.artifacts, &mut self.artifact_model, &g.key, g.last_use, now, |it| artifact_x(it, now, size));
        }
        // forget things gone for a month (keeping them that long lets a rebuild after
        // eviction count as "it was needed after all")
        let cutoff = now - 30 * 86_400;
        self.projects.retain(|_, it| it.seen >= cutoff);
        self.artifacts.retain(|_, it| it.seen >= cutoff);

        if let Some(free) = free {
            let pt = DiskPoint { t: now, free, shared: shared_size };
            match self.disk.last_mut() {
                Some(last) if now - last.t < 6 * 3600 => *last = pt,
                _ => self.disk.push(pt),
            }
            if self.disk.len() > 400 {
                self.disk.drain(..self.disk.len() - 400);
            }
        }
    }

    pub fn artifact_reuse(&self, g: &Group, now: i64) -> f64 {
        let it = self.artifacts.get(&g.key).cloned().unwrap_or(Item {
            last_use: g.last_use,
            uses: 1,
            first_seen: g.last_use,
            ..Default::default()
        });
        self.artifact_model.predict(&artifact_x(&it, now, g.size))
    }

    pub fn project_reuse(&self, key: &str, last_build: i64, size: u64, git_days: f64, now: i64) -> f64 {
        let it = self.projects.get(key).cloned().unwrap_or(Item {
            last_use: last_build,
            uses: 1,
            first_seen: last_build,
            ..Default::default()
        });
        self.project_model.predict(&project_x(&it, now, size, git_days))
    }

    /// Free-space trend: (slope bytes/day, days until full).
    pub fn forecast(&self) -> (Option<f64>, Option<f64>) {
        let Some(last) = self.disk.last() else { return (None, None) };
        let recent: Vec<(f64, f64)> = self
            .disk
            .iter()
            .filter(|p| last.t - p.t <= 60 * 86_400)
            .map(|p| (p.t as f64 / DAY, p.free as f64))
            .collect();
        let slope = stats::weighted_slope(&recent, 7.0);
        (slope, slope.and_then(|s| stats::days_until_full(last.free as f64, s)))
    }

    /// 0 = relaxed, 1 = getting tight, 2 = critical.
    pub fn pressure(&self, free: Option<u64>) -> u8 {
        let (_, dtf) = self.forecast();
        let free = free.or(self.disk.last().map(|p| p.free)).unwrap_or(u64::MAX);
        if dtf.is_some_and(|d| d < 7.0) || free < 5 << 30 {
            2
        } else if dtf.is_some_and(|d| d < 21.0) || free < 20 << 30 {
            1
        } else {
            0
        }
    }

    /// Unusual growth of the shared folder over the last day: (latest/day, typical/day).
    pub fn growth_spike(&self) -> Option<(f64, f64)> {
        let rates: Vec<f64> = self
            .disk
            .windows(2)
            .filter(|w| w[1].t > w[0].t)
            .map(|w| (w[1].shared as f64 - w[0].shared as f64) / days(w[0].t, w[1].t).max(0.25))
            .collect();
        let (latest, hist) = rates.split_last()?;
        let (spike, typical) = stats::is_spike(hist, *latest, 1.0e9);
        spike.then_some((*latest, typical))
    }

    pub fn accuracy(&self) -> (u64, f64) {
        let n = self.artifact_model.n + self.project_model.n;
        let acc = if n == 0 {
            0.0
        } else {
            (self.artifact_model.acc * self.artifact_model.n as f64 + self.project_model.acc * self.project_model.n as f64)
                / n as f64
        };
        (n, acc)
    }
}

/// Thresholds by disk pressure: (artifact reuse cut-off, project reuse cut-off, cap multiplier).
pub fn policy(pressure: u8) -> (f64, f64, f64) {
    match pressure {
        0 => (0.06, 0.10, 1.0),
        1 => (0.15, 0.25, 0.7),
        _ => (0.35, 0.50, 0.4),
    }
}

/// Local hours of recent builds (from artifact use times), for quiet-hour detection.
pub fn build_hours(groups: &[Group], extra: &[i64], now: i64) -> Vec<u32> {
    groups
        .iter()
        .map(|g| g.last_use)
        .chain(extra.iter().copied())
        .filter(|&t| t > 0 && now - t <= 30 * 86_400)
        .filter_map(|t| Local.timestamp_opt(t, 0).single().map(|d| d.hour()))
        .collect()
}

// ---------------- insights ----------------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Level {
    Good,
    Info,
    Warn,
}

#[derive(Clone, Debug)]
pub struct Insight {
    pub level: Level,
    pub title: String,
    pub detail: String,
}

pub struct Analysis {
    pub insights: Vec<Insight>,
    pub stale_artifacts: u64,
    pub days_to_full: Option<f64>,
    pub pressure: u8,
    pub learned: u64,
    pub accuracy: f64,
}

pub fn analyze(
    brain: &Brain,
    groups: &[Group],
    project_rows: &[(String, u64, f64)], // (name, size, reuse prob) for leftover targets
    free: Option<u64>,
    extra_build_times: &[i64],
    auto_at: &str,
    now: i64,
) -> Analysis {
    let mut out = Vec::new();
    let pressure = brain.pressure(free);
    let (tau_a, tau_p, _) = policy(pressure);
    let (slope, dtf) = brain.forecast();

    // disk forecast
    match (dtf, slope) {
        (Some(d), _) if d < 60.0 => out.push(Insight {
            level: if d < 21.0 { Level::Warn } else { Level::Info },
            title: tf("Disk full in about {} days", &[&format!("{d:.0}")]),
            detail: tf("Free space is shrinking by about {}/day. rtidy cleans harder as it gets closer.", &[&human(-slope.unwrap_or(0.0) as u64)]),
        }),
        (_, Some(s)) => out.push(Insight {
            level: Level::Good,
            title: t("Disk space is stable").into(),
            detail: if s >= 0.0 {
                tf("Free space is growing by about {}/day.", &[&human(s as u64)])
            } else {
                tf("Shrinking slowly ({}/day), nothing to worry about.", &[&human(-s as u64)])
            },
        }),
        _ => out.push(Insight {
            level: Level::Info,
            title: t("Learning your disk usage").into(),
            detail: t("A forecast appears after a few days of history.").into(),
        }),
    }

    // stale artifacts
    let stale: Vec<&Group> = groups.iter().filter(|g| brain.artifact_reuse(g, now) < tau_a).collect();
    let stale_bytes: u64 = stale.iter().map(|g| g.size).sum();
    if stale_bytes > 50 << 20 {
        out.push(Insight {
            level: Level::Info,
            title: tf("{} of build artifacts you probably won't need", &[&human(stale_bytes)]),
            detail: tf("{} build units have under {}% chance of being used in the next {} days. They go in the next clean-up.", &[
                &stale.len().to_string(),
                &format!("{:.0}", tau_a * 100.0),
                &format!("{HORIZON_DAYS:.0}"),
            ]),
        });
    }

    // projects unlikely to come back
    let unlikely: Vec<&(String, u64, f64)> = project_rows.iter().filter(|p| p.2 < tau_p).collect();
    if !unlikely.is_empty() {
        let bytes: u64 = unlikely.iter().map(|p| p.1).sum();
        let names: Vec<&str> = unlikely.iter().take(3).map(|p| p.0.as_str()).collect();
        out.push(Insight {
            level: Level::Info,
            title: tf("{} projects you're unlikely to build soon ({})", &[&unlikely.len().to_string(), &human(bytes)]),
            detail: tf("e.g. {}. Their target/ folders are safe to remove.", &[&names.join(", ")]),
        });
    }

    // growth spike
    if let Some((latest, typical)) = brain.growth_spike() {
        out.push(Insight {
            level: Level::Warn,
            title: tf("Unusual growth: +{} in a day", &[&human(latest as u64)]),
            detail: tf("Normally about {}/day. A new toolchain, a big dependency or a release build?", &[&human(typical.max(0.0) as u64)]),
        });
    }

    // quiet hour
    let hours = build_hours(groups, extra_build_times, now);
    let quiet_hour = stats::quietest_hour(&hours, 40);
    if let Some(h) = quiet_hour {
        let current = auto_at.split(':').next().and_then(|x| x.parse::<u32>().ok());
        let same = current.is_some_and(|c| (c as i32 - h as i32).rem_euclid(24).min((h as i32 - c as i32).rem_euclid(24)) < 2);
        out.push(Insight {
            level: Level::Good,
            title: tf("You rarely build around {}:00", &[&format!("{h:02}")]),
            detail: if same {
                t("Daily clean-up already runs then, so it never slows your builds.").into()
            } else {
                t("Smart schedule moves the daily clean-up there.").into()
            },
        });
    }

    let (learned, accuracy) = brain.accuracy();
    out.push(Insight {
        level: Level::Info,
        title: if learned < 30 {
            t("Model is still learning your habits").into()
        } else {
            tf("Prediction accuracy {}%", &[&format!("{:.0}", accuracy * 100.0)])
        },
        detail: tf("{} outcomes learned. Runs on this computer only; nothing is uploaded.", &[&learned.to_string()]),
    });

    Analysis { insights: out, stale_artifacts: stale_bytes, days_to_full: dtf, pressure, learned, accuracy }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(key: &str, last: i64, size: u64) -> Group {
        Group { key: key.into(), paths: vec![], size, last_use: last }
    }

    #[test]
    fn learns_that_rebuilt_artifacts_matter() {
        // Simulate 120 days: "hot" units are used every 2 days, "cold" ones once.
        let mut b = Brain::default();
        let t0 = 1_700_000_000i64;
        for day in 0..120i64 {
            let now = t0 + day * 86_400;
            let mut groups = Vec::new();
            for i in 0..20 {
                let last = t0 + (day - day % 2) * 86_400;
                groups.push(g(&format!("debug/hot{i}"), last, 5 << 20));
            }
            for i in 0..20 {
                groups.push(g(&format!("debug/cold{i}"), t0, 5 << 20));
            }
            b.observe(now, &[], &groups, Some(100 << 30), 1 << 30);
        }
        let now = t0 + 120 * 86_400;
        let hot = b.artifact_reuse(&g("debug/hot0", now - 86_400, 5 << 20), now);
        let cold = b.artifact_reuse(&g("debug/cold0", t0, 5 << 20), now);
        assert!(hot > 0.8, "hot {hot}");
        assert!(cold < 0.05, "cold {cold}");
        assert!(b.artifact_model.n > 50);
    }

    #[test]
    fn pressure_rises_when_disk_shrinks() {
        let mut b = Brain::default();
        let t0 = 1_700_000_000i64;
        for d in 0..10 {
            // 60 GB free, losing 4 GB/day
            b.observe(t0 + d * 86_400, &[], &[], Some((60u64 << 30) - (d as u64) * (4 << 30)), 0);
        }
        let (_, dtf) = b.forecast();
        assert!(dtf.unwrap() < 7.0, "{dtf:?}");
        assert_eq!(b.pressure(None), 2);
    }
}
