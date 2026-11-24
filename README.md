<p align="center"><img src="assets/logo.png" width="96" alt="rtidy logo"></p>
<h1 align="center">rtidy</h1>
<p align="center">Keeps your Rust <code>target/</code> folders tidy — on Windows, macOS and Linux.<br>
<b>English</b> · <a href="README.th.md">ภาษาไทย</a></p>

<p align="center"><img src="assets/showcase/tour.gif" width="760" alt="rtidy tour: setup wizard, dashboard, AI insights"></p>

## The problem

Every Rust project grows its own `target/` folder, often several GB each, scattered across your disk.
Shared dependencies get compiled again and again, and nothing ever cleans up.

## What rtidy does (one click)

1. **One shared target folder** — every project builds into the same place (`build.target-dir` in `~/.cargo/config.toml`), so new `target/` folders stop appearing and shared dependencies compile once.
2. **Lean builds** — dependencies skip debug info (often half the size); your own code keeps it.
3. **Removes leftovers** — old per-project `target/` folders (except projects marked *Keep*).
4. **Trims the shared folder** — drops incremental caches and artifacts you won't reuse, and keeps it under a size cap.
5. **Trims the cargo download cache** in `~/.cargo`.
6. **Cleans itself every day** — Task Scheduler (Windows), launchd (macOS), systemd timer or cron (Linux). Missed runs catch up once, at the next start-up or login.

## Showcase

<table>
<tr>
<td width="50%"><img src="assets/showcase/01-welcome.png" alt="Welcome"><br><b>1. Welcome</b> — first launch opens a setup wizard (EN | ไทย).</td>
<td width="50%"><img src="assets/showcase/02-folders.png" alt="Folders"><br><b>2. Folders</b> — common code folders are detected; add or type your own.</td>
</tr>
<tr>
<td><img src="assets/showcase/03-preferences.png" alt="Preferences"><br><b>3. Preferences</b> — Smart mode, lean builds and daily clean-up, with sensible defaults.</td>
<td><img src="assets/showcase/04-review.png" alt="Review"><br><b>4. Review</b> — see what was found and exactly what will happen, then <i>Tidy up now</i>.</td>
</tr>
<tr>
<td><img src="assets/showcase/05-dashboard.png" alt="Dashboard"><br><b>Dashboard</b> — what can be freed, the disk forecast and one <i>Fix everything</i> button.</td>
<td><img src="assets/showcase/06-projects-ai.png" alt="Projects with AI scores"><br><b>Projects</b> — every leftover <code>target/</code> with size, age and the AI's chance you'll build it again.</td>
</tr>
<tr>
<td><img src="assets/showcase/08-insights-th.png" alt="Insights in Thai"><br><b>Insights (ไทย)</b> — forecast, what you won't reuse, unusual growth and model accuracy.</td>
<td><img src="assets/showcase/07-settings.png" alt="Settings"><br><b>Settings</b> — language, Smart mode, folders and hard limits that always apply.</td>
</tr>
</table>

<sub>Screenshots use a demo setup; your numbers will differ.</sub>

## Use cases

| Situation | What rtidy does for you |
|---|---|
| **Laptop with a small SSD** (256–512 GB) that keeps filling up | Forecasts *"disk full in ~N days"*, cleans harder as space runs out, and keeps the shared folder under a cap you choose. |
| **Lots of small projects** — tutorials, Advent of Code, experiments, cloned examples | Stops creating a new `target/` in each one; common crates like `serde`, `tokio` or `syn` compile once and are shared by all. |
| **Open-source contributor** juggling many repos and branches | Projects you've moved on from are removed first (the model learns which ones you return to); mark active ones *Keep* to protect them. |
| **Game / graphics dev** (Bevy, wgpu, egui) with multi-GB debug builds | Lean builds drop dependency debug info, and anomaly alerts flag a sudden jump such as a new release build or toolchain. |
| **Workstation where builds must never slow down** | Smart schedule learns when you never build and runs the clean-up then; anything used in the last day or two is never touched. |
| **Classroom, bootcamp or team machines** | Installer plus a 4-step wizard: anyone can set it up in a minute, in English or Thai, with no terminal needed. |
| **Terminal-only / servers / CI runners** | `rtidy fix` sets everything up in one command; `rtidy auto` runs on a schedule; `rtidy insights` prints the analysis. |

## Smart mode (on-device AI)

A tiny machine-learning brain — no LLM, no GPU, no internet. It is a few hundred numbers in a JSON file, and each analysis takes milliseconds.

