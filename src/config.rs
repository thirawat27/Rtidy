//! Saved settings, shared by the GUI, `rtidy fix` and the scheduled `rtidy auto`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Folders that contain Rust projects
    pub roots: Vec<PathBuf>,
    /// Delete per-project target/ dirs not built for this many days
    pub days: u64,
    /// Shared dir: drop artifacts untouched for this many days
    pub gc_days: u64,
    /// Shared dir: size cap in GB (0 = no cap)
    pub max_size_gb: u64,
    /// Cargo download cache: drop entries older than this many days (0 = never clean)
    pub cache_days: u64,
    /// Dependencies without debug info
    pub lean: bool,
    /// Daily automatic clean-up
    pub auto_enabled: bool,
    /// HH:MM
    pub auto_at: String,
    /// UI language
    pub lang: crate::i18n::Lang,
    /// AI: learn reuse patterns and disk trends to decide what to clean
    pub smart: bool,
    /// AI: move the daily clean-up to the hour you build least
    pub smart_schedule: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            roots: default_roots(),
            days: 30,
            gc_days: 14,
            max_size_gb: 30,
            cache_days: 60,
            lean: true,
            auto_enabled: true,
            auto_at: "12:30".into(),
            lang: crate::i18n::detect(),
            smart: true,
            smart_schedule: true,
        }
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        Ok(dirs::config_dir().context("no config dir")?.join("rtidy").join("config.json"))
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
        fs::write(p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn max_size(&self) -> Option<u64> {
        (self.max_size_gb > 0).then_some(self.max_size_gb << 30)
    }
}

/// A quick first guess at where the projects are: the usual code folders under home,
/// but only the ones that really contain a Cargo project.
///
/// This runs on the main thread while settings load, so it only probes a known short
/// list — it never walks the machine. `crate::discover::discover_roots` does the real
/// search, in the background, and the UI offers it as "Search this computer".
///
/// Folders are kept only on evidence. The old version added `Documents` and
/// `Downloads` merely because they existed, which pointed rtidy at large folders with
/// no Rust in them and showed the user places they had nothing to do with.
pub fn default_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return Vec::new() };
    let names = [
        "projects", "Projects", "dev", "Dev", "code", "Code", "src", "source", "repos", "workspace",
        "git", "GitHub", "RustroverProjects", "Developer", "rust", "Rust",
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for n in names {
        let p = home.join(n);
        if p.is_dir() && holds_cargo_project(&p, 2) {
            let p = crate::util::tidy_abs(&p);
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Is there a `Cargo.toml` in `dir`, or within `depth` levels below it?
///
/// Deliberately shallow and cheap: this decides whether to *offer* a folder, and the
/// full search is a separate, bounded background job.
fn holds_cargo_project(dir: &Path, depth: usize) -> bool {
    if dir.join("Cargo.toml").is_file() {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else { return false };
    entries.flatten().any(|e| {
        e.file_type().is_ok_and(|t| t.is_dir())
            && !e.file_name().to_string_lossy().starts_with('.')
            && holds_cargo_project(&e.path(), depth - 1)
    })
}
