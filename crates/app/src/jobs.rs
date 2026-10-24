//! Builds and device log streams: starts the work the reducer queued and
//! turns what it produces into actions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lf_mide_build::{
    CancelFlag, Context, ProcessStreamer, Report, Streamer, build_and_run, log_spec,
};
use lf_mide_core::{
    Action, AppState, BuildRequest, Job, LogRequest, Platform, ProjectLayout, prefs_file_name,
};
use lf_mide_system::SystemRunner;
use tokio::sync::mpsc::UnboundedSender;

/// `xcodebuild -list` and similar can be slow on a cold machine.
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);
/// Output batches that may wait for the main loop. Beyond this the job thread
/// stops reading, the pipe fills, and the program itself waits: a flood of
/// output costs time, never unbounded memory.
const MAX_QUEUED_OUTPUT: usize = 64;
/// How often a waiting job thread checks whether the app is still there.
const BACKLOG_POLL: Duration = Duration::from_millis(100);
/// Time given to child processes to hear their stop signal before we exit.
const SHUTDOWN_GRACE: Duration = Duration::from_millis(300);

/// Where Xcode puts build products for a project: a folder in LF-Mide's own
/// cache, named like the project's preferences file, never inside the repo.
pub fn derived_data_dir(cache: &Path, root: &Path, project_name: &str) -> PathBuf {
    let name = prefs_file_name(root, project_name);
    cache
        .join("derived")
        .join(Path::new(&name).with_extension(""))
}

/// The state without its queued jobs, and those jobs.
pub fn take_jobs(state: AppState) -> (AppState, Vec<Job>) {
    let jobs = state.pending_jobs.clone();
    (
        AppState {
            pending_jobs: Vec::new(),
            ..state
        },
        jobs,
    )
}

/// Counts the output batches sent to the main loop and not handled yet.
#[derive(Clone, Default)]
pub struct Backlog(Arc<(Mutex<usize>, Condvar)>);

impl Backlog {
    /// Sends `action`, first waiting while [`MAX_QUEUED_OUTPUT`] batches are
    /// already queued. False when the app has gone.
    fn send(&self, tx: &UnboundedSender<Action>, action: Action) -> bool {
        let (queued, room) = &*self.0;
        let mut queued = queued.lock().unwrap_or_else(|e| e.into_inner());
        while *queued >= MAX_QUEUED_OUTPUT {
            if tx.is_closed() {
                return false;
            }
            queued = room
                .wait_timeout(queued, BACKLOG_POLL)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *queued += 1;
        drop(queued);
        let sent = tx.send(action).is_ok();
        if !sent {
            self.release(1);
        }
        sent
    }

    /// The main loop handled these actions: makes room for as many output
    /// batches as they contained.
    pub fn consumed(&self, actions: &[Action]) {
        let batches = actions
            .iter()
            .filter(|action| {
                matches!(
                    action,
                    Action::BuildOutput { .. } | Action::LogOutput { .. }
                )
            })
            .count();
        if batches > 0 {
            self.release(batches);
        }
    }

    fn release(&self, count: usize) {
        let (queued, room) = &*self.0;
        let mut queued = queued.lock().unwrap_or_else(|e| e.into_inner());
        *queued = queued.saturating_sub(count);
        room.notify_all();
    }
}

/// What jobs need to know about the project.
#[derive(Clone)]
pub struct JobEnv {
    pub backlog: Backlog,
    pub root: PathBuf,
    pub layout: ProjectLayout,
    pub derived_data: PathBuf,
    pub tx: UnboundedSender<Action>,
}

/// The builds and log streams that are running.
#[derive(Default)]
pub struct JobRunner {
    /// Counts gutter updates so only the newest one is applied.
    marks_sequence: Arc<std::sync::atomic::AtomicU64>,
    backlog: Backlog,
    streamer: ProcessStreamer,
    build: Option<(u64, CancelFlag)>,
    logs: BTreeMap<Platform, (u64, CancelFlag)>,
}

impl JobRunner {
    pub fn backlog(&self) -> Backlog {
        self.backlog.clone()
    }

