//! Register `rtidy auto` to run daily (Windows Task Scheduler; crontab line elsewhere).

use crate::util::*;
use crate::AutoOpts;
use anyhow::{bail, Context, Result};
use std::process::Command;

pub const TASK_NAME: &str = "rtidy";

pub fn schedule(opts: &AutoOpts, at: &str, remove: bool) -> Result<()> {
    if cfg!(windows) {
        let status = if remove {
            Command::new("schtasks").args(["/delete", "/tn", TASK_NAME, "/f"]).status()?
        } else {
            let exe = plain(&std::env::current_exe()?);
            let mut tr = format!("\"{exe}\" auto");
            for a in opts.to_args()? {
                if a.contains(' ') { tr.push_str(&format!(" \"{a}\"")) } else { tr.push_str(&format!(" {a}")) }
            }
            Command::new("schtasks")
                .args(["/create", "/tn", TASK_NAME, "/sc", "daily", "/st", at, "/f", "/tr", &tr])
                .status()?
        };
        if !status.success() {
            bail!("schtasks failed");
        }
        println!("{}", if remove { "scheduled task removed" } else { "scheduled task created" });
        if !remove {
            println!("runs daily at {at}; results go to {}", log_path()?.display());
            println!("run it now: schtasks /run /tn {TASK_NAME}");
        }
    } else {
        let (h, m) = at.split_once(':').context("--at must be HH:MM")?;
        let exe = std::env::current_exe()?;
        let args: Vec<String> = opts.to_args()?.iter().map(|a| format!("'{a}'")).collect();
        if remove {
            println!("remove the rtidy line with `crontab -e`");
        } else {
            println!("add this line with `crontab -e`:");
            println!("{m} {h} * * * '{}' auto {}", exe.display(), args.join(" "));
        }
    }
    Ok(())
}
