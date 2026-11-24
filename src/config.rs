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
    /// Also clean other languages' build output (Node, Gradle, Python, .NET …).
    ///
    /// Off by default, and deliberately so: rtidy is a Rust tool, and the shared
    /// target dir — the thing that stops the mess coming back — only exists for Cargo.
    /// This is the extra, for the machine that has a `node_modules` problem too.
    ///
    /// Nothing turns this on by itself. Not a preset, not an upgrade, not a disk that
    /// is nearly full: it widens what rtidy may delete, so it waits for the Settings
    /// switch or `rtidy langs --on <id>`.
    pub all_langs: bool,
    /// Ecosystem ids (see `crate::eco`) left out while `all_langs` is on.
    pub ecos_off: Vec<String>,
    /// Keep deleted artifacts recoverable for this many days (0 = delete outright).
    pub trash_days: u64,
    /// Other languages' package stores: drop entries untouched for this many days
    /// (0 = never touch them). High on purpose — see `crate::stores`.
    pub store_days: u64,
    /// `rtidy watch`: trim after this many minutes with no build activity.
    pub watch_idle_mins: u64,
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
            all_langs: false,
            ecos_off: Vec::new(),
            // Three days is long enough to notice a mistake and short enough that the
            // quarantine never becomes the thing filling the disk.
            trash_days: 3,
            store_days: 120,
            watch_idle_mins: 10,
        }
    }
}

/// Everything rtidy remembers about this machine: settings, the model, the trash index,
/// the savings ledger.
///
/// `RTIDY_HOME` moves the lot somewhere else. That exists because the per-user config
/// directory is resolved through the OS (the Known Folder API on Windows), so no
/// environment variable could redirect it, which made rtidy impossible to exercise —
/// by a test, in CI, or by anyone trying it out — without writing over the real
/// settings of whoever ran it.
pub fn state_dir() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("RTIDY_HOME")
        && !h.is_empty()
    {
        let p = PathBuf::from(h);
        fs::create_dir_all(&p)?;
        return Ok(p);
    }
    Ok(dirs::config_dir().context("no config dir")?.join("rtidy"))
}

/// Where the run log and the scan notes go: per-machine scratch, not settings.
pub fn cache_dir() -> Result<PathBuf> {
    let dir = match std::env::var("RTIDY_HOME") {
        Ok(h) if !h.is_empty() => PathBuf::from(h),
        _ => dirs::cache_dir().context("no cache dir")?.join("rtidy"),
    };
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        Ok(state_dir()?.join("config.json"))
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

    /// The ecosystems this configuration cleans. Always at least Cargo.
    pub fn ecos(&self) -> Vec<&'static crate::eco::Eco> {
        crate::eco::enabled(self.all_langs, &self.ecos_off)
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
