//! Daily `rtidy auto`, the native way on every OS:
//! - Windows: Task Scheduler (per-user, no admin)
//! - macOS:   launchd LaunchAgent (catches up after sleep by itself; after power-off
//!   through `RunAtLoad` + `auto --catch-up`)
//! - Linux:   systemd user timer (Persistent=true catches up after power-off),
//!   falling back to the user's crontab when systemd isn't available (an `@reboot`
//!   line + `auto --catch-up` covers power-off there).
//!
//! `--catch-up HH:MM` makes `auto` run only when the most recent daily slot has not
//! been covered by a finished run, judged from a stamp each finished `auto` leaves.
//! That lets a run-at-login trigger share one command with the daily trigger without
//! cleaning twice.

use crate::util::*;
use anyhow::{bail, Context, Result};
#[allow(unused_imports)]
use std::fs;
#[allow(unused_imports)]
use std::io::Write;
use std::path::PathBuf;
#[allow(unused_imports)]
use std::process::Stdio;

pub const TASK_NAME: &str = "rtidy";
#[allow(dead_code)]
const CRON_TAG: &str = "# rtidy-auto";
#[allow(dead_code)]
const LAUNCHD_LABEL: &str = "dev.rtidy.auto";

/// Path of the running program. Inside an AppImage, the real file is $APPIMAGE
/// (current_exe points into a temporary mount).
pub fn exe_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("APPIMAGE") {
        return Ok(PathBuf::from(p));
    }
    Ok(std::env::current_exe()?)
}

/// Running from somewhere that will disappear (a mounted .dmg, macOS App Translocation):
/// a scheduled task pointing there would break.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn exe_looks_temporary() -> bool {
    exe_path().is_ok_and(|p| {
        let s = p.to_string_lossy();
        s.contains("AppTranslocation") || (cfg!(target_os = "macos") && s.starts_with("/Volumes/"))
    })
}

/// Where the time of the last finished `auto` is kept (next to the log).
fn stamp_path() -> Result<PathBuf> {
    Ok(log_path()?.with_file_name("last-auto"))
}

fn read_stamp() -> Option<i64> {
    fs::read_to_string(stamp_path().ok()?).ok()?.trim().parse().ok()
}

/// Record that a scheduled clean-up just finished.
pub fn record_run() {
    if let Ok(p) = stamp_path() {
        let _ = fs::write(p, chrono::Local::now().timestamp().to_string());
    }
}

/// Start of the most recent daily slot at `h:m` (local time), as a unix timestamp.
/// A trigger that fires a little early still counts as that day's slot.
fn latest_slot(h: u32, m: u32, now: chrono::DateTime<chrono::Local>) -> Option<i64> {
    use chrono::{Duration, TimeZone};
    let probe = now + Duration::minutes(2);
    let at = |d: chrono::NaiveDate| {
        let naive = d.and_hms_opt(h, m, 0)?;
        // A slot inside a DST gap moves to just after it; an ambiguous one takes the first.
        chrono::Local
            .from_local_datetime(&naive)
            .earliest()
            .or_else(|| chrono::Local.from_local_datetime(&(naive + Duration::hours(1))).earliest())
    };
    let today = at(probe.date_naive())?;
    let slot = if today <= probe { today } else { at(probe.date_naive().pred_opt()?)? };
    Some(slot.timestamp())
}

/// Has the most recent `at` slot gone by without a finished run?
/// With no stamp yet there is nothing to compare against: a baseline is written and the
/// answer is no, so installing the task or upgrading rtidy never starts a surprise clean-up.
pub fn missed(at: &str) -> Result<bool> {
    let (h, m) = parse_at(at)?;
    let Some(last) = read_stamp() else {
        record_run();
        return Ok(false);
    };
    Ok(latest_slot(h, m, chrono::Local::now()).is_some_and(|slot| last < slot))
}

fn parse_at(at: &str) -> Result<(u32, u32)> {
    let (h, m) = at.trim().split_once(':').context("time must be HH:MM")?;
    let (h, m): (u32, u32) = (h.parse()?, m.parse()?);
    if h > 23 || m > 59 {
        bail!("time must be HH:MM");
    }
    Ok((h, m))
}

/// Install (or replace) the daily task. `args` go after `auto`; empty = saved settings.
pub fn install(at: &str, args: &[String]) -> Result<()> {
    let (h, m) = parse_at(at)?;
    let exe = exe_path()?;
    #[cfg(windows)]
    return windows::install(&exe, h, m, args);
    #[cfg(target_os = "macos")]
    return macos::install(&exe, h, m, args);
    #[cfg(all(unix, not(target_os = "macos")))]
    return linux::install(&exe, h, m, args);
}

pub fn remove() -> Result<()> {
    #[cfg(windows)]
    return windows::remove();
    #[cfg(target_os = "macos")]
    return macos::remove();
    #[cfg(all(unix, not(target_os = "macos")))]
    return linux::remove();
}

