//! rtidy — find, clean and prevent scattered Rust `target/` directories.
//!
//! Copyright (C) 2026 Thirawat Sinlapasomsak <https://github.com/thirawat27>
//!
//! This program is free software: you can redistribute it and/or modify it under the
//! terms of the GNU General Public License as published by the Free Software
//! Foundation, version 3.
//!
//! This program is distributed in the hope that it will be useful, but WITHOUT ANY
//! WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
//! PARTICULAR PURPOSE. See the GNU General Public License for more details. You should
//! have received a copy of the GNU General Public License along with this program; see
//! the LICENSE file, or <https://www.gnu.org/licenses/>.
//!
//! Run with no arguments to open the app, or `rtidy fix` to do everything from a terminal.
//! Root-cause fix: every project shares ONE target dir (`build.target-dir` in
//! ~/.cargo/config.toml), so new `target/` folders stop appearing; a daily task keeps
//! the shared dir and cargo cache small.

// On Windows, be a GUI app (no console window on double-click / scheduled runs);
// CLI output is re-attached to the parent terminal in `attach_console`.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod brain;
mod cache;
mod config;
mod discover;
mod doctor;
mod engine;
#[cfg(feature = "gui")]
mod gui;
mod i18n;
mod integrate;
mod prune;
mod schedule;
mod shared;
mod targets;
#[macro_use]
mod util;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use config::Config;
use std::path::PathBuf;
use util::{free_space, human, parse_size, plain};

#[derive(Parser)]
#[command(name = "rtidy", version, about = "Keeps your Rust target folders tidy. Run without arguments to open the app.")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// One command, everything: shared target dir, lean builds, delete leftovers,
    /// trim caches, daily auto clean-up. Uses saved settings (or detected folders).
    Fix {
        /// Project folders (saved for next time). Default: saved or auto-detected
        roots: Vec<PathBuf>,
    },
    /// Open the app (same as running with no arguments)
    #[cfg(feature = "gui")]
    Gui,
    /// AI insights: disk forecast, what you won't reuse, unusual growth, quiet hours
    Insights,
    /// Health check: where the space went and what to do about it
    Doctor { roots: Vec<PathBuf> },
    /// List per-project target/ dirs with size and last build time
    Scan {
        roots: Vec<PathBuf>,
        /// Walk every folder, ignoring the remembered "no Rust here" notes
        #[arg(long)]
        deep: bool,
    },
    /// Search every drive for folders that hold Cargo projects
    Find {
        /// Give up after this many seconds and report what was found
        #[arg(long, default_value_t = 20)]
        timeout: u64,
        /// Save the result as the project folders rtidy watches
        #[arg(long)]
        save: bool,
    },
    /// Delete per-project target/ dirs not built for N days
    Clean {
        roots: Vec<PathBuf>,
        /// Only delete if last build is older than this many days (0 = all)
        #[arg(long, default_value_t = 30)]
        days: u64,
        #[arg(long)]
        dry_run: bool,
    },
    /// Make all projects share one target dir
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
        #[arg(long, default_value_t = 14)]
        days: u64,
        /// Keep removing oldest artifacts until under this size (e.g. 30G)
        #[arg(long, value_parser = parse_size)]
        max_size: Option<u64>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Clean the ~/.cargo download cache (re-downloaded on demand)
    Cache {
        #[arg(long, default_value_t = 30)]
        days: u64,
        #[arg(long)]
        dry_run: bool,
    },
    /// Scheduled clean-up. No arguments = saved settings.
    Auto {
        #[command(flatten)]
        opts: AutoOpts,
        /// Run only if the daily HH:MM slot was missed (used by run-at-login triggers)
        #[arg(long, value_name = "HH:MM", hide = true)]
        catch_up: Option<String>,
    },
    /// Register the daily task (no options = use saved settings)
    Schedule {
        #[arg(long)]
        at: Option<String>,
        #[arg(long)]
        remove: bool,
        #[command(flatten)]
        opts: AutoOpts,
    },
    /// Remove the daily task and app-menu entry (before deleting rtidy)
    Uninstall,
    /// Show the log of scheduled runs
    Log {
        #[arg(short, default_value_t = 20)]
        n: usize,
    },
}

#[derive(Args)]
pub struct AutoOpts {
    /// Project folders (default: saved settings)
    roots: Vec<PathBuf>,
    #[arg(long)]
    days: Option<u64>,
    #[arg(long)]
    gc_days: Option<u64>,
    #[arg(long, value_parser = parse_size)]
    max_size: Option<u64>,
    #[arg(long)]
    cache_days: Option<u64>,
    /// Only run when free disk space is below this (e.g. 50G)
    #[arg(long, value_parser = parse_size)]
    min_free: Option<u64>,
}

impl AutoOpts {
    fn is_empty(&self) -> bool {
        self.roots.is_empty()
            && self.days.is_none()
            && self.gc_days.is_none()
            && self.max_size.is_none()
            && self.cache_days.is_none()
            && self.min_free.is_none()
    }

    /// Saved settings overridden by any flags given.
    fn to_config(&self) -> Result<Config> {
        let mut c = Config::load();
        if !self.roots.is_empty() {
            c.roots = self.roots.iter().map(|r| util::tidy_abs(r)).collect();
        }
        if let Some(v) = self.days {
            c.days = v;
        }
        if let Some(v) = self.gc_days {
            c.gc_days = v;
        }
        if let Some(v) = self.max_size {
            c.max_size_gb = v.div_ceil(1 << 30);
        }
        if let Some(v) = self.cache_days {
            c.cache_days = v;
        }
        Ok(c)
    }

