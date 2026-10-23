//! Opening a command (Neovim) in a split pane of the user's terminal.
//!
//! Nothing taken from the repository (the project folder name) is ever put
//! on a command line that a shell, a terminal's input, or AppleScript has to
//! interpret. Instead the app writes a small launcher script (see
//! [`launcher_script`]) that `/bin/sh` reads from a file, and the terminal is
//! only told the path of that script.
//!
//! Planning is pure: [`plan_split`] turns a request into the exact commands
//! to run, so every terminal's behavior is a plain test. [`open_split`] runs
//! them.

use std::path::{Path, PathBuf};

use lf_mide_core::TerminalKind;
use lf_mide_system::{RunError, System};
use thiserror::Error;

use crate::quote::{applescript_quote, shell_quote, shell_word};

/// Longest error text from a tool we show; osascript can echo the script.
const MAX_DETAIL_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// New pane to the right of the current one.
    Right,
    /// New pane below the current one.
    Down,
}

/// What to run in the new pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitSpec {
    pub cwd: PathBuf,
    pub program: String,
    pub args: Vec<String>,
    pub direction: SplitDirection,
}

impl SplitSpec {
    /// `nvim --listen <socket>` in `cwd`, beside the cockpit. Paths that are
    /// not valid UTF-8 are refused: lossy conversion could point at a
    /// different folder.
    pub fn neovim(cwd: &Path, socket: &Path) -> Result<Self, LaunchError> {
        cwd.to_str().ok_or(LaunchError::NonUtf8Path)?;
        let socket = socket.to_str().ok_or(LaunchError::NonUtf8Path)?;
        Ok(Self {
            cwd: cwd.to_path_buf(),
            program: "nvim".to_string(),
            args: vec!["--listen".to_string(), socket.to_string()],
            direction: SplitDirection::Right,
        })
    }
}

