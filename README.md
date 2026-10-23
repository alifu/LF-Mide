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
- `doctor` / `clean` commands to cap JVM memory and free disk space (DerivedData, unused simulators, Gradle caches)

Scope for now: **emulators and simulators only** (no physical devices).

## Roadmap

| Milestone | Contents |
|---|---|
| M0 | Spike: build & run the CMP app with no IDE, check autocomplete |
| M1 | Cockpit MVP: navigation, SDK info, target/device pickers, build, run, logs |
| M2 | Git: gutter, diff view, blame toggle |
| M3 | Memory & disk tuning (`doctor`, `clean`) |
| M4 | Single-window IDE with embedded Neovim |

Until M4, LF-Mide runs **next to Neovim** in a split terminal pane.

## Requirements

| Tool | Purpose |
|---|---|
| Rust (stable, edition 2024) | Build LF-Mide |
| [Ghostty](https://ghostty.org) | Recommended terminal (native macOS, built-in splits, good keyboard support) |
| Neovim ≥ 0.11 | Editor |
| JDK 17 or 21 | Gradle, Kotlin language server |
| Android command-line tools (`platform-tools`, `emulator`, a system image) | Android builds, no Android Studio needed |
| Xcode (CLI tools + one iOS simulator runtime) | iOS SDK and simulators. Xcode stays installed, but you don't need to open it |
| [kotlin-lsp](https://github.com/Kotlin/kotlin-lsp) | Kotlin / Compose completion |
| [xcode-build-server](https://github.com/SolaWing/xcode-build-server) | Swift / ObjC completion via `sourcekit-lsp` |
| [ripgrep](https://github.com/BurntSushi/ripgrep) | Project search |

## Expected CMP project layout

```
<project>/
├── composeApp/        # shared Kotlin + Android app
├── iosApp/            # Xcode project
└── settings.gradle.kts
```

## Build & run

```bash
cargo build --release
cargo run -p lf-mide
```

## Development

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Workspace layout:

```
crates/
└── app/    # lf-mide binary
```

More crates (`core`, `platform`, `vcs`, `editor`, `ui`) are added as the roadmap progresses.
