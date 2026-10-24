//! State changes for the Neovim that runs inside LF-Mide's editor panel.

use std::sync::Arc;

use crate::config::EditorMode;
use crate::screen::Screen;
use crate::state::{AppState, Panel};

/// `q`: with our Neovim running, quitting waits for it to close, so its own
/// "Save changes?" prompt can be answered first.
pub fn quit(state: AppState) -> AppState {
    if !state.embedded {
        return AppState {
            should_quit: true,
            ..state
        };
    }
    AppState {
        quit_pending: true,
        focus: Panel::Editor,
        status: Some("Closing Neovim: answer its question if it asks one".to_string()),
        ..state
    }
}

pub fn started(state: AppState) -> AppState {
    AppState {
        embedded: true,
        editor_screen: None,
        ..state
    }
}

/// A redraw from a Neovim that has already gone is dropped.
pub fn screen(state: AppState, screen: Arc<Screen>) -> AppState {
    if !state.embedded {
        return state;
    }
    AppState {
        editor_screen: Some(screen),
        ..state
    }
}

/// Our Neovim has ended: by `:q`, by answering the quit prompt, or because
/// it failed.
pub fn exited(state: AppState) -> AppState {
    let quitting = state.quit_pending;
    let focus = match state.focus {
        Panel::Editor => Panel::Files,
        other => other,
    };
    AppState {
        embedded: false,
        editor_screen: None,
        editor_socket: None,
        editor_ready: false,
        editor_launching: false,
        editor_launch_wanted: false,
        pending_open: None,
        marks_key: None,
        current_file: None,
        quit_pending: false,
        should_quit: state.should_quit || quitting,
        focus,
        status: Some("Neovim closed: press e to open it again".to_string()),
        ..state
    }
}

pub fn quit_cancelled(state: AppState, reason: Option<String>) -> AppState {
    let status = match reason {
        None => "Quit cancelled".to_string(),
        Some(reason) => format!("Neovim did not quit: {reason}"),
    };
    AppState {
        quit_pending: false,
        status: Some(status),
        ..state
    }
}

/// True when a file the user chooses is shown in our own Neovim (running,
/// or about to be started for it), so the editor panel should get focus.
pub fn shows_files_here(state: &AppState) -> bool {
    state.config.editor == EditorMode::Embedded && (state.embedded || state.editor_socket.is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::config::Config;
    use crate::project::{Project, ProjectLayout};
    use crate::reducer::reduce;
    use crate::screen::RedrawEvent;
    use std::path::PathBuf;

    fn state_with(config: Config) -> AppState {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "demo".into(),
            layout: ProjectLayout::ComposeApp,
        };
        AppState::new(project, config)
    }

    fn state() -> AppState {
        state_with(Config::default())
    }

    fn running() -> AppState {
        // The order the event loop produces.
        [
            Action::OpenEditor,
            Action::EditorLaunchStarted,
            Action::EmbeddedStarted,
            Action::EditorOpened(PathBuf::from("/tmp/s")),
        ]
        .into_iter()
        .fold(state(), reduce)
    }

    fn a_screen() -> Arc<Screen> {
        Arc::new(Screen::default().apply(RedrawEvent::Resize {
            width: 2,
            height: 1,
        }))
    }

    #[test]
    fn quitting_without_our_neovim_ends_at_once() {
        assert!(reduce(state(), Action::Quit).should_quit);
    }

    #[test]
    fn quitting_with_our_neovim_waits_for_it_to_close() {
        let asked = reduce(running(), Action::Quit);

        assert!(!asked.should_quit);
        assert!(asked.quit_pending);
        assert_eq!(asked.focus, Panel::Editor, "its question is answered there");

        let closed = reduce(asked, Action::EditorExited);
        assert!(closed.should_quit);
    }

    #[test]
    fn cancelling_in_neovim_keeps_the_app_running() {
        let cancelled = reduce(
            reduce(running(), Action::Quit),
            Action::EditorQuitCancelled(None),
        );

        assert!(!cancelled.quit_pending && !cancelled.should_quit);
        let closed_later = reduce(cancelled, Action::EditorExited);
        assert!(!closed_later.should_quit, ":q later must not end the app");
    }

    #[test]
    fn a_refusal_says_why_and_q_can_be_tried_again() {
        let refused = reduce(
            reduce(running(), Action::Quit),
            Action::EditorQuitCancelled(Some("E212: Can't open file for writing".into())),
        );

        assert_eq!(
            refused.status.as_deref(),
            Some("Neovim did not quit: E212: Can't open file for writing")
        );
        assert!(reduce(refused, Action::Quit).quit_pending, "asked again");
    }

    #[test]
    fn quit_now_never_waits() {
        assert!(reduce(running(), Action::QuitNow).should_quit);
    }

    #[test]
    fn screens_are_kept_only_while_our_neovim_runs() {
        let shown = reduce(running(), Action::EditorScreen(a_screen()));
        let stale = reduce(state(), Action::EditorScreen(a_screen()));

        assert!(shown.editor_screen.is_some());
        assert!(stale.editor_screen.is_none());
    }

    #[test]
    fn an_exit_forgets_the_editor_and_leaves_its_panel() {
        let mut s = reduce(running(), Action::EditorScreen(a_screen()));
        s.focus = Panel::Editor;

        let after = reduce(s, Action::EditorExited);

        assert!(!after.embedded && after.editor_screen.is_none());
        assert_eq!(after.editor_socket, None);
        assert_eq!(after.focus, Panel::Files);
        assert!(!after.should_quit);
        assert!(
            reduce(after, Action::OpenEditor).editor_launch_wanted,
            "e opens a new one"
        );
    }

    #[test]
    fn a_lost_socket_does_not_end_a_running_embedded_neovim() {
        let after = reduce(running(), Action::EditorLost);

        assert_eq!(after.editor_socket, Some(PathBuf::from("/tmp/s")));
        assert!(after.embedded);
    }

    #[test]
    fn e_with_our_neovim_running_focuses_it_instead_of_starting_another() {
        let after = reduce(running(), Action::OpenEditor);

        assert_eq!(after.focus, Panel::Editor);
        assert!(!after.editor_launch_wanted);
    }

    #[test]
    fn e_focuses_the_embedded_editor_it_starts_but_not_a_split_pane() {
        let split = state_with(Config {
            editor: EditorMode::Split,
            ..Config::default()
        });

        assert_eq!(reduce(state(), Action::OpenEditor).focus, Panel::Editor);
        assert_eq!(reduce(split, Action::OpenEditor).focus, Panel::Files);
    }

    #[test]
    fn choosing_a_file_focuses_the_embedded_editor() {
        let open = |s| {
            reduce(
                s,
                Action::OpenFile {
                    path: PathBuf::from("a.kt"),
                    line: None,
                },
            )
        };

        assert_eq!(open(state()).focus, Panel::Editor, "started for it");
        assert_eq!(open(running()).focus, Panel::Editor);

        let split = state_with(Config {
            editor: EditorMode::Split,
            ..Config::default()
        });
        assert_eq!(open(split).focus, Panel::Files, "the split pane has it");
        let mut attached = state();
        attached.editor_socket = Some(PathBuf::from("/tmp/theirs"));
        assert_eq!(open(attached).focus, Panel::Files, "an attached Neovim");
    }

    #[test]
    fn keys_and_pastes_leave_the_state_alone() {
        let before = running();

        let after = reduce(
            reduce(before.clone(), Action::EditorKeys("i".into())),
            Action::EditorPaste("x".into()),
        );

        assert_eq!(after, before);
    }
}
