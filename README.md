# LF-Mide

A lightweight, keyboard-driven **terminal IDE** for **Compose Multiplatform (CMP)** projects that target Android and iOS, written in Rust.

The goal is to work on a CMP app without keeping Android Studio and Xcode open, saving RAM and disk on low-storage MacBooks.

> **Status:** early development: usable day to day, still changing.

## Planned features

- Vim editing via an embedded [Neovim](https://neovim.io)
- Autocomplete for Kotlin, Compose, Swift, SwiftUI and Objective-C through language servers
- Pick the build target (Android / iOS)
- Pick an Android emulator or iOS simulator and run the app on it
- Show the active Android SDK / `adb` and Xcode / iOS SDK
- Live build status and logs (Gradle, `xcodebuild`, `logcat`, simulator log stream)
- Git integration: active branch, changed lines (new vs old code), blame you can turn on and off
- Keyboard navigation: file tree, fuzzy file finder, project search
- `doctor` command: checks every dependency (Android Studio, Android SDK, `adb`, emulator, JDK, Xcode, simulator runtimes, Neovim, language servers, terminal) and suggests fixes
- Performance status: RAM, swap, memory pressure, and per-process usage of Gradle, Kotlin daemon, language servers, emulator and simulator
- Selectable terminal (iTerm2, Ghostty, WezTerm, Kitty, Terminal.app, tmux)
- `clean` command to cap JVM memory and free disk space (DerivedData, unused simulators, Gradle caches)

Scope for now: **emulators and simulators only** (no physical devices).

## Roadmap

| Milestone | Contents |
|---|---|
| M0 | Spike: build & run the CMP app with no IDE, check autocomplete |
| M1 | Cockpit MVP: doctor, terminal selection, navigation, SDK info, target/device pickers, build, run, logs, performance status |
| M2 | Git: gutter, diff view, blame toggle |
| M3 | Memory & disk tuning (`clean`, JVM heap defaults) |
| M4 | Single-window IDE with embedded Neovim |

Neovim runs **inside LF-Mide's editor panel** (M4). The earlier mode, Neovim in a split pane next to LF-Mide, is
still there with `editor = "split"`.

## Requirements

| Tool | Purpose |
|---|---|
| Rust (stable, edition 2024) | Build LF-Mide |
| A terminal: [iTerm2](https://iterm2.com) (primary), Ghostty, WezTerm, Kitty, Terminal.app, or tmux | Hosts LF-Mide and the Neovim split. Selectable in config |
| Neovim ≥ 0.11 | Editor (its built-in LSP client and completion are used; no plugins needed) |
| JDK 17 or 21 | Gradle, Kotlin language server |
| Android command-line tools (`platform-tools`, `emulator`, a system image) | Android builds, no Android Studio needed |
| Xcode (CLI tools + one iOS simulator runtime) | iOS SDK and simulators. Xcode stays installed, but you don't need to open it |
| [kmp-lsp](https://crates.io/crates/kmp-lsp) | Optional. Kotlin completion: `cargo install --locked kmp-lsp` |
| `sourcekit-lsp` (comes with Xcode) + [xcode-build-server](https://github.com/SolaWing/xcode-build-server) | Swift / ObjC completion |
| [ripgrep](https://github.com/BurntSushi/ripgrep) | Project search |

## Expected CMP project layout

Both CMP templates are supported:

```
<project>/                         <project>/
├── composeApp/  # shared + Android  ├── androidApp/  # Android app
├── iosApp/      # Xcode project     ├── shared/      # shared Kotlin
└── settings.gradle.kts              ├── iosApp/      # Xcode project
                                     └── settings.gradle.kts
```

## Build & run

```bash
cargo build --release
./target/release/lf-mide /path/to/your/cmp-project   # or run it inside the project
```

Keys so far:

| Key | Action |
|---|---|
| `Tab` / `Shift-Tab` | move focus between panels |
| `e` | start Neovim in the editor panel, or focus it (with `editor = "split"`: open it in a split pane) |
| `f` | fuzzy-find a file and open it (`Ctrl-n`/`Ctrl-p` or arrows move, `Enter` opens, `Esc` cancels) |
| `/` | search the project with ripgrep (`Enter` searches, then `Enter` opens the highlighted hit at its line) |
| `t` | switch the build target between Android and iOS |
| `d` | focus the Devices panel |
| `i` | show the environment (SDK and Xcode locations and versions) |
| `B` | build the app and run it on the chosen running device |
| `s` | cancel the build |
| `L` | follow (or stop following) the chosen device's log |
| `o` | focus the Output panel |
| `p` | open the Perf panel (memory and the dev processes using it) |
| `P` | show or hide the RAM / swap indicator in the status bar |
| `C` | show the Git tab (files that differ from HEAD) |
| `=` | diff of the file open in Neovim, against HEAD |
| `A` | show or hide blame at the end of each line in Neovim |
| `T` | pick the terminal |
| `D` | doctor panel |
| `R` | rescan the project files |
| `q` / `Ctrl-C` | quit (inside `f` and `/` every key is text, so `q` is just a letter; in the editor every key is Neovim's) |

In the file tree (focused by default): `j`/`k` or arrows move, `l`/`Right` expands, `h`/`Left` collapses or steps out,
`Enter` opens a file or toggles a directory, `g`/`G` jump to top/bottom, `PageUp`/`PageDown` page.
In the doctor panel: `j`/`k` or arrows scroll, `PageUp`/`PageDown` page, `r` re-runs the checks, `Esc` or `D` closes.

In the Devices panel: `j`/`k` or arrows choose a device, `b` boots it, `x` shuts it down, `r` refreshes the list.

## Devices and targets

The header shows the chosen target and device. The Devices panel lists the Android emulators (AVDs) or iOS simulators
for the current target, with a marker for stopped, starting, running and stopping. Lists refresh every 4 seconds, so an
emulator you start elsewhere shows up too.

- Android: `emulator -list-avds` and `adb devices -l`. `b` starts the AVD, `x` shuts it down through `adb emu kill`.
- iOS: `xcrun simctl list devices available --json`. `b` boots the simulator and opens the Simulator app, `x` shuts it down.
- Tools are taken from `ANDROID_HOME` / `ANDROID_SDK_ROOT` / the default SDK location and then `PATH`. A path found in
  the project's `local.properties` is never used to run anything.
- The chosen target and devices are remembered per project in a private file under the LF-Mide config folder, never
  inside your project.

## Build, run and logs

`B` builds the app and runs it on the chosen device, which must already be running (start it with `b` in the Devices
panel). Output streams into the Output panel while the build runs, and the status bar shows the current step and Gradle
task, then how it ended.

- Android: `./gradlew :androidApp:installDebug` (or `:composeApp:`) for the chosen emulator, then the app is started
  with its `applicationId`.
- iOS: `xcodebuild` for the chosen simulator, with build products in LF-Mide's own cache folder (not in your project),
  then `simctl install` and `simctl launch`.
- When the app is running, its log is followed automatically: `adb logcat` on Android, the simulator's log limited to
  the app's process on iOS.
- Compiler errors and warnings are collected in the **Problems** tab. A failed build with errors opens it for you.
  `Enter` on a problem shows that file at that line in Neovim.
- The build runs your project's own `gradlew` and Xcode project, so it only starts when you press `B`.
- Quitting (or closing the terminal window) stops the build and the log streams, force-killing any that ignore the request.

In the Output panel (`o`): `h`/`l` or `[`/`]` change tab, `1`-`5` jump to a tab, `j`/`k` or arrows scroll, `g` goes to
the oldest line, `G` follows new output again, `c` clears the tab, `F` filters the log by text (`Enter` keeps it, `Esc`
drops it), `v` shows only louder Logcat lines (Debug, Info, Warn, Error, then all again).
Each log keeps the newest 20,000 lines.

## Git: new code, old code and blame

LF-Mide shows what changed in the project without leaving the terminal, and draws it inside your Neovim.

- **Gutter:** in the file Neovim is showing, lines that are new get a green bar, lines that replace others an amber bar,
  and a red mark shows where lines were taken away. It is drawn from the file on disk, so it is current after a save,
  and LF-Mide notices a changed file within a few seconds (and follows you when you switch files in Neovim).
- **Blame (`A`):** at the end of each line, `author · 3 days ago · a1b2c3d`. Lines you changed since the last commit read
  "Not committed yet". Off by default; set `show_blame = true` in the config to start with it on.
- **Git tab (`C`):** every file that differs from HEAD, with M (modified), A (added), D (deleted), R (renamed),
  ? (untracked) and U (conflict); a `+` in front means staged. `j`/`k` choose, `Enter` opens the diff, `o` opens the file
  in Neovim, `r` re-reads the repository.
- **Diff (`Enter` in the Git tab, or `=` for the current file):** a full-screen view of the new text against the old.
  `s` switches between unified and side by side, `j`/`k`/`PageUp`/`PageDown`/`g`/`G` scroll, `n`/`N` jump to the next or
  previous change, `Enter` or `o` opens the file in Neovim at that change, `Esc` closes. `b` chooses what to compare
  against: HEAD, a branch, or one of the last 15 commits.

LF-Mide needs to know which Neovim to talk to: open one with `e`, or start your own with `nvim --listen /tmp/nvim.sock`
and run `lf-mide --nvim-socket /tmp/nvim.sock`. Commit messages, author names and file contents come from the
repository, so control characters are removed before they reach your screen or Neovim.

## Freeing disk and memory: `clean` and `tune`

Low on disk? `lf-mide clean` shows what is taking space and what could go, and **changes nothing** unless you add
`--delete`:

```bash
lf-mide clean                       # look only (run it inside your project to include its build folders)
lf-mide clean --delete              # clean the rebuildable ones, asking before each group
lf-mide clean --delete --include device-support,ide-caches   # also these careful ones
lf-mide clean --delete --yes --only derived-data             # no questions, just this group
```

- **Rebuildable (cleaned by `--delete`):** Xcode's DerivedData and LF-Mide's own iOS build products; the project's
  `build/`, `.gradle/` and `.kotlin/` folders (only ones git already ignores and holds no tracked file in); old Gradle
  versions (the newest two and your project's are kept); Gradle daemon logs older than a week; simulators whose runtime
  is gone (done by `xcrun simctl delete unavailable`).
- **Careful (only with `--include NAME`):** emulator quick-boot snapshots (the next start is a full boot), symbols from
  physical iOS devices, simulator caches, Android Studio's caches, and running Gradle/Kotlin daemons (frees memory; cuts
  a build in progress short). A category is blocked while the emulator or Android Studio it affects is running.
- **For you to review:** the biggest items on most iOS developers' disks: simulator runtimes (those no simulator
  uses are marked), simulators by size, Android virtual devices and system images. LF-Mide prints the exact command
  (`xcrun simctl runtime delete …`, `avdmanager delete avd -n …`) and never runs it.

Safety: every deletion goes through one checked path. A symbolic link is never followed or removed, a path must be
strictly inside the folder it belongs to, and only your home folder and the project are allowed at all. Without a
terminal (or `--yes`) it refuses to delete. Names are checked, so a misspelt category is an error rather than a no-op.

`lf-mide tune` suggests Gradle memory settings you do not have yet: a heap ceiling for the Gradle and Kotlin daemons,
stopping an idle daemon after 30 minutes instead of 3 hours, and the build cache (`--with-configuration-cache` adds that
too). It looks at both your `~/.gradle/gradle.properties` and the project's, and `--apply` adds only the missing
lines to **your** file (a backup is kept; the repository is never touched).

When you start a second emulator or simulator of the same kind, the status line says which one is already running.

## Performance status

The status bar shows `RAM used/total · swap X` (with ▲/▼ when swap is growing or shrinking). It is green normally,
yellow when macOS reports memory pressure "warn" or swap is at least 2 GB, and red on "critical" pressure or 4 GB of
swap. When swap first gets high, LF-Mide suggests the cheapest thing to give back, usually the Gradle daemon.

`p` opens the Perf panel: RAM, pressure, swap and free disk, then the processes that matter, biggest first: Gradle and
Kotlin daemons, `kotlin-lsp`, the Android emulator (named by its AVD), iOS simulators (named by device, all their
processes added up), Neovim, LF-Mide itself, and Android Studio / Xcode if you left them open. Memory is each
process's footprint (what Activity Monitor shows), not resident size, which counts shared pages many times over.

In the panel `j`/`k` move (the highlight stays on its process while the list re-sorts), `x` stops the highlighted
helper: press it twice to confirm. Daemons and the language server get SIGTERM after LF-Mide re-checks the process
is still the one you saw; an emulator or simulator is shut down the normal way. Editors, IDEs and LF-Mide itself are
never offered for stopping.

Samples are taken every 2 seconds, and not at all while both the indicator and the panel are hidden. Settings
(`~/.config/lf-mide/config.toml` or `.lfmide.toml`):

```toml
show_perf = true           # the status-bar indicator
perf_interval_secs = 2     # 1 to 60
swap_warn_mb = 2048        # indicator turns yellow
swap_critical_mb = 4096    # and red
```

## Navigating the project

The file tree and the finder list every regular file under the project root, honoring `.gitignore` (even without a
`.git` folder) and skipping `.git`, `.gradle`, `.kotlin`, `.idea`, `build`, `DerivedData`, `node_modules` and `Pods`.
Symbolic links are never followed or listed. Up to 200,000 files are listed. The finder re-ranks as you type (about
5 ms for 10,000 files, 25 ms at the 200,000 cap); opening or closing a folder in a 200,000-file project takes about
130 ms. Search needs [ripgrep](https://github.com/BurntSushi/ripgrep) and
ignores your own ripgrep config, so results are the same for everyone.

Choosing a file sends it to the Neovim that `e` opened, over its RPC socket. If no Neovim is running yet, LF-Mide
starts one (in the editor panel, or in a split pane with `editor = "split"`) and opens the file (at the matching line for a search hit) as soon as it is up; a second
choice while it is starting just replaces the file waiting to open. If Neovim cannot be started, the status line says
why. The file name is passed as raw bytes and
used only as data (never as part of a Neovim command), so names like `100%.txt` or `a b#c.kt` open correctly.

File and folder names are shown as they are, except that characters which could make a name read differently from
what it is (right-to-left overrides, zero-width marks, control characters) appear as a visible `�`. Quitting LF-Mide
also stops a search or tool command that is still running, instead of leaving it behind. A project folder (or file
names inside it) that is not valid text, as can happen on network or foreign disks, is searched and built using
relative paths, so it is not mangled on the way to ripgrep, Gradle or Xcode. The file tree keeps an ordered index of the
project, so opening or closing a folder takes microseconds even with 200,000 files.

## The editor

`e`, or choosing a file in the tree, the finder or a search, starts Neovim (`nvim --embed`) in the editor panel, in your
project folder, with your own Neovim config. While the editor has focus **every key goes to Neovim**, `Ctrl-C`, `q`
and `Tab` included, except the leader (`C-Space` by default, `leader = "..."` in the config), which hands focus back to
the panels: then `f`, `/`, `B` and the rest work as usual, and `Tab` or `e` returns to the editor. Pastes arrive as one
change (bracketed paste). Neovim's grid follows the panel when the terminal is resized, and the cursor takes Neovim's
shape (block, bar in insert mode, underline in replace mode).

Quitting with `q` closes Neovim first, with Neovim's own question if something is unsaved ("Save changes? Yes, No,
Cancel"); Cancel keeps both running; if Neovim cannot quit (a save failed), the status line says why
and `q` asks again. While the question is open the leader stays with Neovim. `:q` in Neovim closes only Neovim; `e` starts a new one. If the terminal itself
goes away (window closed, `SIGHUP`/`SIGTERM`), there is nobody left to answer, so Neovim is stopped at once and its swap
file keeps the unsaved changes for recovery next time.

Neovim also listens on a socket in LF-Mide's private folder, so opening files, git signs and blame work exactly as in
split mode.

**Completion.** Unless `lsp = false`, LF-Mide sets up Neovim's built-in LSP client for the servers it finds on your
`PATH` and turns on completion as you type (nothing is inserted until you choose an entry):

| Language | Server | Notes |
|---|---|---|
| Kotlin, Java | `kmp-lsp` | Fast (about 80 MB once its index is built; the first index of a project takes ~15 s and ~700 MB). It reads the code's structure without compiling it: members of your own classes, view models, data classes and constructor parameters complete; the Compose DSL (`Modifier.fillMaxWidth`, `LazyColumn`), chains (`Modifier.padding().`) and inferred types (`listOf(1).first`) mostly do not. Errors come from the Gradle build (Problems tab) |
| Swift, Objective-C | `sourcekit-lsp` | Needs `buildServer.json` in the project (see the doctor) |

The official JetBrains `kotlin-lsp` gives no completion for Kotlin Multiplatform projects yet, so it is not used.
If you set up language servers in your own Neovim config, put `lsp = false` in LF-Mide's config to avoid running two.

Not supported in the editor panel yet: the mouse, and widening the editor to the full window.

## The split pane (`editor = "split"`)

`e` starts `nvim --listen <socket>` in a new pane next to LF-Mide, in your project folder, using the terminal you
chose (`T`, or `terminal = "..."` in the config; `auto` detects it from the environment).

| Terminal | What `e` does |
|---|---|
| iTerm2 | splits the current session to the right (AppleScript; macOS asks once to allow Automation) |
| tmux | `split-window -h` (LF-Mide must run inside tmux; tmux wins over the hosting terminal) |
| WezTerm | `wezterm cli split-pane --right` |
| kitty | `kitty @ launch --location=vsplit` (needs `allow_remote_control`; built from the docs, **not tested**) |
| Terminal.app | opens a new window (no splits) |
| Ghostty | cannot be driven from outside: the launcher path is copied to the clipboard, you split a pane (Cmd+D) and paste it |

Nothing taken from the project folder name is ever put on a command line that a shell, a terminal or AppleScript
has to interpret (a cloned repository controls that name). Instead LF-Mide writes `open-nvim.sh` (mode 0700) and the
Neovim socket into a private folder (`lf-mide-<pid>/`, mode 0700) under your temp directory, and the terminal is only
told to run that script's path.

## Doctor

`lf-mide doctor` checks everything LF-Mide and your CMP project need and prints a fix for each problem:

```bash
lf-mide doctor                      # check from the current directory
lf-mide doctor /path/to/project     # check a specific project
lf-mide doctor --json               # machine-readable
```

It covers the Android SDK (`adb`, emulator, `sdkmanager`, build-tools, platforms, system images, AVDs), JDK,
Gradle wrapper, Xcode (selected developer dir, license, first launch), iOS simulator runtimes, Neovim and language
servers, the terminal, `gradle.properties` heap sizes against your RAM, the `buildServer.json` used for Swift
completion, and free disk space. Android Studio is reported as optional. The command exits with `1` if something
required is missing (warnings do not fail it) and never installs or changes anything.

Tools are located through your `PATH`, `ANDROID_HOME` / `ANDROID_SDK_ROOT`, `JAVA_HOME` and the default
`~/Library/Android/sdk`. A project's `local.properties` is repo-controlled, so the doctor never runs tools from
its `sdk.dir`; if no SDK is found it only suggests that directory so you can export `ANDROID_HOME` yourself.

## Configuration

Global: `~/.config/lf-mide/config.toml` (or `$XDG_CONFIG_HOME/lf-mide/config.toml`).
Per project: `.lfmide.toml` in the project root, which overrides the global file.

```toml
leader = "C-Space"     # leaves the editor for the panels (Neovim notation: "C-Space", "C-\\", "F12")
editor = "embedded"    # embedded (in the editor panel) | split (a pane next to LF-Mide)
lsp = true             # set up kmp-lsp / sourcekit-lsp completion in the embedded Neovim
terminal = "iterm2"    # auto | iterm2 | ghostty | wezterm | kitty | terminal | tmux (for split mode)
show_blame = false
show_perf = true
```

Logs: `~/Library/Caches/lf-mide/lf-mide.<date>.log` (7 days kept). Set `LF_MIDE_LOG=debug` for more detail.

## Development

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Workspace layout:

```
crates/
├── app/              # lf-mide binary: CLI, event loop, keymap, background tasks
├── core/             # state, actions, pure reducer, config, project detection, doctor result types
├── build/            # build, install, launch and log pipelines for Android and iOS
├── doctor/           # dependency checks behind `lf-mide doctor`
├── tidy/             # `lf-mide clean` and `lf-mide tune`: disk and memory housekeeping
├── editor/           # Neovim: embedded session, redraw events, key notation, LSP setup, RPC socket calls
├── nav/              # project scan, fuzzy finder, ripgrep search
├── perf/             # memory, swap, disk and dev-process sampling (macOS)
├── platform/         # Android SDK / emulators and Xcode / simulators: list, boot, stop
├── system/           # running commands / reading the machine behind a trait (shared, testable)
├── terminal/         # terminal detection and split-pane launching
├── ui/               # ratatui rendering (pure view of the state)
└── vcs/              # git: branch, changed files, diff, blame
spikes/
└── nvim-embed/       # Phase 0: Neovim embedding proof (standalone crate)
```

More crates are added as the roadmap progresses.

