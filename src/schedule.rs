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

#[cfg_attr(not(windows), allow(dead_code))]
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

/// Record that a scheduled clean-up just finished. Written to a side file and renamed
/// over the stamp, so a power cut mid-write can't leave a half-written one.
pub fn record_run() {
    if let Ok(p) = stamp_path() {
        let tmp = p.with_extension("tmp");
        if fs::write(&tmp, chrono::Local::now().timestamp().to_string()).is_ok() && fs::rename(&tmp, &p).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

/// How far a trigger may fire early, or a clock disagree, and still count as on time.
const GRACE_SECS: i64 = 120;

/// Start of the most recent daily slot at `h:m` in `now`'s time zone, as a unix
/// timestamp. A trigger that fires a little early still counts as that day's slot.
fn latest_slot<Tz: chrono::TimeZone>(h: u32, m: u32, now: &chrono::DateTime<Tz>) -> Option<i64> {
    use chrono::Duration;
    let tz = now.timezone();
    let probe = now.clone() + Duration::seconds(GRACE_SECS);
    let at = |d: chrono::NaiveDate| {
        let naive = d.and_hms_opt(h, m, 0)?;
        // A slot inside a DST gap moves to just after it; an ambiguous one takes the first.
        tz.from_local_datetime(&naive)
            .earliest()
            .or_else(|| tz.from_local_datetime(&(naive + Duration::hours(1))).earliest())
    };
    let day = probe.naive_local().date();
    let today = at(day)?;
    let slot = if today <= probe { today } else { at(day.pred_opt()?)? };
    Some(slot.timestamp())
}

#[derive(Debug, PartialEq, Eq)]
enum CatchUp {
    /// No usable stamp: record one and do nothing.
    Baseline,
    /// The latest slot already has a finished run.
    Skip,
    /// The latest slot went by without one.
    Run,
}

/// The catch-up decision, kept free of the clock and the disk so it can be tested.
fn decide<Tz: chrono::TimeZone>(last: Option<i64>, h: u32, m: u32, now: &chrono::DateTime<Tz>) -> CatchUp {
    let Some(last) = last else { return CatchUp::Baseline };
    // A stamp from the future means the clock was moved back. It says nothing about
    // which slots ran, and trusting it would skip every slot until the clock caught up.
    if last > now.timestamp() + GRACE_SECS {
        return CatchUp::Run;
    }
    match latest_slot(h, m, now) {
        // A run that began up to GRACE early (an early trigger) covers the slot.
        Some(slot) if last < slot - GRACE_SECS => CatchUp::Run,
        _ => CatchUp::Skip,
    }
}

/// Has the most recent `at` slot gone by without a finished run?
/// With no stamp yet (or an unreadable one) there is nothing to compare against: a
/// baseline is written and the answer is no, so installing the task or upgrading rtidy
/// never starts a surprise clean-up.
pub fn missed(at: &str) -> Result<bool> {
    let (h, m) = parse_at(at)?;
    Ok(match decide(read_stamp(), h, m, &chrono::Local::now()) {
        CatchUp::Baseline => {
            record_run();
            false
        }
        CatchUp::Skip => false,
        CatchUp::Run => true,
    })
}

pub enum RunLock {
    /// Keep this alive for the whole run; `None` when locking isn't possible here.
    Held(#[allow(dead_code)] Option<fs::File>),
    /// Another `auto` is running.
    Busy,
}

/// The scheduled-run lock. A login trigger and the daily trigger can land together
/// (cron's @reboot near the daily time), and two clean-ups must not delete the same
/// folders at once. A lock that can't be taken for any reason but contention does not
/// stop the clean-up.
pub fn run_lock() -> RunLock {
    let Some(f) = log_path()
        .ok()
        .and_then(|p| fs::OpenOptions::new().create(true).truncate(false).write(true).open(p.with_file_name("auto.lock")).ok())
    else {
        return RunLock::Held(None);
    };
    match f.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => RunLock::Busy,
        _ => RunLock::Held(Some(f)),
    }
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

/// Asked once per process: the app's settings page shows `mechanism()` on every frame,
/// and starting `systemctl` that often made it stutter on Linux.
fn linux_has_systemd() -> bool {
    static HAS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *HAS.get_or_init(|| {
        cfg!(all(unix, not(target_os = "macos")))
            && quiet_cmd("systemctl")
                .args(["--user", "show-environment"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    })
}

// The text of the launchd agent and the crontab lines is built here, outside the
// per-OS modules, so the tests check it on every platform.

#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The launchd agent. launchd catches up after sleep but not after power-off, so
/// RunAtLoad also starts the job at every login; `--catch-up` makes that a no-op
/// unless a slot was missed, and the daily run itself passes the same check.
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
fn plist_xml(exe: &str, h: u32, m: u32, args: &[String], log: &str) -> String {
    let mut prog = format!("<string>{}</string><string>auto</string>", xml_escape(exe));
    for a in args {
        prog.push_str(&format!("<string>{}</string>", xml_escape(a)));
    }
    prog.push_str(&format!("<string>--catch-up</string><string>{h:02}:{m:02}</string>"));
    format!(
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
        xml_escape(log)
    )
}

/// The crontab lines: the daily one, and an @reboot one because cron never runs a
/// missed job. Words are single-quoted for sh; `%` is escaped because cron turns a
/// bare one into a newline.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos")))), allow(dead_code))]
fn cron_lines(exe: &str, h: u32, m: u32, args: &[String]) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "'\\''").replace('%', "\\%"));
    let mut cmd = format!("{} auto", q(exe));
    for a in args {
        cmd.push(' ');
        cmd.push_str(&q(a));
    }
    // @reboot fires early in boot; a minute's wait lets network and removable drives
    // mount, so the catch-up sees the same folders the daily run would.
    format!("{m} {h} * * * {cmd} {CRON_TAG}\n@reboot sleep 60; {cmd} --catch-up {h:02}:{m:02} {CRON_TAG}")
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Text between `start` and the next `end` after it.
fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = s.find(start)? + start.len();
    Some(&s[from..from + s[from..].find(end)?])
}

/// The Windows task. Settings that used to need a PowerShell call are part of it:
/// missed runs start when the computer is next available, battery doesn't stop it,
/// a second start while one runs is ignored, and a hung run is stopped after 4 hours.
#[cfg_attr(not(any(test, windows)), allow(dead_code))]
fn task_xml(exe: &str, h: u32, m: u32, args: &[String], start_day: chrono::NaiveDate) -> String {
    let mut arguments = String::from("auto");
    for a in args {
        arguments.push(' ');
        if a.is_empty() || a.contains([' ', '\t']) {
            arguments.push_str(&format!("\"{a}\""));
        } else {
            arguments.push_str(a);
        }
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>rtidy: daily clean-up of Rust target folders</Description></RegistrationInfo>
  <Triggers>
    <CalendarTrigger>
      <StartBoundary>{start_day}T{h:02}:{m:02}:00</StartBoundary>
      <ScheduleByDay><DaysInterval>1</DaysInterval></ScheduleByDay>
    </CalendarTrigger>
  </Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT4H</ExecutionTimeLimit>
  </Settings>
  <Actions Context="Author"><Exec><Command>"{}"</Command><Arguments>{}</Arguments></Exec></Actions>
</Task>
"#,
        xml_escape(exe),
        xml_escape(&arguments)
    )
}

#[cfg_attr(not(any(test, windows)), allow(dead_code))]
fn program_in_task_xml(xml: &str) -> Option<PathBuf> {
    let cmd = xml_unescape(between(xml, "<Command>", "</Command>")?.trim());
    Some(PathBuf::from(cmd.trim_matches('"')))
}

#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
fn program_in_plist(plist: &str) -> Option<PathBuf> {
    let array = between(plist, "<key>ProgramArguments</key>", "</array>")?;
    Some(PathBuf::from(xml_unescape(between(array, "<string>", "</string>")?)))
}

/// The first word of a systemd `ExecStart=` or a crontab line, unquoted.
#[cfg_attr(not(any(test, all(unix, not(target_os = "macos")))), allow(dead_code))]
fn program_in_command(line: &str) -> Option<PathBuf> {
    let (q, rest) = match line.trim_start().chars().next()? {
        c @ ('"' | '\'') => (c, &line.trim_start()[1..]),
        _ => return line.split_whitespace().next().map(PathBuf::from),
    };
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c == q => {
                // sh's '\'' idiom: close, escaped quote, reopen
                if q == '\'' && chars.as_str().starts_with("\\''") {
                    out.push('\'');
                    chars.nth(2);
                    continue;
                }
                break;
            }
            '\\' if q == '"' => out.extend(chars.next()),
            '\\' if q == '\'' && chars.as_str().starts_with('%') => out.extend(chars.next()),
            c => out.push(c),
        }
    }
    // systemd doubles `%` inside its quotes
    if q == '"' {
        out = out.replace("%%", "%");
    }
    Some(PathBuf::from(out))
}

