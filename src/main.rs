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
mod eco;
mod engine;
#[cfg(feature = "gui")]
mod gui;
mod i18n;
mod integrate;
mod ledger;
mod plan;
mod preset;
mod prune;
mod rules;
mod schedule;
mod shared;
mod stores;
mod targets;
mod trash;
mod watch;
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
    /// Show exactly what a clean-up would remove, and remove nothing
    Plan {
        roots: Vec<PathBuf>,
        /// Only count artifacts older than this many days (0 = everything)
        #[arg(long, default_value_t = 0)]
        days: u64,
        /// Machine-readable, for CI and scripts
        #[arg(long)]
        json: bool,
    },
    /// Put back what the last clean-up moved aside
    Undo {
        /// Everything still recoverable, not just the last run
        #[arg(long)]
        all: bool,
    },
    /// What is recoverable right now, and for how long
    Trash {
        /// Delete everything recoverable, now
        #[arg(long)]
        empty: bool,
    },
    /// Trim after builds stop, instead of once a day (ctrl-c to stop)
    Watch {
        /// Check once and exit
        #[arg(long)]
        once: bool,
    },
    /// Other languages' package stores: where they are, how big, what is stale
    Stores {
        /// Drop entries untouched for this many days (default: saved setting)
        #[arg(long)]
        days: Option<u64>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Ready-made settings: `rtidy preset` lists them, `rtidy preset ci` applies one
    Preset {
        /// careful | balanced | small-disk | workstation | ci
        name: Option<String>,
        /// Show what it would change, and change nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Which languages rtidy cleans (Rust is always on)
    Langs {
        /// Turn an ecosystem on, by id (`rtidy langs` lists them)
        #[arg(long, value_name = "ID")]
        on: Vec<String>,
        /// Turn one off
        #[arg(long, value_name = "ID")]
        off: Vec<String>,
        /// Turn every other language off, back to Rust only
        #[arg(long, conflicts_with_all = ["on", "off"])]
        rust_only: bool,
    },
    /// Clean until this much is free, for CI runners. Exit 3 if it can't get there.
    Budget {
        /// Target free space, e.g. 50G
        #[arg(value_parser = parse_size)]
        free: u64,
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
        /// Show whether the daily clean-up is on, when it last ran and runs next
        #[arg(long, conflicts_with_all = ["at", "remove"])]
        status: bool,
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
            let ecos = saved.ecos();
            let roots = if roots.is_empty() { saved.roots.clone() } else { roots };
            doctor::run(&roots, &ecos)?;
            if saved.all_langs {
                println!();
                println!("package stores");
                stores::report(&ecos);
            }
            Ok(())
        }
        Cmd::Scan { roots, deep } => {
            targets::scan(&roots_or_cwd(&roots)?, deep, &saved.ecos());
            Ok(())
        }
        Cmd::Find { timeout, save } => {
            let seeds = discover::search_seeds();
            let shown: Vec<String> = seeds.iter().map(|s| plain(s)).collect();
            println!("searching {} ({}s budget)\n", shown.join(", "), timeout);
            let found = discover::discover_roots(std::time::Duration::from_secs(timeout), &saved.ecos());
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
            targets::clean(&roots_or_cwd(&roots)?, days, dry_run, &saved.ecos(), saved.trash_days);
            Ok(())
        }
        Cmd::Setup { target_dir, lean, clean_old } => {
            shared::setup(target_dir, lean)?;
            if !clean_old.is_empty() {
                targets::clean(&clean_old, 0, false, &saved.ecos(), saved.trash_days);
            }
            Ok(())
        }
        Cmd::Gc { days, max_size, dry_run } => {
            // Smart mode has a better answer than "older than N days": ask the model
            // which build units are not coming back. Hard limits still apply.
            let mut cfg = saved;
            cfg.gc_days = days;
            if let Some(m) = max_size {
                cfg.max_size_gb = m.div_ceil(1 << 30);
            }
            if cfg.smart {
                shared::smart_gc(&cfg, &brain::Brain::load(), brain::now(), dry_run).map(drop)
            } else {
                shared::gc(days, cfg.max_size(), dry_run).map(drop)
            }
        }
        Cmd::Cache { days, dry_run } => cache::clean(days, dry_run).map(drop),
        Cmd::Auto { opts: o, catch_up } => {
            // Taken before the catch-up check, so a trigger that waits its turn sees the
            // stamp the earlier run leaves.
            let _lock = match schedule::run_lock() {
                schedule::RunLock::Held(f) => f,
                schedule::RunLock::Busy => {
                    util::enable_file_log();
                    say!("auto: another scheduled clean-up is running, skipped");
                    return Ok(());
                }
            };
            if let Some(at) = &catch_up
                && !schedule::missed(at)?
            {
                return Ok(());
            }
            util::enable_file_log();
            if let Some(at) = &catch_up {
                say!("auto: the {at} clean-up hasn't run since its last slot, running it now");
            }
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
        Cmd::Schedule { at, remove, status, opts } => {
            let mut cfg = saved;
            if status {
                println!("{}", schedule::status(&cfg.auto_at));
                return Ok(());
            }
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
            println!("missed runs catch up once when the computer is back on; check with `rtidy schedule --status`");
            Ok(())
        }
        Cmd::Plan { roots, days, json } => {
            let mut cfg = saved;
            if !roots.is_empty() {
                cfg.roots = roots.iter().map(|r| util::tidy_abs(r)).collect();
            }
            let p = engine::plan_all(&cfg, days, &cfg.ecos());
            if json {
                println!("{}", p.to_json());
                return Ok(());
            }
            // Grouped by kind, because the one question this answers is "what happens
            // to my projects", and a plan that reaches the package stores is hundreds
            // of entries deep — printed flat, the folders the user asked about were
            // buried under other people's cache directories.
            let groups = [
                (plan::Kind::Artifact, "projects"),
                (plan::Kind::Incremental, "shared target dir: incremental caches"),
                (plan::Kind::Unit, "shared target dir: build units"),
                (plan::Kind::Cache, "cargo download cache"),
                (plan::Kind::Store, "other languages' package stores"),
            ];
            for (kind, title) in groups {
                let mut of_kind: Vec<&plan::Action> = p.by_kind(kind).collect();
                // Inside a group, biggest first: the plan as a whole is ordered by value,
                // but a five-line summary of 700 cache entries should show the big ones.
                of_kind.sort_unstable_by_key(|a| std::cmp::Reverse(a.size));
                if of_kind.is_empty() {
                    continue;
                }
                let total: u64 = of_kind.iter().map(|a| a.size).sum();
                println!("{title}  —  {} in {} item(s)", human(total), of_kind.len());
                // Every project folder is listed; the caches are summarised, since one
                // line per entry says nothing a total does not.
                let cap = if kind == plan::Kind::Artifact { usize::MAX } else { 5 };
                for a in of_kind.iter().take(cap) {
                    println!("{}", a.line());
                }
                if let Some(rest) = of_kind.len().checked_sub(cap).filter(|n| *n > 0) {
                    let shown: u64 = of_kind.iter().take(cap).map(|a| a.size).sum();
                    println!("          … {rest} more, {}", human(total - shown));
                }
                println!();
            }
            for (path, why) in p.kept.iter().take(plan::LIST_CAP) {
                println!("{:>10}  keep   {}  ({why})", "-", path.display());
            }
            if !p.kept.is_empty() {
                println!();
            }
            for (eco, bytes) in p.by_eco() {
                let label = eco::by_id(&eco).map(|e| e.label).unwrap_or("other");
                println!("{:>10}  {label}", human(bytes));
            }
            println!();
            println!("{}", plan::summary(&p));
            println!("nothing was removed; `rtidy fix` does it");
            Ok(())
        }
        Cmd::Undo { all } => {
            let (n, bytes) = trash::restore(all)?;
            if n == 0 {
                println!("nothing to put back");
            } else {
                ledger::Ledger::record_restore(bytes);
                println!("restored {n} item(s), {}", human(bytes));
            }
            Ok(())
        }
        Cmd::Trash { empty } => {
            if empty {
                let freed = trash::purge(0);
                println!("freed {}", human(freed));
                return Ok(());
            }
            let items = trash::list();
            if items.is_empty() {
                println!("nothing recoverable");
                return Ok(());
            }
            let now = brain::now();
            for e in &items {
                let eco = eco::by_id(&e.eco).map(|x| x.label).unwrap_or("");
                println!("{:>10}  {:>3}d ago  {:<22}  {}", human(e.size), (now - e.when) / 86_400, eco, e.from.display());
            }
            let total: u64 = items.iter().map(|e| e.size).sum();
            println!();
            println!("{} item(s), {} — kept for {} day(s)", items.len(), human(total), saved.trash_days);
            println!("`rtidy undo` puts the last clean-up back; `rtidy trash --empty` frees it now");
            Ok(())
        }
        Cmd::Watch { once } => watch::run(&saved, once),
        Cmd::Stores { days, dry_run } => {
            let ecos = saved.ecos();
            stores::report(&ecos);
            let days = days.unwrap_or(saved.store_days);
            if days == 0 {
                println!("trimming is off (store_days = 0 in settings)");
                return Ok(());
            }
            let p = stores::plan(&ecos, days);
            if p.is_empty() {
                println!("nothing in them is older than {days} days");
                return Ok(());
            }
            let mode = if dry_run { plan::Mode::Preview } else { plan::Mode::Delete };
            let out = plan::apply(&p, mode, "stores");
            println!(
                "{} {} from {} entries",
                if dry_run { "would free" } else { "freed" },
                human(out.freed),
                out.done
            );
            Ok(())
        }
        Cmd::Preset { name, dry_run } => {
            let cfg = saved;
            let Some(name) = name else {
                let now = preset::current(&cfg);
                for p in preset::ALL.iter() {
                    let mark = if now.is_some_and(|c| c.id == p.id) { "*" } else { " " };
                    println!("{mark} {:<12} {}", p.id, p.detail());
                }
                println!();
                match now {
                    Some(p) => println!("current: {}", p.id),
                    None => println!("current: settings tuned by hand (no preset matches)"),
                }
                println!("rtidy preset small-disk   |   rtidy preset ci --dry-run");
                return Ok(());
            };
            let Some(p) = preset::by_id(&name) else {
                anyhow::bail!("no preset called `{name}` (run `rtidy preset` to list them)");
            };
            let rows = p.diff(&cfg);
            if rows.is_empty() {
                println!("already on {}: nothing to change", p.id);
                return Ok(());
            }
            println!("{} — {}", p.id, p.detail());
            for (what, from, to) in &rows {
                println!("  {:<38} {from}  ->  {to}", i18n::t(what));
            }
            if dry_run {
                println!();
                println!("nothing was saved; drop --dry-run to apply");
                return Ok(());
            }
            let next = p.over(&cfg);
            let reschedule = next.auto_enabled != cfg.auto_enabled;
            next.save()?;
            if reschedule {
                engine::apply_schedule(&next)?;
            }
            println!();
            println!("saved to {}", Config::path()?.display());
            Ok(())
        }
        Cmd::Langs { on, off, rust_only } => {
            let mut cfg = saved;
            for id in on.iter().chain(off.iter()) {
                if eco::by_id(id).is_none() {
                    anyhow::bail!("no ecosystem called `{id}` (run `rtidy langs` to list them)");
                }
            }
            if rust_only {
                cfg.all_langs = false;
                cfg.ecos_off.clear();
            }
            if !on.is_empty() {
                cfg.all_langs = true;
                cfg.ecos_off.retain(|o| !on.contains(o));
            }
            let changed = rust_only || !on.is_empty() || !off.is_empty();
            for id in off {
                if !cfg.ecos_off.contains(&id) {
                    cfg.ecos_off.push(id);
                }
            }
            if changed {
                cfg.save()?;
            }
            let live = cfg.ecos();
            for e in eco::ALL.iter() {
                let on = live.iter().any(|x| x.id == e.id);
                let fixed = if e.id == "cargo" { "  (always on)" } else { "" };
                println!("[{}] {:<10} {}{fixed}", if on { "x" } else { " " }, e.id, e.label);
            }
            println!();
            println!("other languages: {}", if cfg.all_langs { "on" } else { "off" });
            println!("rtidy langs --on node --on gradle   |   rtidy langs --rust-only");
            Ok(())
        }
        Cmd::Budget { free: target, roots } => {
            let mut cfg = saved;
            if !roots.is_empty() {
                cfg.roots = roots.iter().map(|r| util::tidy_abs(r)).collect();
            }
            let place = cfg.roots.first().cloned().or_else(dirs::home_dir).unwrap_or_default();
            let before = free_space(&place).unwrap_or(0);
            if before >= target {
                println!("{} free, already at or above {}", human(before), human(target));
                return Ok(());
            }
            let need = target - before;
            println!("{} free, need {} more", human(before), human(need));
            // Best value first, stopping as soon as the budget is met: a CI runner wants
            // the cheapest set of deletions that gets the build going, not a deep clean.
            let full = engine::plan_all(&cfg, 0, &cfg.ecos());
            let mut taken = plan::Plan::default();
            for a in full.actions {
                if taken.freed() >= need {
                    break;
                }
                taken.push(a);
            }
            let out = plan::apply(&taken, plan::Mode::Delete, "budget");
            let after = free_space(&place).unwrap_or(before + out.freed);
            println!("freed {}, now {} free", human(out.freed), human(after));
            if after < target {
                println!("could not reach {}", human(target));
                std::process::exit(3);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn auto_takes_catch_up_with_other_options() {
        let cli = Cli::try_parse_from(["rtidy", "auto", "/p", "--days", "7", "--catch-up", "03:17"]).unwrap();
        let Some(Cmd::Auto { catch_up, opts }) = cli.cmd else { panic!("not auto") };
        assert_eq!(catch_up.as_deref(), Some("03:17"));
        assert_eq!(opts.days, Some(7));
    }

    #[test]
    fn catch_up_is_only_for_auto_and_hidden() {
        assert!(Cli::try_parse_from(["rtidy", "schedule", "--catch-up", "03:17"]).is_err());
        let mut cmd = Cli::command();
        let help = cmd.find_subcommand_mut("auto").unwrap().render_long_help().to_string();
        assert!(!help.contains("catch-up"));
    }
}
