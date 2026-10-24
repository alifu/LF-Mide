//! The main loop: draw, wait for a key or background action, reduce, repeat.

use color_eyre::Result;
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use lf_mide_core::{
    Action, AppState, DoctorPanel, EditorMode, Project, ProjectPrefs, TerminalKind, reduce,
};
use lf_mide_doctor::system::SystemRunner;
use lf_mide_terminal::{LaunchError, Launched, copy_to_clipboard, open_split};
use ratatui::DefaultTerminal;
use ratatui::layout::Rect;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::embedded_editor::EmbeddedEditor;
use crate::{
    branch_watch, editor, effects, git_effects, jobs, keymap, paths, perf_effects, settings,
};

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
    // Choices remembered from the last run in this project.
    let prefs_path = paths::project_prefs(&state.project.root, &state.project.name);
    if let Some(path) = &prefs_path {
        state = reduce(state, Action::PrefsLoaded(settings::load_prefs(path)));
    }
    let mut saved_prefs = state.prefs();
    effects::spawn_scan(state.project.root.clone(), tx.clone());
    effects::spawn_sdk_info(tx.clone());
    let device_watch = effects::spawn_device_watch(tx.clone());
    let mut grep_scheduler = effects::GrepScheduler::default();
    let cache = paths::log_dir().unwrap_or_else(|| std::env::temp_dir().join("lf-mide"));
    let mut job_runner = jobs::JobRunner::default();
    let job_env = jobs::JobEnv {
        backlog: job_runner.backlog(),
        root: state.project.root.clone(),
        layout: state.project.layout,
        derived_data: jobs::derived_data_dir(&cache, &state.project.root, &state.project.name),
        tx: tx.clone(),
    };
    let mut quit_signals = QuitSignals::install();
    let git_watch = git_effects::spawn_git_watch(state.project.root.clone(), tx.clone());
    let perf_visible = Arc::new(AtomicBool::new(state.config.show_perf));
    let perf_watch = perf_effects::spawn_perf_watch(
        tx.clone(),
        Duration::from_secs(state.config.perf_interval_secs),
        Arc::clone(&perf_visible),
    );
    let mut embedded = EmbeddedEditor::default();
    let sys = SystemRunner::new();
    let result = loop {
        if state.should_quit {
            break Ok(());
        }
        if let Err(err) = terminal.draw(|frame| lf_mide_ui::render(frame, &state)) {
            break Err(err.into());
        }
        let screen = match terminal.size() {
            Ok(size) => Rect::new(0, 0, size.width, size.height),
            Err(err) => break Err(err.into()),
        };
        embedded.fit(screen);
        embedded.update_cursor(&state);
        let input = tokio::select! {
            event = events.next() => Input::Terminal(event),
            Some(first) = rx.recv() => Input::Background(first),
            () = quit_signals.arrived() => Input::Signal,
        };
        let actions = match input {
            Input::Terminal(Some(Ok(Event::Key(key)))) => {
                keymap::action_for(&state, key).into_iter().collect()
            }
            Input::Terminal(Some(Ok(Event::Resize(_, rows)))) => {
                vec![Action::ViewportChanged(rows)]
            }
            Input::Terminal(Some(Ok(Event::Paste(text)))) if state.editor_has_keys() => {
                vec![Action::EditorPaste(text)]
            }
            Input::Terminal(Some(Ok(Event::Paste(text)))) => keymap::paste_actions(&state, &text),
            Input::Terminal(Some(Ok(_))) => Vec::new(),
            Input::Terminal(Some(Err(err))) => break Err(err.into()),
            Input::Terminal(None) => vec![Action::QuitNow],
            Input::Background(first) => drain(first, &mut rx),
            // The terminal window closed or the app was asked to end: leave
            // through the normal path so builds and logs are stopped. There
            // is no one left to answer Neovim; its swap files keep any
            // unsaved changes.
            Input::Signal => vec![Action::QuitNow],
        };

        // Room for more job output now that these were taken off the queue.
        job_runner.backlog().consumed(&actions);
        let was_running = state.doctor == DoctorPanel::Running;
        let scan_was_running = state.files_loading;
        let refresh_devices = actions.iter().any(|a| matches!(a, Action::RefreshDevices));
        let refresh_git = actions.iter().any(|a| matches!(a, Action::RefreshGit));
        let grep_finished = actions
            .iter()
            .any(|a| matches!(a, Action::GrepResults { .. }));
        let terminal_before = state.config.terminal;
        let refresh_files = actions.iter().any(|a| matches!(a, Action::RefreshFiles));
        let opens = file_opens(&actions);
        let finder_before = state.finder.as_ref().map(|f| f.query.clone());
        let grep_before = state.grep.as_ref().and_then(|g| g.searched.clone());
        let files_before = Arc::clone(&state.files);
        let quit_was_pending = state.quit_pending;
        embedded.forward(&actions, &tx);
        state = actions.into_iter().fold(state, reduce);
        if state.quit_pending && !quit_was_pending {
            embedded.confirm_quit(&tx);
        }

        if doctor_run_started(was_running, &state) {
            spawn_doctor(state.project.clone(), tx.clone());
        }
        // Neovim was asked for (by `e`, or by choosing a file with none open).
        if state.editor_launch_wanted && !state.should_quit {
            let started = match state.config.editor {
                EditorMode::Split => {
                    spawn_editor(&state, tx.clone());
                    None
                }
                EditorMode::Embedded => {
                    Some(embedded.start(&state, screen, &sys, tx.clone()).await)
                }
            };
            state = reduce(state, Action::EditorLaunchStarted);
            if let Some(started) = started {
                state = reduce(state, started);
            }
        }
        if state.config.terminal != terminal_before {
            save_terminal_choice(state.config.terminal, &tx);
        }

        // Start or stop the devices the user asked for.
        let (next, commands) = effects::take_commands(state);
        state = next;
        for command in commands {
            effects::spawn_device_command(command, tx.clone());
        }
        if refresh_devices {
            effects::spawn_device_listing(tx.clone());
        }
        if refresh_git {
            git_effects::spawn_git_refresh(state.project.root.clone(), tx.clone());
        }
        perf_visible.store(state.config.show_perf || state.perf_open, Ordering::Relaxed);
        // Builds and log streams the user asked for.
        let (next, queued) = jobs::take_jobs(state);
        state = next;
        for job in queued {
            job_runner.apply(job, &job_env);
        }
        if let Some(prefs) = effects::prefs_to_save(&saved_prefs, &state) {
            save_prefs_quietly(prefs_path.as_deref(), &prefs, &tx);
            saved_prefs = prefs;
        }

        if effects::should_start_scan(refresh_files, scan_was_running) {
            effects::spawn_scan(state.project.root.clone(), tx.clone());
        }
        // Rank when the finder opens, its query changes, or the files do.
        let ranked_before = if Arc::ptr_eq(&files_before, &state.files) {
            finder_before.as_deref()
        } else {
            None
        };
        if let Some(query) = effects::finder_query_to_rank(ranked_before, &state) {
            let matches = lf_mide_nav::fuzzy::rank(&state.files, &query, effects::FINDER_LIMIT);
            state = reduce(state, Action::FinderResults { query, matches });
        }
        // One ripgrep at a time: a new search waits for the running one and
        // only the latest query is kept.
        let mut to_run = Vec::new();
        if grep_finished {
            to_run.extend(grep_scheduler.finished());
        }
        if let Some(query) = effects::grep_query_to_run(grep_before.as_deref(), &state) {
            to_run.extend(grep_scheduler.request(query));
        }
        for query in to_run {
            effects::spawn_grep(state.project.root.clone(), query, tx.clone());
        }
        // Files chosen while Neovim was not running are handed over once it is.
        if let (Some(socket), Some((path, line))) =
            (state.editor_socket.clone(), state.pending_open.clone())
        {
            state = reduce(state, Action::PendingOpenSent);
            let root = state.project.root.clone();
            let patience = effects::open_patience(state.editor_ready);
            effects::spawn_open(root, socket, path, line, patience, tx.clone());
        }
        // Without a running editor these wait in `pending_open` instead.
        if let Some(socket) = &state.editor_socket {
            for (path, line) in opens {
                let root = state.project.root.clone();
                let patience = effects::open_patience(state.editor_ready);
                effects::spawn_open(root, socket.clone(), path, line, patience, tx.clone());
            }
        }
    };
    // Stop the things that start commands, then everything still running:
    // a search or a tool being waited for would otherwise be left behind.
    embedded.shut_down().await;
    embedded.restore_cursor();
    watcher.abort();
    device_watch.abort();
    perf_watch.abort();
    git_watch.abort();
    job_runner.cancel_all();
    lf_mide_system::terminate_running_commands();
    result
}

