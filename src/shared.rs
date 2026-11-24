//! The shared target dir: set it up, keep it small, slim down debug info.

use crate::plan::{Action, Kind, Plan};
use crate::say;
use crate::util::*;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};


/// A project-local `.cargo/config.toml` that sets its own target-dir bypasses the
/// shared dir. One definition, used by both the dashboard and `rtidy doctor`.
pub fn project_override(dir: &Path) -> Option<PathBuf> {
    // `~/.cargo/config.toml` is the *global* config — the very file rtidy writes
    // `build.target-dir` into. When a root is the home directory (or any ancestor of
    // CARGO_HOME) it would otherwise be reported as a project fighting the shared
    // dir, which is backwards: it is the shared dir.
    let global = cargo_config_path().ok();
    for name in ["config.toml", "config"] {
        let p = dir.join(".cargo").join(name);
        if global.as_deref().is_some_and(|g| same_file(g, &p)) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&p) else { continue };
        let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { continue };
        if doc.get("build").and_then(|b| b.get("target-dir")).is_some() {
            return Some(p);
        }
    }
    None
}

/// Two paths that name the same file, comparing them in their plain absolute form so
/// Windows' `\\?\` prefix and a trailing-slash difference do not hide a match.
fn same_file(a: &Path, b: &Path) -> bool {
    tidy_abs(a) == tidy_abs(b)
}

/// Installed rustup toolchains with their sizes, largest first.
pub fn toolchain_sizes() -> Vec<(String, u64)> {
    let Ok(dir) = rustup_home().map(|h| h.join("toolchains")) else { return Vec::new() };
    let paths: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    let mut out: Vec<(String, u64)> = paths
        .into_par_iter()
        .map(|p| (p.file_name().unwrap_or_default().to_string_lossy().into_owned(), dir_size(&p)))
        .collect();
    out.sort_unstable_by_key(|(_, size)| std::cmp::Reverse(*size));
    out
}

