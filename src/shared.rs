//! The shared target dir: set it up, keep it small, slim down debug info.

use crate::say;
use crate::util::*;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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
pub fn profiles(shared: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in fs::read_dir(shared).into_iter().flatten().flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if p.join("deps").is_dir() || p.join(".fingerprint").is_dir() {
            out.push(p);
        } else {
            for sub in fs::read_dir(&p).into_iter().flatten().flatten() {
                if sub.path().join("deps").is_dir() {
                    out.push(sub.path());
                }
            }
        }
    }
    out
}

/// Shrink the shared dir:
/// 1. drop incremental caches (pure speed-up data, usually the biggest part)
/// 2. drop artifacts untouched for `days`
/// 3. if still over `max_size`, drop oldest artifacts until under it
pub fn gc(days: u64, max_size: Option<u64>, dry_run: bool) -> Result<u64> {
    let Some(shared) = shared_target_dir()? else {
        say!("gc: no shared target dir (run `rtidy setup`), skipping");
        return Ok(0);
    };
    if !shared.is_dir() {
        return Ok(0);
    }
    let profs = profiles(&shared);
    let mut freed = 0;
    for p in &profs {
        let inc = p.join("incremental");
        if inc.is_dir() {
            freed += remove(&inc, dry_run);
        }
    }

    let mut entries: Vec<(SystemTime, PathBuf)> = Vec::new();
    for p in &profs {
        for sub in ["deps", "build", ".fingerprint"] {
            for e in fs::read_dir(p.join(sub)).into_iter().flatten().flatten() {
                entries.push((mtime(&e.path()), e.path()));
            }
        }
    }
    entries.sort();

    let cutoff = SystemTime::now() - Duration::from_secs(days * 86_400);
    let mut current = dir_size(&shared).saturating_sub(if dry_run { freed } else { 0 });
    for (t, path) in entries {
        let over = max_size.is_some_and(|m| current > m);
        if t >= cutoff && !over {
            break; // sorted oldest-first: nothing older remains
        }
        let got = remove(&path, dry_run);
        current = current.saturating_sub(got);
        freed += got;
    }
    say!(
        "gc: {} {} in {} (now {})",
        if dry_run { "would free" } else { "freed" },
        human(freed),
        shared.display(),
        human(current)
    );
    Ok(freed)
}