    pub fn apply(&mut self, job: Job, env: &JobEnv) {
        match job {
            Job::Build(request) => self.start_build(request, env),
            Job::CancelBuild { id } => {
                if let Some((running, cancel)) = &self.build
                    && *running == id
                {
                    cancel.cancel();
                }
            }
            Job::StartLogs(request) => self.start_logs(request, env),
            Job::LoadDiff { id, path, base } => {
                crate::git_effects::spawn_load_diff(
                    env.root.clone(),
                    id,
                    path,
                    base,
                    env.tx.clone(),
                );
            }
            Job::LoadBases => {
                crate::git_effects::spawn_load_bases(env.root.clone(), env.tx.clone())
            }
            Job::SyncMarks {
                socket,
                path,
                blame,
            } => crate::git_effects::spawn_sync_marks(
                env.root.clone(),
                socket,
                path,
                blame,
                Arc::clone(&self.marks_sequence),
                env.tx.clone(),
            ),
            Job::FollowEditor { socket } => {
                crate::git_effects::spawn_follow_editor(socket, env.tx.clone());
            }
            Job::StopProcess { pid, kind } => {
                crate::perf_effects::spawn_stop_process(pid, kind, env.tx.clone());
            }
            Job::StopLogs(platform) => {
                if let Some((_, cancel)) = self.logs.remove(&platform) {
                    cancel.cancel();
                }
            }
        }
    }

    /// Stops everything still running: asks politely, gives the stop signals
    /// a moment to take effect (they are sent from the job threads), then
    /// kills whatever is left.
    pub fn cancel_all(&mut self) {
        let mut any = false;
        if let Some((_, cancel)) = self.build.take() {
            cancel.cancel();
            any = true;
        }
        for (_, cancel) in std::mem::take(&mut self.logs).into_values() {
            cancel.cancel();
            any = true;
        }
        if any {
            std::thread::sleep(SHUTDOWN_GRACE);
            self.streamer.kill_all();
        }
    }

    fn start_build(&mut self, request: BuildRequest, env: &JobEnv) {
        let cancel = CancelFlag::default();
        self.build = Some((request.id, cancel.clone()));
        let (env, streamer) = (env.clone(), self.streamer.clone());
        std::thread::spawn(move || run_build(request, &env, &streamer, &cancel));
    }

    fn start_logs(&mut self, request: LogRequest, env: &JobEnv) {
        let cancel = CancelFlag::default();
        let platform = request.device.platform;
        if let Some((_, old)) = self.logs.insert(platform, (request.id, cancel.clone())) {
            old.cancel();
        }
        let (env, streamer) = (env.clone(), self.streamer.clone());
        std::thread::spawn(move || run_logs(request, &env, &streamer, &cancel));
    }
}

/// Sends the pipeline's progress to the app.
struct ChannelReport {
    id: u64,
    tx: UnboundedSender<Action>,
    backlog: Backlog,
}

impl Report for ChannelReport {
    fn step(&mut self, step: &str) {
        let _ = self.tx.send(Action::BuildStep {
            id: self.id,
            step: step.to_string(),
        });
    }

