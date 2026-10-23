//! Background work started by actions: scanning the project, searching it,
//! and showing a file in Neovim. Each result comes back as an [`Action`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lf_mide_core::nav::clean_relative;
use lf_mide_core::{Action, AppState, GrepHit};
use lf_mide_editor::EditorError;
use lf_mide_nav::grep::GrepError;
use lf_mide_nav::scan::Scan;
use tokio::sync::mpsc::UnboundedSender;

/// Finder results kept (the list scrolls, but nobody reads 5000 matches).
pub const FINDER_LIMIT: usize = 200;
/// Search hits kept.
pub const GREP_LIMIT: usize = 500;
/// How long to wait for a just-opened Neovim to start listening.
pub const OPEN_PATIENCE: Duration = Duration::from_secs(8);
/// After the editor has answered once, a missing socket means it is gone.
const READY_PATIENCE: Duration = Duration::from_secs(1);

/// Opens go through one at a time, in the order they were chosen, so two
/// quick choices can never leave the earlier file showing.
static OPEN_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The actions that follow a project scan.
pub fn scan_actions(result: io::Result<Scan>) -> Vec<Action> {
    match result {
        Ok(scan) => {
            let count = scan.files.len();
            let mut actions = vec![Action::FilesLoaded(Arc::new(scan.files))];
            if scan.truncated {
                actions.push(Action::Status(format!(
                    "Showing only part of a very large project ({count} files)"
                )));
            }
            actions
        }
        // Loading must still end, or the tree would say "Scanning…" forever.
        Err(error) => vec![
            Action::FilesLoaded(Arc::new(Vec::new())),
            Action::Status(format!("Cannot read the project folder: {error}")),
        ],
    }
}

pub fn grep_result_action(query: String, result: Result<Vec<GrepHit>, GrepError>) -> Action {
    Action::GrepResults {
        query,
        result: result.map_err(|error| error.to_string()),
    }
}

pub fn open_result_action(path: &Path, result: Result<String, EditorError>) -> Action {
    match result {
        Ok(_) => Action::FileOpened(path.to_path_buf()),
        // The pane was closed: forget the socket so `e` starts a new one.
        Err(EditorError::NotRunning(_)) => Action::EditorLost,
        Err(other) => Action::Status(other.to_string()),
    }
}

/// How long to keep trying to reach Neovim. A fresh editor needs time to
/// start listening; one that already answered once is either there or gone.
pub fn open_patience(editor_ready: bool) -> Duration {
    if editor_ready {
        READY_PATIENCE
    } else {
        OPEN_PATIENCE
    }
}

/// A rescan is only worth starting when none is already running: two scans
/// could otherwise finish out of order and show the older list.
pub fn should_start_scan(refresh_requested: bool, scan_running: bool) -> bool {
    refresh_requested && !scan_running
}

/// Runs at most one ripgrep at a time. While one runs, only the latest
/// requested query is remembered; it starts when the running one ends.
#[derive(Debug, Default)]
pub struct GrepScheduler {
    in_flight: bool,
    pending: Option<String>,
}

impl GrepScheduler {
    /// A search for `query` is wanted: returns it if it should start now.
    pub fn request(&mut self, query: String) -> Option<String> {
        if self.in_flight {
            self.pending = Some(query);
            return None;
        }
        self.in_flight = true;
        Some(query)
    }

    /// The running search ended: returns the next query to start, if any.
    pub fn finished(&mut self) -> Option<String> {
        // A queued query runs next, so the scheduler stays busy for it.
        self.in_flight = self.pending.is_some();
        self.pending.take()
    }
}

/// The absolute path for the project file `relative`, or `None` if it is not
/// a plain path inside `root` (absolute, or containing `..`). `Path::join`
/// would otherwise let such a path replace or leave the root.
pub fn project_file(root: &Path, relative: &Path) -> Option<PathBuf> {
    Some(root.join(clean_relative(relative)?))
}

/// The finder query to rank files for, if it is new since `before`.
pub fn finder_query_to_rank(before: Option<&str>, state: &AppState) -> Option<String> {
    let finder = state.finder.as_ref()?;
    (before != Some(finder.query.as_str())).then(|| finder.query.clone())
}

/// The query of a search that has just started, given what had been
/// searched `before`.
pub fn grep_query_to_run(before: Option<&str>, state: &AppState) -> Option<String> {
    let grep = state.grep.as_ref().filter(|grep| grep.running)?;
    let query = grep.searched.as_deref()?;
    (Some(query) != before).then(|| query.to_string())
}