/// The files the user asked to open in this batch of actions.
fn file_opens(actions: &[Action]) -> Vec<(PathBuf, Option<u32>)> {
    actions
        .iter()
        .filter_map(|action| match action {
            Action::OpenFile { path, line } => Some((path.clone(), *line)),
            _ => None,
        })
        .collect()
}

enum Input {
    Terminal(Option<std::io::Result<Event>>),
    Background(Action),
    Signal,
}

/// SIGHUP (terminal closed) and SIGTERM (asked to end). Without handlers
/// either would end the process at once and leave a running build behind.
struct QuitSignals {
    hangup: Option<Signal>,
    terminate: Option<Signal>,
}

impl QuitSignals {
    fn install() -> Self {
        let listen = |kind: SignalKind| match signal(kind) {
            Ok(signal) => Some(signal),
            Err(error) => {
                tracing::warn!(%error, "cannot listen for a quit signal");
                None
            }
        };
        Self {
            hangup: listen(SignalKind::hangup()),
            terminate: listen(SignalKind::terminate()),
        }
    }

    /// Completes when one of the signals arrives; never if none can be
    /// listened for.
    async fn arrived(&mut self) {
        async fn next(signal: &mut Option<Signal>) {
            match signal {
                Some(signal) => {
                    signal.recv().await;
                }
                None => std::future::pending().await,
            }
        }
        tokio::select! {
            () = next(&mut self.hangup) => {}
            () = next(&mut self.terminate) => {}
        }
    }
}

