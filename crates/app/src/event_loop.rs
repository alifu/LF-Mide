//! The main loop: draw, wait for a key or background action, reduce, repeat.

use color_eyre::Result;
use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use lf_mide_core::{Action, AppState, reduce};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::{branch_watch, keymap};

pub async fn run(mut terminal: DefaultTerminal, initial: AppState) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Action>();
    let watcher = branch_watch::spawn(initial.project.root.clone(), tx);

    let mut events = EventStream::new();
    let mut state = initial;
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
                keymap::action_for(key).into_iter().collect()
            }
            Input::Terminal(Some(Ok(_))) => Vec::new(), // Resize etc.: the next draw handles it.
            Input::Terminal(Some(Err(err))) => break Err(err.into()),
            Input::Terminal(None) => vec![Action::Quit],
            Input::Background(first) => drain(first, &mut rx),
        };
        state = actions.into_iter().fold(state, reduce);
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
fn drain(first: Action, rx: &mut mpsc::UnboundedReceiver<Action>) -> Vec<Action> {
    std::iter::once(first)
        .chain(std::iter::from_fn(|| rx.try_recv().ok()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
