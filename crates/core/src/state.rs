//! Application state. Treated as immutable: the reducer returns new values.

use std::path::PathBuf;

use crate::config::{Config, TerminalKind};
use crate::doctor::Report;
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

/// Assumed until the first resize event arrives.
pub const DEFAULT_TERMINAL_HEIGHT: u16 = 24;

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub project: Project,
    pub config: Config,
    pub branch: Option<BranchInfo>,
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
}

impl AppState {
    pub fn new(project: Project, config: Config) -> Self {
        Self {
            project,
            config,
            branch: None,
            focus: Panel::Editor,
            status: None,
            should_quit: false,
            doctor: DoctorPanel::Closed,
            doctor_scroll: 0,
            terminal_height: DEFAULT_TERMINAL_HEIGHT,
            detected_terminal: None,
            terminal_picker: None,
            editor_socket: None,
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
