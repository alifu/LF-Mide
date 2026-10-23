//! Everything that can change [`AppState`](crate::AppState).

use std::path::PathBuf;

use crate::config::TerminalKind;
use crate::doctor::Report;
use crate::nav::{FileList, FileMatch, GrepHit};
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
    /// Neovim could not be reached; forget it so `e` starts a new one.
    EditorLost,
    /// This project file is now showing in Neovim.
    FileOpened(PathBuf),
    /// A scan of the project finished.
    FilesLoaded(FileList),
    /// Scan the project again.
    RefreshFiles,
    /// Move the file-tree selection (negative = up).
    TreeMove(i16),
    /// Open or close the selected directory.
    TreeToggle,
    /// Open the selected directory, or step into an already open one.
    TreeExpand,
    /// Close the selected directory, or step out to its parent.
    TreeCollapse,
    /// Show this project file in Neovim, at `line` (1-based) if given.
    OpenFile {
        path: PathBuf,
        line: Option<u32>,
    },
    OpenFinder,
    FinderInput(char),
    FinderBackspace,
    /// Move the finder highlight (negative = up).
    FinderMove(i16),
    FinderCancel,
    /// Ranked matches for `query`; ignored if the query has changed since.
    FinderResults {
        query: String,
        matches: Vec<FileMatch>,
    },
    OpenGrep,
    GrepInput(char),
    GrepBackspace,
    GrepMove(i16),
    GrepCancel,
    /// Start searching for the typed query.
    GrepSearch,
    /// The outcome of the search for `query`; ignored if it is out of date.
    GrepResults {
        query: String,
        result: Result<Vec<GrepHit>, String>,
    },
}