    fn to_args(&self) -> Result<Vec<String>> {
        let mut a = Vec::new();
        for r in &self.roots {
            a.push(plain(&util::tidy_abs(r)));
        }
        let mut flag = |name: &str, v: Option<u64>| {
            if let Some(v) = v {
                a.extend([name.to_string(), v.to_string()]);
            }
        };
        flag("--days", self.days);
        flag("--gc-days", self.gc_days);
        flag("--max-size", self.max_size);
        flag("--cache-days", self.cache_days);
        flag("--min-free", self.min_free);
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

/// On Windows we're a GUI-subsystem exe; reconnect to the terminal for CLI output.
fn attach_console() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse_from(wild_args());
    // One read of the settings file covers the language and every command below.
    let saved = Config::load();
    i18n::set(saved.lang);
    #[cfg(feature = "gui")]
    let cmd = match cli.cmd {
        None | Some(Cmd::Gui) => {
            gui::run().map_err(|e| anyhow::anyhow!("{e}"))?;
            return Ok(());
        }
        Some(c) => c,
    };
    // Built without the desktop app: there is nothing to open, so show the commands.
    #[cfg(not(feature = "gui"))]
    let cmd = match cli.cmd {
        Some(c) => c,
        None => {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            println!("this build has no desktop app; install with the `gui` feature for one.");
            return Ok(());
        }
    };
    match cmd {
        #[cfg(feature = "gui")]
        Cmd::Gui => unreachable!(),
        Cmd::Uninstall => {
            schedule::remove()?;
            integrate::unintegrate()?;
            println!("removed the daily task and app-menu entry.");
            println!("your shared target folder setting stays in ~/.cargo/config.toml (build.target-dir);");
            println!("delete that line if you want per-project target/ folders back.");
            Ok(())
        }
        Cmd::Fix { roots } => {
            let mut cfg = saved;
            if !roots.is_empty() {
                cfg.roots = roots.iter().map(|r| util::tidy_abs(r)).collect();
            }
            let folders: Vec<String> = cfg.roots.iter().map(|r| r.display().to_string()).collect();
            println!("project folders: {}", folders.join(", "));
            engine::fix_all(&cfg)?;
            println!("\nsettings saved to {}", Config::path()?.display());
            Ok(())
        }
        Cmd::Insights => {
            let mut cfg = saved;
            cfg.smart = true;
            let s = engine::snapshot(&cfg);
            for i in &s.insights {
                let tag = match i.level() {
                    brain::Level::Good => "[ok]  ",
                    brain::Level::Info => "[ai]  ",
                    brain::Level::Warn => "[warn]",
                };
                println!("{tag} {}\n       {}", i.title(), i.detail());
            }
            for r in s.targets.iter().filter(|r| !r.keep) {
                if let Some(p) = r.reuse {
                    println!("{:>5.0}%  {:>10}  {}", p * 100.0, human(r.size), r.path.display());
                }
            }
            Ok(())
        }
        Cmd::Doctor { roots } => {
            let roots = if roots.is_empty() { saved.roots } else { roots };
            doctor::run(&roots)
        }
        Cmd::Scan { roots, deep } => {
            targets::scan(&roots_or_cwd(&roots)?, deep);
            Ok(())
        }
        Cmd::Find { timeout, save } => {
            let seeds = discover::search_seeds();
            let shown: Vec<String> = seeds.iter().map(|s| plain(s)).collect();
            println!("searching {} ({}s budget)\n", shown.join(", "), timeout);
            let found = discover::discover_roots(std::time::Duration::from_secs(timeout));
            if found.is_empty() {
                println!("no folders with Cargo projects found");
                return Ok(());
            }
            for f in &found {
                println!("  {}", f.display());
            }
            println!("\n{} folder(s)", found.len());
            if save {
                let mut cfg = saved;
                cfg.roots = found;
                cfg.save()?;
                println!("saved to {}", Config::path()?.display());
            }
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
        Cmd::Auto { opts: o, catch_up } => {
            if let Some(at) = catch_up
                && !schedule::missed(&at)?
            {
                return Ok(());
            }
            util::enable_file_log();
            let cfg = o.to_config()?;
            if let (Some(min), Some(free)) = (o.min_free, cfg.roots.first().and_then(|r| free_space(r)))
                && free >= min
            {
                say!("auto: {} free (>= {}), nothing to do", human(free), human(min));
                schedule::record_run();
                return Ok(());
            }
            engine::auto_from_config(&cfg)?;
            schedule::record_run();
            Ok(())
        }
        Cmd::Schedule { at, remove, opts } => {
            let mut cfg = saved;
            if remove {
                schedule::remove()?;
                cfg.auto_enabled = false;
                cfg.save()?;
                println!("daily clean-up removed");
                return Ok(());
            }
            if let Some(at) = at {
                cfg.auto_at = at;
            }
            let args = if opts.is_empty() { Vec::new() } else { opts.to_args()? };
            schedule::install(&cfg.auto_at, &args)?;
            cfg.auto_enabled = true;
            cfg.save()?;
            println!("daily clean-up at {}; log: {}", cfg.auto_at, util::log_path()?.display());
            Ok(())
        }
        Cmd::Log { n } => {
            // Only the tail is ever printed, so only the tail is read.
            let text = util::read_tail(&util::log_path()?, 256 << 10);
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

/// clap prints --help/errors before we attach the console; attach early when there are args.
fn wild_args() -> Vec<std::ffi::OsString> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.len() > 1 {
        attach_console();
    }
    args
}