pub fn shared_target_dir() -> Result<Option<PathBuf>> {
    if let Ok(d) = std::env::var("CARGO_TARGET_DIR") {
        return Ok(Some(PathBuf::from(d)));
    }
    let (_, _, doc) = read_cargo_config()?;
    Ok(doc
        .get("build")
        .and_then(|b| b.get("target-dir"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from))
}

pub fn is_lean(doc: &toml_edit::DocumentMut) -> bool {
    doc.get("profile")
        .and_then(|p| p.get("dev"))
        .and_then(|d| d.get("package"))
        .and_then(|p| p.get("*"))
        .and_then(|s| s.get("debug"))
        .is_some()
}

pub fn setup(target_dir: Option<PathBuf>, lean: bool) -> Result<()> {
    let dir = match target_dir {
        Some(d) => d,
        None => dirs::cache_dir().context("no cache dir")?.join("cargo-target"),
    };
    fs::create_dir_all(&dir)?;
    let dir = plain(&dir.canonicalize()?);

    let (path, text, mut doc) = read_cargo_config()?;
    if !doc.contains_key("build") {
        doc.insert("build", toml_edit::table());
    }
    doc["build"]["target-dir"] = toml_edit::value(dir.clone());
    if lean {
        set_lean(&mut doc);
    }
    write_cargo_config(&path, &text, &doc)?;
    say!("set build.target-dir = {dir} in {}", path.display());
    say!("every project now builds into one folder; new target/ dirs stop appearing");
    if lean {
        say!("lean: dependencies build without debug info (your own crates keep full debug info)");
    }
    Ok(())
}

/// Idempotent: keep an existing shared dir, otherwise create the default one.
pub fn ensure_setup(lean: bool) -> Result<()> {
    if shared_target_dir()?.is_none() {
        return setup(None, lean);
    }
    if lean {
        let (path, text, mut doc) = read_cargo_config()?;
        if !is_lean(&doc) {
            set_lean(&mut doc);
            write_cargo_config(&path, &text, &doc)?;
            say!("lean: dependencies build without debug info");
        }
    }
    Ok(())
}

/// Dependencies without debug info: often halves target size and speeds up linking.
/// Your workspace crates keep full debug info, so debugging your code is unaffected.
fn set_lean(doc: &mut toml_edit::DocumentMut) {
    let mut dbg = toml_edit::Table::new();
    dbg["debug"] = toml_edit::value(false);
    let mut pkg = toml_edit::Table::new();
    pkg.set_implicit(true);
    pkg.insert("*", toml_edit::Item::Table(dbg));
    let mut dev = toml_edit::Table::new();
    dev.set_implicit(true);
    dev.insert("package", toml_edit::Item::Table(pkg));
    if !doc.contains_key("profile") {
        let mut prof = toml_edit::Table::new();
        prof.set_implicit(true);
        doc.insert("profile", toml_edit::Item::Table(prof));
    }
    let prof = doc["profile"].as_table_mut().expect("profile is a table");
    match prof.get_mut("dev").and_then(|d| d.as_table_mut()) {
        Some(existing) => {
            existing.insert("package", dev.remove("package").unwrap());
        }
        None => {
            prof.insert("dev", toml_edit::Item::Table(dev));
        }
    }
}

/// Profile dirs like <shared>/debug, <shared>/release, <shared>/<triple>/debug.
/// Does this directory look like a Cargo profile directory?
///
/// Cargo 1.100 reorganised the target directory. Build units used to sit in
/// `<profile>/deps` with their fingerprints in `<profile>/.fingerprint`; they now sit
/// in `<profile>/build/<crate>/<hash>/` with `out/` and `fingerprint/` inside. rtidy
/// recognises both, so it keeps working on older toolchains and on directories left
/// behind by them.
///
/// Missing the new layout is not a cosmetic problem: `profiles` feeds the garbage
/// collector, the incremental-cache sweep and the model's view of what you rebuild,
/// so when it found nothing the shared folder was never shrunk and the model never
/// saw a single build unit.
pub fn is_profile_dir(p: &Path) -> bool {
    p.join("build").is_dir()            // cargo >= 1.100 (and build scripts before it)
        || p.join("deps").is_dir()      // cargo <= 1.99
        || p.join(".fingerprint").is_dir()
        || p.join("incremental").is_dir()
}

pub fn profiles(shared: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(shared).into_iter().flatten().flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if is_profile_dir(&p) {
            out.push(p);
        } else {
            // A cross-compilation target puts the profiles one level down:
            // `<shared>/<triple>/debug`.
            for sub in fs::read_dir(&p).into_iter().flatten().flatten() {
                let s = sub.path();
                if s.is_dir() && is_profile_dir(&s) {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// Incremental caches inside the shared dir, as plan actions.
///
/// Incremental data is the compiler's own scratch space: it makes the *next* build of
/// something you are actively working on faster and is worth nothing once you have
/// moved on, which is why it goes in every plan, whatever the ages elsewhere say.
fn incremental_actions(shared: &Path) -> Vec<Action> {
    let dirs: Vec<PathBuf> = profiles(shared).into_iter().map(|p| p.join("incremental")).filter(|p| p.is_dir()).collect();
    dirs.into_par_iter()
        .map(|p| {
            let size = dir_size(&p);
            let age = age_days(crate::util::mtime_of(&fs::metadata(&p).ok()?));
            Some(Action::new(Kind::Incremental, p, size, crate::eco::cargo(), age, crate::i18n::t("incremental cache").into()))
        })
        .flatten()
        .collect()
}

/// AI-guided shrink, as a plan: evict the build units the model thinks you won't reuse,
/// stricter (and with a lower size cap) as disk pressure rises. Hard limits still apply.
pub fn smart_gc_plan(cfg: &crate::config::Config, b: &crate::brain::Brain, now: i64) -> Result<Plan> {
    let mut plan = Plan::default();
    let Some(shared) = shared_target_dir()? else {
        say!("gc: no shared target dir (run `rtidy setup`), skipping");
        return Ok(plan);
    };
    if !shared.is_dir() {
        return Ok(plan);
    }
    for a in incremental_actions(&shared) {
        plan.push(a);
    }
    let free = free_space(&shared);
    let pressure = b.pressure(free);
    let (tau, _, mult) = crate::brain::policy(pressure);
    let cap = cfg.max_size().map(|c| (c as f64 * mult) as u64);
    let min_age = if pressure >= 2 { 1.0 } else { 2.0 };
    let hard_age = (cfg.gc_days as f64) * 2.0;

    let groups = crate::brain::scan_artifacts(&shared);
    let mut scored: Vec<(f64, &crate::brain::Group)> = groups.iter().map(|g| (b.artifact_reuse(g, now), g)).collect();
    scored.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let mut current: u64 = groups.iter().map(|g| g.size).sum();
    for (p, g) in scored {
        let age = ((now - g.last_use) as f64 / 86_400.0).max(0.0);
        if age < min_age {
            continue; // never touch what you used in the last day or two
        }
        let over = cap.is_some_and(|c| current > c);
        if !(p < tau || over || age > hard_age) {
            continue;
        }
        let why = if over {
            crate::i18n::t("over the size cap").into()
        } else {
            crate::i18n::tf("{}% chance you use it again", &[&format!("{:.0}", p * 100.0)])
        };
        for (path, size) in &g.paths {
            let mut a = Action::new(Kind::Unit, path.clone(), *size, crate::eco::cargo(), age as u64, why.clone());
            a.reuse = Some(p);
            plan.push(a);
        }
        current = current.saturating_sub(g.size);
    }
    say!("ai gc: planned {} (pressure {pressure}, cut-off {:.0}%) in {}", human(plan.freed()), tau * 100.0, shared.display());
    Ok(plan)
}

/// Run the AI-guided shrink. Hard limits from Settings still apply.
pub fn smart_gc(cfg: &crate::config::Config, b: &crate::brain::Brain, now: i64, dry_run: bool) -> Result<u64> {
    let plan = smart_gc_plan(cfg, b, now)?;
    let mode = if dry_run { crate::plan::Mode::Preview } else { crate::plan::Mode::Delete };
    let out = crate::plan::apply(&plan, mode, "gc");
    say!("ai gc: {} {}", if dry_run { "would free" } else { "freed" }, human(out.freed));
    Ok(out.freed)
}

/// Shrink the shared dir, as a plan:
/// 1. drop incremental caches (pure speed-up data, usually the biggest part)
/// 2. drop artifacts untouched for `days`
/// 3. if still over `max_size`, drop oldest artifacts until under it
pub fn gc_plan(days: u64, max_size: Option<u64>) -> Result<Plan> {
    let mut plan = Plan::default();
    let Some(shared) = shared_target_dir()? else {
        say!("gc: no shared target dir (run `rtidy setup`), skipping");
        return Ok(plan);
    };
    if !shared.is_dir() {
        return Ok(plan);
    }
    for a in incremental_actions(&shared) {
        plan.push(a);
    }

    // The same build units the model sees, so both the old and the new Cargo layout are
    // understood in one place, and each unit's size and last-use time are already
    // measured — no second walk at delete time.
    let mut units = crate::brain::scan_artifacts(&shared);
    units.sort_unstable_by_key(|g| g.last_use); // oldest first

    let now = crate::brain::now();
    let cutoff = now - (days * 86_400) as i64;
    let mut current = dir_size(&shared).saturating_sub(plan.freed());
    for g in units {
        let over = max_size.is_some_and(|m| current > m);
        if g.last_use >= cutoff && !over {
            break; // sorted oldest-first: nothing older remains
        }
        let age = ((now - g.last_use) / 86_400).max(0) as u64;
        let why = if over {
            crate::i18n::t("over the size cap").into()
        } else {
            crate::i18n::tf("unused for {} days", &[&age.to_string()])
        };
        for (path, size) in &g.paths {
            plan.push(Action::new(Kind::Unit, path.clone(), *size, crate::eco::cargo(), age, why.clone()));
        }
        current = current.saturating_sub(g.size);
    }
    Ok(plan)
}

/// Shrink the shared dir.
pub fn gc(days: u64, max_size: Option<u64>, dry_run: bool) -> Result<u64> {
    let plan = gc_plan(days, max_size)?;
    let mode = if dry_run { crate::plan::Mode::Preview } else { crate::plan::Mode::Delete };
    let out = crate::plan::apply(&plan, mode, "gc");
    say!("gc: {} {}", if dry_run { "would free" } else { "freed" }, human(out.freed));
    Ok(out.freed)
}
