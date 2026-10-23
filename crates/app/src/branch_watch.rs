//! Polls git in the background and reports branch changes as actions.

use std::path::PathBuf;
use std::time::Duration;

use lf_mide_core::{Action, BranchInfo};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Result of one poll; the error is kept as text so results can be compared.
pub type Poll = Result<Option<BranchInfo>, String>;

/// Decides which action (if any) a new poll result should produce, so the UI
/// only re-renders and only reports an error when something changed.
pub fn update_for(previous: Option<&Poll>, current: &Poll) -> Option<Action> {
    if previous == Some(current) {
        return None;
    }
    Some(match current {
        Ok(branch) => Action::BranchChanged(branch.clone()),
        Err(message) => Action::Status(format!("git: {message}")),
    })
}

/// Starts polling; abort the returned handle to stop it.
pub fn spawn(root: PathBuf, tx: UnboundedSender<Action>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut previous: Option<Poll> = None;
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        // A slow `git status` must not trigger back-to-back catch-up polls.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let path = root.clone();
            let current = tokio::task::spawn_blocking(move || lf_mide_vcs::branch_info(&path))
                .await
                .map_err(|err| format!("branch watcher crashed: {err}"))
                .and_then(|result| result.map_err(|err| err.to_string()));
            if let Err(message) = &current {
                tracing::warn!(%message, "reading git branch failed");
            }
            if let Some(action) = update_for(previous.as_ref(), &current)
                && tx.send(action).is_err()
            {
                return; // UI has shut down.
            }
            previous = Some(current);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn main_branch(dirty: bool) -> Poll {
        Ok(Some(BranchInfo {
            name: "main".into(),
            detached: false,
            dirty,
        }))
    }

    #[test]
    fn first_poll_always_reports() {
        assert_eq!(
            update_for(None, &main_branch(false)),
            Some(Action::BranchChanged(main_branch(false).unwrap()))
        );
    }

    #[test]
    fn unchanged_poll_reports_nothing() {
        assert_eq!(
            update_for(Some(&main_branch(false)), &main_branch(false)),
            None
        );
    }

    #[test]
    fn dirty_change_reports_branch() {
        let action = update_for(Some(&main_branch(false)), &main_branch(true));

        assert_eq!(
            action,
            Some(Action::BranchChanged(main_branch(true).unwrap()))
        );
    }

    #[test]
    fn new_error_reports_status_once() {
        let error: Poll = Err("boom".into());

        assert_eq!(
            update_for(Some(&main_branch(false)), &error),
            Some(Action::Status("git: boom".into()))
        );
        assert_eq!(update_for(Some(&error), &error), None);
    }
}
