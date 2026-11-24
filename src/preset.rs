//! Ready-made settings, so nobody has to understand eight numbers to get started.
//!
//! Settings has eight thresholds in it — how old a leftover must be, how long the shared
//! dir keeps a build unit, the size cap, the download cache, the quarantine, the package
//! stores, the idle window — and every one of them is a real decision with a real
//! trade-off. That is fine for someone who wants to tune it, and no help at all to
//! someone who just knows that their laptop is full, or that their build machine must
//! never stall.
//!
//! A preset is one of those answers, written down: pick "Small disk" and the eight
//! numbers move together to the shape that fits a small disk. Nothing here is special —
//! every preset only writes the same fields the Settings page already has, so a preset
//! is a starting point to adjust, never a mode the rest of the program has to know about.
//!
//! What presets deliberately leave alone: the project folders, the language of the app,
//! the clean-up time, and the other-languages switch with the ecosystems ticked off under
//! it. Those are about *this* machine and this person, not about how hard to clean —
//! and the language switch especially, because it changes *what* rtidy may delete rather
//! than how eagerly. It stays off until someone turns it on, whatever preset is picked.

use crate::config::Config;
use crate::i18n::t;

pub struct Preset {
    /// Stable id for the command line and the settings file.
    pub id: &'static str,
    /// Shown on the button.
    pub name: &'static str,
    /// One line under it: who this is for.
    pub about: &'static str,
    apply: fn(&mut Config),
}

pub static ALL: [Preset; 5] = [
    Preset {
        id: "careful",
        name: "Careful",
        about: "Only what is clearly dead. Keeps a fortnight of undo.",
        apply: |c| {
            c.days = 90;
            c.gc_days = 45;
            c.max_size_gb = 0; // no cap: nothing is removed just for being big
            c.cache_days = 0;  // leave the download cache alone entirely
            c.trash_days = 14;
            c.store_days = 0;
            c.smart = true;
            c.smart_schedule = true;
            c.auto_enabled = true;
            c.watch_idle_mins = 30;
        },
    },
    Preset {
        id: "balanced",
        name: "Balanced",
        about: "The default. Tidy without thinking about it.",
        apply: |c| {
            c.days = 30;
            c.gc_days = 14;
            c.max_size_gb = 30;
            c.cache_days = 60;
            c.trash_days = 3;
            c.store_days = 120;
            c.smart = true;
            c.smart_schedule = true;
            c.auto_enabled = true;
            c.watch_idle_mins = 10;
        },
    },
    Preset {
        id: "small-disk",
        name: "Small disk",
        about: "A 256 GB laptop that keeps filling up. Cleans harder and sooner.",
        apply: |c| {
            c.days = 7;
            c.gc_days = 5;
            c.max_size_gb = 15;
            c.cache_days = 14;
            // One day of undo. On a disk this tight, a long quarantine is just the
            // problem again under another name.
            c.trash_days = 1;
            c.store_days = 60;
            c.smart = true;
            c.smart_schedule = true;
            c.auto_enabled = true;
            c.watch_idle_mins = 5;
        },
    },
    Preset {
        id: "workstation",
        name: "Workstation",
        about: "Builds must never stall. Patient thresholds, a week of undo, Rust only.",
        apply: |c| {
            c.days = 60;
            c.gc_days = 30;
            c.max_size_gb = 60;
            c.cache_days = 90;
            c.trash_days = 7;
            c.store_days = 0;
            c.smart = true;
            c.smart_schedule = true;
            c.auto_enabled = true;
            c.watch_idle_mins = 20;
        },
    },
    Preset {
        id: "ci",
        name: "CI runner",
        about: "A machine nobody sits at: no quarantine, no daily task, nothing held back.",
        apply: |c| {
            c.days = 0;
            c.gc_days = 1;
            c.max_size_gb = 10;
            c.cache_days = 7;
            // Nothing to undo on a runner, and holding bytes back is the one thing a
            // build agent cannot afford.
            c.trash_days = 0;
            c.store_days = 30;
            // A predictable runner beats a clever one: the same inputs should clean the
            // same amount every time, and a model that has learned nothing yet would
            // make the first builds behave differently from the hundredth.
            c.smart = false;
            c.smart_schedule = false;
            // The pipeline decides when to clean, not a daily task.
            c.auto_enabled = false;
            c.watch_idle_mins = 10;
        },
    },
];

pub fn by_id(id: &str) -> Option<&'static Preset> {
    ALL.iter().find(|p| p.id == id)
}

impl Preset {
    /// This preset's settings, over the top of `base` (which keeps its folders,
    /// language, clean-up time and per-ecosystem ticks).
    pub fn over(&self, base: &Config) -> Config {
        let mut c = base.clone();
        (self.apply)(&mut c);
        c
    }