/// Next start of the daily `h:m` slot strictly after `now`, for display.
fn next_slot<Tz: chrono::TimeZone>(h: u32, m: u32, now: &chrono::DateTime<Tz>) -> Option<chrono::DateTime<Tz>> {
    let tz = now.timezone();
    let mut day = now.naive_local().date();
    for _ in 0..3 {
        let naive = day.and_hms_opt(h, m, 0)?;
        let t = tz
            .from_local_datetime(&naive)
            .earliest()
            .or_else(|| tz.from_local_datetime(&(naive + chrono::Duration::hours(1))).earliest());
        if let Some(t) = t.filter(|t| t > now) {
            return Some(t);
        }
        day = day.succ_opt()?;
    }
    None
}

/// The program the installed task starts, if it can be read.
pub fn installed_program() -> Option<PathBuf> {
    #[cfg(windows)]
    return windows::installed_program();
    #[cfg(target_os = "macos")]
    return macos::installed_program();
    #[cfg(all(unix, not(target_os = "macos")))]
    return linux::installed_program();
}

/// Something wrong with the installed task that the user should fix, with the fix.
/// `None` when it looks fine or can't be judged.
pub fn problem() -> Option<(String, &'static str)> {
    if !is_installed() {
        return None;
    }
    let prog = installed_program()?;
    if prog.exists() {
        return None;
    }
    Some((
        format!("the daily clean-up starts {}, which no longer exists", prog.display()),
        "run `rtidy schedule` from where rtidy lives now (or turn the daily clean-up off and on in the app)",
    ))
}

