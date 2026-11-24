//! Finding where the Rust projects on this computer actually are.
//!
//! The old guess was a hardcoded list of folder names under the home directory
//! (`Documents`, `Downloads`, …) offered up whenever the folder merely existed. That
//! suggested places with no Rust in them at all, and missed the ones that mattered:
//! projects on a second drive were never found, because nothing outside the home
//! directory was ever looked at.
//!
//! This module searches for real evidence instead — a `Cargo.toml` on disk — starting
//! from every place a project could plausibly live on this OS (every fixed drive on
//! Windows, the usual mount points elsewhere). The search is bounded by depth, by a
//! visited-directory budget and by wall-clock time, so it stays predictable on a
//! machine with a very large or very slow filesystem.

use crate::util::plain_path;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use walkdir::WalkDir;

/// How deep below a starting point to look. A project sits at `D:\code\thing` (2) or
/// `C:\Users\me\source\repos\thing` (5) — beyond that it is almost always vendored
/// code inside another project.
const MAX_DEPTH: usize = 6;

/// Stop after visiting this many directories, however much is left.
const DIR_BUDGET: usize = 300_000;

/// Most roots to return.
const MAX_ROOTS: usize = 8;

/// Directories never worth descending into: version control and package caches
/// (huge, and any `Cargo.toml` inside is a dependency, not the user's project),
/// OS and application install trees, and rtidy's own concerns.
fn skip(name: &str) -> bool {
    // Any dotted directory. These are configuration and cache by convention, never
    // where someone keeps the project they are working on. Editors and AI tools in
    // particular keep whole copies of checkouts under `~/.kiro`, `~/.kimi` and
    // `~/.claude`, and those copies were being offered as the user's own projects.
    if name.starts_with('.') {
        return true;
    }
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        // caches and vendored code
        ".git" | ".hg" | ".svn" | "node_modules" | "target" | ".cargo" | ".rustup"
            | "vendor" | ".venv" | "venv" | "__pycache__" | ".gradle" | ".m2" | ".npm"
            | ".pnpm-store" | ".nuget" | ".conda" | "site-packages"
            // OS / app trees
            | "windows" | "winsxs" | "program files" | "program files (x86)" | "programdata"
            | "$recycle.bin" | "system volume information" | "recovery" | "appdata"
            | "windowsapps" | "msocache" | "perflogs"
            | "/proc" | "/sys" | "/dev"
            | "proc" | "sys" | "dev" | "lost+found"
            | "library" | "system" | "applications" | "private"
            // big non-code things people keep around
            | "steamlibrary" | "onedrivetemp"
    ) || lower.starts_with('$')
}

/// A directory holds a project if one of the enabled ecosystems claims it.
///
/// With other languages off this is `Cargo.toml` and nothing else, which is what root
/// discovery meant before the ecosystem table existed. With them on, a machine whose
/// code lives under a folder full of Node projects finally gets that folder offered.
fn is_project(dir: &Path, ecos: &[&'static crate::eco::Eco]) -> bool {
    ecos.iter().any(|e| e.claims(dir))
}

/// Every place on this OS worth starting a search from.
///
/// On Windows that is each drive letter that is actually mounted; elsewhere it is the
/// home directory plus the conventional places other filesystems get mounted.
pub fn search_seeds() -> Vec<PathBuf> {
    let mut seeds: Vec<PathBuf> = Vec::new();
    let mut push = |p: PathBuf| {
        if p.is_dir() && !seeds.contains(&p) {
            seeds.push(p);
        }
    };

    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            push(PathBuf::from(format!("{}:\\", letter as char)));
        }
    } else {
        push(PathBuf::from("/home"));
        push(PathBuf::from("/Users"));
        push(PathBuf::from("/mnt"));
        push(PathBuf::from("/media"));
        push(PathBuf::from("/Volumes"));
        push(PathBuf::from("/srv"));
        push(PathBuf::from("/opt"));
        push(PathBuf::from("/workspace"));
    }
    if let Some(home) = dirs::home_dir() {
        push(home);
    }
    seeds
}

