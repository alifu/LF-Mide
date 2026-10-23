//! Background work started by actions: scanning the project, searching it,
//! and showing a file in Neovim. Each result comes back as an [`Action`].

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use lf_mide_core::nav::clean_relative;
use lf_mide_core::{Action, AppState, DeviceAction, DeviceCommand, GrepHit, ProjectPrefs};
use lf_mide_editor::EditorError;
use lf_mide_nav::grep::GrepError;
use lf_mide_nav::scan::Scan;
use lf_mide_platform::{Listing, PlatformError};
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

/// How often devices are listed again.
pub const DEVICE_POLL: Duration = Duration::from_secs(4);
/// Booting a simulator or stopping an emulator can take a while.
const DEVICE_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// A device listing as the action that puts it in the state.
pub fn listing_action(listing: Listing) -> Action {
    Action::DevicesLoaded {
        devices: Arc::new(listing.devices),
        errors: listing.errors,
    }
}

/// What follows a finished start/stop: a refresh if it worked, otherwise the
/// failure (which also ends the "starting..." display).
pub fn command_result_actions(
    command: &DeviceCommand,
    result: Result<(), PlatformError>,
) -> Vec<Action> {
    match result {
        Ok(()) => vec![Action::RefreshDevices],
        Err(error) => vec![Action::DeviceActionFailed {
            platform: command.device.platform,
            id: command.device.id.clone(),
            message: error.to_string(),
        }],
    }
}

/// The state without its queued device commands, and those commands.
pub fn take_commands(state: AppState) -> (AppState, Vec<DeviceCommand>) {
    let commands = state.pending_commands.clone();
    (
        AppState {
            pending_commands: Vec::new(),
            ..state
        },
        commands,
    )
}

/// The preferences to save, if they differ from `before`.
pub fn prefs_to_save(before: &ProjectPrefs, state: &AppState) -> Option<ProjectPrefs> {
    let now = state.prefs();
    (&now != before).then_some(now)
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

/// Finds out what is installed (on a thread; it runs a few tools).
pub fn spawn_sdk_info(tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        let info = lf_mide_platform::sdk_info(&lf_mide_system::SystemRunner::new());
        let _ = tx.send(Action::SdkLoaded(info));
    });
}

/// Lists emulators and simulators once, now.
pub fn spawn_device_listing(tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        let listing = lf_mide_platform::list_all(&lf_mide_system::SystemRunner::new());
        let _ = tx.send(listing_action(listing));
    });
}

/// Lists devices every [`DEVICE_POLL`] for as long as the app runs. Every
/// poll is reported (not only changes): the state counts polls to give up on
/// a device that never finished starting.
pub fn spawn_device_watch(tx: UnboundedSender<Action>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(DEVICE_POLL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let listing = tokio::task::spawn_blocking(|| {
                lf_mide_platform::list_all(&lf_mide_system::SystemRunner::new())
            })
            .await;
            let Ok(listing) = listing else { continue };
            if tx.send(listing_action(listing)).is_err() {
                return;
            }
        }
    })
}

/// Starts or stops a device on a thread and reports how it went.
pub fn spawn_device_command(command: DeviceCommand, tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        let sys = lf_mide_system::SystemRunner::with_timeout(DEVICE_COMMAND_TIMEOUT);
        let result = match command.action {
            DeviceAction::Boot => lf_mide_platform::boot(&sys, &command.device),
            DeviceAction::Stop => lf_mide_platform::stop(&sys, &command.device),
        };
        for action in command_result_actions(&command, result) {
            if tx.send(action).is_err() {
                return;
            }
        }
    });
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

    // --- devices ---

    fn pixel() -> lf_mide_core::Device {
        lf_mide_core::Device {
            platform: lf_mide_core::Platform::Android,
            id: "Pixel_8".into(),
            name: "Pixel_8".into(),
            detail: String::new(),
            state: lf_mide_core::DeviceState::Stopped,
            serial: None,
        }
    }

    fn boot_command() -> DeviceCommand {
        DeviceCommand {
            action: DeviceAction::Boot,
            device: pixel(),
        }
    }

    #[test]
    fn a_listing_becomes_a_devices_loaded_action() {
        let listing = Listing {
            devices: vec![pixel()],
            errors: std::collections::BTreeMap::from([(
                lf_mide_core::Platform::Ios,
                "no xcrun".to_string(),
            )]),
        };

        let action = listing_action(listing.clone());

        assert_eq!(
            action,
            Action::DevicesLoaded {
                devices: Arc::new(listing.devices),
                errors: listing.errors
            }
        );
    }

    #[test]
    fn a_finished_command_refreshes_and_a_failed_one_reports_why() {
        let ok = command_result_actions(&boot_command(), Ok(()));
        let failed = command_result_actions(
            &boot_command(),
            Err(PlatformError::Failed(
                "could not start Pixel_8: denied".into(),
            )),
        );
        let missing = command_result_actions(
            &boot_command(),
            Err(PlatformError::ToolMissing {
                tool: "emulator",
                hint: "install it",
            }),
        );

        assert_eq!(ok, vec![Action::RefreshDevices]);
        assert_eq!(
            failed,
            vec![Action::DeviceActionFailed {
                platform: lf_mide_core::Platform::Android,
                id: "Pixel_8".into(),
                message: "could not start Pixel_8: denied".into(),
            }]
        );
        assert!(
            matches!(&missing[0], Action::DeviceActionFailed { message, .. } if message.contains("`emulator` was not found"))
        );
    }

    #[test]
    fn queued_commands_are_taken_out_of_the_state_once() {
        let mut s = state();
        s.pending_commands = vec![boot_command()];

        let (without, commands) = take_commands(s);

        assert_eq!(commands, vec![boot_command()]);
        assert!(without.pending_commands.is_empty());
        let (_, none) = take_commands(without);
        assert!(none.is_empty());
    }

    #[test]
    fn preferences_are_saved_only_when_they_changed() {
        let s = state();
        let before = s.prefs();

        assert_eq!(prefs_to_save(&before, &s), None);
        let switched = reduce(s, Action::ToggleTarget);
        let changed = prefs_to_save(&before, &switched).unwrap();
        assert_eq!(changed.target, Some(lf_mide_core::Platform::Ios));
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
