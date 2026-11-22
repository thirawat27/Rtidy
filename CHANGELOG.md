# Changelog

All notable changes to rtidy are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and rtidy uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] — 2026-11-22

First release.

rtidy finds, cleans and prevents scattered Rust `target/` directories. It fixes the
cause rather than the symptom: every project is pointed at one shared target folder,
so new `target/` directories stop appearing, and a daily task keeps that folder and
the cargo download cache small.

### Added

- **One-click setup.** `rtidy fix`, or **Fix everything** in the app, sets
  `build.target-dir`, builds dependencies without debug info, removes leftover
  per-project `target/` folders, trims the cargo cache and registers the daily
  clean-up.
- **Desktop app** with a first-run wizard, a dashboard, per-project list, settings and
  an activity log. English and Thai throughout, switchable at any time.
- **Smart mode.** A small on-device model learns which build artifacts and projects
  you come back to, and cleans the ones you are unlikely to need first. It also
  forecasts when the disk will fill, flags unusual growth, and moves the daily
  clean-up to the hour you build least. Everything stays on this computer.
- **Project discovery** that searches every drive and mount point for folders that
  really hold a `Cargo.toml`, rather than guessing from folder names.
- **Scheduled clean-up** using each platform's own mechanism: Task Scheduler on
  Windows, a launchd agent on macOS, a systemd user timer on Linux (falling back to
  cron). All of them catch up after the machine has been off.
- **CLI** for everything the app does: `fix`, `scan`, `clean`, `setup`, `gc`,
  `cache`, `doctor`, `insights`, `find`, `schedule`, `log`, `uninstall`.
- `.rtidy-keep` next to a `Cargo.toml` protects that project's `target/` from any
  clean-up.
- `rtidy doctor` explains where the disk space went and what to do about it.
- **Installable with cargo**, either from crates.io or straight from the repository:

  ```sh
  cargo install rtidy                        # app + CLI
  cargo install rtidy --no-default-features  # CLI only
  cargo install --git https://github.com/thirawat27/Rtidy
  ```

  The desktop app is the `gui` feature and is on by default. Turning it off drops the
  dependency tree from 135 crates to 55 and the binary to about 2 MB, and removes the
  need for X11/Wayland development headers on Linux, so a CLI-only install builds
  anywhere. Everything except the window behaves the same.

### Performance

Measured on a 112,000-directory drive:

- Repeat scans are about **30× faster** (6.1 s → 0.2 s). rtidy remembers which
  folders held no Rust and skips them while they are untouched; `--deep` always walks
  everything.
- A first, cold scan is about **1.3× faster** (6.1 s → 4.7 s) by walking independent
  subtrees in parallel. Beyond that the walk is bound by the filesystem, not by
  rtidy: an empty walk that does no work at all costs the same.
- Deleting is about **2.6× faster** (2.1 s → 0.8 s), removing independent target
  folders at the same time.
- Sizing no longer happens twice. Anything already measured is deleted without being
  walked a second time.
- The project tree is walked once per operation instead of twice, and the two file
  opens that every directory used to cost are gone.

### Notes

- Requires Rust 1.99 or newer to build; the crate uses the 2024 edition.
- Understands both the Cargo target layout used up to 1.99 and the one introduced in
  1.100, so a shared folder filled by either toolchain is managed correctly.
- The run log is capped and rotated, so an unattended daily clean-up cannot fill the
  disk it is meant to be freeing.
- All icons are drawn as vector shapes rather than typed as emoji, so they render
  identically on Windows, macOS and Linux and do not depend on a system font.
- Licensed under the GNU General Public License v3.0.
