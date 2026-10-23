//! Everything that can change [`AppState`](crate::AppState).

use crate::state::BranchInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    FocusNext,
    FocusPrev,
    BranchChanged(Option<BranchInfo>),
    /// A user-facing message shown in the status bar (e.g. a recoverable error).
    Status(String),
}
