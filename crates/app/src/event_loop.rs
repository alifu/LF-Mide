//! The main loop: draw, wait for a key or background action, reduce, repeat.

use color_eyre::Result;
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use lf_mide_core::{Action, AppState, DoctorPanel, Project, TerminalKind, reduce};
use lf_mide_doctor::system::SystemRunner;
use lf_mide_terminal::{LaunchError, Launched, copy_to_clipboard, open_split};
use ratatui::DefaultTerminal;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::{branch_watch, editor, keymap, paths, settings};

/// The first AppleScript call may wait for the user to allow access.
const EDITOR_LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);

pub async fn run(mut terminal: DefaultTerminal, initial: AppState) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Action>();
    let watcher = branch_watch::spawn(initial.project.root.clone(), tx.clone());

    let mut events = EventStream::new();
    let rows = terminal.size()?.height;
    let detected = lf_mide_terminal::detect(&SystemRunner::new());
    let mut state = reduce(initial, Action::ViewportChanged(rows));
    state = reduce(state, Action::TerminalDetected(detected));
    let result = loop {
        if state.should_quit {
            break Ok(());
        }
        if let Err(err) = terminal.draw(|frame| lf_mide_ui::render(frame, &state)) {
            break Err(err.into());
        }
        let input = tokio::select! {
            event = events.next() => Input::Terminal(event),
            Some(first) = rx.recv() => Input::Background(first),
        };
        let actions = match input {
            Input::Terminal(Some(Ok(Event::Key(key)))) => {
                keymap::action_for(&state, key).into_iter().collect()
            }
            Input::Terminal(Some(Ok(Event::Resize(_, rows)))) => {
                vec![Action::ViewportChanged(rows)]
            }
            Input::Terminal(Some(Ok(_))) => Vec::new(),
            Input::Terminal(Some(Err(err))) => break Err(err.into()),
            Input::Terminal(None) => vec![Action::Quit],
            Input::Background(first) => drain(first, &mut rx),
        };

        let was_running = state.doctor == DoctorPanel::Running;
        let open_editor = actions.iter().any(|a| matches!(a, Action::OpenEditor));
        let terminal_before = state.config.terminal;
        state = actions.into_iter().fold(state, reduce);

        if doctor_run_started(was_running, &state) {
            spawn_doctor(state.project.clone(), tx.clone());
        }
        if open_editor {
            spawn_editor(&state, tx.clone());
        }
        if state.config.terminal != terminal_before {
            save_terminal_choice(state.config.terminal, &tx);
        }
    };
    watcher.abort();
    result
}

enum Input {
    Terminal(Option<std::io::Result<Event>>),
    Background(Action),
}

/// Collects everything already queued so a burst of background actions
/// costs one redraw instead of one per action.
fn drain(first: Action, rx: &mut UnboundedReceiver<Action>) -> Vec<Action> {
    std::iter::once(first)
        .chain(std::iter::from_fn(|| rx.try_recv().ok()))
        .collect()
}

/// True when the panel just entered `Running`, which is the cue to start
/// the checks. Staying in `Running` must not start a second run.
fn doctor_run_started(was_running: bool, state: &AppState) -> bool {
    !was_running && state.doctor == DoctorPanel::Running
}

/// Opens Neovim in a split pane of the user's terminal. Runs on a thread:
/// the first `osascript` call can wait on a macOS permission prompt.
fn spawn_editor(state: &AppState, tx: UnboundedSender<Action>) {
    let configured = state.config.terminal;
    let root = state.project.root.clone();
    std::thread::spawn(move || {
        let sys = SystemRunner::with_timeout(EDITOR_LAUNCH_TIMEOUT);
        let Some(kind) = lf_mide_terminal::resolve(configured, &sys) else {
            let _ = tx.send(Action::Status(LaunchError::Undetected.to_string()));
            return;
        };
        let prepared = match editor::prepare(&root, &std::env::temp_dir(), std::process::id()) {
            Ok(prepared) => prepared,
            Err(message) => {
                let _ = tx.send(Action::Status(message));
                return;
            }
        };
        let result = open_split(&sys, kind, &prepared.spec, &prepared.launcher);
        // A terminal we can't split: put the command on the clipboard instead.
        let copied = match &result {
            Ok(Launched::Manual { command, .. }) => copy_to_clipboard(&sys, command).is_ok(),
            _ => false,
        };
        let _ = tx.send(editor_result_action(prepared.socket, result, copied));
    });
}

