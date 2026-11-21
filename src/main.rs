//! rtidy — find, clean and prevent scattered Rust `target/` directories.
//!
//! Root-cause fix: `rtidy setup` points every project at ONE shared target dir
//! (via `build.target-dir` in ~/.cargo/config.toml), so new `target/` folders
//! stop appearing. `rtidy auto` then keeps that shared dir small and removes any
//! leftover per-project `target/` folders. `rtidy schedule` runs `auto` daily.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

#[derive(Parser)]
#[command(name = "rtidy", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List per-project target/ dirs with size and last build time
    Scan {
        /// Folders to search (default: current folder)
        roots: Vec<PathBuf>,
    },
    /// Delete per-project target/ dirs not built for N days
    Clean {
        roots: Vec<PathBuf>,
        /// Only delete if last build is older than this many days (0 = all)
        #[arg(long, default_value_t = 30)]
        days: u64,
        /// Show what would be deleted without deleting
        #[arg(long)]
        dry_run: bool,
    },
    /// Root-cause fix: make all projects share one target dir
    Setup {
        /// Shared target dir (default: <cache dir>/cargo-target)
        #[arg(long)]
        target_dir: Option<PathBuf>,
        /// Also delete the old per-project target/ dirs under these roots
        #[arg(long)]
        clean_old: Vec<PathBuf>,
    },
    /// Shrink the shared target dir: drop incremental caches and stale artifacts
    Gc {
        /// Remove artifacts not touched for this many days
        #[arg(long, default_value_t = 14)]
        days: u64,
        #[arg(long)]
        dry_run: bool,
    },
    /// Run clean + gc in one go (what the scheduled task runs)
    Auto {
        roots: Vec<PathBuf>,
        #[arg(long, default_value_t = 30)]
        days: u64,
        #[arg(long, default_value_t = 14)]
        gc_days: u64,
    },
    /// Register a daily scheduled task that runs `rtidy auto`
    Schedule {
        roots: Vec<PathBuf>,
        /// Time of day, HH:MM
        #[arg(long, default_value = "21:00")]
        at: String,
        #[arg(long, default_value_t = 30)]
        days: u64,
        /// Remove the scheduled task instead
        #[arg(long)]
        remove: bool,
    },
}

const TASK_NAME: &str = "rtidy";

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Scan { roots } => scan(&roots_or_cwd(roots)?),
        Cmd::Clean { roots, days, dry_run } => clean(&roots_or_cwd(roots)?, days, dry_run).map(|_| ()),
        Cmd::Setup { target_dir, clean_old } => setup(target_dir, &clean_old),
        Cmd::Gc { days, dry_run } => gc(days, dry_run).map(|_| ()),
        Cmd::Auto { roots, days, gc_days } => {
            let roots = roots_or_cwd(roots)?;
            let a = clean(&roots, days, false)?;
            let b = gc(gc_days, false)?;
            println!("rtidy auto: freed {} total", human(a + b));
            Ok(())
        }
        Cmd::Schedule { roots, at, days, remove } => schedule(roots, &at, days, remove),
    }
}

// ---------- discovery ----------

struct Target {
    path: PathBuf,
    size: u64,
    last_build: SystemTime,
}

fn roots_or_cwd(roots: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if roots.is_empty() {
        Ok(vec![std::env::current_dir()?])
    } else {
        Ok(roots)
    }
}

/// A Cargo target dir: named `target`, sibling of Cargo.toml, with a Cargo marker inside.
fn is_cargo_target(dir: &Path) -> bool {
    dir.file_name().is_some_and(|n| n == "target")
        && dir.parent().is_some_and(|p| p.join("Cargo.toml").is_file())
        && (dir.join("CACHEDIR.TAG").exists()
            || dir.join(".rustc_info.json").exists()
            || dir.join("debug").is_dir()
            || dir.join("release").is_dir())
}

