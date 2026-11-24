//! Per-project build output: find it, measure it, plan what goes.
//!
//! "Per-project build output" meant a Cargo `target/` dir and nothing else until the
//! ecosystem table arrived (`crate::eco`). The walk is the same walk — one pass, split
//! across the thread pool, with a prune cache remembering the subtrees that held
//! nothing — it just asks the table what counts instead of hard-coding Cargo.
//!
//! With other languages off, the table has one row in it and this behaves exactly as it
//! did before.

use crate::eco::{self, Eco};
use crate::plan::{Action, Kind, Plan};
use crate::rules::Rules;
use crate::say;
use crate::util::*;
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

pub struct Target {
    pub path: PathBuf,
    pub size: u64,
    pub last_build: SystemTime,
    pub keep: bool,
    /// Which ecosystem's output this is.
    pub eco: &'static Eco,
    /// The project's own rules, from `.rtidy.toml`.
    pub rules: Rules,
}

/// Directories no build output will ever be under.
///
/// A root is often a whole drive, and the time a scan takes is almost entirely the time
/// spent walking subtrees like `SteamLibrary` or `Windows` that cannot contain anything
/// rtidy would clean.
///
/// Dot-directories are deliberately *not* skipped here: worktrees under `.claude`,
/// `.worktrees` and the like hold real build output worth gigabytes. (Root *discovery*
/// does skip them — see `crate::discover` — because a checkout copied into a tool's
/// cache is not a project the user is working on.)
fn skip_dir(name: &str) -> bool {
    // ASCII-insensitive: Windows paths vary in case between machines.
    const SKIP: [&str; 17] = [
        // version control and the package caches rtidy treats as stores, not artifacts
        ".git",
        ".hg",
        ".svn",
        ".cargo",
        ".rustup",
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
    /// Build output directories, with the ecosystem that owns each.
    pub artifacts: Vec<(PathBuf, &'static Eco)>,
    /// Directories holding a manifest of an enabled ecosystem.
    ///
    /// The model used to learn about projects only from leftover `target/` folders, so
    /// the moment rtidy did its job and removed them it had nothing left to watch and
    /// never learned anything. Noticing the manifest as the walk passes it costs no
    /// extra syscall and keeps every project visible.
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
        self.artifacts.extend(other.artifacts);
        self.projects.extend(other.projects);
        self.overrides.extend(other.overrides);
        self.visited += other.visited;
        self.skipped += other.skipped;
        self
    }

    /// Nothing here that rtidy would ever act on or want to watch.
    ///
    /// A folder holding a project counts as interesting even with no build output in it
    /// yet: the model watches projects, and pruning them away would blind it again.
    fn is_empty_of_projects(&self) -> bool {
        self.artifacts.is_empty() && self.overrides.is_empty() && self.projects.is_empty()
    }
}

/// Everything one walk needs to know about what it is looking for.
struct Lens<'a> {
    ecos: &'a [&'static Eco],
    manifests: Vec<&'static str>,
}

impl<'a> Lens<'a> {
    fn new(ecos: &'a [&'static Eco]) -> Self {
        Self { manifests: eco::manifest_names(ecos), ecos }
    }
}

/// Walk one subtree, single-threaded.
fn walk_subtree(start: &Path, lens: &Lens) -> TreeScan {
    let mut out = TreeScan::default();
    let mut visited = 0u32;
    let mut it = WalkDir::new(start).follow_links(false).into_iter();
    while let Some(entry) = it.next() {
        let Ok(e) = entry else { continue };
        let name = e.file_name().to_string_lossy();
        if !e.file_type().is_dir() {
            // A manifest names its directory as a project. Seen in passing, so it costs
            // nothing beyond the listing the walk already did.
            if lens.manifests.iter().any(|m| name == *m)
                && let Some(parent) = e.path().parent()
            {
                out.projects.push(parent.to_path_buf());
            }
            continue;
        }
        // A `.cargo` directory means its *parent* may pin its own target-dir. Noticing
        // it as we pass costs nothing — the walk already listed it — and the config
        // files are then read for just those few directories.
        if name == ".cargo" {
            if let Some(parent) = e.path().parent()
                && let Some(p) = crate::shared::project_override(parent)
            {
                out.overrides.push(p);
            }
            it.skip_current_dir();
            continue;
        }
        // Artifacts rtidy moved aside earlier are not leftovers to report again.
        if crate::trash::is_trash(&name) {
            it.skip_current_dir();
            continue;
        }
        // A name an enabled ecosystem uses for build output. Whether or not the manifest
        // check confirms it, there is no reason to walk inside: either it is build output
        // (and its contents are not separately interesting) or it is a plain folder that
        // happens to be called `build`, in which case a project nested inside it is a
        // deliberate, rare arrangement rtidy would rather miss than go spelunking for.
        if eco::any_artifact_name(&name, lens.ecos) {
            if let Some(owner) = eco::artifact_of(e.path(), lens.ecos) {
                out.artifacts.push((e.path().to_path_buf(), owner));
            }
            it.skip_current_dir();
            continue;
        }
        if e.depth() > 0 && skip_dir(&name) {
            it.skip_current_dir();
            continue;
        }
        visited += 1;
    }
    out.visited = visited;
    out
}