/// One process to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    Run(Vec<Invocation>),
    /// The terminal can't be driven from outside: show the user what to do.
    Manual {
        note: String,
        command: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launched {
    Opened,
    Manual { note: String, command: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LaunchError {
    #[error("could not tell which terminal this is; press T to choose one")]
    Undetected,
    #[error("tmux is selected but LF-Mide is not running inside a tmux session")]
    NotInsideTmux,
    #[error("`{0}` was not found; install it or choose another terminal with T")]
    ToolMissing(String),
    #[error(
        "macOS blocked LF-Mide from controlling {0}: allow it in System Settings > Privacy & Security > Automation"
    )]
    AutomationDenied(String),
    #[error("`{0}` timed out (macOS may be waiting for you to allow access)")]
    TimedOut(String),
    #[error("`{program}` failed: {detail}")]
    Failed { program: String, detail: String },
    #[error("a path is not valid UTF-8, so it can't be passed to the terminal safely")]
    NonUtf8Path,
}

/// Contents of the launcher script: `/bin/sh` code that enters the project
/// folder and replaces itself with the program. Every value is single-quoted
/// for `sh`, and the file is read by `sh` itself, never typed into a shell.
pub fn launcher_script(spec: &SplitSpec) -> Result<String, LaunchError> {
    let cwd = spec.cwd.to_str().ok_or(LaunchError::NonUtf8Path)?;
    let program = shell_quote(&spec.program);
    let command: Vec<String> = std::iter::once(&spec.program)
        .chain(&spec.args)
        .map(|word| shell_quote(word))
        .collect();
    let missing = shell_quote(&format!(
        "LF-Mide: {} not found in your PATH. Press Enter to close.",
        spec.program
    ));
    // Without the check the pane would close the instant `exec` fails.
    Ok(format!(
        "#!/bin/sh\n\
         cd {cwd} || exit 1\n\
         command -v {program} >/dev/null 2>&1 || {{\n  echo {missing}\n  read -r _\n  exit 127\n}}\n\
         exec {command}\n",
        cwd = shell_quote(cwd),
        command = command.join(" "),
    ))
}

/// The commands that open `spec` in a split of `kind`. `launcher` is the
/// path of the script from [`launcher_script`], which shell-based terminals
/// are told to run.
pub fn plan_split(
    kind: TerminalKind,
    spec: &SplitSpec,
    launcher: &Path,
    sys: &dyn System,
) -> Result<Plan, LaunchError> {
    let cwd = spec.cwd.to_str().ok_or(LaunchError::NonUtf8Path)?;
    // The only thing a shell or AppleScript is ever asked to read: the path
    // of our own launcher script (it holds no repository-controlled text).
    let command = shell_word(launcher.to_str().ok_or(LaunchError::NonUtf8Path)?);
    let side_by_side = spec.direction == SplitDirection::Right;
    let run = |program: &str, args: Vec<String>| {
        Ok(Plan::Run(vec![Invocation {
            program: program.to_string(),
            args,
        }]))
    };
    // `osascript -e <line> -e <line> ...`
    let applescript = |lines: Vec<String>| {
        lines
            .into_iter()
            .flat_map(|line| ["-e".to_string(), line])
            .collect::<Vec<_>>()
    };
    // The program and its arguments as separate words, for terminals whose
    // CLI takes a command line instead of a shell string.
    let argv = || std::iter::once(spec.program.clone()).chain(spec.args.clone());

    match kind {
        TerminalKind::Auto => Err(LaunchError::Undetected),
        TerminalKind::Ghostty => Ok(Plan::Manual {
            note: "Ghostty can't be controlled by other programs.".to_string(),
            command,
        }),
        TerminalKind::Tmux => {
            if sys.env("TMUX").is_none_or(|value| value.is_empty()) {
                return Err(LaunchError::NotInsideTmux);
            }
            let flag = if side_by_side { "-h" } else { "-v" };
            run(
                "tmux",
                vec!["split-window".to_string(), flag.to_string(), command],
            )
        }
        // iTerm2 calls a split with a vertical divider (panes side by side)
        // "vertically".
        TerminalKind::Iterm2 => {
            let verb = if side_by_side {
                "split vertically"
            } else {
                "split horizontally"
            };
            run(
                "osascript",
                applescript(vec![
                    "tell application \"iTerm\"".to_string(),
                    "tell current session of current window".to_string(),
                    format!(
                        "{verb} with default profile command {}",
                        applescript_quote(&command)
                    ),
                    "end tell".to_string(),
                    "end tell".to_string(),
                ]),
            )
        }
        // Terminal.app has no splits: use a new window.
        TerminalKind::Terminal => run(
            "osascript",
            applescript(vec![
                "tell application \"Terminal\"".to_string(),
                "activate".to_string(),
                format!("do script {}", applescript_quote(&command)),
                "end tell".to_string(),
            ]),
        ),
        TerminalKind::Wezterm => {
            let flag = if side_by_side { "--right" } else { "--bottom" };
            let head = ["cli", "split-pane", flag, "--cwd", cwd, "--"].map(String::from);
            run("wezterm", head.into_iter().chain(argv()).collect())
        }
        // Built from kitty's documentation; not verified on a real kitty.
        TerminalKind::Kitty => {
            let location = if side_by_side {
                "--location=vsplit"
            } else {
                "--location=hsplit"
            };
            let head = [
                "@".to_string(),
                "launch".to_string(),
                location.to_string(),
                format!("--cwd={cwd}"),
                "--".to_string(),
            ];
            run("kitty", head.into_iter().chain(argv()).collect())
        }
    }
}

/// The `osascript` call that puts `text` on the clipboard. Plain
/// StandardAdditions, so it needs no Automation permission.
pub fn clipboard_invocation(text: &str) -> Invocation {
    Invocation {
        program: "osascript".to_string(),
        args: vec![
            "-e".to_string(),
            format!("set the clipboard to {}", applescript_quote(text)),
        ],
    }
}

/// Copies `text` to the clipboard.
pub fn copy_to_clipboard(sys: &dyn System, text: &str) -> Result<(), LaunchError> {
    run_invocation(sys, &clipboard_invocation(text), "macOS")
}

/// Plans and runs the split.
pub fn open_split(
    sys: &dyn System,
    kind: TerminalKind,
    spec: &SplitSpec,
    launcher: &Path,
) -> Result<Launched, LaunchError> {
    let invocations = match plan_split(kind, spec, launcher, sys)? {
        Plan::Manual { note, command } => return Ok(Launched::Manual { note, command }),
        Plan::Run(invocations) => invocations,
    };
    for invocation in &invocations {
        run_invocation(sys, invocation, kind.label())?;
    }
    Ok(Launched::Opened)
}

/// Runs one planned command and turns every way it can fail into a message
/// for the user. `app` names what osascript was talking to.
fn run_invocation(sys: &dyn System, invocation: &Invocation, app: &str) -> Result<(), LaunchError> {
    let program = invocation.program.clone();
    let args: Vec<&str> = invocation.args.iter().map(String::as_str).collect();
    match sys.run(&program, &args) {
        Ok(output) if output.success => Ok(()),
        Ok(output) => {
            let detail = output.stderr.trim();
            Err(if is_automation_denied(detail) {
                LaunchError::AutomationDenied(app.to_string())
            } else {
                let detail = if detail.is_empty() {
                    "exited with an error".to_string()
                } else {
                    shorten(detail)
                };
                LaunchError::Failed { program, detail }
            })
        }
        Err(RunError::NotFound) => Err(LaunchError::ToolMissing(program)),
        Err(RunError::TimedOut) => Err(LaunchError::TimedOut(program)),
        Err(RunError::Other(detail)) => Err(LaunchError::Failed { program, detail }),
    }
}

/// `text` cut to [`MAX_DETAIL_CHARS`] characters.
fn shorten(text: &str) -> String {
    if text.chars().count() <= MAX_DETAIL_CHARS {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(MAX_DETAIL_CHARS).collect();
    cut.push('…');
    cut
}

/// osascript reports a blocked Apple event as error -1743.
fn is_automation_denied(stderr: &str) -> bool {
    stderr.contains("-1743") || stderr.contains("Not authorized")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_system::fake::FakeSystem;
    use std::process::Command;

    const LAUNCHER: &str = "/var/folders/zz/T/lf-mide-42/open-nvim.sh";

    fn spec(direction: SplitDirection) -> SplitSpec {
        SplitSpec {
            cwd: PathBuf::from("/work/pos"),
            program: "nvim".into(),
            args: vec!["--listen".into(), "/tmp/lf/nvim.sock".into()],
            direction,
        }
    }

    fn plan(kind: TerminalKind, direction: SplitDirection) -> Plan {
        let sys = FakeSystem::new().env("TMUX", "x");
        plan_split(kind, &spec(direction), Path::new(LAUNCHER), &sys).unwrap()
    }

    fn only_invocation(plan: Plan) -> Invocation {
        let Plan::Run(mut runs) = plan else {
            panic!("expected Run, got {plan:?}")
        };
        assert_eq!(runs.len(), 1);
        runs.remove(0)
    }

    fn command_key(inv: &Invocation) -> String {
        format!("{} {}", inv.program, inv.args.join(" "))
    }

    // --- the launcher script ------------------------------------------

    /// Folder names a hostile repository could have, including ones with
    /// control characters, quotes, backslashes and newlines.
    const HOSTILE_DIRS: &[&str] = &[
        "plain",
        "with space",
        "it's",
        "say \"hi\"",
        "back\\slash",
        "x\\'; open -a Calculator; #",
        "new\nline",
        "ctrl\u{15}u and\rreturn",
        "esc\u{1b}[201~paste",
        "$(touch pwned) `touch pwned2`",
        "-dash",
        "é✓日本",
    ];

    fn run_script(script: &str) -> std::process::Output {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("launch.sh");
        std::fs::write(&path, script).unwrap();
        Command::new("sh")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    #[test]
    fn launcher_script_runs_the_program_in_a_folder_with_any_name() {
        for name in HOSTILE_DIRS {
            let root = tempfile::TempDir::new().unwrap();
            let dir = root.path().join(name);
            std::fs::create_dir(&dir).unwrap();
            let spec = SplitSpec {
                cwd: dir.clone(),
                program: "pwd".into(),
                args: vec![],
                ..spec(SplitDirection::Right)
            };

            let out = run_script(&launcher_script(&spec).unwrap());

            let printed = String::from_utf8_lossy(&out.stdout)
                .trim_end_matches('\n')
                .to_string();
            let expected = std::fs::canonicalize(&dir).unwrap();
            assert_eq!(
                std::fs::canonicalize(&printed).unwrap(),
                expected,
                "for {name:?}"
            );
            assert!(!root.path().join("pwned").exists() && !dir.join("pwned").exists());
        }
    }

    #[test]
    fn launcher_script_passes_arguments_through_unchanged() {
        let args: Vec<String> = HOSTILE_DIRS.iter().map(|s| s.to_string()).collect();
        let spec = SplitSpec {
            cwd: PathBuf::from("/"),
            program: "printf".into(),
            args: std::iter::once("<%s>".to_string())
                .chain(args.clone())
                .collect(),
            ..spec(SplitDirection::Right)
        };

        let out = run_script(&launcher_script(&spec).unwrap());

        let expected: String = args.iter().map(|a| format!("<{a}>")).collect();
        assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
    }

    #[test]
    fn launcher_script_explains_a_missing_program_instead_of_closing_silently() {
        let spec = SplitSpec {
            program: "definitely-not-installed-xyz".into(),
            args: vec![],
            cwd: PathBuf::from("/"),
            ..spec(SplitDirection::Right)
        };

        let out = run_script(&launcher_script(&spec).unwrap());

        assert!(!out.status.success());
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("definitely-not-installed-xyz not found")
        );
    }

    #[test]
    fn launcher_script_fails_instead_of_running_in_the_wrong_folder() {
        let spec = SplitSpec {
            cwd: PathBuf::from("/definitely/not/a/dir"),
            program: "pwd".into(),
            args: vec![],
            ..spec(SplitDirection::Right)
        };

        let out = run_script(&launcher_script(&spec).unwrap());

        assert!(!out.status.success());
        assert!(out.stdout.is_empty());
    }

    #[test]
    fn launcher_script_starts_with_a_shebang_and_quotes_every_value() {
        let script = launcher_script(&spec(SplitDirection::Right)).unwrap();

        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("cd '/work/pos' || exit 1\n"));
        assert!(script.contains("exec 'nvim' '--listen' '/tmp/lf/nvim.sock'\n"));
    }

    #[test]
    fn non_utf8_paths_are_refused_rather_than_changed() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(OsStr::from_bytes(b"/work/caf\xe9"));

        assert_eq!(
            SplitSpec::neovim(bad, Path::new("/tmp/s")).unwrap_err(),
            LaunchError::NonUtf8Path
        );
        assert_eq!(
            SplitSpec::neovim(Path::new("/work"), bad).unwrap_err(),
            LaunchError::NonUtf8Path
        );
        let bad_cwd = SplitSpec {
            cwd: bad.to_path_buf(),
            ..spec(SplitDirection::Right)
        };
        assert_eq!(
            launcher_script(&bad_cwd).unwrap_err(),
            LaunchError::NonUtf8Path
        );
        let wez = plan_split(
            TerminalKind::Wezterm,
            &bad_cwd,
            Path::new(LAUNCHER),
            &FakeSystem::new(),
        );
        assert_eq!(wez.unwrap_err(), LaunchError::NonUtf8Path);
        let bad_launcher = plan_split(
            TerminalKind::Tmux,
            &spec(SplitDirection::Right),
            bad,
            &FakeSystem::new().env("TMUX", "x"),
        );
        assert_eq!(bad_launcher.unwrap_err(), LaunchError::NonUtf8Path);
    }

    #[test]
    fn neovim_spec_listens_on_the_socket_in_the_project() {
        let s = SplitSpec::neovim(Path::new("/work/pos"), Path::new("/tmp/lf/nvim.sock")).unwrap();

        assert_eq!(s, spec(SplitDirection::Right));
    }

    // --- what each terminal is told to run ----------------------------

    #[test]
    fn iterm2_splits_the_current_session_and_runs_only_the_launcher() {
        let right = only_invocation(plan(TerminalKind::Iterm2, SplitDirection::Right));
        let down = only_invocation(plan(TerminalKind::Iterm2, SplitDirection::Down));

        let script = |verb: &str| {
            vec![
                "-e".to_string(),
                "tell application \"iTerm\"".to_string(),
                "-e".to_string(),
                "tell current session of current window".to_string(),
                "-e".to_string(),
                format!(
                    "{verb} with default profile command {}",
                    applescript_quote(LAUNCHER)
                ),
                "-e".to_string(),
                "end tell".to_string(),
                "-e".to_string(),
                "end tell".to_string(),
            ]
        };
        assert_eq!(right.program, "osascript");
        assert_eq!(right.args, script("split vertically"));
        assert_eq!(down.args, script("split horizontally"));
    }

    #[test]
    fn the_project_folder_never_appears_in_what_the_terminal_is_told() {
        let hostile = SplitSpec {
            cwd: PathBuf::from("/x\"; do shell script \"touch /tmp/pwned\"; \u{15}\r\n"),
            ..spec(SplitDirection::Right)
        };
        let sys = FakeSystem::new().env("TMUX", "x");

        for kind in [
            TerminalKind::Iterm2,
            TerminalKind::Terminal,
            TerminalKind::Tmux,
            TerminalKind::Ghostty,
        ] {
            let plan = plan_split(kind, &hostile, Path::new(LAUNCHER), &sys).unwrap();

            let everything = match plan {
                Plan::Run(runs) => runs
                    .iter()
                    .flat_map(|r| r.args.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
                Plan::Manual { command, .. } => command,
            };
            assert!(
                !everything.contains("do shell script"),
                "{kind:?}: {everything}"
            );
            assert!(!everything.contains('\u{15}'), "{kind:?}");
            assert!(everything.contains(LAUNCHER), "{kind:?}");
        }
    }

    /// Compiles (does not run) the generated AppleScript with the real
    /// `osacompile`, so no window opens and no permission is requested.
    fn compiles(kind: TerminalKind, launcher: &Path, app_path: &str) {
        if !Path::new(app_path).exists() || !Path::new("/usr/bin/osacompile").exists() {
            return;
        }
        let Plan::Run(runs) = plan_split(
            kind,
            &spec(SplitDirection::Right),
            launcher,
            &FakeSystem::new(),
        )
        .unwrap() else {
            panic!("expected a runnable plan")
        };
        let out_dir = tempfile::TempDir::new().unwrap();
        let output = Command::new("osacompile")
            .arg("-o")
            .arg(out_dir.path().join("script.scpt"))
            .args(&runs[0].args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{:?}",
            String::from_utf8_lossy(&output.stderr),
            runs[0].args
        );
    }

    #[test]
    fn generated_applescript_compiles_for_unusual_launcher_paths() {
        for path in [
            LAUNCHER,
            "/tmp/with space/open-nvim.sh",
            "/tmp/it's/open-nvim.sh",
            "/tmp/q\"uote/open-nvim.sh",
        ] {
            compiles(
                TerminalKind::Iterm2,
                Path::new(path),
                "/Applications/iTerm.app",
            );
            compiles(
                TerminalKind::Terminal,
                Path::new(path),
                "/System/Applications/Utilities/Terminal.app",
            );
        }
    }

    #[test]
    fn terminal_app_opens_a_new_window_running_the_launcher() {
        let inv = only_invocation(plan(TerminalKind::Terminal, SplitDirection::Right));

        assert_eq!(inv.program, "osascript");
        assert_eq!(
            inv.args,
            vec![
                "-e".to_string(),
                "tell application \"Terminal\"".to_string(),
                "-e".to_string(),
                "activate".to_string(),
                "-e".to_string(),
                format!("do script {}", applescript_quote(LAUNCHER)),
                "-e".to_string(),
                "end tell".to_string(),
            ]
        );
    }

    #[test]
    fn tmux_splits_beside_or_below_inside_a_session() {
        let right = only_invocation(plan(TerminalKind::Tmux, SplitDirection::Right));
        let down = only_invocation(plan(TerminalKind::Tmux, SplitDirection::Down));

        assert_eq!(right.program, "tmux");
        assert_eq!(
            right.args,
            vec![
                "split-window".to_string(),
                "-h".to_string(),
                LAUNCHER.to_string()
            ]
        );
        assert_eq!(down.args[1], "-v");
    }

    #[test]
    fn a_launcher_path_with_unusual_characters_is_quoted_for_the_shell() {
        let sys = FakeSystem::new().env("TMUX", "x");

        let plan = plan_split(
            TerminalKind::Tmux,
            &spec(SplitDirection::Right),
            Path::new("/tmp/a b/it's.sh"),
            &sys,
        )
        .unwrap();

        assert_eq!(only_invocation(plan).args[2], "'/tmp/a b/it'\\''s.sh'");
    }

    #[test]
    fn tmux_outside_a_session_is_an_error() {
        let err = plan_split(
            TerminalKind::Tmux,
            &spec(SplitDirection::Right),
            Path::new(LAUNCHER),
            &FakeSystem::new(),
        )
        .unwrap_err();

        assert_eq!(err, LaunchError::NotInsideTmux);
    }

    #[test]
    fn wezterm_uses_its_cli_with_argv_not_a_shell() {
        let right = only_invocation(plan(TerminalKind::Wezterm, SplitDirection::Right));
        let down = only_invocation(plan(TerminalKind::Wezterm, SplitDirection::Down));

        let expect = |flag: &str| -> Vec<String> {
            [
                "cli",
                "split-pane",
                flag,
                "--cwd",
                "/work/pos",
                "--",
                "nvim",
                "--listen",
                "/tmp/lf/nvim.sock",
            ]
            .map(String::from)
            .to_vec()
        };
        assert_eq!(right.program, "wezterm");
        assert_eq!(right.args, expect("--right"));
        assert_eq!(down.args, expect("--bottom"));
    }

    #[test]
    fn kitty_launches_a_split_window_through_remote_control() {
        let right = only_invocation(plan(TerminalKind::Kitty, SplitDirection::Right));
        let down = only_invocation(plan(TerminalKind::Kitty, SplitDirection::Down));

        assert_eq!(right.program, "kitty");
        assert_eq!(
            right.args,
            [
                "@",
                "launch",
                "--location=vsplit",
                "--cwd=/work/pos",
                "--",
                "nvim",
                "--listen",
                "/tmp/lf/nvim.sock"
            ]
            .map(String::from)
            .to_vec()
        );
        assert_eq!(down.args[2], "--location=hsplit");
    }

    #[test]
    fn ghostty_cannot_be_driven_so_the_user_gets_a_short_command() {
        let plan = plan(TerminalKind::Ghostty, SplitDirection::Right);

        let Plan::Manual { note, command } = plan else {
            panic!("expected Manual")
        };
        assert!(note.contains("Ghostty"));
        assert_eq!(command, LAUNCHER);
    }

    #[test]
    fn auto_must_be_resolved_first() {
        let err = plan_split(
            TerminalKind::Auto,
            &spec(SplitDirection::Right),
            Path::new(LAUNCHER),
            &FakeSystem::new(),
        )
        .unwrap_err();

        assert_eq!(err, LaunchError::Undetected);
    }

    #[test]
    fn clipboard_invocation_sets_the_clipboard_with_a_quoted_literal() {
        let inv = clipboard_invocation("cd '/x' && exec \"nvim\"");

        assert_eq!(inv.program, "osascript");
        assert_eq!(
            inv.args,
            vec![
                "-e".to_string(),
                format!(
                    "set the clipboard to {}",
                    applescript_quote("cd '/x' && exec \"nvim\"")
                )
            ]
        );
    }

    #[test]
    fn clipboard_script_compiles_for_hostile_text() {
        if !Path::new("/usr/bin/osacompile").exists() {
            return;
        }
        for text in [
            "plain",
            "it's",
            "say \"hi\"",
            "back\\slash",
            "multi\nline",
            "$(id) `id`",
        ] {
            let inv = clipboard_invocation(text);
            let out_dir = tempfile::TempDir::new().unwrap();

            let output = Command::new("osacompile")
                .arg("-o")
                .arg(out_dir.path().join("c.scpt"))
                .args(&inv.args)
                .output()
                .unwrap();

            assert!(
                output.status.success(),
                "{text:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn copy_to_clipboard_runs_the_script_and_reports_failures() {
        let key = command_key(&clipboard_invocation("hello"));

        assert_eq!(
            copy_to_clipboard(&FakeSystem::new().ok(&key, ""), "hello"),
            Ok(())
        );
        assert_eq!(
            copy_to_clipboard(&FakeSystem::new(), "hello"),
            Err(LaunchError::ToolMissing("osascript".into()))
        );
        assert!(matches!(
            copy_to_clipboard(&FakeSystem::new().fails(&key, "boom"), "hello"),
            Err(LaunchError::Failed { .. })
        ));
    }

    // --- running the plan ---------------------------------------------

    fn iterm_key() -> String {
        command_key(&only_invocation(plan(
            TerminalKind::Iterm2,
            SplitDirection::Right,
        )))
    }

    fn open(sys: &FakeSystem, kind: TerminalKind) -> Result<Launched, LaunchError> {
        open_split(sys, kind, &spec(SplitDirection::Right), Path::new(LAUNCHER))
    }

    #[test]
    fn open_split_runs_the_planned_command() {
        let sys = FakeSystem::new().ok(&iterm_key(), "");

        assert_eq!(open(&sys, TerminalKind::Iterm2), Ok(Launched::Opened));
    }

    #[test]
    fn open_split_passes_manual_plans_through_without_running_anything() {
        let result = open(&FakeSystem::new(), TerminalKind::Ghostty).unwrap();

        assert!(matches!(result, Launched::Manual { ref command, .. } if command == LAUNCHER));
    }

    #[test]
    fn open_split_reports_a_missing_tool() {
        let err = open(&FakeSystem::new(), TerminalKind::Iterm2).unwrap_err();

        assert_eq!(err, LaunchError::ToolMissing("osascript".into()));
    }

    #[test]
    fn open_split_recognises_the_macos_automation_block() {
        let denied = "execution error: Not authorized to send Apple events to iTerm. (-1743)";
        let sys = FakeSystem::new().fails(&iterm_key(), denied);

        let err = open(&sys, TerminalKind::Iterm2).unwrap_err();

        assert_eq!(err, LaunchError::AutomationDenied("iTerm2".into()));
        assert!(err.to_string().contains("Automation"));
    }

    #[test]
    fn open_split_reports_other_failures_with_the_tools_message() {
        let sys = FakeSystem::new().fails(&iterm_key(), "  boom \n");

        let err = open(&sys, TerminalKind::Iterm2).unwrap_err();

        assert_eq!(
            err,
            LaunchError::Failed {
                program: "osascript".into(),
                detail: "boom".into()
            }
        );
    }

    #[test]
    fn long_tool_messages_are_shortened() {
        let sys = FakeSystem::new().fails(&iterm_key(), &"x".repeat(5000));

        let LaunchError::Failed { detail, .. } = open(&sys, TerminalKind::Iterm2).unwrap_err()
        else {
            panic!()
        };

        assert_eq!(detail.chars().count(), MAX_DETAIL_CHARS + 1, "{detail}");
        assert!(detail.ends_with('…'));
    }

    #[test]
    fn shorten_keeps_short_text_and_cuts_on_character_boundaries() {
        assert_eq!(shorten("short"), "short");
        let multibyte = "é".repeat(MAX_DETAIL_CHARS + 10);
        assert_eq!(shorten(&multibyte).chars().count(), MAX_DETAIL_CHARS + 1);
    }

    #[test]
    fn open_split_reports_a_timeout() {
        let sys = FakeSystem::new().timeout(&iterm_key());

        assert_eq!(
            open(&sys, TerminalKind::Iterm2).unwrap_err(),
            LaunchError::TimedOut("osascript".into())
        );
    }
}
