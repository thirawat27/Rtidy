//! Per-project `target/` folders: find, list, delete.

use crate::say;
use crate::util::*;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

/// Put this empty file next to Cargo.toml to protect a project's target/ from cleaning.
pub const KEEP_MARKER: &str = ".rtidy-keep";

pub struct Target {
    pub path: PathBuf,
    pub size: u64,
    pub last_build: SystemTime,
    pub keep: bool,
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

/// Directories a Rust build output will never be under.
///
/// A root is often a whole drive, and the time a scan takes is almost entirely the
/// time spent walking subtrees like `SteamLibrary` or `Windows` that cannot contain
/// anything rtidy would clean.
///
/// Dot-directories are deliberately *not* skipped here: worktrees under `.claude`,
/// `.worktrees` and the like hold real `target/` folders worth gigabytes. (Root
/// *discovery* does skip them — see `crate::discover` — because a checkout copied
/// into a tool's cache is not a project the user is working on.)
fn skip_dir(name: &str) -> bool {
    // ASCII-insensitive: Windows paths vary in case between machines.
    const SKIP: [&str; 20] = [
        // package caches and vendored code: any target/ in here belongs to a dependency
        ".git",
        ".hg",
        ".svn",
        "node_modules",
        ".cargo",
        ".rustup",
        ".nuget",
        ".gradle",
        // OS and application trees
        "appdata",
        "$recycle.bin",
        "system volume information",
        "windows",
        "winsxs",
        "program files",
        "program files (x86)",
        "programdata",
        "windowsapps",
        "msocache",
        "perflogs",
        "$winreagent",
    ];
    SKIP.iter().any(|s| name.eq_ignore_ascii_case(s))
}

/// What one pass over the project tree finds.
#[derive(Default)]
pub struct TreeScan {
    /// Cargo `target/` directories.
    pub targets: Vec<PathBuf>,
    /// Directories holding a `Cargo.toml`.
    ///
    /// The model used to learn about projects only from leftover `target/` folders,
    /// so the moment rtidy did its job and removed them it had nothing left to watch
    /// and never learned anything. Noticing the manifest as the walk passes it costs
    /// no extra syscall and keeps every project visible.
    pub projects: Vec<PathBuf>,
    /// Project-local `.cargo/config.toml` files that set their own `build.target-dir`.
    pub overrides: Vec<PathBuf>,
    /// Directories actually walked, for the prune cache and for reporting.
    pub visited: u32,
    /// Directories a prune note let this scan skip.
    pub skipped: u32,
}

impl TreeScan {
    fn merge(mut self, other: Self) -> Self {
        self.targets.extend(other.targets);
        self.projects.extend(other.projects);
        self.overrides.extend(other.overrides);
        self.visited += other.visited;
        self.skipped += other.skipped;
        self
    }

    /// Nothing here that rtidy would ever act on or want to watch.
    ///
    /// A folder holding a project counts as interesting even with no `target/` in it
    /// yet: the model watches projects, and pruning them away would blind it again.
    fn is_empty_of_rust(&self) -> bool {
        self.targets.is_empty() && self.overrides.is_empty() && self.projects.is_empty()
    }
}

/// Walk one subtree, single-threaded.
fn walk_subtree(start: &Path) -> TreeScan {
    let mut out = TreeScan::default();
    let mut visited = 0u32;
    let mut it = WalkDir::new(start).follow_links(false).into_iter();
    while let Some(entry) = it.next() {
        let Ok(e) = entry else { continue };
        let name = e.file_name().to_string_lossy();
        if !e.file_type().is_dir() {
            // A manifest names its directory as a project. Seen in passing, so it
            // costs nothing beyond the listing the walk already did.
            if name == "Cargo.toml"
                && let Some(parent) = e.path().parent()
            {
                out.projects.push(parent.to_path_buf());
            }
            continue;
        }
        // A `.cargo` directory means its *parent* may pin its own target-dir.
        // Noticing it as we pass costs nothing — the walk already listed it — and
        // the config files are then read for just those few directories. Asking
        // every directory whether it has a `.cargo` child cost one stat each, and
        // on a drive-sized root those stats were most of the scan.
        if name == ".cargo" {
            if let Some(parent) = e.path().parent()
                && let Some(p) = crate::shared::project_override(parent)
            {
                out.overrides.push(p);
            }
            it.skip_current_dir();
            continue;
        }
        if e.depth() > 0 && skip_dir(&name) {
            it.skip_current_dir();
            continue;
        }
        visited += 1;
        if is_cargo_target(e.path()) {
            out.targets.push(e.path().to_path_buf());
            it.skip_current_dir();
        }
    }
    out.visited = visited;
    out
}

/// How many levels below a root to cut into separate units of work.
///
/// One level does not share the load evenly: a drive root has a few enormous children
/// and a long tail of tiny ones, so a single-level split leaves one core walking the
/// big one while the others finish and idle. Two levels gives the pool enough pieces
/// to keep every core busy, without the bookkeeping of splitting deeper.
const SPLIT_DEPTH: usize = 2;

/// Collect the units of work below `start`, checking each directory on the way.
///
/// Directories above the split depth are handled here and now; everything below
/// becomes a unit for the pool.
fn split_units(start: &Path, depth: usize, units: &mut Vec<PathBuf>) -> TreeScan {
    let mut here = TreeScan::default();
    if start.join(".cargo").is_dir()
        && let Some(p) = crate::shared::project_override(start)
    {
        here.overrides.push(p);
    }
    if start.join("Cargo.toml").is_file() {
        here.projects.push(start.to_path_buf());
    }
    if is_cargo_target(start) {
        here.targets.push(start.to_path_buf());
        return here; // nothing below a target dir is ours
    }
    for e in fs::read_dir(start).into_iter().flatten().flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) || skip_dir(&e.file_name().to_string_lossy()) {
            continue;
        }
        if depth > 1 {
            here = here.merge(split_units(&e.path(), depth - 1, units));
        } else {
            units.push(e.path());
        }
    }
    here
}