/// Scans the project on a thread and reports the result.
pub fn spawn_scan(root: PathBuf, tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        for action in scan_actions(lf_mide_nav::scan::scan(&root)) {
            if tx.send(action).is_err() {
                return;
            }
        }
    });
}

/// Searches the project with ripgrep on a thread.
pub fn spawn_grep(root: PathBuf, query: String, tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        let sys = lf_mide_system::SystemRunner::new();
        let result = lf_mide_nav::grep::grep(&sys, &root, &query, GREP_LIMIT);
        let _ = tx.send(grep_result_action(query, result));
    });
}

/// Shows `path` (relative to `root`) in the Neovim on `socket`.
pub fn spawn_open(
    root: PathBuf,
    socket: PathBuf,
    path: PathBuf,
    line: Option<u32>,
    patience: Duration,
    tx: UnboundedSender<Action>,
) {
    let Some(target) = project_file(&root, &path) else {
        let _ = tx.send(Action::Status(
            "Refusing to open a path outside the project".to_string(),
        ));
        return;
    };
    tokio::spawn(async move {
        let _turn = OPEN_TURN.lock().await;
        // `?` (Debug) escapes control characters, so a hostile file name
        // cannot forge log lines.
        tracing::info!(socket = ?socket, file = ?target, ?line, "opening file in Neovim");
        let result = lf_mide_editor::open_file(&socket, &target, line, 0, patience).await;
        tracing::info!(?result, "open file finished");
        let _ = tx.send(open_result_action(&path, result));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_core::{Config, Project, ProjectLayout, reduce};

    fn state() -> AppState {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "demo".into(),
            layout: ProjectLayout::ComposeApp,
        };
        AppState::new(project, Config::default())
    }

    fn files(action: &Action) -> Vec<String> {
        let Action::FilesLoaded(list) = action else {
            panic!("expected FilesLoaded, got {action:?}")
        };
        list.iter().map(|p| p.display().to_string()).collect()
    }

    #[test]
    fn a_finished_scan_loads_the_files() {
        let scan = Scan {
            files: vec![PathBuf::from("a.kt"), PathBuf::from("b/c.kt")],
            truncated: false,
        };

        let actions = scan_actions(Ok(scan));

        assert_eq!(actions.len(), 1);
        assert_eq!(files(&actions[0]), vec!["a.kt", "b/c.kt"]);
    }

    #[test]
    fn a_truncated_scan_says_so() {
        let scan = Scan {
            files: vec![PathBuf::from("a.kt")],
            truncated: true,
        };

        let actions = scan_actions(Ok(scan));

        assert_eq!(actions.len(), 2);
        assert!(
            matches!(&actions[1], Action::Status(m) if m.contains("very large") && m.contains('1')),
            "{actions:?}"
        );
    }

    #[test]
    fn a_failed_scan_ends_loading_with_an_empty_list_and_a_reason() {
        let actions = scan_actions(Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "denied",
        )));

        assert!(files(&actions[0]).is_empty());
        assert!(matches!(&actions[1], Action::Status(m) if m.contains("denied")));
    }

    #[test]
    fn search_results_and_errors_become_one_action_each() {
        let hit = GrepHit {
            path: PathBuf::from("a.kt"),
            line: 1,
            column: 0,
            text: "t".into(),
        };

        let ok = grep_result_action("q".into(), Ok(vec![hit.clone()]));
        let err = grep_result_action(
            "(".into(),
            Err(GrepError::Failed("error: unclosed group".into())),
        );
        let missing = grep_result_action("q".into(), Err(GrepError::RgMissing));

        assert_eq!(
            ok,
            Action::GrepResults {
                query: "q".into(),
                result: Ok(vec![hit])
            }
        );
        assert_eq!(
            err,
            Action::GrepResults {
                query: "(".into(),
                result: Err("error: unclosed group".into())
            }
        );
        assert!(
            matches!(missing, Action::GrepResults { result: Err(m), .. } if m.contains("brew install ripgrep"))
        );
    }

    #[test]
    fn opening_reports_success_loss_and_rejection() {
        let path = Path::new("src/App.kt");

        let ok = open_result_action(path, Ok("/p/src/App.kt".into()));
        let lost = open_result_action(path, Err(EditorError::NotRunning("/tmp/s".into())));
        let rejected = open_result_action(path, Err(EditorError::Rejected("E37".into())));
        let slow = open_result_action(path, Err(EditorError::TimedOut));

        assert_eq!(ok, Action::FileOpened(PathBuf::from("src/App.kt")));
        assert_eq!(lost, Action::EditorLost);
        assert_eq!(rejected, Action::Status("Neovim: E37".into()));
        assert!(matches!(slow, Action::Status(m) if m.contains("did not answer")));
    }

    #[test]
    fn a_proven_editor_gets_a_short_wait_and_a_fresh_one_a_long_wait() {
        assert_eq!(open_patience(false), OPEN_PATIENCE);
        assert!(open_patience(true) < Duration::from_secs(2));
        assert!(open_patience(true) < open_patience(false));
    }

    #[test]
    fn a_rescan_starts_only_when_asked_and_none_is_running() {
        assert!(should_start_scan(true, false));
        assert!(!should_start_scan(true, true), "one scan at a time");
        assert!(!should_start_scan(false, false));
        assert!(!should_start_scan(false, true));
    }

    #[test]
    fn the_first_search_starts_at_once() {
        let mut scheduler = GrepScheduler::default();

        assert_eq!(scheduler.request("a".into()), Some("a".into()));
    }

    #[test]
    fn searches_requested_while_one_runs_collapse_to_the_latest() {
        let mut scheduler = GrepScheduler::default();
        scheduler.request("a".into());

        assert_eq!(scheduler.request("b".into()), None);
        assert_eq!(scheduler.request("c".into()), None);
        assert_eq!(
            scheduler.finished(),
            Some("c".into()),
            "only the latest, once the first ends"
        );
    }

    #[test]
    fn the_scheduler_goes_idle_when_nothing_is_waiting() {
        let mut scheduler = GrepScheduler::default();
        scheduler.request("a".into());

        assert_eq!(scheduler.finished(), None);
        assert_eq!(
            scheduler.request("b".into()),
            Some("b".into()),
            "idle again, so it starts at once"
        );
    }

    #[test]
    fn a_queued_search_keeps_the_scheduler_busy_until_it_too_finishes() {
        let mut scheduler = GrepScheduler::default();
        scheduler.request("a".into());
        scheduler.request("b".into());
        assert_eq!(scheduler.finished(), Some("b".into()));

        assert_eq!(scheduler.request("c".into()), None, "b is running now");
        assert_eq!(scheduler.finished(), Some("c".into()));
        assert_eq!(scheduler.finished(), None);
    }

    #[test]
    fn only_plain_relative_paths_resolve_inside_the_project() {
        let root = Path::new("/work/pos");

        assert_eq!(
            project_file(root, Path::new("src/App.kt")),
            Some(PathBuf::from("/work/pos/src/App.kt"))
        );
        assert_eq!(
            project_file(root, Path::new("./a.kt")),
            Some(PathBuf::from("/work/pos/a.kt"))
        );
        assert_eq!(
            project_file(root, Path::new("/etc/passwd")),
            None,
            "join would replace the root"
        );
        assert_eq!(project_file(root, Path::new("../secret")), None);
        assert_eq!(project_file(root, Path::new("a/../../b")), None);
        assert_eq!(project_file(root, Path::new("")), None);
    }

    #[test]
    fn the_finder_is_ranked_when_opened_and_whenever_the_query_changes() {
        let closed = state();
        let opened = reduce(closed.clone(), Action::OpenFinder);
        let typed = reduce(opened.clone(), Action::FinderInput('a'));

        assert_eq!(finder_query_to_rank(None, &closed), None);
        assert_eq!(
            finder_query_to_rank(None, &opened),
            Some(String::new()),
            "just opened"
        );
        assert_eq!(finder_query_to_rank(Some(""), &opened), None, "unchanged");
        assert_eq!(finder_query_to_rank(Some(""), &typed), Some("a".into()));
        assert_eq!(
            finder_query_to_rank(Some("a"), &closed),
            None,
            "closed meanwhile"
        );
    }

    #[test]
    fn a_search_runs_once_when_it_starts() {
        let typed = reduce(reduce(state(), Action::OpenGrep), Action::GrepInput('x'));
        let started = reduce(typed.clone(), Action::GrepSearch);

        assert_eq!(
            grep_query_to_run(None, &typed),
            None,
            "typing alone does not search"
        );
        assert_eq!(grep_query_to_run(None, &started), Some("x".into()));
        assert_eq!(
            grep_query_to_run(Some("x"), &started),
            None,
            "already running that search"
        );
        assert_eq!(grep_query_to_run(None, &state()), None);
    }

    #[test]
    fn a_finished_search_is_never_started_again() {
        let started = reduce(
            reduce(reduce(state(), Action::OpenGrep), Action::GrepInput('x')),
            Action::GrepSearch,
        );
        let done = reduce(
            started,
            Action::GrepResults {
                query: "x".into(),
                result: Ok(vec![]),
            },
        );

        assert_eq!(grep_query_to_run(None, &done), None);
    }
}