fn find_targets(roots: &[PathBuf]) -> Vec<Target> {
    let mut out = Vec::new();
    for root in roots {
        let mut it = WalkDir::new(root).follow_links(false).into_iter();
        while let Some(entry) = it.next() {
            let Ok(e) = entry else { continue };
            if !e.file_type().is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy();
            if e.depth() > 0 && (name == ".git" || name == "node_modules" || name == ".cargo") {
                it.skip_current_dir();
                continue;
            }
            if is_cargo_target(e.path()) {
                it.skip_current_dir();
                out.push(Target {
                    path: e.path().to_path_buf(),
                    size: dir_size(e.path()),
                    last_build: last_build(e.path()),
                });
            }
        }
    }
    out.sort_by(|a, b| b.size.cmp(&a.size));
    out
}

fn dir_size(p: &Path) -> u64 {
    WalkDir::new(p)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Newest mtime among the target dir's top-level entries (profiles, .rustc_info.json).
fn last_build(p: &Path) -> SystemTime {
    fs::read_dir(p)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.metadata().ok()?.modified().ok())
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

fn age_days(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).unwrap_or_default().as_secs() / 86_400
}

// ---------- commands ----------

fn scan(roots: &[PathBuf]) -> Result<()> {
    let ts = find_targets(roots);
    let total: u64 = ts.iter().map(|t| t.size).sum();
    for t in &ts {
        println!("{:>10}  {:>4}d ago  {}", human(t.size), age_days(t.last_build), t.path.display());
    }
    println!("{} target dirs, {} total", ts.len(), human(total));
    if let Some(shared) = shared_target_dir()? {
        if shared.exists() {
            println!("shared target dir: {} ({})", shared.display(), human(dir_size(&shared)));
        }
    }
    Ok(())
}

fn clean(roots: &[PathBuf], days: u64, dry_run: bool) -> Result<u64> {
    let mut freed = 0;
    for t in find_targets(roots) {
        if age_days(t.last_build) < days {
            continue;
        }
        if dry_run {
            println!("would delete {} ({})", t.path.display(), human(t.size));
        } else {
            match fs::remove_dir_all(&t.path) {
                Ok(()) => println!("deleted {} ({})", t.path.display(), human(t.size)),
                Err(e) => {
                    eprintln!("skip {}: {e}", t.path.display());
                    continue;
                }
            }
        }
        freed += t.size;
    }
    println!("{} {}", if dry_run { "would free" } else { "freed" }, human(freed));
    Ok(freed)
}

fn cargo_home() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("CARGO_HOME") {
        return Ok(PathBuf::from(h));
    }
    Ok(dirs::home_dir().context("no home dir")?.join(".cargo"))
}

fn cargo_config_path() -> Result<PathBuf> {
    let home = cargo_home()?;
    let legacy = home.join("config");
    if legacy.is_file() && !home.join("config.toml").exists() {
        return Ok(legacy);
    }
    Ok(home.join("config.toml"))
}

fn shared_target_dir() -> Result<Option<PathBuf>> {
    if let Ok(d) = std::env::var("CARGO_TARGET_DIR") {
        return Ok(Some(PathBuf::from(d)));
    }
    let path = cargo_config_path()?;
    let Ok(text) = fs::read_to_string(&path) else { return Ok(None) };
    let doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("parse {}", path.display()))?;
    Ok(doc
        .get("build")
        .and_then(|b| b.get("target-dir"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from))
}

fn setup(target_dir: Option<PathBuf>, clean_old: &[PathBuf]) -> Result<()> {
    let dir = match target_dir {
        Some(d) => d,
        None => dirs::cache_dir().context("no cache dir")?.join("cargo-target"),
    };
    fs::create_dir_all(&dir)?;
    let dir = dir.canonicalize()?;
    let dir_str = strip_verbatim(&dir);

    let path = cargo_config_path()?;
    let text = fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("parse {}", path.display()))?;
    if !text.is_empty() {
        fs::copy(&path, path.with_extension("toml.bak"))?;
    }
    if doc.get("build").is_none() {
        doc["build"] = toml_edit::table();
    }
    doc["build"]["target-dir"] = toml_edit::value(dir_str.clone());
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, doc.to_string())?;
    println!("set build.target-dir = {dir_str} in {}", path.display());
    println!("all projects now build into one folder; new target/ dirs will stop appearing.");

    if !clean_old.is_empty() {
        clean(clean_old, 0, false)?;
    }
    Ok(())
}

