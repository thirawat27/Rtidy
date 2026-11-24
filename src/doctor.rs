//! `rtidy doctor`: where did the disk space go, and what to do about it.

use crate::util::*;
use crate::{cache, shared, targets};
use anyhow::Result;
use std::path::PathBuf;

fn ok(msg: &str) {
    println!("  [ok]   {msg}");
}
fn warn(msg: &str, fix: &str) {
    println!("  [warn] {msg}");
    println!("         fix: {fix}");
}
fn info(msg: &str) {
    println!("  [info] {msg}");
}

pub fn run(roots: &[PathBuf]) -> Result<()> {
    let roots_disp: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
    println!("rtidy doctor  (scanning {})\n", roots_disp.join(", "));

    // --- disk ---
    println!("disk");
    if let Some(free) = roots.first().and_then(|r| free_space(r)) {
        if free < 20 << 30 {
            warn(&format!("only {} free", human(free)), "rtidy auto --cache-days 30 <roots>");
        } else {
            ok(&format!("{} free", human(free)));
        }
    }

    // --- shared target dir (the root-cause fix) ---
    println!("\nshared target dir");
    let (_, _, doc) = read_cargo_config()?;
    let shared_dir = shared::shared_target_dir()?;
    match &shared_dir {
        Some(d) => {
            ok(&format!("{} ({})", d.display(), human(dir_size(d))));
            if std::env::var("CARGO_TARGET_DIR").is_ok() {
                info("set via CARGO_TARGET_DIR env var (overrides ~/.cargo/config.toml)");
            }
        }
        None => warn(
            "not set: every project creates its own target/ folder",
            "rtidy setup --lean",
        ),
    }
    if shared::is_lean(&doc) {
        ok("lean: dependencies built without debug info");
    } else {
        warn("dependencies built with full debug info (often ~50% of target size)", "rtidy setup --lean");
    }

    // --- leftovers and overrides ---
    println!("\nprojects");
    // One pass over the project tree for both the target dirs and the overrides.
    let scan = targets::scan_tree(roots);
    let overrides = scan.overrides;
    let ts = targets::size_targets(scan.targets);
    let total: u64 = ts.iter().map(|t| t.size).sum();
    if ts.is_empty() {
        ok("no per-project target/ folders");
    } else {
        let msg = format!("{} per-project target/ folders, {}", ts.len(), human(total));
        if shared_dir.is_some() {
            warn(&format!("{msg} (leftovers from before setup)"), "rtidy clean --days 0 <roots>");
        } else {
            warn(&msg, "rtidy setup --lean, then rtidy clean --days 0 <roots>");
        }
        for t in ts.iter().take(5) {
            println!("         {:>10}  {}", human(t.size), t.path.display());
        }
    }
    for p in &overrides {
        warn(
            &format!("{} sets its own target-dir", p.display()),
            "remove build.target-dir there to use the shared dir",
        );
    }

    // --- cargo caches ---
    println!("\ncargo download cache");
    let c = cache::sizes()?;
    let csum = c.registry_src + c.registry_cache + c.git;
    let line = format!(
        "{} (sources {}, .crate {}, git {})",
        human(csum),
        human(c.registry_src),
        human(c.registry_cache),
        human(c.git)
    );
    if csum > 5 << 30 { warn(&line, "rtidy cache --days 30") } else { ok(&line) }

    // --- toolchains ---
    println!("\nrust toolchains");
    let tcs = shared::toolchain_sizes();
    for (name, size) in &tcs {
        println!("         {:>10}  {name}", human(*size));
    }
    let dated = tcs.iter().filter(|(n, _)| n.starts_with("nightly-20") || n.starts_with("1.")).count();
    if tcs.len() > 2 && dated > 0 {
        warn(
            &format!("{} toolchains installed, {dated} pinned/dated", tcs.len()),
            "rustup toolchain uninstall <name> for ones you no longer use",
        );
    } else if !tcs.is_empty() {
        ok(&format!("{} toolchain(s)", tcs.len()));
    }

    // --- automation ---
    println!("\nautomation");
    if crate::schedule::is_installed() {
        match crate::schedule::problem() {
            Some((what, fix)) => warn(&what, fix),
            None => ok(&format!("daily clean-up registered ({})", crate::schedule::mechanism())),
        }
    } else {
        warn("no scheduled clean-up", "rtidy schedule <roots>");
    }
    if cfg!(windows)
        && let Some(d) = &shared_dir
    {
        info(&format!(
            "builds are faster if Defender skips the target dir (admin PowerShell):\n         Add-MpPreference -ExclusionPath \"{}\"",
            d.display()
        ));
    }
    if let Ok(log) = log_path()
        && let Some(last) = read_tail(&log, 8 << 10).lines().last().map(str::to_string)
    {
        info(&format!("last run: {last}"));
    }
    Ok(())
}
