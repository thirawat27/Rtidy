# Changelog

All notable changes to rtidy are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and rtidy uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.1.0] — 2026-11-24

### Fixed

- The daily clean-up now catches up after the computer was switched off on macOS and
  on Linux systems using cron, not only after sleep. The launchd agent runs at login
  and cron gets an `@reboot` line, both through `rtidy auto --catch-up HH:MM`, which
  cleans only when the latest daily slot was missed (rtidy keeps a `last-auto` stamp
  next to its log), so a login never cleans twice in one day.
- On Windows, failing to turn on "run missed task" is now reported instead of being
  ignored silently.
- Scheduled clean-ups take a lock, so a login trigger and the daily trigger landing
  together (or the agent reloading itself after smart mode moves the time) can no
  longer clean the same folders at once.
- crontab lines escape `%`, which cron otherwise turns into a line break.
- On Windows the task is registered in one step from a task file instead of
  `schtasks` followed by PowerShell: changing the schedule is about 2.4× faster, the
  "run missed task" setting can no longer be left off, a hung run is stopped after
  4 hours, and long folder lists no longer hit the 261-character command limit.
- The settings page no longer starts `systemctl` on every frame on Linux.
- The `last-auto` stamp is replaced atomically, and cron's `@reboot` catch-up waits a
  minute so network and removable drives are mounted first.

- The app's preview could be acted on with one click. The button that carried a plan out
  sat in the preview's own header, in the same colour as the dashboard's main action, and
  a single press removed everything listed. It is a secondary button now, it asks a second
  time with the amount on it, and the preview says plainly that nothing has been removed
  yet. Carrying a plan out also acts on exactly the list that was shown, rather than
  scanning again and possibly removing something that was never on screen.

### Added

- `rtidy schedule --status` shows whether the daily clean-up is on, how missed runs
  catch up on this system, when it last finished and when it runs next. It, and
  `rtidy doctor`, warn when the task points at a copy of rtidy that no longer exists.
- Catch-up runs say so in the log.
- **Pick the clean-up time yourself in the app.** The dashboard card, Settings and
  the setup wizard have an hour and minute picker (drag, scroll or type) with
  one-click presets (night 03:00, lunch 12:30, evening 18:00). The dashboard applies
  the new time without a trip to Settings, and shows when the next run is and how
  long until then. Setting a time by hand turns Smart schedule off, and the app says
  so; before, the time box was greyed out whenever Smart schedule was on.
- The dashboard warns when the daily task starts a copy of rtidy that no longer
  exists, with a one-click **Repair**.
- **Preview, then clean.** Nothing is removed on the spot any more. A clean-up first
  builds a plan — every action with its size, age, reuse chance, estimated rebuild
  seconds and a reason — and `plan::apply` is now the only code in rtidy that removes
  anything. The app has a **Preview** tab and a *Preview…* button on the dashboard
  showing what would go, the total, the estimated rebuild time and a breakdown per
  language and per kind, with a *Tidy up now* button. `rtidy plan [folders] [--days N]
  [--json]` prints the same plan in the terminal and removes nothing.
- **Undo.** Project build folders are no longer unlinked: they are renamed into a
  `.rtidy-trash` folder at the top of the same drive (the same filesystem, so the move
  is instant whatever the size) and recorded in an index, and `rtidy undo [--all]`, or
  *Undo last clean-up* in the app, puts the last run back. Entries older than
  `trash_days` — 3 days by default, 0–30 in Settings, 0 to delete outright — are purged
  at the start of every clean-up, which is when the space is actually handed back.
  Caches and build units are not quarantined: they re-download or rebuild themselves,
  and keeping copies would hold space back for nothing. A scheduled run on a critically
  full disk deletes outright instead, and says so. `rtidy trash [--empty]` shows what is
  recoverable, and for how long.
- **Other languages, as an opt-in extra.** rtidy stays a Rust tool: Cargo is always on
  and remains the only ecosystem with the prevention trick behind it, one shared target
  folder. A single switch in Settings, **Other languages (extra)**, adds 16 more to the
  same scan, preview, undo and daily run: Node (`node_modules`, `.next`, `.nuxt`,
  `.turbo`, `.svelte-kit`, `.parcel-cache`), Python, .NET, Gradle, Maven, Go, CMake,
  Unity, Unreal, Godot, Zig, Elixir, Swift, Haskell, Dart/Flutter and PHP/Composer. Each
  is identified by the manifest beside the build folder, so `target/` is Cargo next to a
  `Cargo.toml` and Maven next to a `pom.xml`, and a folder called `build` with no
  manifest beside it is never touched. Individual languages can be ticked off in
  Settings or with `rtidy langs --on <id>`, `--off <id>`, `--rust-only`; `rtidy langs`
  lists them.