pub fn is_installed() -> bool {
    #[cfg(windows)]
    return windows::is_installed();
    #[cfg(target_os = "macos")]
    return macos::is_installed();
    #[cfg(all(unix, not(target_os = "macos")))]
    return linux::is_installed();
}

/// Human description of the mechanism, for the UI.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub fn mechanism() -> &'static str {
    if cfg!(windows) {
        "Windows Task Scheduler"
    } else if cfg!(target_os = "macos") {
        "launchd"
    } else if linux_has_systemd() {
        "systemd timer"
    } else {
        "cron"
    }
}

fn linux_has_systemd() -> bool {
    cfg!(all(unix, not(target_os = "macos")))
        && quiet_cmd("systemctl")
            .args(["--user", "show-environment"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

// ---------------- Windows ----------------
#[cfg(windows)]
mod windows {
    use super::*;
    use std::path::Path;

    pub fn install(exe: &Path, h: u32, m: u32, args: &[String]) -> Result<()> {
        let mut tr = format!("\"{}\" auto", plain(exe));
        for a in args {
            if a.contains(' ') { tr.push_str(&format!(" \"{a}\"")) } else { tr.push_str(&format!(" {a}")) }
        }
        let st = format!("{h:02}:{m:02}");
        let out = quiet_cmd("schtasks")
            .args(["/create", "/tn", TASK_NAME, "/sc", "daily", "/st", &st, "/f", "/tr", &tr])
            .output()?;
        if !out.status.success() {
            bail!("schtasks: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        // Run missed runs at next start-up, and also on laptop battery.
        let ok = quiet_cmd("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "$t=Get-ScheduledTask -TaskName 'rtidy'; $s=$t.Settings; $s.StartWhenAvailable=$true; \
                 $s.DisallowStartIfOnBatteries=$false; $s.StopIfGoingOnBatteries=$false; \
                 Set-ScheduledTask -TaskName 'rtidy' -Settings $s | Out-Null",
            ])
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            crate::say!(
                "warning: could not turn on \"run missed task\"; a daily clean-up missed while the computer is off will be skipped"
            );
        }
        Ok(())
    }

    pub fn remove() -> Result<()> {
        let _ = quiet_cmd("schtasks").args(["/delete", "/tn", TASK_NAME, "/f"]).output()?;
        Ok(())
    }

    pub fn is_installed() -> bool {
        quiet_cmd("schtasks")
            .args(["/query", "/tn", TASK_NAME])
            .output()
            .is_ok_and(|o| o.status.success())
    }
}

// ---------------- macOS ----------------
#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::path::Path;

    fn plist_path() -> Result<PathBuf> {
        Ok(dirs::home_dir()
            .context("no home")?
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")))
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    fn uid() -> String {
        quiet_cmd("id")
            .arg("-u")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "501".into())
    }

    pub fn install(exe: &Path, h: u32, m: u32, args: &[String]) -> Result<()> {
        let mut prog = format!("<string>{}</string><string>auto</string>", xml_escape(&exe.display().to_string()));
        for a in args {
            prog.push_str(&format!("<string>{}</string>", xml_escape(a)));
        }
        // launchd catches up after sleep but not after power-off. RunAtLoad also starts the
        // job at every login; `--catch-up` makes that a no-op unless a slot was missed.
        prog.push_str(&format!("<string>--catch-up</string><string>{h:02}:{m:02}</string>"));
        let log = log_path()?;
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key><array>{prog}</array>
  <key>StartCalendarInterval</key><dict><key>Hour</key><integer>{h}</integer><key>Minute</key><integer>{m}</integer></dict>
  <key>RunAtLoad</key><true/>
  <key>StandardErrorPath</key><string>{}</string>
  <key>ProcessType</key><string>Background</string>
  <key>LowPriorityIO</key><true/>
  <key>Nice</key><integer>10</integer>
</dict>
</plist>
"#,
            xml_escape(&log.display().to_string())
        );
        let path = plist_path()?;
        let _ = remove();
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, plist)?;
        let target = format!("gui/{}", uid());
        let ok = quiet_cmd("launchctl")
            .args(["bootstrap", &target, &path.display().to_string()])
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            // older macOS
            quiet_cmd("launchctl").args(["load", "-w", &path.display().to_string()]).status()?;
        }
        Ok(())
    }

    pub fn remove() -> Result<()> {
        let path = plist_path()?;
        if path.exists() {
            let target = format!("gui/{}/{LAUNCHD_LABEL}", uid());
            let _ = quiet_cmd("launchctl").args(["bootout", &target]).stderr(Stdio::null()).status();
            let _ = quiet_cmd("launchctl")
                .args(["unload", &path.display().to_string()])
                .stderr(Stdio::null())
                .status();
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    pub fn is_installed() -> bool {
        plist_path().is_ok_and(|p| p.exists())
    }
}

// ---------------- Linux / BSD ----------------
#[cfg(all(unix, not(target_os = "macos")))]
mod linux {
    use super::*;
    use std::path::Path;

    fn unit_dir() -> Result<PathBuf> {
        Ok(dirs::config_dir().context("no config dir")?.join("systemd/user"))
    }

    fn systemctl(args: &[&str]) -> Result<()> {
        let ok = quiet_cmd("systemctl").arg("--user").args(args).status()?.success();
        if !ok {
            bail!("systemctl --user {} failed", args.join(" "));
        }
        Ok(())
    }

    /// systemd unit quoting: wrap in double quotes, escape \ and ".
    fn sd_quote(s: &str) -> String {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('%', "%%"))
    }

    pub fn install(exe: &Path, h: u32, m: u32, args: &[String]) -> Result<()> {
        let _ = remove();
        if linux_has_systemd() {
            let dir = unit_dir()?;
            fs::create_dir_all(&dir)?;
            let mut cmd = format!("{} auto", sd_quote(&exe.display().to_string()));
            for a in args {
                cmd.push(' ');
                cmd.push_str(&sd_quote(a));
            }
            fs::write(
                dir.join("rtidy.service"),
                format!(
                    "[Unit]\nDescription=rtidy: tidy Rust target folders\n\n[Service]\nType=oneshot\nExecStart={cmd}\nNice=10\nIOSchedulingClass=idle\n"
                ),
            )?;
            fs::write(
                dir.join("rtidy.timer"),
                format!(
                    "[Unit]\nDescription=Daily rtidy clean-up\n\n[Timer]\nOnCalendar=*-*-* {h:02}:{m:02}:00\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n"
                ),
            )?;
            systemctl(&["daemon-reload"])?;
            systemctl(&["enable", "--now", "rtidy.timer"])?;
        } else {
            let quoted: Vec<String> = args.iter().map(|a| format!("'{}'", a.replace('\'', "'\\''"))).collect();
            let exe = exe.display().to_string().replace('\'', "'\\''");
            let args = quoted.join(" ");
            // cron never runs a missed job; the @reboot line catches up after power-off.
            let lines = format!(
                "{m} {h} * * * '{exe}' auto {args} {CRON_TAG}\n\
                 @reboot '{exe}' auto {args} --catch-up {h:02}:{m:02} {CRON_TAG}"
            );
            write_crontab(Some(&lines))?;
        }
        Ok(())
    }

    pub fn remove() -> Result<()> {
        let dir = unit_dir()?;
        if dir.join("rtidy.timer").exists() {
            let _ = quiet_cmd("systemctl")
                .args(["--user", "disable", "--now", "rtidy.timer"])
                .stderr(Stdio::null())
                .status();
            let _ = fs::remove_file(dir.join("rtidy.timer"));
            let _ = fs::remove_file(dir.join("rtidy.service"));
            let _ = quiet_cmd("systemctl").args(["--user", "daemon-reload"]).status();
        }
        if read_crontab().contains(CRON_TAG) {
            write_crontab(None)?;
        }
        Ok(())
    }

    pub fn is_installed() -> bool {
        unit_dir().is_ok_and(|d| d.join("rtidy.timer").exists()) || read_crontab().contains(CRON_TAG)
    }

    fn read_crontab() -> String {
        quiet_cmd("crontab")
            .arg("-l")
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default()
    }

    fn write_crontab(new_line: Option<&str>) -> Result<()> {
        let mut lines: Vec<String> =
            read_crontab().lines().filter(|l| !l.contains(CRON_TAG)).map(str::to_string).collect();
        if let Some(l) = new_line {
            lines.push(l.to_string());
        }
        let mut child = quiet_cmd("crontab")
            .arg("-")
            .stdin(Stdio::piped())
            .spawn()
            .context("neither systemd nor cron is available")?;
        child.stdin.take().unwrap().write_all((lines.join("\n") + "\n").as_bytes())?;
        if !child.wait()?.success() {
            bail!("crontab failed");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Local, TimeZone};

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).earliest().unwrap()
    }

    #[test]
    fn slot_is_today_once_the_time_has_passed() {
        let slot = latest_slot(3, 17, local(2026, 3, 10, 9, 0)).unwrap();
        assert_eq!(slot, local(2026, 3, 10, 3, 17).timestamp());
    }

    #[test]
    fn slot_is_yesterday_before_the_time() {
        let slot = latest_slot(23, 0, local(2026, 3, 10, 9, 0)).unwrap();
        assert_eq!(slot, local(2026, 3, 9, 23, 0).timestamp());
    }

    #[test]
    fn a_slightly_early_trigger_counts_as_todays_slot() {
        let slot = latest_slot(3, 17, local(2026, 3, 10, 3, 16)).unwrap();
        assert_eq!(slot, local(2026, 3, 10, 3, 17).timestamp());
    }
}
