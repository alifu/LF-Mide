//! Application state. Treated as immutable: the reducer returns new values.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{Config, TerminalKind};
use crate::doctor::Report;
use crate::nav::{FileList, FileMatch, GrepHit, TreeRow};
use crate::project::Project;

/// Focusable areas of the main layout, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Files,
    Editor,
    Devices,
    Bottom,
}

impl Panel {
    pub const ORDER: [Panel; 4] = [Panel::Files, Panel::Editor, Panel::Devices, Panel::Bottom];

    pub fn next(self) -> Panel {
        self.offset(1)
    }

    pub fn prev(self) -> Panel {
        self.offset(Self::ORDER.len() - 1)
    }

    fn offset(self, by: usize) -> Panel {
        let index = Self::ORDER
            .iter()
            .position(|panel| *panel == self)
            .unwrap_or(0);
        Self::ORDER[(index + by) % Self::ORDER.len()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// Branch name, or the short commit id when HEAD is detached.
    pub name: String,
    pub detached: bool,
    pub dirty: bool,
}

/// The doctor overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoctorPanel {
    Closed,
    /// Checks are running in the background.
    Running,
    Ready(Report),
}

/// The fuzzy file finder overlay.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FinderState {
    pub query: String,
    /// Best matches for `query`, filled in after each change of the query.
    pub matches: Arc<Vec<FileMatch>>,
    pub selected: usize,
}

/// The project search overlay.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GrepState {
    pub query: String,
    /// The query the current `hits` belong to, once a search was started.
    pub searched: Option<String>,
    pub hits: Arc<Vec<GrepHit>>,
    pub selected: usize,
    pub running: bool,
    /// Why the last search failed (for example an invalid pattern).
    pub error: Option<String>,
}

/// Longest query accepted in either overlay.
pub const MAX_QUERY_CHARS: usize = 200;

/// Assumed until the first resize event arrives.
pub const DEFAULT_TERMINAL_HEIGHT: u16 = 24;

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub project: Project,
    pub config: Config,
    pub branch: Option<BranchInfo>,
    /// False until the first git check has finished, so "no git repo" is
    /// not shown while it is merely unknown.
    pub branch_known: bool,
    pub focus: Panel,
    pub status: Option<String>,
    pub should_quit: bool,
    pub doctor: DoctorPanel,
    /// First visible line of the doctor report.
    pub doctor_scroll: u16,
    pub terminal_height: u16,
    /// What `TerminalKind::Auto` resolved to at startup, if anything.
    pub detected_terminal: Option<TerminalKind>,
    /// Highlighted row of the terminal picker while it is open.
    pub terminal_picker: Option<usize>,
    /// Where the Neovim we launched listens, once it is open.
    pub editor_socket: Option<PathBuf>,
    /// True once a file was shown in that Neovim, i.e. its socket is known
    /// to work; opens can then give up quickly if it has gone.
    pub editor_ready: bool,
    /// Every project file (relative paths); empty until the first scan ends.
    pub files: FileList,
    pub files_loading: bool,
    /// Directories open in the file tree (relative paths).
    pub expanded: BTreeSet<PathBuf>,
    /// The visible tree rows, derived from `files` and `expanded`.
    pub tree: Arc<Vec<TreeRow>>,
    pub tree_selected: usize,
    /// First visible tree row.
    pub tree_top: usize,
    pub finder: Option<FinderState>,
    pub grep: Option<GrepState>,
}

impl AppState {
    pub fn new(project: Project, config: Config) -> Self {
        Self {
            project,
            config,
            branch: None,
            branch_known: false,
            focus: Panel::Files,
            status: None,
            should_quit: false,
            doctor: DoctorPanel::Closed,
            doctor_scroll: 0,
            terminal_height: DEFAULT_TERMINAL_HEIGHT,
            detected_terminal: None,
            terminal_picker: None,
            editor_socket: None,
            editor_ready: false,
            files: Arc::new(Vec::new()),
            files_loading: true,
            expanded: BTreeSet::new(),
            tree: Arc::new(Vec::new()),
            tree_selected: 0,
            tree_top: 0,
            finder: None,
            grep: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_and_prev_wrap_around() {
        assert_eq!(Panel::Files.next(), Panel::Editor);
        assert_eq!(Panel::Bottom.next(), Panel::Files);
        assert_eq!(Panel::Files.prev(), Panel::Bottom);
        assert_eq!(Panel::Devices.prev(), Panel::Editor);
    }
}