- **Package stores.** npm, pnpm, yarn, pip, uv, Gradle, Maven, NuGet, Go (build and
  module cache), Hex, pub, Composer and Zig are already shared per user, so there is
  nothing to prevent there — what they do is grow forever. rtidy finds them, measures
  them and trims entries untouched for `store_days` (120 days by default) through
  `rtidy stores [--days N] [--dry-run]`. It deliberately never relocates them: those are
  environment variables (`GOCACHE`, `GRADLE_USER_HOME`, `NUGET_PACKAGES`) read by
  shells, CI jobs and IDEs, so `rtidy doctor` prints the variable and leaves the decision
  to the user.
- **Ranked by what it costs to rebuild**, not by size. The plan is ordered by bytes freed
  per second of rebuilding, discounted by the model's reuse chance, so a big folder that
  takes twenty minutes to produce again ranks below a slightly smaller one that takes
  five seconds. Every row in the Projects tab and in the preview shows its estimated
  rebuild time.
- **Watch mode.** `rtidy watch [--once]` polls the shared target folder and trims
  incremental caches, and enforces the size cap, once building has stopped for
  `watch_idle_mins` (10 by default), instead of waiting for the daily run. It never
  removes a project's own build folders; those stay with the daily run and the app.
- **Per-repository rules.** A committed `.rtidy.toml` beside the manifest can say
  `keep = true`, `days = 90`, `trash_days = 7`, `all_langs = true` and
  `except = [".venv"]`. The app's *Keep* checkbox writes it, so the rule travels with the
  repository; the older empty `.rtidy-keep` marker still works and still means
  `keep = true`. The preview lists what was left alone and why.
- **For CI and teams.** `rtidy plan --json` prints the whole plan as JSON, and
  `rtidy budget 50G [folders]` frees the cheapest set of things needed to reach that much
  free space, exiting with code 3 if it cannot get there.
- **Savings history.** Every run records what it freed, per ecosystem. The app's
  **Activity** tab shows the all-time total, the days since the first run, the average
  per day, how many runs there have been and the split per language. Anything restored is
  subtracted again, so the figure stays honest.
- `rtidy gc` uses the model to decide which build units are not coming back when Smart
  mode is on, rather than age alone. Hard limits from Settings still apply.
- **Presets.** Settings holds eight thresholds, each a real trade-off, which is fine for
  someone tuning it and no help to someone who just knows their laptop is full. Five
  ready-made answers — **Careful**, **Balanced** (the shipped default), **Small disk**,
  **Workstation** and **CI runner** — set all of them at once, from Settings, from the
  setup wizard, or with `rtidy preset <name>`; `rtidy preset` lists them and says which
  one the current settings match, and `--dry-run` prints the changes line by line. A
  preset never touches the project folders, the app's language, the clean-up time, or the
  **Other languages** switch and the ecosystems under it — that switch widens what rtidy
  may delete rather than how eagerly, so nothing but the person turns it on. Adjusting
  anything afterwards simply means no preset is selected any more.
- **CI keeps less behind.** A build used to upload its logs and every installer with
  GitHub's default 90-day retention, on every push and every pull request, and each
  branch wrote its own copy of the Rust cache. Retention is now a week for logs and a
  fortnight for installers (a day on pull requests), the installers are deleted as soon
  as a release carries them, pull requests restore the cache without writing a new one,
  a new push to a pull request cancels the run it supersedes, and a closed pull
  request's caches are removed on the spot. `tidy up after the build` can also be run by
  hand to clear artifacts older than a chosen age. The workflow's default permissions
  dropped to read-only, with write granted to the two jobs that need it.
- `RTIDY_HOME` puts everything rtidy remembers — settings, the model, the trash index,
  the savings history and the run log — in one folder of your choosing. The per-user
  config directory is resolved through the OS (the Known Folder API on Windows), so no
  environment variable could redirect it, which meant rtidy could not be exercised by a
  test, in CI, or by anyone trying it out, without writing over the real settings of
  whoever ran it.

### Fixed (app)

- A mistyped time such as `25:99` could be saved, and the schedule then failed
  silently. The picker only produces valid times.
- Saving Settings no longer freezes the window while the task is re-registered, and
  a failure to update the schedule is shown instead of being ignored.

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
