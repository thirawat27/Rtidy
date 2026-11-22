use anyhow::{Context, Result};
use rayon::prelude::*;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;
use walkdir::WalkDir;

/// The log file, opened once by `enable_file_log` and held for the whole run.
/// Scheduled clean-ups emit a line per deleted folder; re-opening the file for each
/// one cost a create+append+close syscall round-trip per line.
static LOG_FILE: Mutex<Option<File>> = Mutex::new(None);

/// Keep the run log from growing without bound: a daily task appends to it forever.
/// Past the cap the older half goes and the recent tail is kept.
const LOG_FILE_CAP: u64 = 2 << 20;

fn rotate_log(path: &Path) {
    let Ok(len) = fs::metadata(path).map(|m| m.len()) else { return };
    if len <= LOG_FILE_CAP {
        return;
    }
    let tail = read_tail(path, LOG_FILE_CAP / 2);
    let _ = fs::write(path, tail);
}

/// Roughly the last `max_bytes` of a text file (the whole file if it is smaller).
/// Seeking beats reading a megabyte of history to find the final `auto:` line.
pub fn read_tail(path: &Path, max_bytes: u64) -> String {
    let Ok(mut f) = File::open(path) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes);
    if start > 0 && f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    if f.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buf).into_owned();
    match start {
        // Seeking lands mid-line: drop the partial first one.
        1.. => text.split_once('\n').map(|(_, rest)| rest.to_owned()).unwrap_or_default(),
        0 => text,
    }
}

/// Also append every `say!` line to the log file (used by scheduled runs).
pub fn enable_file_log() {
    if let Ok(path) = log_path() {
        rotate_log(&path);
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path)
            && let Ok(mut slot) = LOG_FILE.lock()
        {
            *slot = Some(f);
        }
    }
}

/// Messages for the GUI log panel (drained by the GUI each frame).
pub static LOG_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Upper bound on buffered log lines. Without the GUI draining it (`rtidy auto`,
/// `rtidy fix`) this buffer has no other reader, so a big clean-up would otherwise
/// grow it without limit. Past the cap the oldest half goes, keeping the recent tail.
const LOG_LINES_CAP: usize = 4096;

pub fn say(msg: &str) {
    println!("{msg}");
    if let Ok(mut v) = LOG_LINES.lock() {
        if v.len() >= LOG_LINES_CAP {
            v.drain(..LOG_LINES_CAP / 2);
        }
        v.push(msg.to_string());
    }
    if let Ok(mut slot) = LOG_FILE.lock()
        && let Some(f) = slot.as_mut()
    {
        let now = humantime::format_rfc3339_seconds(SystemTime::now());
        let _ = writeln!(f, "{now} {msg}");
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

/// Total bytes of a directory tree.
///
/// The top-level children are sized in parallel. A Cargo target dir is a handful of
/// very large subtrees (`deps`, `build`, `.fingerprint`, `incremental`), so splitting
/// the work there keeps every core busy instead of one thread issuing every `stat`.
/// Nesting inside another rayon job is fine: work-stealing just folds it in.
pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(p) else {
        return walk_size(p); // not a directory (or unreadable): fall back
    };
    let mut files = 0;
    let mut dirs = Vec::new();
    for e in rd.flatten() {
        match e.file_type() {
            Ok(ft) if ft.is_dir() => dirs.push(e.path()),
            Ok(ft) if ft.is_file() => files += e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => {}
        }
    }
    files + dirs.par_iter().map(|d| walk_size(d)).sum::<u64>()
}

/// Single-threaded recursive size. `file_type()` comes free from the directory read,
/// so non-files never cost a `metadata()` call.
fn walk_size(p: &Path) -> u64 {
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

/// Modified time from metadata already in hand, so callers that read an entry's
/// metadata for its size don't pay a second `stat` for its age.
pub fn mtime_of(md: &fs::Metadata) -> SystemTime {
    md.modified().unwrap_or(SystemTime::UNIX_EPOCH)
}

pub fn age_days(t: SystemTime) -> u64 {
    SystemTime::now().duration_since(t).unwrap_or_default().as_secs() / 86_400
}

/// Delete a file or folder; returns bytes freed (0 on failure, e.g. locked by a running build).
///
/// Prefer `remove_known` wherever the size is already at hand: measuring a tree costs a
/// full recursive walk, and every caller that scanned the folder first already has it.
pub fn remove(p: &Path, dry_run: bool) -> u64 {
    remove_known(p, size_of(p), dry_run)
}

/// `remove` for callers that already measured `p`, skipping the pre-delete walk.
pub fn remove_known(p: &Path, size: u64, dry_run: bool) -> u64 {
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

/// `plain`, keeping it a path. Anything stored in the config or shown in the UI goes
/// through this: the extended-length prefix is an internal Windows detail, and it
/// looked like corruption when it reached the folder list on screen.
pub fn plain_path(p: &Path) -> PathBuf {
    PathBuf::from(plain(p))
}

/// Absolute, tidy form of a path that need not already exist.
///
/// `canonicalize` is the usual way to absolutise, but it also resolves symlinks, hands
/// back the `\\?\` form on Windows, and fails outright on a path that is not there.
/// `std::path::absolute` does none of that.
pub fn tidy_abs(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    plain_path(&abs)
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
    fn tail_reads_only_the_end() {
        let dir = std::env::temp_dir().join(format!("rtidy-tail-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("log.txt");

        // Smaller than the window: the whole file comes back.
        fs::write(&f, "a
b
c
").unwrap();
        assert_eq!(read_tail(&f, 1024), "a
b
c
");

        // Larger than the window: the tail comes back, and the partial first line
        // the seek landed in the middle of is dropped.
        let body: String = (0..500).map(|i| format!("line{i}
")).collect();
        fs::write(&f, &body).unwrap();
        let tail = read_tail(&f, 64);
        assert!(body.ends_with(&tail), "tail must be a suffix of the file");
        assert!(tail.len() <= 64, "tail {} bytes", tail.len());
        assert!(tail.starts_with("line"), "partial first line not dropped: {tail:?}");
        assert!(tail.ends_with("line499
"));

        // Empty and missing files are not errors.
        fs::write(&f, "").unwrap();
        assert_eq!(read_tail(&f, 64), "");
        assert_eq!(read_tail(&dir.join("nope.txt"), 64), "");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn known_size_removal_matches_measured() {
        let dir = std::env::temp_dir().join(format!("rtidy-rm-{}", std::process::id()));
        let tree = dir.join("tree");
        fs::create_dir_all(tree.join("sub")).unwrap();
        fs::write(tree.join("a.bin"), vec![0u8; 3000]).unwrap();
        fs::write(tree.join("sub/b.bin"), vec![0u8; 1500]).unwrap();

        let measured = size_of(&tree);
        assert_eq!(measured, 4500);
        // Dry run reports the size without touching anything.
        assert_eq!(remove_known(&tree, measured, true), measured);
        assert!(tree.exists());
        // And the real delete reports the same number `remove` would have walked for.
        assert_eq!(remove_known(&tree, measured, false), measured);
        assert!(!tree.exists());

        fs::remove_dir_all(&dir).unwrap();
    }

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
