//! `rtidy watch`: trim a few minutes after a build, instead of once a day.
//!
//! The daily task is the right default — it needs nothing running and it survives a
//! reboot — but it is also the wrong shape for the machine this was written on. A day of
//! work leaves the shared dir full of incremental caches for crates that were touched
//! once, at 10am, and they sit there until the scheduled run that night. Watch mode
//! closes that gap: it notices that building has stopped, waits out a quiet period, and
//! trims the parts that are pure speed-up data for work already finished.
//!
//! It is polling, not a filesystem notifier, on purpose. There is exactly one directory
//! worth watching (the shared target dir), one `stat` per tick answers the only question
//! being asked, and a notifier would mean a new dependency, a thread per root and a
//! queue of events on every build — vastly more machinery for the same answer.
//!
//! Nothing here deletes a project's own artifacts: this runs while someone is working,
//! and the "they stopped building" guess is not a good enough reason to remove something
//! that takes minutes to produce. Those stay with the daily run, the preview and the app.

use crate::config::Config;
use crate::plan::{Mode, Plan};
use crate::say;
use crate::util::{human, mtime_of};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How often to look. Short enough that a trim follows the idle window closely, long
/// enough that the process is invisible on a battery.
const TICK: Duration = Duration::from_secs(30);

/// What the watcher knows between ticks.
struct State {
    /// Newest build activity seen so far.
    last_build: SystemTime,
    /// When the last trim happened, so one quiet evening is not trimmed forty times.
    last_trim: Option<SystemTime>,
}

/// Should a trim run now?
///
/// Pure, so the rule is testable without waiting half an hour: building has to have
/// stopped for `idle`, and something has to have been built since the last trim.
fn due(now: SystemTime, s: &State, idle: Duration) -> bool {
    let Ok(quiet) = now.duration_since(s.last_build) else { return false };
    if quiet < idle {
        return false;
    }
    match s.last_trim {
        // Nothing new since the last trim: there is nothing left to take.
        Some(t) => s.last_build > t,
        None => true,
    }
}

/// Newest modification time anywhere in the watched set, shallowly.
///
/// Only the top level of each profile directory is read: Cargo touches the profile dir
/// and its `incremental`/`deps` children on every build, so a deep walk would cost
/// thousands of `stat` calls to learn the same thing.
fn newest(dirs: &[PathBuf]) -> SystemTime {
    let mut best = SystemTime::UNIX_EPOCH;
    for d in dirs {
        if let Ok(md) = std::fs::metadata(d) {
            best = best.max(mtime_of(&md));
        }
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            if let Ok(md) = e.metadata() {
                best = best.max(mtime_of(&md));
            }
        }
    }
    best
}

/// Directories whose timestamps mean "a build happened".
fn watched(shared: &Path) -> Vec<PathBuf> {
    let mut v = vec![shared.to_path_buf()];
    v.extend(crate::shared::profiles(shared));
    v
}

/// What a trim does: incremental caches, and the size cap if the shared dir is over it.
fn trim_plan(cfg: &Config) -> Result<Plan> {
    // `gc_plan` with a huge age floor gives the incremental caches and nothing else
    // unless the cap is exceeded, which is exactly the light-touch pass wanted here.
    crate::shared::gc_plan(u64::MAX / 86_400, cfg.max_size())
}

/// Run the watcher. `once` does a single tick and returns, for testing and for cron-like
/// setups that would rather call rtidy than keep a process alive.
pub fn run(cfg: &Config, once: bool) -> Result<()> {
    let Some(shared) = crate::shared::shared_target_dir()? else {
        say!("watch: no shared target dir yet — run `rtidy setup` first");
        return Ok(());
    };
    let idle = Duration::from_secs(cfg.watch_idle_mins.max(1) * 60);
    let dirs = watched(&shared);
    let mut state = State { last_build: newest(&dirs), last_trim: None };
    say!(
        "watch: {} · trims {} after builds stop · ctrl-c to stop",
        shared.display(),
        humantime::format_duration(idle)
    );

    loop {
        let now = SystemTime::now();
        let seen = newest(&dirs);
        if seen > state.last_build {
            state.last_build = seen;
        }
        if due(now, &state, idle) {
            let plan = trim_plan(cfg)?;
            if plan.is_empty() {
                state.last_trim = Some(now);
            } else {
                let out = crate::plan::apply(&plan, Mode::Delete, "watch");
                say!("watch: freed {} after {} idle", human(out.freed), humantime::format_duration(idle));
                state.last_trim = Some(now);
            }
            // Purging expired trash is cheap and this is the quietest moment there is.
            crate::trash::purge(cfg.trash_days);
        }
        if once {
            return Ok(());
        }
        std::thread::sleep(TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs_ago(n: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(10_000 - n)
    }
    const NOW: SystemTime = SystemTime::UNIX_EPOCH;

    fn now() -> SystemTime {
        NOW + Duration::from_secs(10_000)
    }

    #[test]
    fn waits_for_the_idle_window() {
        let s = State { last_build: secs_ago(60), last_trim: None };
        assert!(!due(now(), &s, Duration::from_secs(600)), "one minute of quiet is not ten");
        let s = State { last_build: secs_ago(700), last_trim: None };
        assert!(due(now(), &s, Duration::from_secs(600)));
    }

    #[test]
    fn does_not_trim_twice_for_the_same_quiet_period() {
        let s = State { last_build: secs_ago(700), last_trim: Some(secs_ago(100)) };
        assert!(!due(now(), &s, Duration::from_secs(600)), "nothing built since the last trim");
        let s = State { last_build: secs_ago(700), last_trim: Some(secs_ago(900)) };
        assert!(due(now(), &s, Duration::from_secs(600)), "a build happened after the last trim");
    }
}