/// Collects everything already queued so a burst of background actions
/// costs one redraw instead of one per action.
/// Most background actions handled before drawing again, so a flood of log
/// lines cannot starve the keyboard.
const MAX_DRAIN: usize = 512;

fn drain(first: Action, rx: &mut UnboundedReceiver<Action>) -> Vec<Action> {
    std::iter::once(first)
        .chain(std::iter::from_fn(|| rx.try_recv().ok()))
        .take(MAX_DRAIN)
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
            let _ = tx.send(Action::EditorLaunchFailed(
                LaunchError::Undetected.to_string(),
            ));
            return;
        };
        let prepared = match editor::prepare(&root, &std::env::temp_dir(), std::process::id()) {
            Ok(prepared) => prepared,
            Err(message) => {
                let _ = tx.send(Action::EditorLaunchFailed(message));
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

/// Remembers the build target and chosen devices. A failure is shown but
/// never fatal: these are conveniences.
fn save_prefs_quietly(
    path: Option<&std::path::Path>,
    prefs: &ProjectPrefs,
    tx: &UnboundedSender<Action>,
) {
    let Some(path) = path else { return };
    if let Err(error) = settings::save_prefs(path, prefs) {
        tracing::warn!(%error, "could not save project preferences");
        let _ = tx.send(Action::Status(format!(
            "Could not remember your choices: {error}"
        )));
    }
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
        Ok(Launched::Manual { note, .. }) if copied => Action::EditorLaunchFailed(format!(
            "{note} Command copied to the clipboard: split a pane and paste it."
        )),
        Ok(Launched::Manual { note, command }) => {
            Action::EditorLaunchFailed(format!("{note} Run in a new pane: {command}"))
        }
        Err(err) => Action::EditorLaunchFailed(err.to_string()),
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

    #[tokio::test]
    async fn a_hangup_or_terminate_signal_is_noticed() {
        for signal in [libc::SIGHUP, libc::SIGTERM] {
            let mut signals = QuitSignals::install();

            // SAFETY: both signals are being handled by `signals`, so raising
            // them cannot end the test process.
            unsafe { libc::raise(signal) };

            tokio::time::timeout(Duration::from_secs(5), signals.arrived())
                .await
                .expect("the signal was not noticed");
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
    fn drain_stops_at_the_cap_and_leaves_the_rest_queued() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        for _ in 0..MAX_DRAIN + 100 {
            tx.send(Action::FocusNext).unwrap();
        }

        let first = drain(Action::Quit, &mut rx);

        assert_eq!(first.len(), MAX_DRAIN);
        assert!(
            rx.try_recv().is_ok(),
            "the remainder waits for the next round"
        );
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
    fn file_opens_collects_only_open_requests_in_order() {
        let actions = vec![
            Action::FocusNext,
            Action::OpenFile {
                path: PathBuf::from("a.kt"),
                line: Some(3),
            },
            Action::Quit,
            Action::OpenFile {
                path: PathBuf::from("b.kt"),
                line: None,
            },
        ];

        assert_eq!(
            file_opens(&actions),
            vec![
                (PathBuf::from("a.kt"), Some(3)),
                (PathBuf::from("b.kt"), None)
            ]
        );
        assert!(file_opens(&[]).is_empty());
    }

    #[test]
    fn a_launched_editor_reports_its_socket() {
        let action = editor_result_action(PathBuf::from("/tmp/s"), Ok(Launched::Opened), false);

        assert_eq!(action, Action::EditorOpened(PathBuf::from("/tmp/s")));
    }

    #[test]
    fn a_manual_launch_says_the_command_is_on_the_clipboard() {
        let action = editor_result_action(PathBuf::from("/tmp/s"), Ok(manual()), true);

        let Action::EditorLaunchFailed(message) = action else {
            panic!("expected the launch to be reported as not done")
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

        let Action::EditorLaunchFailed(message) = action else {
            panic!("expected the launch to be reported as not done")
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
            Action::EditorLaunchFailed(LaunchError::NotInsideTmux.to_string())
        );
    }
}
