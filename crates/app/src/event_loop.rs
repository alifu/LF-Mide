//! The main loop: draw, wait for a key or background action, reduce, repeat.

use color_eyre::Result;
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use lf_mide_core::{Action, AppState, DoctorPanel, Project, reduce};
use lf_mide_doctor::system::SystemRunner;
use ratatui::DefaultTerminal;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::{branch_watch, keymap};

pub async fn run(mut terminal: DefaultTerminal, initial: AppState) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Action>();
    let watcher = branch_watch::spawn(initial.project.root.clone(), tx.clone());

    let mut events = EventStream::new();
    let rows = terminal.size()?.height;
    let mut state = reduce(initial, Action::ViewportChanged(rows));
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
        state = actions.into_iter().fold(state, reduce);
        if doctor_run_started(was_running, &state) {
            spawn_doctor(state.project.clone(), tx.clone());
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

/// Runs the doctor on a plain thread: its commands can take seconds, and a
/// detached thread never delays quitting the way a blocking-pool task would.
fn spawn_doctor(project: Project, tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        let report = lf_mide_doctor::diagnose(&SystemRunner::new(), Some(&project));
        // The receiver is gone when the app is quitting; nothing to report to.
        let _ = tx.send(Action::DoctorFinished(report));
    });
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
}
