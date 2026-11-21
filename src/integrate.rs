//! Make rtidy show up like a normal app when it was downloaded as a plain binary:
//! - Linux:   ~/.local/share/applications/rtidy.desktop + icon
//! - Windows: Start menu shortcut
//! - macOS:   the .app bundle from the .dmg already is an app; nothing to do.

use crate::schedule::exe_path;
#[allow(unused_imports)]
use anyhow::{Context, Result};
#[allow(unused_imports)]
use std::fs;
#[allow(unused_imports)]
use std::path::PathBuf;

#[allow(dead_code)]
const LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");

/// Whether this OS needs/supports an "add to app menu" step.
pub fn supported() -> bool {
    cfg!(any(windows, all(unix, not(target_os = "macos"))))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_file() -> Result<PathBuf> {
    Ok(dirs::data_dir().context("no data dir")?.join("applications/rtidy.desktop"))
}

#[cfg(windows)]
fn shortcut() -> Result<PathBuf> {
    Ok(dirs::data_dir().context("no data dir")?.join(r"Microsoft\Windows\Start Menu\Programs\rtidy.lnk"))
}

pub fn is_integrated() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    return desktop_file().is_ok_and(|p| p.exists())
        || std::path::Path::new("/usr/share/applications/rtidy.desktop").exists();
    #[cfg(windows)]
    return shortcut().is_ok_and(|p| p.exists());
    #[cfg(target_os = "macos")]
    return true;
}

pub fn integrate() -> Result<()> {
    let exe = exe_path()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let data = dirs::data_dir().context("no data dir")?;
        let icon_dir = data.join("icons/hicolor/256x256/apps");
        fs::create_dir_all(&icon_dir)?;
        fs::write(icon_dir.join("rtidy.png"), LOGO_PNG)?;
        let df = desktop_file()?;
        fs::create_dir_all(df.parent().unwrap())?;
        fs::write(
            &df,
            format!(
                "[Desktop Entry]\nType=Application\nName=rtidy\nGenericName=Rust target cleaner\n\
                 Comment=Keeps your Rust target folders tidy\nExec=\"{}\"\nIcon=rtidy\nTerminal=false\n\
                 Categories=Development;Utility;\nKeywords=rust;cargo;target;clean;disk;\n",
                exe.display()
            ),
        )?;
        let _ = crate::util::quiet_cmd("update-desktop-database").arg(df.parent().unwrap()).status();
    }
    #[cfg(windows)]
    {
        let lnk = shortcut()?;
        let script = format!(
            "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Description='Keeps your Rust target folders tidy';$s.Save()",
            lnk.display().to_string().replace('\'', "''"),
            crate::util::plain(&exe).replace('\'', "''")
        );
        let ok = crate::util::quiet_cmd("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .status()?
            .success();
        if !ok {
            anyhow::bail!("could not create Start menu shortcut");
        }
    }
    let _ = exe;
    Ok(())
}

pub fn unintegrate() -> Result<()> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = fs::remove_file(desktop_file()?);
        if let Some(d) = dirs::data_dir() {
            let _ = fs::remove_file(d.join("icons/hicolor/256x256/apps/rtidy.png"));
        }
    }
    #[cfg(windows)]
    {
        let _ = fs::remove_file(shortcut()?);
    }
    Ok(())
}