    fn lines(&mut self, lines: Vec<String>) {
        self.backlog
            .send(&self.tx, Action::BuildOutput { id: self.id, lines });
    }
}

fn run_build(request: BuildRequest, env: &JobEnv, streamer: &ProcessStreamer, cancel: &CancelFlag) {
    let started = Instant::now();
    let sys = SystemRunner::with_timeout(TOOL_TIMEOUT);
    let ctx = Context {
        root: &env.root,
        layout: env.layout,
        derived_data: &env.derived_data,
        device: &request.device,
    };
    let mut report = ChannelReport {
        id: request.id,
        tx: env.tx.clone(),
        backlog: env.backlog.clone(),
    };
    let outcome = build_and_run(&ctx, &sys, streamer, &mut report, cancel);
    let _ = env.tx.send(Action::BuildFinished {
        id: request.id,
        outcome,
        millis: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    });
}

fn run_logs(request: LogRequest, env: &JobEnv, streamer: &ProcessStreamer, cancel: &CancelFlag) {
    let platform = request.device.platform;
    let ended = |reason: Option<String>| {
        let _ = env.tx.send(Action::LogEnded {
            platform,
            id: request.id,
            reason,
        });
    };
    let sys = SystemRunner::with_timeout(TOOL_TIMEOUT);
    let spec = match log_spec(&request.device, &env.derived_data, &sys) {
        Ok(spec) => spec,
        Err(reason) => return ended(Some(reason)),
    };
    let exit = streamer.stream(&spec, cancel, &mut |lines| {
        env.backlog.send(
            &env.tx,
            Action::LogOutput {
                platform,
                id: request.id,
                lines,
            },
        );
    });
    match exit {
        _ if cancel.is_cancelled() => ended(None),
        Ok(exit) => ended(Some(match exit.code {
            Some(code) => format!("exited with code {code}"),
            None => "stopped by a signal".to_string(),
        })),
        Err(error) => ended(Some(format!("could not start: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use lf_mide_core::{Config, Device, DeviceState, Project};

    use super::*;

    fn state() -> AppState {
        AppState::new(
            Project {
                root: PathBuf::from("/p"),
                name: "demo".into(),
                layout: ProjectLayout::ComposeApp,
            },
            Config::default(),
        )
    }

    #[test]
    fn derived_data_lives_in_our_cache_under_a_name_unique_to_the_project() {
        let a = derived_data_dir(Path::new("/cache"), Path::new("/a/proj"), "proj");
        let b = derived_data_dir(Path::new("/cache"), Path::new("/b/proj"), "proj");

        assert!(a.starts_with("/cache/derived"));
        assert_ne!(a, b);
        assert_eq!(a.extension(), None);
        assert!(!a.to_string_lossy().ends_with(".toml"));
    }

    fn output(id: u64) -> Action {
        Action::BuildOutput {
            id,
            lines: vec!["x".into()],
        }
    }

    #[test]
    fn output_waits_for_room_once_too_many_batches_are_queued() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let backlog = Backlog::default();
        for n in 0..MAX_QUEUED_OUTPUT as u64 {
            assert!(backlog.send(&tx, output(n)));
        }
        let (sender, sent) = (backlog.clone(), tx.clone());
        let blocked = std::thread::spawn(move || sender.send(&sent, output(999)));

        std::thread::sleep(Duration::from_millis(300));
        assert!(!blocked.is_finished(), "the extra batch should be waiting");
        let mut handled = Vec::new();
        while let Ok(action) = rx.try_recv() {
            handled.push(action);
        }
        backlog.consumed(&handled);

        assert!(blocked.join().unwrap());
        assert!(matches!(
            rx.try_recv(),
            Ok(Action::BuildOutput { id: 999, .. })
        ));
    }

    #[test]
    fn only_output_batches_count_against_the_backlog() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let backlog = Backlog::default();
        assert!(backlog.send(&tx, output(1)));

        backlog.consumed(&[Action::FocusNext, Action::Quit]);

        assert_eq!(*backlog.0.0.lock().unwrap(), 1);
        backlog.consumed(&[output(1), output(2)]);
        assert_eq!(*backlog.0.0.lock().unwrap(), 0, "never below zero");
    }

    #[test]
    fn a_waiting_job_gives_up_when_the_app_is_gone() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let backlog = Backlog::default();
        for n in 0..MAX_QUEUED_OUTPUT as u64 {
            backlog.send(&tx, output(n));
        }
        let (sender, sent) = (backlog.clone(), tx.clone());
        let waiting = std::thread::spawn(move || sender.send(&sent, output(1)));
        std::thread::sleep(Duration::from_millis(200));

        drop(rx);

        assert!(!waiting.join().unwrap());
    }

    #[test]
    fn taking_jobs_empties_the_queue() {
        let mut s = state();
        s.pending_jobs = vec![Job::StopLogs(Platform::Ios), Job::CancelBuild { id: 3 }];

        let (after, jobs) = take_jobs(s);

        assert!(after.pending_jobs.is_empty());
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn a_missing_log_tool_ends_the_stream_with_a_reason() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let env = JobEnv {
            backlog: Backlog::default(),
            root: PathBuf::from("/nonexistent"),
            layout: ProjectLayout::ComposeApp,
            derived_data: PathBuf::from("/nonexistent/derived"),
            tx,
        };
        let device = Device {
            platform: Platform::Ios,
            id: "23F0A11F-0DD8-4766-853D-987DE9852BAC".into(),
            name: "iPhone".into(),
            detail: String::new(),
            state: DeviceState::Running,
            serial: None,
        };

        run_logs(
            LogRequest { id: 9, device },
            &env,
            &ProcessStreamer::default(),
            &CancelFlag::default(),
        );

        match rx.try_recv() {
            Ok(Action::LogEnded {
                platform: Platform::Ios,
                id: 9,
                reason: Some(reason),
            }) => assert!(reason.contains("build the app first"), "{reason}"),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
