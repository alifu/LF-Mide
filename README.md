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

Keys so far: `Tab` / `Shift-Tab` move focus, `e` opens Neovim in a split pane, `T` picks the terminal, `D` opens the
doctor panel, `q` or `Ctrl-C` quits.
In the doctor panel: `j`/`k` or arrows scroll, `PageUp`/`PageDown` page, `r` re-runs the checks, `Esc` or `D` closes.

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
├── doctor/           # dependency checks behind `lf-mide doctor`
├── system/           # running commands / reading the machine behind a trait (shared, testable)
├── terminal/         # terminal detection and split-pane launching
├── ui/               # ratatui rendering (pure view of the state)
└── vcs/              # git: branch, dirty state
spikes/
└── nvim-embed/       # Phase 0: Neovim embedding proof (standalone crate)
```

More crates (`platform`, `editor`) are added as the roadmap progresses.