/// One pass over every root, with the subtrees walked in parallel.
///
/// Walking a drive-sized root one directory at a time was the slowest thing rtidy
/// did. The tree is cut into independent subtrees and handed to the thread pool,
/// which sizes itself to the machine: this uses the cores that are actually there,
/// rather than spawning a thread per folder.
/// A full pass that walks everything, ignoring the prune cache.
pub fn scan_all(roots: &[PathBuf]) -> TreeScan {
    scan_inner(roots, None).0
}

/// The usual pass: skips subtrees a recent walk found to hold no Rust.
///
/// See `crate::prune` for what that means and what it costs.
pub fn scan_tree(roots: &[PathBuf]) -> TreeScan {
    let mut cache = crate::prune::Prune::load();
    let (scan, learned) = scan_inner(roots, Some(&cache));
    // Fold in what this pass learned, then persist.
    for (dir, dirs, empty) in learned {
        if empty {
            cache.remember_empty(&dir, dirs);
        } else {
            cache.forget(&dir);
        }
    }
    cache.tidy();
    let _ = cache.save();
    scan
}

/// What one unit's walk taught us: its path, how many directories it held, and
/// whether it turned out to contain nothing rtidy acts on.
type Learned = (PathBuf, u32, bool);

/// Returns the scan plus what each unit taught the prune cache.
fn scan_inner(roots: &[PathBuf], cache: Option<&crate::prune::Prune>) -> (TreeScan, Vec<Learned>) {
    let mut units: Vec<PathBuf> = Vec::new();
    let mut top = TreeScan::default();
    for root in roots {
        top = top.merge(split_units(root, SPLIT_DEPTH, &mut units));
    }

    let walked: Vec<(TreeScan, Option<Learned>)> = units
        .par_iter()
        .map(|u| {
            if cache.is_some_and(|c| c.can_skip(u)) {
                // Untouched since a walk that found nothing: leave it alone.
                return (TreeScan { skipped: 1, ..Default::default() }, None);
            }
            let scan = walk_subtree(u);
            let learned = (u.clone(), scan.visited, scan.is_empty_of_rust());
            (scan, Some(learned))
        })
        .collect();

    let mut out = top;
    let mut learned: Vec<Learned> = Vec::new();
    for (scan, note) in walked {
        out = out.merge(scan);
        learned.extend(note);
    }
    (out, learned)
}

pub fn find_targets(roots: &[PathBuf]) -> Vec<Target> {
    size_targets(scan_tree(roots).targets)
}

/// Measure the target dirs a scan turned up. Sizing is the slow part: do it in parallel.
pub fn size_targets(paths: Vec<PathBuf>) -> Vec<Target> {
    let mut out: Vec<Target> = paths
        .into_par_iter()
        .map(|p| Target {
            size: dir_size(&p),
            last_build: last_build(&p),
            keep: p.parent().is_some_and(|proj| proj.join(KEEP_MARKER).exists()),
            path: p,
        })
        .collect();
    out.sort_unstable_by_key(|t| std::cmp::Reverse(t.size));
    out
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

pub fn scan(roots: &[PathBuf], deep: bool) {
    let scan = if deep { scan_all(roots) } else { scan_tree(roots) };
    let skipped = scan.skipped;
    let ts = size_targets(scan.targets);
    for t in &ts {
        let tag = if t.keep { " [keep]" } else { "" };
        println!("{:>10}  {:>4}d ago  {}{tag}", human(t.size), age_days(t.last_build), t.path.display());
    }
    let total: u64 = ts.iter().map(|t| t.size).sum();
    println!("{} target dirs, {} total", ts.len(), human(total));
    if skipped > 0 {
        println!("skipped {skipped} folder(s) known to hold no Rust (--deep to walk everything)");
    }
}

pub fn clean(roots: &[PathBuf], days: u64, dry_run: bool) -> u64 {
    clean_found(find_targets(roots), days, dry_run)
}

/// `clean` for callers that already scanned, so the tree is walked and sized once.
pub fn clean_found(found: Vec<Target>, days: u64, dry_run: bool) -> u64 {
    let victims: Vec<Target> = found.into_iter().filter(|t| !t.keep && age_days(t.last_build) >= days).collect();

    // Removing a folder is I/O bound and the folders are independent. On NTFS in
    // particular, deleting several large target dirs at once finishes far sooner than
    // one after another, where each removal waits on the filesystem by itself.
    // Results are collected first so the log still reads in a stable order.
    let done: Vec<(PathBuf, u64, u64)> = victims
        .into_par_iter()
        .map(|t| {
            let got = remove_known(&t.path, t.size, dry_run);
            (t.path, t.size, got)
        })
        .collect();

    let mut freed = 0;
    for (path, size, got) in done {
        if got == 0 && size > 0 {
            say!("skip (in use?) {}", path.display());
            continue;
        }
        say!("{} {} ({})", if dry_run { "would delete" } else { "deleted" }, path.display(), human(got));
        freed += got;
    }
    say!("targets: {} {}", if dry_run { "would free" } else { "freed" }, human(freed));
    freed
}
