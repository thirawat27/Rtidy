//! Saved settings, shared by the GUI, `rtidy fix` and the scheduled `rtidy auto`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

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
        (self.max_size_gb > 0).then(|| self.max_size_gb << 30)
    }
}

/// Common places people keep code. Falls back to the home folder.
pub fn default_roots() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else { return vec![] };
    let names = [
        "projects", "Projects", "dev", "Dev", "code", "Code", "src", "source", "repos", "workspace",
        "git", "GitHub", "RustroverProjects", "Developer", "Documents", "Desktop", "Downloads",
    ];
    let mut out: Vec<PathBuf> = Vec::new();
    for n in names {
        let p = home.join(n);
        if let Ok(c) = p.canonicalize() {
            if c.is_dir() && !out.contains(&c) {
                out.push(c);
            }
        }
    }
    if out.is_empty() {
        out.push(home);
    }
    out
}
