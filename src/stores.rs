//! The other half of the disk: per-user package and build caches.
//!
//! rtidy's whole argument for Rust is that the mess is structural — every project keeps
//! its own `target/`, so you clean and it comes back — and that the fix is to make them
//! share one. The other toolchains already did that: npm, Gradle, Maven, NuGet, Go, uv,
//! pub and Hex all download into one place per user. So there is nothing to prevent
//! there, and claiming otherwise would be a feature that does nothing.
//!
//! What they do instead is grow forever, because nothing ever expires. That is what this
//! module is for: find each store, measure it, and plan away the entries nobody has
//! touched in months. The same preview, the same ranking and the same ledger as the rest.
//!
//! What this module deliberately does *not* do is relocate anyone's store. Those are
//! environment variables (`GOCACHE`, `GRADLE_USER_HOME`, `NUGET_PACKAGES`) read by
//! shells, CI scripts and IDEs that rtidy does not own; setting them behind someone's
//! back is how a toolchain stops finding its own packages. `rtidy doctor` prints the
//! variable and leaves the decision where it belongs.

use crate::eco::{self, Eco, Store};
use crate::plan::{Action, Kind, Plan};
use crate::util::{age_days, dir_size, human, mtime_of};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Found {
    pub eco: &'static Eco,
    pub store: &'static Store,
    pub path: PathBuf,
    pub size: u64,
}

impl Found {
    /// The environment variable that would move it, when there is one.
    pub fn env(&self) -> Option<&'static str> {
        self.store.env
    }
}

/// Every store of the enabled ecosystems that exists on this machine, largest first.
///
/// Sizing these is a lot of walking — `~/.m2/repository` alone can be six figures of
/// files — so they are measured in parallel and only when asked for.
pub fn survey(ecos: &[&'static Eco]) -> Vec<Found> {
    let targets: Vec<(&'static Eco, &'static Store, PathBuf)> = ecos
        .iter()
        .flat_map(|e| e.stores.iter().filter_map(move |s| eco::store_path(s).map(|p| (*e, s, p))))
        .collect();
    let mut out: Vec<Found> = targets
        .into_par_iter()
        .map(|(eco, store, path)| Found { size: dir_size(&path), eco, store, path })
        .collect();
    out.sort_unstable_by_key(|f| std::cmp::Reverse(f.size));
    out
}

pub fn total(found: &[Found]) -> u64 {
    found.iter().map(|f| f.size).sum()
}

/// How deep inside a store its individual entries sit.
///
/// A package cache is usually `<store>/<name>/<version>`, but Go's build cache and
/// npm's `_cacache` fan out into hashed buckets whose own timestamps say nothing about
/// the contents. Rather than special-case each one, go a level deeper whenever the top
/// level is just a handful of buckets.
fn entry_depth(dir: &Path) -> usize {
    let n = fs::read_dir(dir).into_iter().flatten().flatten().count();
    if n <= 32 { 2 } else { 1 }
}

fn entries(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let here: Vec<PathBuf> = fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    if depth <= 1 {
        return here;
    }
    here.into_iter()
        .flat_map(|p| {
            if p.is_dir() {
                let kids: Vec<PathBuf> = fs::read_dir(&p).into_iter().flatten().flatten().map(|e| e.path()).collect();
                if kids.is_empty() { vec![p] } else { kids }
            } else {
                vec![p]
            }
        })
        .collect()
}

/// Entries in these stores that nothing has touched for `days`.
///
/// `days` is deliberately its own setting and defaults high: a package you have not
/// built against in six months costs seconds to fetch again, while one from last week
/// is probably a dependency of something you are working on right now.
pub fn plan(ecos: &[&'static Eco], days: u64) -> Plan {
    let found = survey(ecos);
    let why = crate::i18n::tf("in a shared store, untouched for {} days", &[&days.to_string()]);
    let actions: Vec<Action> = found
        .par_iter()
        .flat_map(|f| {
            let depth = entry_depth(&f.path);
            entries(&f.path, depth)
                .into_par_iter()
                .filter_map(|p| {
                    let md = fs::metadata(&p).ok()?;
                    let age = age_days(mtime_of(&md));
                    if age < days {
                        return None;
                    }
                    let size = if md.is_file() { md.len() } else { dir_size(&p) };
                    // Not worth an entry in the plan, and the preview should not be ten
                    // thousand lines of nothing.
                    (size > 64 << 10).then(|| Action::new(Kind::Store, p, size, f.eco, age, why.clone()))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let mut p = Plan { actions, kept: Vec::new() };
    p.rank();
    p
}

/// One line per store, for `rtidy doctor` and `rtidy stores`.
pub fn report(ecos: &[&'static Eco]) {
    let found = survey(ecos);
    if found.is_empty() {
        println!("no package stores found for the enabled ecosystems");
        return;
    }
    for f in &found {
        let env = f.env().map(|v| format!("  (${v})")).unwrap_or_default();
        println!("{:>10}  {:<20} {}{env}", human(f.size), f.store.label, f.path.display());
    }
    println!("{} in {} store(s)", human(total(&found)), found.len());
    println!("these are shared per user already — rtidy trims them, and never moves them");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bucketed_store_is_read_one_level_deeper() {
        let d = std::env::temp_dir().join(format!("rtidy-stores-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        // Four buckets, each holding two entries: the entries are what should be seen.
        for b in 0..4 {
            for e in 0..2 {
                fs::create_dir_all(d.join(format!("bucket{b}")).join(format!("entry{e}"))).unwrap();
            }
        }
        assert_eq!(entry_depth(&d), 2);
        assert_eq!(entries(&d, entry_depth(&d)).len(), 8);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_wide_store_is_read_at_the_top_level() {
        let d = std::env::temp_dir().join(format!("rtidy-stores2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        for i in 0..40 {
            fs::create_dir_all(d.join(format!("pkg{i}"))).unwrap();
        }
        assert_eq!(entry_depth(&d), 1);
        assert_eq!(entries(&d, 1).len(), 40);
        let _ = fs::remove_dir_all(&d);
    }
}
