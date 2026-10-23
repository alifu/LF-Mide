//! Application state. Treated as immutable: the reducer returns new values.

use crate::config::Config;
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

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub project: Project,
    pub config: Config,
    pub branch: Option<BranchInfo>,
    pub focus: Panel,
    pub status: Option<String>,
    pub should_quit: bool,
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