- **Reuse prediction.** An online logistic-regression model scores every build unit in the shared folder, and every leftover project `target/`, by the chance you'll use it again within 14 days. Inputs are how recently and how often it was used, how long it has existed, its size and recent git activity. Units you won't need go first; anything used in the last day or two is never touched.
- **It learns from outcomes.** Every prediction is checked 14 days later. If rtidy removed something and you rebuilt it, the model learns it was wrong.
- **Disk forecast.** Weighted regression over free-space history gives "disk full in ~N days". The cleaning threshold and the size cap tighten automatically as the disk fills up.
- **Anomaly alerts.** A robust median/MAD test flags unusual growth, for example a new toolchain or a huge dependency.
- **Smart schedule.** rtidy learns the hours you never build and moves the daily clean-up there.

The **Insights** tab explains every decision. Hard limits from Settings still apply, and Smart mode can be switched off.

## Language

English and Thai (ไทย). rtidy picks your system language and you can switch anytime (EN | ไทย in the header or Settings). A Thai font (Noto Sans Thai, OFL) is bundled, so Thai renders on every OS.

## Install

Build from source (see below), or download installers from **Actions → build → Artifacts**:

| OS | File |
|---|---|
| Windows | `rtidy-setup-x64.exe` (installer) or `rtidy-windows-x64-portable.exe` |
| macOS (Intel + Apple Silicon) | `rtidy-macos-universal.dmg` — drag to Applications. First launch: right-click → Open (not notarized) |
| Linux | `rtidy_<ver>_amd64.deb`, `rtidy-x86_64.AppImage`, or `rtidy-linux-x86_64.tar.gz` |

### With cargo

```sh
cargo install rtidy                           # app + CLI
cargo install rtidy --no-default-features     # CLI only, no desktop app
```

The desktop app is the `gui` feature, and it is on by default. It brings in `eframe`,
which is most of rtidy's dependency tree (135 crates against 55) and needs X11/Wayland
development headers on Linux. `--no-default-features` builds just the command-line tool:
a 2 MB binary that compiles anywhere with no system libraries. Everything except the
window works the same.

Straight from the repository, without crates.io:

```sh
cargo install --git https://github.com/thirawat27/Rtidy
```

## Use

**Open the app** → the setup wizard walks you through folders and options → **Tidy up now**. Done; it keeps itself tidy from then on.

Prefer the terminal? One command does the same:

```sh
rtidy fix            # uses detected/saved folders
rtidy fix ~/code     # or name them
```

<details>
<summary>All commands</summary>

| Command | What it does |
|---|---|
| `rtidy` | Open the app |
| `rtidy fix [folders]` | Everything in one go (saves settings) |
| `rtidy insights` | AI insights: forecast, what you won't reuse, unusual growth |
| `rtidy doctor [folders]` | Where the space went and how to fix it |
| `rtidy scan [folders]` | List `target/` folders with size and age |
| `rtidy clean [folders] --days 30 [--dry-run]` | Delete `target/` not built for N days |
| `rtidy setup [--lean] [--target-dir P]` | Shared target folder only |
| `rtidy gc [--days 14] [--max-size 30G]` | Trim the shared folder |
| `rtidy cache [--days 30]` | Trim `~/.cargo` downloads |
| `rtidy auto` | What the daily task runs (saved settings) |
| `rtidy schedule [--at 21:00] / --remove` | Daily task on/off |
| `rtidy log` | Results of past daily runs |
| `rtidy uninstall` | Remove the daily task and menu entry |

Put an empty `.rtidy-keep` file next to a project's `Cargo.toml` to never clean it.
</details>

## Good to know

- Two projects building at the same time wait for each other's lock on the shared folder.
- Binaries with the same name from different projects overwrite each other in `<shared>/debug/`; use `cargo run` / `cargo install` rather than copying from there.
- A project with its own `target-dir` setting keeps it (`rtidy doctor` lists these).
- On Windows the app is a GUI program; in a terminal its output appears after the prompt returns.
- To undo the shared folder, delete `target-dir` under `[build]` in `~/.cargo/config.toml` (a backup is saved as `config.toml.bak`).

## Author

**Thirawat Sinlapasomsak** — [github.com/thirawat27](https://github.com/thirawat27)

## License

Copyright © 2026 Thirawat Sinlapasomsak.

rtidy is free software under the [GNU General Public License v3.0](LICENSE). You may
use, study, share and change it; if you distribute a changed version, it has to stay
under the same licence and ship its source. It comes with no warranty.