/// Remembers the picked terminal in the global config file.
fn save_terminal_choice(kind: TerminalKind, tx: &UnboundedSender<Action>) {
    let outcome = match paths::global_config() {
        Some(path) => settings::save_terminal(&path, kind).map_err(|err| err.to_string()),
        None => Err("no home directory".to_string()),
    };
    if let Err(reason) = outcome {
        tracing::warn!(%reason, "could not save the terminal choice");
        let _ = tx.send(Action::Status(format!(
            "Terminal changed, but saving it failed: {reason}"
        )));
    }
}

/// Runs the doctor on a plain thread: its commands can take seconds, and a
/// detached thread never delays quitting the way a blocking-pool task would.
fn spawn_doctor(project: Project, tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        let report = lf_mide_doctor::diagnose(&SystemRunner::new(), Some(&project));
        // The receiver is gone when the app is quitting; nothing to report to.
        let _ = tx.send(Action::DoctorFinished(report));
    });
}

/// What the app should do once the editor launch finished.
///
/// `copied` says whether the command of a manual launch reached the clipboard.
fn editor_result_action(
    socket: PathBuf,
    result: Result<Launched, LaunchError>,
    copied: bool,
) -> Action {
    match result {
        Ok(Launched::Opened) => Action::EditorOpened(socket),
        Ok(Launched::Manual { note, .. }) if copied => Action::Status(format!(
            "{note} Command copied to the clipboard: split a pane and paste it."
        )),
        Ok(Launched::Manual { note, command }) => {
            Action::Status(format!("{note} Run in a new pane: {command}"))
        }
        Err(err) => Action::Status(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_core::{Config, ProjectLayout, Report};
    use std::path::PathBuf;

    fn state_with(doctor: DoctorPanel) -> AppState {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "demo".into(),
            layout: ProjectLayout::ComposeApp,
        };
        AppState {
            doctor,
            ..AppState::new(project, Config::default())
        }
    }

    #[test]
    fn drain_collects_queued_actions_in_order() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(Action::FocusNext).unwrap();
        tx.send(Action::FocusPrev).unwrap();

        let actions = drain(Action::Quit, &mut rx);

        assert_eq!(
            actions,
            vec![Action::Quit, Action::FocusNext, Action::FocusPrev]
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn a_run_starts_only_on_the_transition_into_running() {
        let running = state_with(DoctorPanel::Running);
        let closed = state_with(DoctorPanel::Closed);
        let ready = state_with(DoctorPanel::Ready(Report::default()));

        assert!(doctor_run_started(false, &running));
        assert!(!doctor_run_started(true, &running), "already running");
        assert!(!doctor_run_started(false, &closed));
        assert!(!doctor_run_started(true, &ready), "finished, not started");
    }

    fn manual() -> Launched {
        Launched::Manual {
            note: "Ghostty can't be controlled by other programs.".into(),
            command: "cd '/p' && exec 'nvim'".into(),
        }
    }

    #[test]
    fn a_launched_editor_reports_its_socket() {
        let action = editor_result_action(PathBuf::from("/tmp/s"), Ok(Launched::Opened), false);

        assert_eq!(action, Action::EditorOpened(PathBuf::from("/tmp/s")));
    }

    #[test]
    fn a_manual_launch_says_the_command_is_on_the_clipboard() {
        let action = editor_result_action(PathBuf::from("/tmp/s"), Ok(manual()), true);

        let Action::Status(message) = action else {
            panic!("expected a status")
        };
        assert!(
            message.starts_with("Ghostty can't be controlled"),
            "{message}"
        );
        assert!(message.contains("copied"), "{message}");
        assert!(
            !message.contains("exec"),
            "the command stays out of the status line: {message}"
        );
    }

    #[test]
    fn a_manual_launch_shows_the_command_when_copying_failed() {
        let action = editor_result_action(PathBuf::from("/tmp/s"), Ok(manual()), false);

        let Action::Status(message) = action else {
            panic!("expected a status")
        };
        assert!(message.ends_with("cd '/p' && exec 'nvim'"), "{message}");
    }

    #[test]
    fn a_failed_launch_shows_the_reason() {
        let action = editor_result_action(
            PathBuf::from("/tmp/s"),
            Err(LaunchError::NotInsideTmux),
            false,
        );

        assert_eq!(
            action,
            Action::Status(LaunchError::NotInsideTmux.to_string())
        );
    }
}
