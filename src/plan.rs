//! Decide first, then act.
//!
//! Every clean-up used to be one function that chose a folder, deleted it and printed a
//! line about it, all in the same breath. That is why the app had no preview: there was
//! no moment at which rtidy knew what it was about to do and had not yet done it, and
//! nothing to hand the window except text after the fact.
//!
//! A `Plan` is that moment. It is a list of `Action`s with sizes, ages, the model's reuse
//! chance and what each one costs to rebuild — enough to show, to sort, to filter, to
//! print as JSON for a CI job, and to total up before anything is touched. `apply` is the
//! only thing in rtidy that removes anything, which is also the only sane place to put
//! the trash, the ledger and the per-ecosystem accounting.

use crate::eco::Eco;
use crate::util::{human, remove_known};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A project's own build output directory (`target/`, `node_modules`, `bin`, …).
    /// The only kind that goes to the trash: the only kind whose loss would hurt.
    Artifact,
    /// Incremental-compilation cache inside the shared dir: pure speed-up data.
    Incremental,
    /// One build unit in the shared dir.
    Unit,
    /// Downloaded package cache (`~/.cargo/registry`, npm cache, …).
    Cache,
    /// A per-user toolchain store rtidy trims but never relocates.
    Store,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Artifact => "artifact",
            Kind::Incremental => "incremental",
            Kind::Unit => "build unit",
            Kind::Cache => "cache",
            Kind::Store => "store",
        }
    }

    /// Recoverable kinds. A package cache re-downloads itself and a build unit rebuilds
    /// itself from a cache that is still there; keeping copies of either would hold back
    /// space for no real gain, and would put tens of thousands of entries in the index.
    fn trashable(self) -> bool {
        self == Kind::Artifact
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Action {
    pub kind: Kind,
    pub path: PathBuf,
    pub size: u64,
    /// Ecosystem id (`crate::eco`), or `cargo` for the shared dir and its caches.
    pub eco: String,
    /// Days since this was last used or built.
    pub age_days: u64,
    /// The model's chance it gets used again inside the horizon, when Smart mode is on.
    pub reuse: Option<f64>,
    /// Estimated seconds to produce it again. The other half of the trade rtidy is
    /// making on the user's behalf: 12 GB back for 40 seconds is a good deal, and
    /// 80 MB back for nine minutes is not.
    pub rebuild_secs: f64,
    /// Why it is in the plan, already translated, for the preview.
    pub why: String,
}

impl Action {
    pub fn new(kind: Kind, path: PathBuf, size: u64, eco: &Eco, age_days: u64, why: String) -> Self {
        Self {
            kind,
            path,
            size,
            eco: eco.id.to_string(),
            age_days,
            reuse: None,
            rebuild_secs: eco.rebuild_secs(size),
            why,
        }
    }

    /// How this action reads in a list: size, age, kind, reuse chance, rebuild estimate.
    pub fn line(&self) -> String {
        let reuse = self.reuse.map(|p| format!("  {:>3.0}% reuse", p * 100.0)).unwrap_or_default();
        format!(
            "{:>10}  {:>4}d  {:<11}{reuse}  ~{} rebuild  {}",
            human(self.size),
            self.age_days,
            self.kind.label(),
            rebuild_time(self.rebuild_secs),
            self.path.display()
        )
    }

    /// Bytes freed per second of rebuilding. The ranking that matters: size alone puts a
    /// slow, nearly-finished build at the top just because it is big.
    pub fn value(&self) -> f64 {
        let secs = self.rebuild_secs.max(0.5);
        // An artifact the model expects to be used again is worth much less to delete,
        // however big it is.
        let keepish = self.reuse.unwrap_or(0.0);
        (self.size as f64 / secs) * (1.0 - keepish)
    }
}

#[derive(Clone, Default, Debug, Serialize)]
pub struct Plan {
    pub actions: Vec<Action>,
    /// Things rtidy decided *not* to do, and why — shown in the preview so the absence
    /// of an obvious candidate is explained rather than mysterious.
    pub kept: Vec<(PathBuf, String)>,
}

impl Plan {
    pub fn freed(&self) -> u64 {
        self.actions.iter().map(|a| a.size).sum()
    }

    pub fn rebuild_secs(&self) -> f64 {
        self.actions.iter().map(|a| a.rebuild_secs).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    pub fn push(&mut self, a: Action) {
        self.actions.push(a);
    }

    pub fn extend(&mut self, other: Plan) {
        self.actions.extend(other.actions);
        self.kept.extend(other.kept);
    }

    /// Best value first: most bytes per second of rebuilding.
    pub fn rank(&mut self) {
        self.actions.sort_unstable_by(|a, b| b.value().total_cmp(&a.value()));
    }

    pub fn by_kind(&self, k: Kind) -> impl Iterator<Item = &Action> {
        self.actions.iter().filter(move |a| a.kind == k)
    }

    /// Bytes per ecosystem id, largest first.
    pub fn by_eco(&self) -> Vec<(String, u64)> {
        let mut m: std::collections::BTreeMap<String, u64> = Default::default();
        for a in &self.actions {
            *m.entry(a.eco.clone()).or_default() += a.size;
        }
        let mut v: Vec<(String, u64)> = m.into_iter().collect();
        v.sort_unstable_by_key(|(_, b)| std::cmp::Reverse(*b));
        v
    }

    /// One line per action, for `--dry-run` and for the app's preview list.
    pub fn lines(&self) -> Vec<String> {
        self.actions.iter().map(Action::line).collect()
    }

    /// The whole plan as JSON, for CI and for scripting (`--json`).
    pub fn to_json(&self) -> String {
        #[derive(Serialize)]
        struct Out<'a> {
            freed: u64,
            rebuild_secs: u64,
            actions: &'a [Action],
            by_eco: Vec<(String, u64)>,
        }
        serde_json::to_string_pretty(&Out {
            freed: self.freed(),
            rebuild_secs: self.rebuild_secs().round() as u64,
            actions: &self.actions,
            by_eco: self.by_eco(),
        })
        .unwrap_or_default()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Work out what would happen and touch nothing.
    Preview,
    /// Artifacts go to the trash (recoverable); caches are removed.
    Trash,
    /// Everything is removed outright.
    Delete,
}

#[derive(Default, Debug)]
pub struct Outcome {
    pub freed: u64,
    /// Bytes moved to the trash: on disk until the trash is purged, which is why this
    /// is reported apart from `freed` instead of being added to it.
    pub quarantined: u64,
    pub done: usize,
    pub failed: Vec<(PathBuf, String)>,
}

/// Carry out a plan. The only place in rtidy that removes anything.
pub fn apply(plan: &Plan, mode: Mode, how: &str) -> Outcome {
    let mut out = Outcome::default();
    if mode == Mode::Preview {
        out.freed = plan.freed();
        out.done = plan.actions.len();
        return out;
    }
    let run = crate::brain::now();
    let mut per_eco: Vec<(String, u64)> = Vec::new();
    let mut add = |eco: &str, bytes: u64| match per_eco.iter_mut().find(|(e, _)| e == eco) {
        Some((_, b)) => *b += bytes,
        None => per_eco.push((eco.to_string(), bytes)),
    };

    for a in &plan.actions {
        if !a.path.exists() {
            continue; // gone since the plan was made: a build, or another run
        }
        let to_trash = mode == Mode::Trash && a.kind.trashable();
        if to_trash {
            match crate::trash::stash(&a.path, a.size, run, &a.eco) {
                Ok(_) => {
                    out.quarantined += a.size;
                    out.done += 1;
                    add(&a.eco, a.size);
                    crate::say!("moved aside {} ({}) — recoverable", a.path.display(), human(a.size));
                    continue;
                }
                // A rename that cannot happen is not a reason to leave a full disk full;
                // fall through and delete, saying so.
                Err(e) => crate::say!("note: couldn't move {} aside ({e}); deleting instead", a.path.display()),
            }
        }
        let got = remove_known(&a.path, a.size, false);
        if got == 0 && a.size > 0 {
            out.failed.push((a.path.clone(), String::from("in use?")));
            crate::say!("skip (in use?) {}", a.path.display());
            continue;
        }
        out.freed += got;
        out.done += 1;
        add(&a.eco, got);
    }
    crate::ledger::Ledger::record(how, out.freed + out.quarantined, &per_eco);
    out
}

/// What the plan amounts to, in one translated line.
pub fn summary(plan: &Plan) -> String {
    crate::i18n::tf(
        "{} to free · about {} to rebuild if you need it again",
        &[&human(plan.freed()), &rebuild_time(plan.rebuild_secs())],
    )
}

/// A rebuild estimate in the unit that carries information.
///
/// Rounding to minutes turned twenty seconds into "about 0 min", which reads as "free"
/// — the opposite of what the number is there to say.
pub fn rebuild_time(secs: f64) -> String {
    let secs = secs.max(0.0);
    if secs < 90.0 {
        crate::i18n::tf("{} s", &[&format!("{secs:.0}")])
    } else if secs < 5400.0 {
        crate::i18n::tf("{} min", &[&format!("{:.0}", secs / 60.0)])
    } else {
        crate::i18n::tf("{} h", &[&format!("{:.1}", secs / 3600.0)])
    }
}

/// How many entries of one kind are worth listing before they stop being information.
pub const LIST_CAP: usize = 40;

/// Does this path sit inside a directory rtidy is already planning to remove?
///
/// Deleting a parent and then a child is both wasted work and a misleading total, since
/// the child's bytes get counted twice.
pub fn covered(path: &Path, actions: &[Action]) -> bool {
    actions.iter().any(|a| a.path != path && path.starts_with(&a.path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn act(size: u64, secs: f64, reuse: Option<f64>) -> Action {
        let mut a = Action::new(Kind::Artifact, PathBuf::from("/x"), size, crate::eco::cargo(), 10, String::new());
        a.rebuild_secs = secs;
        a.reuse = reuse;
        a
    }

    #[test]
    fn ranking_prefers_cheap_to_rebuild_over_merely_big() {
        let big_slow = act(8 << 30, 1200.0, None);
        let small_fast = act(1 << 30, 5.0, None);
        assert!(small_fast.value() > big_slow.value());
    }

    #[test]
    fn a_likely_rebuild_is_worth_less_to_delete() {
        let cold = act(1 << 30, 10.0, Some(0.05));
        let warm = act(1 << 30, 10.0, Some(0.95));
        assert!(cold.value() > warm.value());
    }

    #[test]
    fn preview_touches_nothing_and_still_totals() {
        let mut p = Plan::default();
        p.push(act(1 << 20, 1.0, None));
        let o = apply(&p, Mode::Preview, "test");
        assert_eq!((o.freed, o.done, o.quarantined), (1 << 20, 1, 0));
    }

    #[test]
    fn children_of_a_planned_delete_are_recognised() {
        let a = [Action::new(Kind::Artifact, PathBuf::from("/a/target"), 1, crate::eco::cargo(), 0, String::new())];
        assert!(covered(Path::new("/a/target/debug"), &a));
        assert!(!covered(Path::new("/a/other"), &a));
        assert!(!covered(Path::new("/a/target"), &a), "an action does not cover itself");
    }
}