/// How many levels below a root to cut into separate units of work.
///
/// One level does not share the load evenly: a drive root has a few enormous children
/// and a long tail of tiny ones, so a single-level split leaves one core walking the big
/// one while the others finish and idle. Two levels gives the pool enough pieces to keep
/// every core busy, without the bookkeeping of splitting deeper.
const SPLIT_DEPTH: usize = 2;

/// Collect the units of work below `start`, checking each directory on the way.
fn split_units(start: &Path, depth: usize, units: &mut Vec<PathBuf>, lens: &Lens) -> TreeScan {
    let mut here = TreeScan::default();
    if start.join(".cargo").is_dir()
        && let Some(p) = crate::shared::project_override(start)
    {
        here.overrides.push(p);
    }
    if lens.manifests.iter().any(|m| start.join(m).is_file()) {
        here.projects.push(start.to_path_buf());
    }
    if let Some(owner) = eco::artifact_of(start, lens.ecos) {
        here.artifacts.push((start.to_path_buf(), owner));
        return here; // nothing below an artifact dir is ours
    }
    for e in fs::read_dir(start).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.file_type().is_ok_and(|t| t.is_dir()) || skip_dir(&name) || crate::trash::is_trash(&name) {
            continue;
        }
        if depth > 1 {
            here = here.merge(split_units(&e.path(), depth - 1, units, lens));
        } else {
            units.push(e.path());
        }
    }
    here
}

