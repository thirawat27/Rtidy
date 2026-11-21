use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use walkdir::WalkDir;

static LOG_TO_FILE: AtomicBool = AtomicBool::new(false);

/// Also append every `say!` line to the log file (used by scheduled runs).
pub fn enable_file_log() {
    LOG_TO_FILE.store(true, Ordering::Relaxed);
}

/// Messages for the GUI log panel (drained by the GUI each frame).
pub static LOG_LINES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn say(msg: &str) {
    println!("{msg}");
    if let Ok(mut v) = LOG_LINES.lock() {
        v.push(msg.to_string());
    }
    if LOG_TO_FILE.load(Ordering::Relaxed) {
        if let Ok(path) = log_path() {
            if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
                let now = humantime::format_rfc3339_seconds(SystemTime::now());
                let _ = writeln!(f, "{now} {msg}");
            }
        }
    }
}

#[macro_export]
macro_rules! say {
    ($($t:tt)*) => { $crate::util::say(&format!($($t)*)) };
}

pub fn log_path() -> Result<PathBuf> {
    let dir = dirs::cache_dir().context("no cache dir")?.join("rtidy");
    fs::create_dir_all(&dir)?;
    Ok(dir.join("rtidy.log"))
}

pub fn dir_size(p: &Path) -> u64 {
    WalkDir::new(p)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

pub fn size_of(p: &Path) -> u64 {
    if p.is_dir() {
        dir_size(p)
    } else {
        fs::metadata(p).map(|m| m.len()).unwrap_or(0)
    }
}

pub fn mtime(p: &Path) -> SystemTime {
    fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

pub fn age_days(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).unwrap_or_default().as_secs() / 86_400
}

/// Delete a file or folder; returns bytes freed (0 on failure, e.g. locked by a running build).
pub fn remove(p: &Path, dry_run: bool) -> u64 {
    let size = size_of(p);
    if dry_run {
        return size;
    }
    let r = if p.is_dir() { fs::remove_dir_all(p) } else { fs::remove_file(p) };
    if r.is_ok() { size } else { 0 }
}

pub fn human(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", U[i])
}

/// "20G", "500MB", "1.5t", "1048576" → bytes.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let up = s.trim().to_ascii_uppercase();
    let t = up.strip_suffix("IB").or_else(|| up.strip_suffix('B')).unwrap_or(&up);
    let (num, mul) = match t.chars().last() {
        Some('K') => (&t[..t.len() - 1], 1u64 << 10),
        Some('M') => (&t[..t.len() - 1], 1 << 20),
        Some('G') => (&t[..t.len() - 1], 1 << 30),
        Some('T') => (&t[..t.len() - 1], 1 << 40),
        _ => (t, 1),
    };
    let n: f64 = num.trim().parse().map_err(|_| format!("bad size: {s} (try 20G, 500M)"))?;
    Ok((n * mul as f64) as u64)
}

pub fn cargo_home() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("CARGO_HOME") {
        return Ok(PathBuf::from(h));
    }
    Ok(dirs::home_dir().context("no home dir")?.join(".cargo"))
}

pub fn rustup_home() -> Result<PathBuf> {
    if let Ok(h) = std::env::var("RUSTUP_HOME") {
        return Ok(PathBuf::from(h));
    }
    Ok(dirs::home_dir().context("no home dir")?.join(".rustup"))
}

pub fn cargo_config_path() -> Result<PathBuf> {
    let home = cargo_home()?;
    let legacy = home.join("config");
    if legacy.is_file() && !home.join("config.toml").exists() {
        return Ok(legacy);
    }
    Ok(home.join("config.toml"))
}

pub fn read_cargo_config() -> Result<(PathBuf, String, toml_edit::DocumentMut)> {
    let path = cargo_config_path()?;
    let text = fs::read_to_string(&path).unwrap_or_default();
    let doc = text.parse().with_context(|| format!("parse {}", path.display()))?;
    Ok((path, text, doc))
}

pub fn write_cargo_config(path: &Path, old_text: &str, doc: &toml_edit::DocumentMut) -> Result<()> {
    if !old_text.is_empty() {
        fs::copy(path, path.with_extension("toml.bak"))?;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, doc.to_string())?;
    Ok(())
}

/// On Windows, canonicalize() gives `\\?\C:\...`; cargo and schtasks prefer plain `C:\...`.
pub fn plain(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

/// A Command that never flashes a console window on Windows (we may be a GUI app).
pub fn quiet_cmd(program: &str) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub fn free_space(p: &Path) -> Option<u64> {
    fs4::available_space(p).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sizes() {
        assert_eq!(parse_size("20G").unwrap(), 20 << 30);
        assert_eq!(parse_size("500mb").unwrap(), 500 << 20);
        assert_eq!(parse_size("1.5T").unwrap(), (1.5 * (1u64 << 40) as f64) as u64);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("2GiB").unwrap(), 2 << 30);
        assert!(parse_size("abc").is_err());
    }
}
