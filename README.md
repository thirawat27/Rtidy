# rtidy

Find, clean and **prevent** scattered Rust `target/` directories.

## Install

```sh
cargo install --path .
```

## Quick start (fix the root cause once)

```sh
rtidy setup --clean-old C:\projects     # one shared target dir + delete old target/ folders
rtidy schedule C:\projects              # daily auto-clean at 21:00
```

`setup` writes `build.target-dir` into `~/.cargo/config.toml` (backup saved as `config.toml.bak`),
so every project builds into one folder (default: `%LOCALAPPDATA%\cargo-target`).
Shared dependencies are compiled once instead of once per project.

## Commands

| Command | What it does |
|---|---|
| `rtidy scan [roots]` | List `target/` dirs with size and last build time |
| `rtidy clean [roots] --days 30 [--dry-run]` | Delete `target/` dirs not built for N days |
| `rtidy setup [--target-dir P] [--clean-old roots]` | Make all projects share one target dir |
| `rtidy gc --days 14 [--dry-run]` | Shrink the shared dir: drop `incremental/` and artifacts untouched for N days |
| `rtidy auto [roots]` | `clean` + `gc` (what the schedule runs) |
| `rtidy schedule [roots] --at 21:00` / `--remove` | Windows Task Scheduler (prints a crontab line on Linux/macOS) |

## Notes

- Two projects building at the same time will wait on each other's lock in the shared dir.
- Binaries with the same name from different projects overwrite each other in `<shared>/debug/`.
- A project that sets its own `target-dir` (or `CARGO_TARGET_DIR`) keeps using it.
- Files in use by a running build are skipped and retried on the next run.