/// Human summary for `rtidy schedule --status`.
pub fn status(at: &str) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    if !is_installed() {
        return "daily clean-up: off (turn on with `rtidy schedule`)".into();
    }
    let _ = writeln!(s, "daily clean-up: on, every day at {at} ({})", mechanism());
    let catch_up = if cfg!(windows) {
        "at the next start-up or login"
    } else if cfg!(target_os = "macos") {
        "after sleep, and at the next login after power-off"
    } else if linux_has_systemd() {
        "at the next login (at boot with `loginctl enable-linger`)"
    } else {
        "at the next boot (@reboot)"
    };
    let _ = writeln!(s, "missed runs:    caught up once, {catch_up}");
    let fmt = |t: chrono::DateTime<chrono::Local>| t.format("%Y-%m-%d %H:%M").to_string();
    let last = read_stamp().and_then(|t| chrono::DateTime::from_timestamp(t, 0)).map(|t| fmt(t.with_timezone(&chrono::Local)));
    let _ = writeln!(s, "last finished:  {}", last.as_deref().unwrap_or("not yet"));
    if let Ok((h, m)) = parse_at(at)
        && let Some(next) = next_slot(h, m, &chrono::Local::now())
    {
        let _ = writeln!(s, "next run:       {}", fmt(next));
    }
    match problem() {
        Some((what, fix)) => {
            let _ = write!(s, "problem:        {what}\n                fix: {fix}");
        }
        None => s.truncate(s.trim_end().len()),
    }
    s
}

