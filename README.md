# LF-Mide

A lightweight, keyboard-driven **terminal IDE** for **Compose Multiplatform (CMP)** projects that target Android and iOS, written in Rust.

The goal is to work on a CMP app without keeping Android Studio and Xcode open, saving RAM and disk on low-storage MacBooks.

> **Status:** early development (project scaffold only).

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

Until M4, LF-Mide runs **next to Neovim** in a split terminal pane.

## Requirements

| Tool | Purpose |
|---|---|
| Rust (stable, edition 2024) | Build LF-Mide |
| A terminal: [iTerm2](https://iterm2.com) (primary), Ghostty, WezTerm, Kitty, Terminal.app, or tmux | Hosts LF-Mide and the Neovim split. Selectable in config |
| Neovim ≥ 0.11 | Editor |
| JDK 17 or 21 | Gradle, Kotlin language server |
| Android command-line tools (`platform-tools`, `emulator`, a system image) | Android builds, no Android Studio needed |
| Xcode (CLI tools + one iOS simulator runtime) | iOS SDK and simulators. Xcode stays installed, but you don't need to open it |
| [kotlin-lsp](https://github.com/Kotlin/kotlin-lsp) | Optional. Kotlin completion for plain JVM Gradle projects (it does not support Kotlin Multiplatform yet) |
| [xcode-build-server](https://github.com/SolaWing/xcode-build-server) | Swift / ObjC completion via `sourcekit-lsp` |
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
| `e` | open Neovim in a split pane (do this first) |
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
| `T` | pick the terminal |
| `D` | doctor panel |
| `R` | rescan the project files |
| `q` / `Ctrl-C` | quit (inside `f` and `/` every key is text, so `q` is just a letter) |

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

Choosing a file sends it to the Neovim that `e` opened, over its RPC socket. The file name is passed as raw bytes and
used only as data (never as part of a Neovim command), so names like `100%.txt` or `a b#c.kt` open correctly.

## Terminal and the editor split

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
leader = "C-Space"     # prefix for IDE commands
terminal = "iterm2"    # auto | iterm2 | ghostty | wezterm | kitty | terminal | tmux
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
├── editor/           # shows a file in the running Neovim over its RPC socket
├── nav/              # project scan, fuzzy finder, ripgrep search
├── perf/             # memory, swap, disk and dev-process sampling (macOS)
├── platform/         # Android SDK / emulators and Xcode / simulators: list, boot, stop
├── system/           # running commands / reading the machine behind a trait (shared, testable)
├── terminal/         # terminal detection and split-pane launching
├── ui/               # ratatui rendering (pure view of the state)
└── vcs/              # git: branch, dirty state
spikes/
└── nvim-embed/       # Phase 0: Neovim embedding proof (standalone crate)
```

More crates are added as the roadmap progresses.