/// A full pass that walks everything, ignoring the prune cache.
pub fn scan_all(roots: &[PathBuf], ecos: &[&'static Eco]) -> TreeScan {
    scan_inner(roots, None, ecos).0
}

/// The usual pass: skips subtrees a recent walk found to hold nothing.
///
/// See `crate::prune` for what that means and what it costs.
pub fn scan_tree(roots: &[PathBuf], ecos: &[&'static Eco]) -> TreeScan {
    let mut cache = crate::prune::Prune::load();
    // Turning another language on changes what counts as interesting, so notes taken
    // under the old answer cannot be trusted. Clearing them costs one slow scan once.
    let fingerprint: String = ecos.iter().map(|e| e.id).collect::<Vec<_>>().join(",");
    if cache.lens() != fingerprint {
        cache.relens(&fingerprint);
    }
    let (scan, learned) = scan_inner(roots, Some(&cache), ecos);
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

/// What one unit's walk taught us: its path, how many directories it held, and whether
/// it turned out to contain nothing rtidy acts on.
type Learned = (PathBuf, u32, bool);

fn scan_inner(roots: &[PathBuf], cache: Option<&crate::prune::Prune>, ecos: &[&'static Eco]) -> (TreeScan, Vec<Learned>) {
    let lens = Lens::new(ecos);
    let mut units: Vec<PathBuf> = Vec::new();
    let mut top = TreeScan::default();
    for root in roots {
        top = top.merge(split_units(root, SPLIT_DEPTH, &mut units, &lens));
    }

    let walked: Vec<(TreeScan, Option<Learned>)> = units
        .par_iter()
        .map(|u| {
            if cache.is_some_and(|c| c.can_skip(u)) {
                // Untouched since a walk that found nothing: leave it alone.
                return (TreeScan { skipped: 1, ..Default::default() }, None);
            }
            let scan = walk_subtree(u, &lens);
            let learned = (u.clone(), scan.visited, scan.is_empty_of_projects());
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

pub fn find_targets(roots: &[PathBuf], ecos: &[&'static Eco]) -> Vec<Target> {
    size_targets(scan_tree(roots, ecos).artifacts)
}

/// Measure the artifact dirs a scan turned up. Sizing is the slow part: do it in parallel.
pub fn size_targets(paths: Vec<(PathBuf, &'static Eco)>) -> Vec<Target> {
    let mut out: Vec<Target> = paths
        .into_par_iter()
        .map(|(p, eco)| {
            let rules = p.parent().map(Rules::for_project).unwrap_or_default();
            Target {
                size: dir_size(&p),
                last_build: last_build(&p),
                keep: rules.keep || rules.exempt(&p),
                eco,
                rules,
                path: p,
            }
        })
        .collect();
    out.sort_unstable_by_key(|t| std::cmp::Reverse(t.size));
    out
}

/// Newest mtime among the artifact dir's top-level entries.
fn last_build(p: &Path) -> SystemTime {
    fs::read_dir(p)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.metadata().ok()?.modified().ok())
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

pub fn scan(roots: &[PathBuf], deep: bool, ecos: &[&'static Eco]) {
    let scan = if deep { scan_all(roots, ecos) } else { scan_tree(roots, ecos) };
    let skipped = scan.skipped;
    let ts = size_targets(scan.artifacts);
    for t in &ts {
        let tag = if t.keep { " [keep]" } else { "" };
        let kind = if ecos.len() > 1 { format!("  {:<22}", t.eco.label) } else { String::new() };
        println!("{:>10}  {:>4}d ago{kind}  {}{tag}", human(t.size), age_days(t.last_build), t.path.display());
    }
    let total: u64 = ts.iter().map(|t| t.size).sum();
    println!("{} artifact dirs, {} total", ts.len(), human(total));
    if skipped > 0 {
        println!("skipped {skipped} folder(s) known to hold nothing (--deep to walk everything)");
    }
}

/// What would go, and why, without touching anything.
///
/// `days` is the floor; a project's `.rtidy.toml` can raise it for itself.
pub fn plan(found: &[Target], days: u64) -> Plan {
    let mut p = Plan::default();
    for t in found {
        if t.keep {
            let why = match &t.rules.from {
                Some(f) => crate::i18n::tf("kept by {}", &[&f.display().to_string()]),
                None => crate::i18n::t("kept by a .rtidy-keep marker").to_string(),
            };
            p.kept.push((t.path.clone(), why));
            continue;
        }
        let age = age_days(t.last_build);
        let floor = t.rules.days.unwrap_or(days);
        if age < floor {
            p.kept.push((
                t.path.clone(),
                crate::i18n::tf("built {} days ago (waits for {})", &[&age.to_string(), &floor.to_string()]),
            ));
            continue;
        }
        let why = crate::i18n::tf("not built for {} days", &[&age.to_string()]);
        p.push(Action::new(Kind::Artifact, t.path.clone(), t.size, t.eco, age, why));
    }
    p.rank();
    p
}

pub fn clean(roots: &[PathBuf], days: u64, dry_run: bool, ecos: &[&'static Eco], trash_days: u64) -> u64 {
    clean_found(&find_targets(roots, ecos), days, dry_run, trash_days)
}

/// `clean` for callers that already scanned, so the tree is walked and sized once.
pub fn clean_found(found: &[Target], days: u64, dry_run: bool, trash_days: u64) -> u64 {
    let p = plan(found, days);
    let mode = match (dry_run, trash_days) {
        (true, _) => crate::plan::Mode::Preview,
        (false, 0) => crate::plan::Mode::Delete,
        (false, _) => crate::plan::Mode::Trash,
    };
    if dry_run {
        for l in p.lines() {
            say!("would free {l}");
        }
    }
    let out = crate::plan::apply(&p, mode, "clean");
    let moved = if out.quarantined > 0 {
        crate::i18n::tf(" ({} recoverable)", &[&human(out.quarantined)])
    } else {
        String::new()
    };
    say!(
        "artifacts: {} {}{moved}",
        if dry_run { "would free" } else { "freed" },
        human(out.freed + out.quarantined)
    );
    out.freed + out.quarantined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_rule_can_raise_the_age_floor_for_itself() {
        let eco = eco::cargo();
        let old = SystemTime::now() - std::time::Duration::from_secs(40 * 86_400);
        let strict = Target {
            path: PathBuf::from("/a/target"),
            size: 1 << 30,
            last_build: old,
            keep: false,
            eco,
            rules: Rules { days: Some(90), ..Default::default() },
        };
        let plain = Target {
            path: PathBuf::from("/b/target"),
            size: 1 << 30,
            last_build: old,
            keep: false,
            eco,
            rules: Rules::default(),
        };
        let p = plan(&[strict, plain], 30);
        assert_eq!(p.actions.len(), 1);
        assert_eq!(p.actions[0].path, PathBuf::from("/b/target"));
        assert_eq!(p.kept.len(), 1, "the 90-day project is kept, with a reason");
    }

    #[test]
    fn kept_projects_never_enter_the_plan() {
        let t = Target {
            path: PathBuf::from("/a/target"),
            size: 9,
            last_build: SystemTime::UNIX_EPOCH,
            keep: true,
            eco: eco::cargo(),
            rules: Rules::default(),
        };
        let p = plan(&[t], 0);
        assert!(p.is_empty() && p.kept.len() == 1);
    }
}
