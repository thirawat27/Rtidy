//! rtidy — find, clean and prevent scattered Rust `target/` directories.
//!
//! Root-cause fix: `rtidy setup` points every project at ONE shared target dir
//! (via `build.target-dir` in ~/.cargo/config.toml), so new `target/` folders
//! stop appearing. `rtidy auto` keeps that shared dir and the cargo cache small
//! and removes leftover per-project `target/` folders. `rtidy schedule` runs it daily.

mod cache;
mod doctor;
mod schedule;
mod shared;
mod targets;
#[macro_use]
mod util;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use util::{free_space, human, parse_size, plain};

#[derive(Parser)]
#[command(name = "rtidy", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Health check: where the space went and what to do about it
    Doctor {
        /// Folders with your projects (default: current folder)
        roots: Vec<PathBuf>,
    },
    /// List per-project target/ dirs with size and last build time
    Scan { roots: Vec<PathBuf> },
    /// Delete per-project target/ dirs not built for N days
    Clean {
        roots: Vec<PathBuf>,
        /// Only delete if last build is older than this many days (0 = all)
        #[arg(long, default_value_t = 30)]
        days: u64,
        #[arg(long)]
        dry_run: bool,
    },
    /// Root-cause fix: make all projects share one target dir
    Setup {
        /// Shared target dir (default: <cache dir>/cargo-target)
        #[arg(long)]
        target_dir: Option<PathBuf>,
        /// Build dependencies without debug info (much smaller, faster links)
        #[arg(long)]
        lean: bool,
        /// Also delete the old per-project target/ dirs under these folders
        #[arg(long, num_args = 1..)]
        clean_old: Vec<PathBuf>,
    },
    /// Shrink the shared target dir
    Gc {
        /// Remove artifacts not touched for this many days
        #[arg(long, default_value_t = 14)]
        days: u64,
        /// Then keep removing oldest artifacts until the dir is under this size (e.g. 30G)
        #[arg(long, value_parser = parse_size)]
        max_size: Option<u64>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Clean the ~/.cargo download cache (re-downloaded on demand)
    Cache {
        /// Remove entries older than this many days (0 = all)
        #[arg(long, default_value_t = 30)]
        days: u64,
        #[arg(long)]
        dry_run: bool,
    },
    /// Run everything in one go (what the scheduled task runs)
    Auto(AutoOpts),
    /// Register a daily task that runs `rtidy auto` with the same options
    Schedule {
        /// Time of day, HH:MM
        #[arg(long, default_value = "21:00")]
        at: String,
        /// Remove the scheduled task instead
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        opts: AutoOpts,
    },
    /// Show the log of scheduled runs
    Log {
        /// Number of lines
        #[arg(short, default_value_t = 20)]
        n: usize,
    },
}

#[derive(Args)]
pub struct AutoOpts {
    /// Folders with your projects (default: current folder)
    roots: Vec<PathBuf>,
    /// Delete per-project target/ dirs not built for this many days
    #[arg(long, default_value_t = 30)]
    days: u64,
    /// gc: drop shared-dir artifacts untouched for this many days
    #[arg(long, default_value_t = 14)]
    gc_days: u64,
    /// gc: cap the shared target dir at this size (e.g. 30G)
    #[arg(long, value_parser = parse_size)]
    max_size: Option<u64>,
    /// Also clean the cargo download cache older than this many days
    #[arg(long)]
    cache_days: Option<u64>,
    /// Only run when free disk space is below this (e.g. 50G)
    #[arg(long, value_parser = parse_size)]
    min_free: Option<u64>,
}

impl AutoOpts {
    /// Re-serialize as CLI args (absolute paths) for the scheduled task.
    pub fn to_args(&self) -> Result<Vec<String>> {
        let mut a = Vec::new();
        for r in roots_or_cwd(&self.roots)? {
            a.push(plain(&r.canonicalize()?));
        }
        a.extend(["--days".into(), self.days.to_string(), "--gc-days".into(), self.gc_days.to_string()]);
        if let Some(v) = self.max_size {
            a.extend(["--max-size".into(), v.to_string()]);
        }
        if let Some(v) = self.cache_days {
            a.extend(["--cache-days".into(), v.to_string()]);
        }
        if let Some(v) = self.min_free {
            a.extend(["--min-free".into(), v.to_string()]);
        }
        Ok(a)
    }
}

fn roots_or_cwd(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if roots.is_empty() {
        Ok(vec![std::env::current_dir()?])
    } else {
        Ok(roots.to_vec())
    }
}

fn auto(o: &AutoOpts) -> Result<()> {
    util::enable_file_log();
    let roots = roots_or_cwd(&o.roots)?;
    if let (Some(min), Some(free)) = (o.min_free, free_space(&roots[0])) {
        if free >= min {
            say!("auto: {} free (>= {}), nothing to do", human(free), human(min));
            return Ok(());
        }
    }
    let mut freed = targets::clean(&roots, o.days, false);
    freed += shared::gc(o.gc_days, o.max_size, false)?;
    if let Some(d) = o.cache_days {
        freed += cache::clean(d, false)?;
    }
    say!("auto: freed {} total", human(freed));
    Ok(())
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Doctor { roots } => doctor::run(&roots_or_cwd(&roots)?),
        Cmd::Scan { roots } => {
            targets::scan(&roots_or_cwd(&roots)?);
            Ok(())
        }
        Cmd::Clean { roots, days, dry_run } => {
            targets::clean(&roots_or_cwd(&roots)?, days, dry_run);
            Ok(())
        }
        Cmd::Setup { target_dir, lean, clean_old } => {
            shared::setup(target_dir, lean)?;
            if !clean_old.is_empty() {
                targets::clean(&clean_old, 0, false);
            }
            Ok(())
        }
        Cmd::Gc { days, max_size, dry_run } => shared::gc(days, max_size, dry_run).map(drop),
        Cmd::Cache { days, dry_run } => cache::clean(days, dry_run).map(drop),
        Cmd::Auto(o) => auto(&o),
        Cmd::Schedule { at, remove, opts } => schedule::schedule(&opts, &at, remove),
        Cmd::Log { n } => {
            let text = std::fs::read_to_string(util::log_path()?).unwrap_or_default();
            let lines: Vec<&str> = text.lines().collect();
            for l in &lines[lines.len().saturating_sub(n)..] {
                println!("{l}");
            }
            if lines.is_empty() {
                println!("no runs logged yet");
            }
            Ok(())
        }
    }
}
