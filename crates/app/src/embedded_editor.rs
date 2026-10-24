//! The event loop's side of the Neovim in the editor panel: starting it,
//! passing keys, pastes and size changes on, quitting, and the cursor shape.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::cursor::SetCursorStyle;
use lf_mide_core::screen::CursorShape;
use lf_mide_core::{Action, AppState, EditorTheme};
use lf_mide_editor::embedded::{Launch, Session, SessionEvent};
use lf_mide_editor::lsp::LspServers;
use lf_mide_system::System;
use ratatui::layout::Rect;
use tokio::sync::mpsc::UnboundedSender;

use crate::paths;

/// Neovim refuses tinier grids; a smaller panel just shows part of it.
const MIN_COLUMNS: u16 = 10;
const MIN_ROWS: u16 = 3;
/// How long quitting waits for Neovim to end after SIGTERM.
const SHUTDOWN_PATIENCE: Duration = Duration::from_secs(2);

const KOTLIN_SERVER: &str = "kmp-lsp";
const SWIFT_SERVER: &str = "sourcekit-lsp";

#[derive(Default)]
pub struct EmbeddedEditor {
    session: Option<Session>,
    /// The grid size Neovim was last given.
    size: Option<(u16, u16)>,
    /// Starts so far, for a fresh socket name each time.
    starts: u32,
    cursor: Option<CursorShape>,
}

impl EmbeddedEditor {
    /// Starts Neovim in the project root, sized to the editor panel of a
    /// `screen`-sized terminal. Returns what to tell the reducer.
    pub async fn start(
        &mut self,
        state: &AppState,
        screen: Rect,
        sys: &dyn System,
        tx: UnboundedSender<Action>,
    ) -> Action {
        let dir = match paths::editor_dir(&std::env::temp_dir(), std::process::id()) {
            Ok(dir) => dir,
            Err(error) => {
                return Action::EditorLaunchFailed(format!(
                    "Cannot prepare the Neovim folder: {error}"
                ));
            }
        };
        self.starts += 1;
        // A new name each time: a socket left by a Neovim that crashed
        // would make `--listen` fail.
        let socket = socket_name(&dir, self.starts);
        let (width, height) = grid_size(screen);
        let launch = Launch {
            program: "nvim".into(),
            args: Vec::new(),
            cwd: state.project.root.clone(),
            socket: socket.clone(),
            width,
            height,
            lsp: state.config.lsp.then(|| language_servers(sys)),
            line_numbers: state.config.line_numbers,
            hyrule: state.config.editor_theme == EditorTheme::Hyrule,
        };
        match Session::start(launch, move |event| {
            for action in actions_for(event, &socket) {
                let _ = tx.send(action);
            }
        })
        .await
        {
            Ok(session) => {
                self.session = Some(session);
                self.size = Some((width, height));
                Action::EmbeddedStarted
            }
            Err(error) => Action::EditorLaunchFailed(error.to_string()),
        }
    }

    /// Passes on what this batch of actions has for Neovim.
    pub fn forward(&mut self, actions: &[Action], tx: &UnboundedSender<Action>) {
        let Some(session) = &self.session else {
            return;
        };
        // Every request is tried, so a full queue drops only what did not fit.
        let mut failure = None;
        for action in actions {
            let sent = match action {
                Action::EditorKeys(keys) => session.input(keys.clone()),
                Action::EditorPaste(text) => session.paste(text.clone()),
                Action::EditorMouse(mouse) => session.mouse(*mouse),
                _ => Ok(()),
            };
            if let Err(error) = sent {
                failure = Some(error);
            }
        }
        if let Some(error) = failure {
            let _ = tx.send(Action::Status(format!("{error}: some keys were not sent")));
        }
        if actions.iter().any(|a| matches!(a, Action::EditorExited)) {
            self.session = None;
            self.size = None;
        }
    }

    pub fn confirm_quit(&self, tx: &UnboundedSender<Action>) {
        let Some(session) = &self.session else {
            return;
        };
        if let Err(error) = session.confirm_quit() {
            let _ = tx.send(Action::EditorQuitCancelled(Some(error.to_string())));
        }
    }

    /// Keeps Neovim's grid the size of the editor panel.
    pub fn fit(&mut self, screen: Rect) {
        let Some(session) = &self.session else {
            return;
        };
        let size = grid_size(screen);
        if self.size != Some(size) && session.resize(size.0, size.1).is_ok() {
            self.size = Some(size);
        }
    }

    /// The terminal cursor takes Neovim's shape while Neovim has the keys.
    pub fn update_cursor(&mut self, state: &AppState) {
        let shape = state
            .editor_screen
            .as_ref()
            .filter(|_| state.editor_has_keys())
            .map(|screen| screen.cursor_shape());
        if shape == self.cursor {
            return;
        }
        self.cursor = shape;
        let style = match shape {
            Some(CursorShape::Vertical) => SetCursorStyle::SteadyBar,
            Some(CursorShape::Horizontal) => SetCursorStyle::SteadyUnderScore,
            Some(CursorShape::Block) => SetCursorStyle::SteadyBlock,
            None => SetCursorStyle::DefaultUserShape,
        };
        if let Err(error) = crossterm::execute!(std::io::stdout(), style) {
            tracing::debug!(%error, "cannot set the cursor shape");
        }
    }

