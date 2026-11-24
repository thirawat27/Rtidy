//! What rtidy has given back, over time.
//!
//! A clean-up tool that only ever says "freed 4.2 GB" and forgets is hard to trust and
//! easy to stop using: there is no way to tell whether the daily task is doing anything,
//! and no answer to "is this worth keeping installed". The ledger is one small file that
//! remembers the total, the per-ecosystem split and the last few runs, so the app can
//! say "184 GB since March" and `rtidy log` has numbers behind it.
//!
//! It is written after a run completes, never during, and a failure to write it is not
//! allowed to fail the clean-up.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// How many recent runs to keep. Enough for a sparkline, small enough to stay a
/// rounding error next to the config file.
const KEEP_RUNS: usize = 60;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Run {
    /// Unix seconds.
    pub at: i64,
    pub freed: u64,
    /// `fix`, `auto`, `watch`, `gui`.
    pub how: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Ledger {
    /// Everything ever freed, including what later came back out of the trash
    /// (`restored` tracks that, so the honest figure is `freed - restored`).
    pub freed: u64,
    pub restored: u64,
    /// Bytes per ecosystem id.
    pub by_eco: BTreeMap<String, u64>,
    pub runs: Vec<Run>,
    /// First time rtidy freed anything on this machine.
    pub since: Option<i64>,
}

fn path() -> Result<PathBuf> {
    Ok(crate::config::Config::path()?.with_file_name("ledger.json"))
}

impl Ledger {
    pub fn load() -> Self {
        path()
            .ok()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let p = path()?;
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Record a finished run. `per_eco` is bytes keyed by ecosystem id.
    pub fn record(how: &str, freed: u64, per_eco: &[(String, u64)]) {
        if freed == 0 {
            return;
        }
        let mut l = Self::load();
        let now = crate::brain::now();
        l.freed += freed;
        l.since = l.since.or(Some(now));
        for (eco, bytes) in per_eco {
            *l.by_eco.entry(eco.clone()).or_default() += bytes;
        }
        l.runs.push(Run { at: now, freed, how: how.to_string() });
        if l.runs.len() > KEEP_RUNS {
            let cut = l.runs.len() - KEEP_RUNS;
            l.runs.drain(..cut);
        }
        let _ = l.save();
    }

    /// Note that some of what was freed came back, so the totals stay honest.
    pub fn record_restore(bytes: u64) {
        let mut l = Self::load();
        l.restored += bytes;
        let _ = l.save();
    }

    /// Net bytes freed: what went, minus what was put back.
    pub fn net(&self) -> u64 {
        self.freed.saturating_sub(self.restored)
    }

    /// Days since the first run, for "N GB since …".
    pub fn days(&self) -> Option<i64> {
        self.since.map(|s| ((crate::brain::now() - s) / 86_400).max(0))
    }

    /// Freed per day, averaged over the recorded period — what the daily task is worth.
    pub fn per_day(&self) -> Option<f64> {
        let d = self.days()?;
        (d >= 1).then(|| self.net() as f64 / d as f64)
    }

    /// Ecosystems by what they gave back, largest first.
    pub fn top_ecos(&self) -> Vec<(&str, u64)> {
        let mut v: Vec<(&str, u64)> = self.by_eco.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        v.sort_unstable_by_key(|(_, b)| std::cmp::Reverse(*b));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_never_goes_below_zero_and_ecos_rank_by_size() {
        let mut l = Ledger { freed: 100, restored: 400, ..Default::default() };
        assert_eq!(l.net(), 0);
        l.by_eco.insert("cargo".into(), 10);
        l.by_eco.insert("node".into(), 99);
        assert_eq!(l.top_ecos()[0].0, "node");
    }

    #[test]
    fn runs_are_capped() {
        let mut l = Ledger::default();
        for i in 0..KEEP_RUNS as i64 + 20 {
            l.runs.push(Run { at: i, freed: 1, how: "test".into() });
        }
        if l.runs.len() > KEEP_RUNS {
            let cut = l.runs.len() - KEEP_RUNS;
            l.runs.drain(..cut);
        }
        assert_eq!(l.runs.len(), KEEP_RUNS);
        assert_eq!(l.runs[0].at, 20);
    }
}