// ---------------- Windows ----------------
#[cfg(windows)]
mod windows {
    use super::*;
    use std::path::Path;

    /// One `schtasks /create /xml` call registers the task with every setting at once.
    /// The old way (schtasks, then PowerShell to turn on "run missed task") cost about
    /// two seconds of PowerShell start-up on every change and could leave a task that
    /// silently skipped missed runs when PowerShell was blocked. A task file also has no
    /// 261-character limit on the command line, so long folder lists fit.
    pub fn install(exe: &Path, h: u32, m: u32, args: &[String]) -> Result<()> {
        let today = chrono::Local::now().date_naive();
        let xml = task_xml(&plain(exe), h, m, args, today);
        let file = std::env::temp_dir().join(format!("rtidy-task-{}.xml", std::process::id()));
        // Task Scheduler reads task files as UTF-16 (little endian, with a BOM).
        let bytes: Vec<u8> =
            [0xFEFFu16].into_iter().chain(xml.encode_utf16()).flat_map(u16::to_le_bytes).collect();
        fs::write(&file, bytes)?;
        let out = quiet_cmd("schtasks")
            .args(["/create", "/tn", TASK_NAME, "/f", "/xml"])
            .arg(&file)
            .output();
        let _ = fs::remove_file(&file);
        let out = out?;
        if !out.status.success() {
            bail!("schtasks: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(())
    }

    /// The program the registered task starts, when it can be read reliably.
    pub fn installed_program() -> Option<PathBuf> {
        let out = quiet_cmd("schtasks").args(["/query", "/tn", TASK_NAME, "/xml"]).output().ok()?;
        // Redirected output comes in the console code page; a non-ASCII path can't be
        // decoded reliably from it, so it is not judged at all.
        if !out.status.success() || !out.stdout.is_ascii() {
            return None;
        }
        program_in_task_xml(&String::from_utf8_lossy(&out.stdout))
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

    fn uid() -> String {
        quiet_cmd("id")
            .arg("-u")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "501".into())
    }

    pub fn install(exe: &Path, h: u32, m: u32, args: &[String]) -> Result<()> {
        let plist = plist_xml(&exe.display().to_string(), h, m, args, &log_path()?.display().to_string());
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

    pub fn installed_program() -> Option<PathBuf> {
        program_in_plist(&fs::read_to_string(plist_path().ok()?).ok()?)
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
            write_crontab(Some(&cron_lines(&exe.display().to_string(), h, m, args)))?;
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

    pub fn installed_program() -> Option<PathBuf> {
        if let Some(unit) = unit_dir().ok().and_then(|d| fs::read_to_string(d.join("rtidy.service")).ok()) {
            let exec = unit.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
            return program_in_command(exec);
        }
        let crontab = read_crontab();
        let line = crontab.lines().find(|l| l.contains(CRON_TAG) && !l.starts_with('@'))?;
        // skip the five time fields, keeping the command's own spacing
        let mut rest = line;
        for _ in 0..5 {
            rest = rest.trim_start();
            rest = &rest[rest.find(char::is_whitespace)?..];
        }
        program_in_command(rest)
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
    use chrono::{DateTime, FixedOffset, MappedLocalTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

    // ---- a time zone with daylight saving, so DST edges are tested on any machine ----

    /// UTC+0, except UTC+1 from 2026-03-08 02:00 to 2026-11-01 02:00 (local):
    /// that spring-forward day has no 02:00-02:59, that fall-back day has 01:00-01:59 twice.
    #[derive(Clone, Copy, Debug)]
    struct Dst;

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d).unwrap().and_hms_opt(h, mi, 0).unwrap()
    }

    impl TimeZone for Dst {
        type Offset = FixedOffset;
        fn from_offset(_: &FixedOffset) -> Self {
            Dst
        }
        fn offset_from_utc_datetime(&self, t: &NaiveDateTime) -> FixedOffset {
            let summer = *t >= utc(2026, 3, 8, 2, 0) && *t < utc(2026, 11, 1, 1, 0);
            FixedOffset::east_opt(if summer { 3600 } else { 0 }).unwrap()
        }
        fn offset_from_utc_date(&self, _: &NaiveDate) -> FixedOffset {
            FixedOffset::east_opt(0).unwrap()
        }
        fn offset_from_local_datetime(&self, l: &NaiveDateTime) -> MappedLocalTime<FixedOffset> {
            let fits = |secs: i64| {
                let off = FixedOffset::east_opt(secs as i32).unwrap();
                (self.offset_from_utc_datetime(&(*l - chrono::Duration::seconds(secs))) == off).then_some(off)
            };
            match (fits(3600), fits(0)) {
                (Some(a), Some(b)) => MappedLocalTime::Ambiguous(a, b),
                (Some(a), None) | (None, Some(a)) => MappedLocalTime::Single(a),
                (None, None) => MappedLocalTime::None,
            }
        }
        fn offset_from_local_date(&self, _: &NaiveDate) -> MappedLocalTime<FixedOffset> {
            MappedLocalTime::Single(FixedOffset::east_opt(0).unwrap())
        }
    }

    // ---- helpers: a Bangkok-like fixed zone (UTC+7, no DST) ----

    fn bkk() -> FixedOffset {
        FixedOffset::east_opt(7 * 3600).unwrap()
    }
    fn at(d: u32, h: u32, mi: u32) -> DateTime<FixedOffset> {
        bkk().with_ymd_and_hms(2026, 3, d, h, mi, 0).unwrap()
    }
    fn ts(d: u32, h: u32, mi: u32) -> Option<i64> {
        Some(at(d, h, mi).timestamp())
    }

    // ---- which slot is "the latest" ----

    #[test]
    fn slot_is_today_once_the_time_has_passed() {
        assert_eq!(latest_slot(3, 17, &at(10, 9, 0)), ts(10, 3, 17));
    }

    #[test]
    fn slot_is_yesterday_before_the_time() {
        assert_eq!(latest_slot(23, 0, &at(10, 9, 0)), ts(9, 23, 0));
    }

    #[test]
    fn a_slightly_early_trigger_counts_as_todays_slot() {
        assert_eq!(latest_slot(3, 17, &at(10, 3, 16)), ts(10, 3, 17));
        // ...but not one early by more than the grace
        assert_eq!(latest_slot(3, 17, &at(10, 3, 10)), ts(9, 3, 17));
    }

    #[test]
    fn midnight_and_last_minute_slots() {
        assert_eq!(latest_slot(0, 0, &at(10, 0, 0)), ts(10, 0, 0));
        assert_eq!(latest_slot(0, 0, &at(10, 23, 57)), ts(10, 0, 0));
        assert_eq!(latest_slot(23, 59, &at(10, 0, 1)), ts(9, 23, 59));
        // across a month boundary
        let feb28 = bkk().with_ymd_and_hms(2026, 2, 28, 23, 0, 0).unwrap().timestamp();
        assert_eq!(latest_slot(23, 0, &at(1, 1, 0)), Some(feb28));
        // the grace crossing midnight: 23:59 trigger for a 00:00 slot is tomorrow's slot
        assert_eq!(latest_slot(0, 0, &at(9, 23, 59)), ts(10, 0, 0));
    }

    #[test]
    fn slot_in_a_dst_gap_moves_past_the_gap() {
        // 02:30 does not exist on 2026-03-08; the slot becomes 03:30 local (= 02:30 UTC).
        let now = Dst.from_utc_datetime(&utc(2026, 3, 8, 9, 0));
        assert_eq!(latest_slot(2, 30, &now), Some(utc(2026, 3, 8, 2, 30).and_utc().timestamp()));
    }

    #[test]
    fn ambiguous_slot_on_fall_back_takes_the_first() {
        // 01:30 happens twice on 2026-11-01; the first is at UTC+1, i.e. 00:30 UTC.
        let now = Dst.from_utc_datetime(&utc(2026, 11, 1, 9, 0));
        assert_eq!(latest_slot(1, 30, &now), Some(utc(2026, 11, 1, 0, 30).and_utc().timestamp()));
    }

    #[test]
    fn dst_day_does_not_cause_a_skip_or_a_double_run() {
        // ran yesterday at the slot; machine off over the spring-forward night
        let last = Some(Dst.from_utc_datetime(&utc(2026, 3, 7, 2, 30)).timestamp());
        let now = Dst.from_utc_datetime(&utc(2026, 3, 8, 9, 0));
        assert_eq!(decide(last, 2, 30, &now), CatchUp::Run);
        // after that run the day is covered
        let later = now + chrono::Duration::hours(3);
        assert_eq!(decide(Some(now.timestamp()), 2, 30, &later), CatchUp::Skip);
    }

    // ---- the catch-up decision ----

    #[test]
    fn no_stamp_writes_a_baseline_instead_of_cleaning() {
        assert_eq!(decide(None, 3, 17, &at(10, 9, 0)), CatchUp::Baseline);
    }

    #[test]
    fn todays_slot_already_ran() {
        assert_eq!(decide(ts(10, 3, 20), 3, 17, &at(10, 9, 0)), CatchUp::Skip);
    }

    #[test]
    fn off_overnight_runs_once_at_login() {
        let now = at(10, 9, 0);
        assert_eq!(decide(ts(9, 3, 20), 3, 17, &now), CatchUp::Run);
        // the run records `now`; a second login the same day does nothing
        assert_eq!(decide(Some(now.timestamp()), 3, 17, &at(10, 13, 0)), CatchUp::Skip);
    }

    #[test]
    fn off_for_days_runs_once_not_once_per_day() {
        let now = at(10, 9, 0);
        assert_eq!(decide(ts(3, 3, 20), 3, 17, &now), CatchUp::Run);
        assert_eq!(decide(Some(now.timestamp()), 3, 17, &at(10, 9, 5)), CatchUp::Skip);
    }

    #[test]
    fn before_todays_slot_checks_yesterdays() {
        assert_eq!(decide(ts(9, 23, 5), 23, 0, &at(10, 9, 0)), CatchUp::Skip);
        assert_eq!(decide(ts(9, 22, 0), 23, 0, &at(10, 9, 0)), CatchUp::Run);
    }

    #[test]
    fn early_login_run_covers_the_daily_trigger_minutes_later() {
        // a login at 03:16 runs today's slot early...
        assert_eq!(decide(ts(9, 3, 20), 3, 17, &at(10, 3, 16)), CatchUp::Run);
        // ...and finishes at 03:16; the 03:17 daily trigger must not clean again
        assert_eq!(decide(ts(10, 3, 16), 3, 17, &at(10, 3, 17)), CatchUp::Skip);
    }

    #[test]
    fn daily_trigger_on_time_runs() {
        assert_eq!(decide(ts(9, 3, 25), 3, 17, &at(10, 3, 17)), CatchUp::Run);
    }

    #[test]
    fn stamp_from_the_future_is_not_trusted() {
        // clock moved back a day: the stamp claims a run tomorrow
        assert_eq!(decide(ts(11, 9, 0), 3, 17, &at(10, 9, 0)), CatchUp::Run);
        // a stamp a few seconds ahead (clock jitter) is still trusted
        let jitter = Some(at(10, 9, 0).timestamp() + 30);
        assert_eq!(decide(jitter, 3, 17, &at(10, 9, 0)), CatchUp::Skip);
    }

    #[test]
    fn moving_the_time_later_does_not_trigger_a_catch_up() {
        // ran at 03:17 today, then smart schedule moves the slot to 14:17
        assert_eq!(decide(ts(10, 3, 20), 14, 17, &at(10, 10, 0)), CatchUp::Skip);
    }

    #[test]
    fn moving_the_time_earlier() {
        // ran at 22:17 yesterday, slot moved to 04:17, now 10:00: today's 04:17 was missed
        assert_eq!(decide(ts(9, 22, 20), 4, 17, &at(10, 10, 0)), CatchUp::Run);
        // ran at 05:00 today, slot moved to 04:17: covered
        assert_eq!(decide(ts(10, 5, 0), 4, 17, &at(10, 10, 0)), CatchUp::Skip);
    }

    #[test]
    fn slot_is_local_time() {
        let now = at(10, 9, 0);
        // 03:17 in UTC is a different instant than 03:17 in UTC+7
        assert_ne!(latest_slot(3, 17, &now), latest_slot(3, 17, &now.with_timezone(&Utc)));
    }

    // ---- --catch-up / schedule time parsing ----

    #[test]
    fn parse_at_accepts_and_rejects() {
        assert_eq!(parse_at("03:17").unwrap(), (3, 17));
        assert_eq!(parse_at(" 7:05 ").unwrap(), (7, 5));
        assert_eq!(parse_at("0:0").unwrap(), (0, 0));
        assert_eq!(parse_at("23:59").unwrap(), (23, 59));
        for bad in ["24:00", "12:60", "-1:00", "abc", "", "12", "12:", ":30", "1:2:3", "12.30"] {
            assert!(parse_at(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    // ---- generated launchd agent / crontab ----

    #[test]
    fn plist_runs_at_load_with_catch_up() {
        let p = plist_xml("/Applications/rtidy.app/Contents/MacOS/rtidy", 3, 7, &[], "/tmp/rtidy.log");
        assert!(p.contains("<key>RunAtLoad</key><true/>"));
        assert!(p.contains("<string>auto</string><string>--catch-up</string><string>03:07</string>"));
        assert!(p.contains("<key>Hour</key><integer>3</integer><key>Minute</key><integer>7</integer>"));
    }

    #[test]
    fn plist_escapes_paths_and_keeps_argument_order() {
        let args = vec!["/Users/a & b/<code>".to_string(), "--days".to_string(), "7".to_string()];
        let p = plist_xml("/Users/a & b/rtidy", 23, 59, &args, "/Users/a & b/log");
        assert!(p.contains("<string>/Users/a &amp; b/rtidy</string><string>auto</string>"));
        assert!(p.contains(
            "<string>/Users/a &amp; b/&lt;code&gt;</string><string>--days</string><string>7</string><string>--catch-up</string><string>23:59</string>"
        ));
        assert!(!p.contains("a & b"));
    }

    #[test]
    fn cron_has_a_daily_and_a_reboot_line_both_tagged() {
        let c = cron_lines("/usr/bin/rtidy", 3, 7, &[]);
        let lines: Vec<&str> = c.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "7 3 * * * '/usr/bin/rtidy' auto # rtidy-auto");
        assert_eq!(lines[1], "@reboot sleep 60; '/usr/bin/rtidy' auto --catch-up 03:07 # rtidy-auto");
        // removal filters line by line on the tag, so both must carry it
        assert!(lines.iter().all(|l| l.contains(CRON_TAG)));
    }

    #[test]
    fn cron_quotes_spaces_quotes_and_percent() {
        let args = vec!["/home/me/it's 100%".to_string(), "--days".to_string(), "7".to_string()];
        let c = cron_lines("/opt/r tidy/rtidy", 0, 0, &args);
        let cmd = r"'/opt/r tidy/rtidy' auto '/home/me/it'\''s 100\%' '--days' '7'";
        assert_eq!(c, format!("0 0 * * * {cmd} # rtidy-auto\n@reboot sleep 60; {cmd} --catch-up 00:00 # rtidy-auto"));
    }

    // ---- Windows task file ----

    #[test]
    fn task_xml_carries_every_setting_in_one_file() {
        let day = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        let x = task_xml(r"C:\Program Files\rtidy\rtidy.exe", 0, 17, &[], day);
        assert!(x.contains("<StartBoundary>2026-11-24T00:17:00</StartBoundary>"));
        for setting in [
            "<StartWhenAvailable>true</StartWhenAvailable>",
            "<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
            "<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
            "<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
            "<ExecutionTimeLimit>PT4H</ExecutionTimeLimit>",
            "<LogonType>InteractiveToken</LogonType>",
        ] {
            assert!(x.contains(setting), "missing {setting}");
        }
        assert!(x.contains(r#"<Command>"C:\Program Files\rtidy\rtidy.exe"</Command><Arguments>auto</Arguments>"#));
    }

    #[test]
    fn task_xml_quotes_and_escapes_arguments() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let args = vec![r"D:\R & D\proj".to_string(), "--days".to_string(), "7".to_string()];
        let x = task_xml(r"C:\a&b\rtidy.exe", 9, 5, &args, day);
        assert!(x.contains(r#"<Arguments>auto "D:\R &amp; D\proj" --days 7</Arguments>"#));
        assert!(x.contains(r#"<Command>"C:\a&amp;b\rtidy.exe"</Command>"#));
    }

    #[test]
    fn task_xml_has_no_length_limit_for_many_folders() {
        // schtasks /tr refused commands over 261 characters
        let args: Vec<String> = (0..40).map(|i| format!(r"D:\projects\some long folder name {i}")).collect();
        let x = task_xml(r"C:\rtidy.exe", 3, 0, &args, NaiveDate::from_ymd_opt(2026, 1, 1).unwrap());
        assert!(between(&x, "<Arguments>", "</Arguments>").unwrap().len() > 1000);
    }

    // ---- reading back which program a task starts ----

    #[test]
    fn program_round_trips_through_every_format() {
        let day = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let win = r"C:\a & b\rtidy.exe";
        assert_eq!(program_in_task_xml(&task_xml(win, 3, 0, &[], day)), Some(PathBuf::from(win)));
        // the format schtasks itself exports
        let exported = "<Exec>\r\n      <Command>\"C:\\Users\\me\\rtidy.exe\"</Command>\r\n      <Arguments>auto</Arguments>";
        assert_eq!(program_in_task_xml(exported), Some(PathBuf::from(r"C:\Users\me\rtidy.exe")));

        let mac = "/Users/a & b/<x>/rtidy";
        assert_eq!(program_in_plist(&plist_xml(mac, 3, 0, &[], "/log")), Some(PathBuf::from(mac)));

        let unix = "/home/me/it's 100%/r tidy";
        let cron = cron_lines(unix, 3, 0, &["--days".into()]);
        let daily = cron.lines().next().unwrap();
        assert_eq!(program_in_command(daily.splitn(6, ' ').nth(5).unwrap()), Some(PathBuf::from(unix)));
        // systemd's quoting: "…" with \\, \" and %%
        assert_eq!(program_in_command(r#""/opt/a \"b\" 100%%/rtidy" auto"#), Some(PathBuf::from(r#"/opt/a "b" 100%/rtidy"#)));
        assert_eq!(program_in_command("/usr/bin/rtidy auto"), Some(PathBuf::from("/usr/bin/rtidy")));
    }

    // ---- next run, for the status line ----

    #[test]
    fn next_slot_is_later_today_or_tomorrow() {
        assert_eq!(next_slot(23, 0, &at(10, 9, 0)).map(|t| t.timestamp()), ts(10, 23, 0));
        assert_eq!(next_slot(3, 17, &at(10, 9, 0)).map(|t| t.timestamp()), ts(11, 3, 17));
        // exactly at the slot: that one is happening now, the next is tomorrow
        assert_eq!(next_slot(3, 17, &at(10, 3, 17)).map(|t| t.timestamp()), ts(11, 3, 17));
        // over a DST gap the slot still exists, moved past the gap
        let eve = Dst.from_utc_datetime(&utc(2026, 3, 7, 12, 0));
        assert_eq!(next_slot(2, 30, &eve).map(|t| t.timestamp()), Some(utc(2026, 3, 8, 2, 30).and_utc().timestamp()));
    }
}
