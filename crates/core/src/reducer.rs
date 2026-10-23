//! Pure state transitions.

use crate::action::Action;
use crate::state::AppState;

/// Consumes `state` and returns the state that results from applying
/// `action`. Taking ownership keeps updates pure without copying the whole
/// state on every action.
pub fn reduce(state: AppState, action: Action) -> AppState {
    match action {
        Action::Quit => AppState {
            should_quit: true,
            ..state
        },
        Action::FocusNext => AppState {
            focus: state.focus.next(),
            ..state
        },
        Action::FocusPrev => AppState {
            focus: state.focus.prev(),
            ..state
        },
        // A successful poll supersedes any earlier git error message.
        Action::BranchChanged(branch) => AppState {
            branch,
            status: None,
            ..state
        },
        Action::Status(message) => AppState {
            status: Some(message),
            ..state
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::project::{Project, ProjectLayout};
    use crate::state::{BranchInfo, Panel};
    use std::path::PathBuf;

    fn state() -> AppState {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "demo".into(),
            layout: ProjectLayout::ComposeApp,
        };
        AppState::new(project, Config::default())
    }

    #[test]
    fn quit_sets_flag() {
        let after = reduce(state(), Action::Quit);

        assert!(after.should_quit);
    }

    #[test]
    fn focus_actions_cycle_panels() {
        let start = state();

        let next = reduce(start, Action::FocusNext);
        let back = reduce(next.clone(), Action::FocusPrev);

        assert_eq!(next.focus, Panel::Devices);
        assert_eq!(back.focus, Panel::Editor);
    }

    #[test]
    fn branch_changed_replaces_branch() {
        let branch = BranchInfo {
            name: "main".into(),
            detached: false,
            dirty: true,
        };

        let after = reduce(state(), Action::BranchChanged(Some(branch.clone())));

        assert_eq!(after.branch, Some(branch));
    }

    #[test]
    fn status_sets_message() {
        let after = reduce(state(), Action::Status("git failed".into()));

        assert_eq!(after.status.as_deref(), Some("git failed"));
    }

    #[test]
    fn branch_changed_clears_previous_status() {
        let failed = reduce(state(), Action::Status("git: boom".into()));

        let recovered = reduce(failed, Action::BranchChanged(None));

        assert_eq!(recovered.status, None);
    }
}