/// Walk `seeds` and collect every directory that holds a `Cargo.toml`.
///
/// Stops descending once a project is found: a crate's subdirectories belong to that
/// project, so there is nothing below worth reporting as a separate root.
fn find_projects(seeds: &[PathBuf], budget: Duration, ecos: &[&'static crate::eco::Eco]) -> Vec<PathBuf> {
    let started = Instant::now();
    let mut visited = 0usize;
    let mut projects = Vec::new();

    for seed in seeds {
        // A drive root is depth 0, so allow the full depth below each seed.
        let mut it = WalkDir::new(seed).follow_links(false).max_depth(MAX_DEPTH).into_iter();
        while let Some(entry) = it.next() {
            if visited >= DIR_BUDGET || started.elapsed() >= budget {
                return projects;
            }
            let Ok(e) = entry else { continue };
            if !e.file_type().is_dir() {
                continue;
            }
            visited += 1;
            if e.depth() > 0 && skip(&e.file_name().to_string_lossy()) {
                it.skip_current_dir();
                continue;
            }
            if is_project(e.path(), ecos) {
                projects.push(e.path().to_path_buf());
                it.skip_current_dir();
            }
        }
    }
    projects
}

/// The folders that actually hold Rust projects on this computer, busiest first.
///
/// Returns the *parents* of the projects found, so the result is a short list of
/// places to watch rather than one entry per crate. A folder is dropped when one of
/// its ancestors is already in the list, so nothing is scanned twice later.
pub fn discover_roots(budget: Duration, ecos: &[&'static crate::eco::Eco]) -> Vec<PathBuf> {
    roots_from(find_projects(&search_seeds(), budget, ecos))
}

/// Group project directories into the smallest useful set of parent folders.
fn roots_from(projects: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();
    for p in projects {
        // A project directly on a drive root (`D:\thing`) makes the root itself the
        // folder to watch; otherwise it is the project's parent.
        let parent = p.parent().map(Path::to_path_buf).unwrap_or(p);
        *counts.entry(plain_path(&parent)).or_default() += 1;
    }

    let mut ranked: Vec<(PathBuf, usize)> = counts.into_iter().collect();
    // Most projects first; ties broken by path so the order is stable between runs.
    ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    let mut out: Vec<PathBuf> = Vec::new();
    for (dir, _) in ranked {
        if out.len() >= MAX_ROOTS {
            break;
        }
        // Skip anything already covered by a folder we kept.
        if out.iter().any(|kept| dir.starts_with(kept)) {
            continue;
        }
        out.push(dir);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_projects_under_their_parent() {
        let roots = roots_from(vec![
            PathBuf::from("/code/alpha"),
            PathBuf::from("/code/beta"),
            PathBuf::from("/elsewhere/gamma"),
        ]);
        // Busiest folder first.
        assert_eq!(roots[0], PathBuf::from("/code"));
        assert_eq!(roots[1], PathBuf::from("/elsewhere"));
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn drops_folders_already_covered_by_an_ancestor() {
        let roots = roots_from(vec![
            PathBuf::from("/code/a"),
            PathBuf::from("/code/b"),
            PathBuf::from("/code/c"),
            // Nested deeper: already inside /code, so it must not appear again.
            PathBuf::from("/code/nested/deep/d"),
        ]);
        assert_eq!(roots, vec![PathBuf::from("/code")]);
    }

    #[test]
    fn caps_the_number_of_roots() {
        let projects: Vec<PathBuf> = (0..40).map(|i| PathBuf::from(format!("/r{i}/proj"))).collect();
        assert_eq!(roots_from(projects).len(), MAX_ROOTS);
    }

    #[test]
    fn no_projects_means_no_roots() {
        assert!(roots_from(Vec::new()).is_empty());
    }

    #[test]
    fn caches_and_system_trees_are_skipped() {
        for name in ["node_modules", "target", "Windows", "Program Files", "$Recycle.Bin", "AppData"] {
            assert!(skip(name), "{name} should be skipped");
        }
        // Every dotted directory, including the editor and AI-tool caches that keep
        // whole copies of checkouts inside them.
        for name in [".git", ".cargo", ".rustup", ".kiro", ".kimi", ".claude", ".vscode", ".cache"] {
            assert!(skip(name), "{name} should be skipped");
        }
        for name in ["src", "projects", "Rtidy", "my-app", "code"] {
            assert!(!skip(name), "{name} should not be skipped");
        }
    }

    #[test]
    fn seeds_exist_on_this_machine() {
        // Whatever the OS, there must be at least one real place to search.
        let seeds = search_seeds();
        assert!(!seeds.is_empty());
        assert!(seeds.iter().all(|s| s.is_dir()));
    }
}
