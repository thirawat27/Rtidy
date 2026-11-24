//! `.rtidy.toml`: a project's own rules, committed to the repository.
//!
//! The `.rtidy-keep` marker said one thing — never clean this — and it said it only on
//! the machine it was created on, because nobody commits an empty dotfile. A team that
//! shares a repository needs the rule to travel with it: the release branch nobody
//! rebuilds can say `days = 120`, the integration suite that takes an hour to warm up
//! can say `keep = true`, and everyone who clones it gets that.
//!
//! The marker still works and still means `keep = true`; this is the file that can say
//! more. Machine-wide settings (which languages, the schedule, the shared dir) stay in
//! Settings, where the person who owns the machine decides.

use std::path::{Path, PathBuf};

/// The file name, beside the project's manifest.
pub const FILE: &str = ".rtidy.toml";

/// Put this empty file next to the manifest to protect a project's artifacts.
pub const KEEP_MARKER: &str = ".rtidy-keep";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    /// Never clean this project's artifacts.
    pub keep: bool,
    /// Override how old this project's artifacts must be before they go.
    pub days: Option<u64>,
    /// Override how long deleted artifacts stay recoverable.
    pub trash_days: Option<u64>,
    /// Override whether other languages are cleaned inside this project.
    pub all_langs: Option<bool>,
    /// Artifact directory names to leave alone here, e.g. `[".venv"]`.
    pub except: Vec<String>,
    /// Where the rules came from, for the UI and for `rtidy doctor`.
    pub from: Option<PathBuf>,
}

impl Rules {
    /// Read the rules for the project in `dir`. Absent files mean default rules, and a
    /// malformed one is reported but never silently ignored.
    pub fn for_project(dir: &Path) -> Self {
        let mut r = Self { keep: dir.join(KEEP_MARKER).exists(), ..Default::default() };
        let path = dir.join(FILE);
        let Ok(text) = std::fs::read_to_string(&path) else { return r };
        r.from = Some(path.clone());
        let doc = match text.parse::<toml_edit::DocumentMut>() {
            Ok(d) => d,
            Err(e) => {
                crate::say!("warning: ignoring {} ({e})", path.display());
                return r;
            }
        };
        let int = |k: &str| doc.get(k).and_then(|v| v.as_integer()).and_then(|v| u64::try_from(v).ok());
        let boolean = |k: &str| doc.get(k).and_then(|v| v.as_bool());
        r.keep |= boolean("keep").unwrap_or(false);
        r.days = int("days");
        r.trash_days = int("trash_days");
        r.all_langs = boolean("all_langs");
        r.except = doc
            .get("except")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        r
    }

    /// Is this artifact directory exempt by name?
    pub fn exempt(&self, artifact: &Path) -> bool {
        let Some(name) = artifact.file_name().and_then(|n| n.to_str()) else { return false };
        self.except.iter().any(|e| e.eq_ignore_ascii_case(name))
    }

    /// Write `keep` into the project's `.rtidy.toml`, creating or updating it.
    ///
    /// The app's Keep switch used to create a `.rtidy-keep` marker, which worked and
    /// told the rest of the team nothing. This writes the file that can be committed,
    /// and removes the old marker when it is no longer what is keeping the project.
    pub fn set_keep(dir: &Path, keep: bool) -> anyhow::Result<()> {
        let path = dir.join(FILE);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut doc = text.parse::<toml_edit::DocumentMut>().unwrap_or_default();
        if keep {
            doc["keep"] = toml_edit::value(true);
            if text.is_empty() {
                // Say what the file is for; it is going into somebody's repository.
                let note = "# rtidy rules for this project (https://github.com/thirawat27/Rtidy)\n";
                doc.decor_mut().set_prefix(note);
            }
            std::fs::write(&path, doc.to_string())?;
        } else {
            doc.remove("keep");
            // Nothing but the comment left: the file has no reason to exist any more.
            if doc.as_table().is_empty() {
                let _ = std::fs::remove_file(&path);
            } else {
                std::fs::write(&path, doc.to_string())?;
            }
        }
        let marker = dir.join(KEEP_MARKER);
        if !keep && marker.exists() {
            let _ = std::fs::remove_file(marker);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("rtidy-rules-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn reads_values_and_the_old_marker() {
        let d = tmp("read");
        std::fs::write(d.join(FILE), "days = 90\nexcept = [\".venv\"]\n").unwrap();
        let r = Rules::for_project(&d);
        assert_eq!(r.days, Some(90));
        assert!(!r.keep);
        assert!(r.exempt(Path::new(".venv")) && !r.exempt(Path::new("target")));

        std::fs::write(d.join(KEEP_MARKER), "").unwrap();
        assert!(Rules::for_project(&d).keep, "the old marker still protects a project");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn set_keep_round_trips_and_cleans_up_after_itself() {
        let d = tmp("keep");
        Rules::set_keep(&d, true).unwrap();
        assert!(Rules::for_project(&d).keep);
        Rules::set_keep(&d, false).unwrap();
        assert!(!Rules::for_project(&d).keep);
        assert!(!d.join(FILE).exists(), "an empty rules file is removed, not left behind");

        // An unrelated setting in the file survives turning Keep off.
        std::fs::write(d.join(FILE), "days = 10\nkeep = true\n").unwrap();
        Rules::set_keep(&d, false).unwrap();
        let r = Rules::for_project(&d);
        assert_eq!((r.keep, r.days), (false, Some(10)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_broken_file_does_not_take_the_scan_down() {
        let d = tmp("broken");
        std::fs::write(d.join(FILE), "days = = =").unwrap();
        assert_eq!(Rules::for_project(&d).days, None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