    /// Ends Neovim (the app is closing without asking it) and waits a
    /// moment, so it can write its swap files before the app exits.
    pub async fn shut_down(&self) {
        let Some(session) = &self.session else {
            return;
        };
        session.terminate();
        if !session.wait_exit(SHUTDOWN_PATIENCE).await {
            tracing::warn!("Neovim did not end after SIGTERM");
        }
    }

    /// Leaves the terminal's own cursor shape behind.
    pub fn restore_cursor(&mut self) {
        if self.cursor.is_some() {
            let _ = crossterm::execute!(std::io::stdout(), SetCursorStyle::DefaultUserShape);
            self.cursor = None;
        }
    }
}

fn actions_for(event: SessionEvent, socket: &Path) -> Vec<Action> {
    match event {
        SessionEvent::Ready { lsp_error, .. } => {
            std::iter::once(Action::EditorOpened(socket.to_path_buf()))
                .chain(lsp_error.map(|error| Action::Status(format!("Completion is off: {error}"))))
                .collect()
        }
        SessionEvent::Screen(screen) => vec![Action::EditorScreen(screen)],
        SessionEvent::Failed(message) => vec![Action::EditorLaunchFailed(message)],
        SessionEvent::QuitCancelled { reason } => vec![Action::EditorQuitCancelled(reason)],
        SessionEvent::Exited => vec![Action::EditorExited],
    }
}

/// The grid size for the editor panel of a terminal this size.
pub fn grid_size(screen: Rect) -> (u16, u16) {
    let area = lf_mide_ui::editor_area(screen);
    (area.width.max(MIN_COLUMNS), area.height.max(MIN_ROWS))
}

/// The installed servers: kmp-lsp for Kotlin, sourcekit-lsp for Swift.
pub fn language_servers(sys: &dyn System) -> LspServers {
    LspServers {
        kotlin: sys.which(KOTLIN_SERVER),
        swift: sys.which(SWIFT_SERVER),
    }
}

/// Where the Neovim of start number `n` listens.
fn socket_name(dir: &Path, n: u32) -> PathBuf {
    dir.join(format!("embedded-{n}.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_system::fake::FakeSystem;
    use std::sync::Arc;

    #[test]
    fn session_events_become_actions() {
        let socket = Path::new("/tmp/s.sock");
        let screen = Arc::new(lf_mide_core::screen::Screen::default());
        let ready = |lsp_error| SessionEvent::Ready {
            servers: 2,
            lsp_error,
        };

        assert_eq!(
            actions_for(ready(None), socket),
            vec![Action::EditorOpened(socket.to_path_buf())]
        );
        assert_eq!(
            actions_for(ready(Some("no vim.lsp.config".into())), socket),
            vec![
                Action::EditorOpened(socket.to_path_buf()),
                Action::Status("Completion is off: no vim.lsp.config".into())
            ]
        );
        assert_eq!(
            actions_for(SessionEvent::Screen(Arc::clone(&screen)), socket),
            vec![Action::EditorScreen(screen)]
        );
        assert_eq!(
            actions_for(SessionEvent::Failed("no".into()), socket),
            vec![Action::EditorLaunchFailed("no".into())]
        );
        assert_eq!(
            actions_for(SessionEvent::QuitCancelled { reason: None }, socket),
            vec![Action::EditorQuitCancelled(None)]
        );
        assert_eq!(
            actions_for(SessionEvent::Exited, socket),
            vec![Action::EditorExited]
        );
    }

    #[test]
    fn the_grid_matches_the_panel_but_never_gets_tiny() {
        let full = grid_size(Rect::new(0, 0, 200, 50));
        let tiny = grid_size(Rect::new(0, 0, 5, 5));

        assert_eq!(full, {
            let area = lf_mide_ui::editor_area(Rect::new(0, 0, 200, 50));
            (area.width, area.height)
        });
        assert_eq!(tiny, (MIN_COLUMNS, MIN_ROWS));
    }

    #[test]
    fn only_installed_language_servers_are_used() {
        let sys = FakeSystem::new()
            .env("PATH", "/opt/bin:relative")
            .file("/opt/bin/kmp-lsp", "");

        let servers = language_servers(&sys);

        assert_eq!(servers.kotlin, Some(PathBuf::from("/opt/bin/kmp-lsp")));
        assert_eq!(servers.swift, None);
    }

    #[test]
    fn each_start_gets_its_own_socket() {
        let dir = Path::new("/tmp/lf-mide-1");

        assert_ne!(socket_name(dir, 1), socket_name(dir, 2));
    }
}
