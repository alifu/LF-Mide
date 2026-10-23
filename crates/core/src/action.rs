//! Everything that can change [`AppState`](crate::AppState).

use std::path::PathBuf;

use crate::config::TerminalKind;
use crate::doctor::Report;
use crate::state::BranchInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    FocusNext,
    FocusPrev,
    BranchChanged(Option<BranchInfo>),
    /// A user-facing message shown in the status bar (e.g. a recoverable error).
    Status(String),
    /// Open the doctor overlay (starting a run) or close it.
    ToggleDoctor,
    /// Run the doctor again while the overlay is open.
    RefreshDoctor,
    /// A background doctor run finished.
    DoctorFinished(Report),
    /// Scroll the doctor report by this many lines (negative = up).
    DoctorScroll(i16),
    /// The terminal is now this many rows tall.
    ViewportChanged(u16),
    /// `TerminalKind::Auto` resolves to this terminal (or none).
    TerminalDetected(Option<TerminalKind>),
    OpenTerminalPicker,
    /// Move the picker highlight by this many rows (wraps around).
    TerminalPickerMove(i16),
    TerminalPickerSelect,
    TerminalPickerCancel,
    /// The user asked for Neovim in a split pane (the app performs it).
    OpenEditor,
    /// Neovim is running and listening on this socket.
    EditorOpened(PathBuf),
}
