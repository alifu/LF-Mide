//! Shared helpers for the ui tests.

use std::path::PathBuf;
use std::sync::Arc;

use lf_mide_core::{Action, AppState, Config, Project, ProjectLayout, reduce};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

use crate::render;

pub(crate) fn state() -> AppState {
    let project = Project {
        root: PathBuf::from("/p"),
        name: "demo".into(),
        layout: ProjectLayout::ComposeApp,
    };
    AppState::new(project, Config::default())
}

/// `state` after a scan that found `paths`.
pub(crate) fn with_files(state: AppState, paths: &[&str]) -> AppState {
    let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
    reduce(state, Action::FilesLoaded(Arc::new(files)))
}

pub(crate) fn buffer(state: &AppState, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| render(frame, state)).unwrap();
    terminal.backend().buffer().clone()
}

/// The rendered screen (100x24) as text, one line per row.
pub(crate) fn screen(state: &AppState) -> String {
    screen_sized(state, 100, 24)
}

pub(crate) fn screen_sized(state: &AppState, width: u16, height: u16) -> String {
    let buffer = buffer(state, width, height);
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `(column, row)` of the first occurrence of `needle` on the screen.
pub(crate) fn find_on_screen(text: &str, needle: &str) -> Option<(u16, u16)> {
    text.lines().enumerate().find_map(|(row, line)| {
        let byte = line.find(needle)?;
        let column = line[..byte].chars().count();
        Some((column as u16, row as u16))
    })
}