/// On Windows, canonicalize() gives `\\?\C:\...`; cargo prefers plain `C:\...`.
fn strip_verbatim(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

fn gc(days: u64, dry_run: bool) -> Result<u64> {
    let Some(shared) = shared_target_dir()? else {
        println!("no shared target dir configured (run `rtidy setup`); skipping gc");
        return Ok(0);
    };
    if !shared.is_dir() {
        return Ok(0);
    }
    let cutoff = SystemTime::now() - Duration::from_secs(days * 86_400);
    let mut freed = 0;

    // Profile dirs: <shared>/debug, <shared>/release, <shared>/<triple>/debug, ...
    let mut profiles = Vec::new();
    for e in fs::read_dir(&shared)?.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        if p.join("deps").is_dir() {
            profiles.push(p);
        } else {
            for sub in fs::read_dir(&p).into_iter().flatten().flatten() {
                if sub.path().join("deps").is_dir() {
                    profiles.push(sub.path());
                }
            }
        }
    }

    for prof in profiles {
        // Incremental caches are pure speed-ups and usually the biggest part: always drop.
        let inc = prof.join("incremental");
        if inc.is_dir() {
            freed += remove(&inc, dry_run);
        }
        // Stale per-crate artifacts (cargo-sweep style, by mtime).
        for sub in ["deps", "build", ".fingerprint"] {
            for e in fs::read_dir(prof.join(sub)).into_iter().flatten().flatten() {
                let old = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t < cutoff)
                    .unwrap_or(false);
                if old {
                    freed += remove(&e.path(), dry_run);
                }
            }
        }
    }
    println!("gc {}: {} in {}", if dry_run { "would free" } else { "freed" }, human(freed), shared.display());
    Ok(freed)
}

fn remove(p: &Path, dry_run: bool) -> u64 {
    let size = if p.is_dir() { dir_size(p) } else { fs::metadata(p).map(|m| m.len()).unwrap_or(0) };
    if dry_run {
        return size;
    }
    let r = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
    match r {
        Ok(()) => size,
        Err(_) => 0, // likely in use by a running build; try again next time
    }
}

fn schedule(roots: Vec<PathBuf>, at: &str, days: u64, remove: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let roots: Vec<String> = roots_or_cwd(roots)?
        .iter()
        .map(|r| r.canonicalize().map(|p| strip_verbatim(&p)))
        .collect::<std::io::Result<_>>()?;

    if cfg!(windows) {
        let status = if remove {
            std::process::Command::new("schtasks")
                .args(["/delete", "/tn", TASK_NAME, "/f"])
                .status()?
        } else {
            let mut tr = format!("\"{}\" auto --days {days}", strip_verbatim(&exe));
            for r in &roots {
                tr.push_str(&format!(" \"{r}\""));
            }
            std::process::Command::new("schtasks")
                .args(["/create", "/tn", TASK_NAME, "/sc", "daily", "/st", at, "/f", "/tr", &tr])
                .status()?
        };
        if !status.success() {
            bail!("schtasks failed");
        }
        println!("{}", if remove { "scheduled task removed" } else { "scheduled task created" });
    } else {
        let (h, m) = at.split_once(':').context("--at must be HH:MM")?;
        let quoted: Vec<String> = roots.iter().map(|r| format!("'{r}'")).collect();
        println!("add this line with `crontab -e`:");
        println!("{m} {h} * * * '{}' auto --days {days} {}", exe.display(), quoted.join(" "));
    }
    Ok(())
}

fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}