    /// The translated name and description, for the UI.
    pub fn label(&self) -> &'static str {
        t(self.name)
    }

    pub fn detail(&self) -> &'static str {
        t(self.about)
    }

    /// Every line `diff` can produce, in order. English, translated where shown.
    pub const LABELS: [&'static str; 7] = [
        "Leftover build folders older than",
        "Shared folder: unused for",
        "Shared folder size cap",
        "Cargo downloads older than",
        "Recoverable for",
        "Package stores: untouched for",
        "Smart mode",
    ];

    /// The lines a preset changes, as `("Leftovers older than", "30 days" -> "7 days")`,
    /// so the terminal and the app can both say what a click would do. The label is the
    /// English key; translate it where it is shown.
    pub fn diff(&self, base: &Config) -> Vec<(&'static str, String, String)> {
        let after = self.over(base);
        let days = |n: u64| match n {
            0 => t("immediately").to_string(),
            1 => t("1 day").to_string(),
            n => crate::i18n::tf("{} days", &[&n.to_string()]),
        };
        let off = |n: u64, text: String| match n {
            0 => t("off").to_string(),
            _ => text,
        };
        let on = |b: bool| if b { t("on") } else { t("off") }.to_string();
        let rows: [(&'static str, String, String); 7] = [
            (Self::LABELS[0], days(base.days), days(after.days)),
            (Self::LABELS[1], days(base.gc_days), days(after.gc_days)),
            (
                Self::LABELS[2],
                off(base.max_size_gb, format!("{} GB", base.max_size_gb)),
                off(after.max_size_gb, format!("{} GB", after.max_size_gb)),
            ),
            (Self::LABELS[3], off(base.cache_days, days(base.cache_days)), off(after.cache_days, days(after.cache_days))),
            (Self::LABELS[4], off(base.trash_days, days(base.trash_days)), off(after.trash_days, days(after.trash_days))),
            (Self::LABELS[5], off(base.store_days, days(base.store_days)), off(after.store_days, days(after.store_days))),
            (Self::LABELS[6], on(base.smart), on(after.smart)),
        ];
        rows.into_iter().filter(|(_, a, b)| a != b).collect()
    }
}

/// The preset `cfg` currently matches, if any. Used to show which button is active, and
/// to be honest when the answer is "none of them any more".
pub fn current(cfg: &Config) -> Option<&'static Preset> {
    ALL.iter().find(|p| p.over(cfg) == *cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Config {
        Config { roots: vec!["/code".into()], auto_at: "07:00".into(), ..Default::default() }
    }

    #[test]
    fn balanced_is_the_shipped_default() {
        let c = base();
        assert_eq!(by_id("balanced").unwrap().over(&c), c, "the default settings are the Balanced preset");
        assert_eq!(current(&c).map(|p| p.id), Some("balanced"));
    }

    #[test]
    fn a_preset_keeps_what_is_not_its_business() {
        let c = base();
        let after = by_id("small-disk").unwrap().over(&c);
        assert_eq!(after.roots, c.roots, "project folders are the user's");
        assert_eq!(after.auto_at, c.auto_at, "so is the clean-up time");
        assert_eq!(after.lang, c.lang);
        assert_eq!(after.lean, c.lean);
        assert!(!after.all_langs, "other languages stay off unless the person asks");
    }

    #[test]
    fn hand_tuning_a_value_means_no_preset_matches() {
        let mut c = base();
        c.days = 31;
        assert!(current(&c).is_none());
    }

    /// The one setting a preset must never decide for someone: it changes what rtidy is
    /// allowed to delete, not how eagerly it deletes.
    #[test]
    fn no_preset_touches_the_other_languages_switch() {
        for on in [false, true] {
            let c = Config { all_langs: on, ..base() };
            for p in ALL.iter() {
                assert_eq!(p.over(&c).all_langs, on, "{} changed all_langs", p.id);
            }
        }
    }

    #[test]
    fn the_ci_preset_never_holds_bytes_back_or_schedules_itself() {
        let after = by_id("ci").unwrap().over(&base());
        assert_eq!(after.trash_days, 0);
        assert!(!after.auto_enabled);
        assert!(!after.smart, "a runner should clean the same amount every time");
    }

    /// The presets keep their text as data, so the usual `t("…")` scan cannot see it.
    #[test]
    fn every_preset_name_label_and_description_is_translated() {
        let mut missing = Vec::new();
        for p in ALL.iter() {
            for key in [p.name, p.about] {
                if crate::i18n::thai_for(key).is_none() {
                    missing.push(key);
                }
            }
        }
        for key in Preset::LABELS {
            if crate::i18n::thai_for(key).is_none() {
                missing.push(key);
            }
        }
        assert!(missing.is_empty(), "missing Thai: {missing:#?}");
    }

    #[test]
    fn the_diff_lists_only_what_changes_and_reads_in_words() {
        let c = base(); // balanced
        assert!(by_id("balanced").unwrap().diff(&c).is_empty(), "no change, nothing to say");
        let rows = by_id("ci").unwrap().diff(&c);
        assert!(rows.iter().any(|(k, _, to)| *k == "Recoverable for" && to == "off"));
        assert!(rows.iter().any(|(k, _, to)| *k == "Smart mode" && to == "off"));
        assert!(!rows.iter().any(|(k, ..)| k.contains("languages")));
    }
}
