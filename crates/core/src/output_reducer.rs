//! Reducer logic for builds, device logs and the Output panel.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::devices::{Device, DeviceState};
use crate::layout::OUTPUT_VIEWPORT_ROWS;
use crate::logparse::{BuildLine, parse_build_line};
use crate::nav::clean_relative;
use crate::output::{
    BuildOutcome, BuildPhase, BuildRequest, BuildState, Job, LogBuffer, LogRequest, MAX_PROBLEMS,
    OutputTab, Severity,
};
use crate::state::{AppState, LogStream, MAX_QUERY_CHARS};

impl AppState {
    /// The log shown in a tab, for the tabs that are plain logs.
    pub fn log(&self, tab: OutputTab) -> Option<&Arc<LogBuffer>> {
        match tab {
            OutputTab::Build => Some(&self.build_log),
            OutputTab::Logcat => Some(&self.logcat),
            OutputTab::IosLog => Some(&self.ios_log),
            OutputTab::Problems | OutputTab::Git => None,
        }
    }

    fn log_mut(&mut self, tab: OutputTab) -> Option<&mut LogBuffer> {
        let log = match tab {
            OutputTab::Build => &mut self.build_log,
            OutputTab::Logcat => &mut self.logcat,
            OutputTab::IosLog => &mut self.ios_log,
            OutputTab::Problems | OutputTab::Git => return None,
        };
        Some(Arc::make_mut(log))
    }

    /// Where the highlighted problem is, as a path inside the project and a
    /// line, or why it cannot be opened.
    pub fn selected_problem_target(&self) -> Result<(PathBuf, Option<u32>), String> {
        let problem = self
            .problems
            .get(self.problem_selected)
            .ok_or_else(|| "No problem selected".to_string())?;
        let path = problem
            .path
            .as_deref()
            .ok_or_else(|| "This problem has no source location".to_string())?;
        project_relative(&self.project.root, Path::new(path))
            .map(|relative| (relative, problem.line))
            .ok_or_else(|| format!("{path} is outside the project"))
    }
}

/// `path` relative to `root`, if it is inside it (no `..` tricks).
fn project_relative(root: &Path, path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return clean_relative(path);
    }
    clean_relative(path.strip_prefix(root).ok()?)
}

fn say(state: AppState, message: impl Into<String>) -> AppState {
    AppState {
        status: Some(message.into()),
        ..state
    }
}

/// Adds lines to a tab's log. A reader scrolled back keeps looking at the
/// same lines instead of being dragged along by new output.
fn append(mut state: AppState, tab: OutputTab, lines: Vec<String>) -> AppState {
    let added = lines.len();
    if let Some(log) = state.log_mut(tab) {
        log.push_lines(lines);
        let len = log.len();
        let scroll = &mut state.output_scroll[tab.index()];
        if *scroll > 0 {
            *scroll = (*scroll + added).min(len.saturating_sub(OUTPUT_VIEWPORT_ROWS));
        }
    }
    state
}

pub(crate) fn run_app(state: AppState) -> AppState {
    if state.build.phase == BuildPhase::Running {
        return say(state, "A build is already running: press s to cancel it");
    }
    let Some(device) = state.selected_device().cloned() else {
        return say(state, "No device selected: press d, then pick one");
    };
    if device.state != DeviceState::Running {
        let message = format!(
            "{} is not running: press d, then b to start it",
            device.name
        );
        return say(state, message);
    }
    let id = state.last_job_id + 1;
    let mut pending_jobs = state.pending_jobs.clone();
    pending_jobs.push(Job::Build(BuildRequest {
        id,
        device: device.clone(),
    }));
    let mut output_scroll = state.output_scroll;
    output_scroll[OutputTab::Build.index()] = 0;
    AppState {
        status: Some(format!("Building for {}…", device.name)),
        build: BuildState {
            phase: BuildPhase::Running,
            platform: Some(device.platform),
            id,
            step: "Building".to_string(),
            device: Some(device.name),
            ..BuildState::default()
        },
        build_log: Arc::new(LogBuffer::default()),
        problems: Arc::new(Vec::new()),
        problem_selected: 0,
        output_tab: OutputTab::Build,
        output_scroll,
        last_job_id: id,
        pending_jobs,
        ..state
    }
}

pub(crate) fn cancel_build(state: AppState) -> AppState {
    if state.build.phase != BuildPhase::Running {
        return say(state, "No build is running");
    }
    let mut pending_jobs = state.pending_jobs.clone();
    pending_jobs.push(Job::CancelBuild { id: state.build.id });
    AppState {
        status: Some("Cancelling the build…".to_string()),
        pending_jobs,
        ..state
    }
}

fn is_current_build(state: &AppState, id: u64) -> bool {
    state.build.id == id && state.build.phase == BuildPhase::Running
}

pub(crate) fn build_step(state: AppState, id: u64, step: String) -> AppState {
    if !is_current_build(&state, id) {
        return state;
    }
    AppState {
        build: BuildState {
            step,
            task: None,
            ..state.build.clone()
        },
        ..state
    }
}

pub(crate) fn build_output(mut state: AppState, id: u64, lines: Vec<String>) -> AppState {
    if !is_current_build(&state, id) {
        return state;
    }
    let mut build = state.build.clone();
    let mut problems = Arc::clone(&state.problems);
    for line in &lines {
        match parse_build_line(line) {
            BuildLine::Task(task) => build.task = Some(task),
            BuildLine::Problem(problem) => {
                let known = problems.contains(&problem);
                if !known && problems.len() < MAX_PROBLEMS {
                    match problem.severity {
                        Severity::Error => build.errors += 1,
                        Severity::Warning => build.warnings += 1,
                    }
                    Arc::make_mut(&mut problems).push(problem);
                }
            }
            BuildLine::Finished { .. } | BuildLine::Other => {}
        }
    }
    state.build = build;
    state.problems = problems;
    append(state, OutputTab::Build, lines)
}

pub(crate) fn build_finished(
    state: AppState,
    id: u64,
    outcome: BuildOutcome,
    millis: u64,
) -> AppState {
    if !is_current_build(&state, id) {
        return state;
    }
    let device_name = state.build.device.clone().unwrap_or_default();
    let seconds = millis as f64 / 1000.0;
    let (phase, summary, status) = match &outcome {
        BuildOutcome::Succeeded => (
            BuildPhase::Succeeded,
            format!("— {device_name}: done in {seconds:.1}s —"),
            format!("Build succeeded in {seconds:.1}s"),
        ),
        BuildOutcome::Failed(reason) => {
            let why = reason.clone().unwrap_or_else(|| "build failed".to_string());
            (
                BuildPhase::Failed,
                format!("— {why} —"),
                format!("Build failed: {why}"),
            )
        }
        BuildOutcome::Cancelled => (
            BuildPhase::Cancelled,
            "— cancelled —".to_string(),
            "Build cancelled".to_string(),
        ),
    };
    let had_errors = state.build.errors > 0;
    let device = state
        .devices
        .iter()
        .find(|d| {
            d.platform == state.build.platform.unwrap_or(state.target) && d.name == device_name
        })
        .cloned();
    let mut state = append(
        AppState {
            build: BuildState {
                phase,
                task: None,
                millis: Some(millis),
                ..state.build.clone()
            },
            status: Some(status),
            ..state
        },
        OutputTab::Build,
        vec![summary],
    );
    match (phase, device) {
        (BuildPhase::Succeeded, Some(device)) => {
            state = follow_logs(state, &device);
        }
        (BuildPhase::Failed, _) if had_errors => {
            state.output_tab = OutputTab::Problems;
            state.output_filter.editing = false;
        }
        _ => {}
    }
    state
}

/// Starts following `device`'s log, replacing another device's on the same
/// platform, unless it is already followed.
fn follow_logs(state: AppState, device: &Device) -> AppState {
    if state
        .log_streams
        .get(&device.platform)
        .is_some_and(|stream| stream.device == device.name)
    {
        return AppState {
            output_tab: OutputTab::log_of(device.platform),
            ..state
        };
    }
    let id = state.last_job_id + 1;
    let tab = OutputTab::log_of(device.platform);
    let mut pending_jobs = state.pending_jobs.clone();
    pending_jobs.push(Job::StartLogs(LogRequest {
        id,
        device: device.clone(),
    }));
    let mut log_streams = state.log_streams.clone();
    log_streams.insert(
        device.platform,
        LogStream {
            id,
            device: device.name.clone(),
        },
    );
    let marker = format!("— following {} —", device.name);
    let mut state = append(
        AppState {
            last_job_id: id,
            pending_jobs,
            log_streams,
            ..state
        },
        tab,
        vec![marker],
    );
    state.output_tab = tab;
    state.output_filter.editing = false;
    state
}

pub(crate) fn toggle_logs(state: AppState) -> AppState {
    let Some(device) = state.selected_device().cloned() else {
        return say(state, "No device selected: press d, then pick one");
    };
    let followed = state
        .log_streams
        .get(&device.platform)
        .is_some_and(|stream| stream.device == device.name);
    if followed {
        let mut log_streams = state.log_streams.clone();
        log_streams.remove(&device.platform);
        let mut pending_jobs = state.pending_jobs.clone();
        pending_jobs.push(Job::StopLogs(device.platform));
        return AppState {
            status: Some(format!("Stopped following {}", device.name)),
            log_streams,
            pending_jobs,
            ..state
        };
    }
    if device.state != DeviceState::Running {
        let message = format!("{} is not running: there is no log to follow", device.name);
        return say(state, message);
    }
    follow_logs(state, &device)
}

pub(crate) fn log_output(
    state: AppState,
    platform: crate::devices::Platform,
    id: u64,
    lines: Vec<String>,
) -> AppState {
    if state.log_streams.get(&platform).map(|s| s.id) != Some(id) {
        return state;
    }
    append(state, OutputTab::log_of(platform), lines)
}

pub(crate) fn log_ended(
    state: AppState,
    platform: crate::devices::Platform,
    id: u64,
    reason: Option<String>,
) -> AppState {
    if state.log_streams.get(&platform).map(|s| s.id) != Some(id) {
        return state;
    }
    let mut log_streams = state.log_streams.clone();
    log_streams.remove(&platform);
    let line = match reason {
        Some(reason) => format!("— log stream ended: {reason} —"),
        None => "— log stream ended —".to_string(),
    };
    append(
        AppState {
            log_streams,
            ..state
        },
        OutputTab::log_of(platform),
        vec![line],
    )
}

pub(crate) fn show_tab(state: AppState, tab: OutputTab) -> AppState {
    AppState {
        output_tab: tab,
        ..state
    }
}

pub(crate) fn scroll(state: AppState, delta: i32) -> AppState {
    let tab = state.output_tab;
    if tab == OutputTab::Problems {
        let last = state.problems.len().saturating_sub(1) as i64;
        let selected = (state.problem_selected as i64 - i64::from(delta)).clamp(0, last);
        return AppState {
            problem_selected: selected as usize,
            ..state
        };
    }
    let Some(len) = state.log(tab).map(|log| log.len()) else {
        return state;
    };
    let max = len.saturating_sub(OUTPUT_VIEWPORT_ROWS) as i64;
    let scrolled = (state.output_scroll[tab.index()] as i64 + i64::from(delta)).clamp(0, max);
    let mut output_scroll = state.output_scroll;
    output_scroll[tab.index()] = scrolled as usize;
    AppState {
        output_scroll,
        ..state
    }
}

pub(crate) fn follow(state: AppState) -> AppState {
    let mut output_scroll = state.output_scroll;
    output_scroll[state.output_tab.index()] = 0;
    AppState {
        output_scroll,
        ..state
    }
}

fn with_filter(state: AppState, change: impl FnOnce(&mut crate::output::OutputFilter)) -> AppState {
    let mut output_filter = state.output_filter.clone();
    change(&mut output_filter);
    AppState {
        output_filter,
        ..state
    }
}

pub(crate) fn filter_open(state: AppState) -> AppState {
    if !matches!(
        state.output_tab,
        OutputTab::Build | OutputTab::Logcat | OutputTab::IosLog
    ) {
        return say(state, "Filtering works on the log tabs");
    }
    with_filter(state, |filter| filter.editing = true)
}

pub(crate) fn filter_input(state: AppState, ch: char) -> AppState {
    if !state.output_filter.editing
        || ch.is_control()
        || state.output_filter.text.chars().count() >= MAX_QUERY_CHARS
    {
        return state;
    }
    with_filter(state, |filter| filter.text.push(ch))
}

pub(crate) fn filter_backspace(state: AppState) -> AppState {
    if !state.output_filter.editing {
        return state;
    }
    with_filter(state, |filter| {
        filter.text.pop();
    })
}

pub(crate) fn filter_done(state: AppState) -> AppState {
    with_filter(state, |filter| filter.editing = false)
}

pub(crate) fn filter_clear(state: AppState) -> AppState {
    with_filter(state, |filter| {
        filter.text.clear();
        filter.editing = false;
    })
}

pub(crate) fn cycle_level(state: AppState) -> AppState {
    if state.output_tab != OutputTab::Logcat {
        return say(state, "Levels apply to the Logcat tab");
    }
    with_filter(state, |filter| filter.min_level = filter.min_level.cycled())
}

pub(crate) fn clear(mut state: AppState) -> AppState {
    let tab = state.output_tab;
    if tab == OutputTab::Problems {
        state.problems = Arc::new(Vec::new());
        state.problem_selected = 0;
        return state;
    }
    if let Some(log) = state.log_mut(tab) {
        log.clear();
    }
    state.output_scroll[tab.index()] = 0;
    state
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::action::Action;
    use crate::config::Config;
    use crate::devices::Platform;
    use crate::output::Problem;
    use crate::project::{Project, ProjectLayout};
    use crate::reducer::reduce;

    fn device(platform: Platform, id: &str, name: &str, state: DeviceState) -> Device {
        Device {
            platform,
            id: id.into(),
            name: name.into(),
            detail: String::new(),
            state,
            serial: (platform == Platform::Android).then(|| "emulator-5554".to_string()),
        }
    }

    fn with(devices: Vec<Device>) -> AppState {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "demo".into(),
            layout: ProjectLayout::AndroidAppShared,
        };
        reduce(
            AppState::new(project, Config::default()),
            Action::DevicesLoaded {
                devices: Arc::new(devices),
                errors: BTreeMap::new(),
            },
        )
    }

    fn running_android() -> AppState {
        with(vec![device(
            Platform::Android,
            "Pixel_8",
            "Pixel_8",
            DeviceState::Running,
        )])
    }

    fn building() -> AppState {
        reduce(running_android(), Action::RunApp)
    }

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn output(id: u64, items: &[&str]) -> Action {
        Action::BuildOutput {
            id,
            lines: lines(items),
        }
    }

    // --- starting and cancelling a build ---

    #[test]
    fn running_the_app_queues_a_build_for_the_chosen_running_device() {
        let after = building();

        assert_eq!(after.build.phase, BuildPhase::Running);
        assert_eq!(after.build.id, 1);
        assert_eq!(after.build.platform, Some(Platform::Android));
        assert_eq!(after.output_tab, OutputTab::Build);
        assert!(matches!(
            &after.pending_jobs[..],
            [Job::Build(BuildRequest { id: 1, device })] if device.id == "Pixel_8"
        ));
    }

    #[test]
    fn running_clears_the_previous_builds_output_and_problems() {
        let mut s = building();
        s = reduce(s, output(1, &["e: /p/A.kt:1:1 boom", "text"]));
        s = reduce(
            s,
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Failed(None),
                millis: 10,
            },
        );

        let again = reduce(s, Action::RunApp);

        assert!(again.build_log.is_empty() && again.problems.is_empty());
        assert_eq!((again.build.errors, again.build.id), (0, 2));
    }

    #[test]
    fn running_without_a_running_device_explains_what_to_do() {
        let stopped = with(vec![device(
            Platform::Android,
            "Pixel_8",
            "Pixel_8",
            DeviceState::Stopped,
        )]);
        let none = with(Vec::new());

        let a = reduce(stopped, Action::RunApp);
        let b = reduce(none, Action::RunApp);

        assert!(a.status.as_deref().unwrap().contains("not running"));
        assert!(b.status.as_deref().unwrap().contains("No device"));
        assert!(a.pending_jobs.is_empty() && b.pending_jobs.is_empty());
        assert_eq!(a.build.phase, BuildPhase::Idle);
    }

    #[test]
    fn a_second_run_while_building_is_refused() {
        let again = reduce(building(), Action::RunApp);

        assert_eq!(again.pending_jobs.len(), 1);
        assert!(again.status.as_deref().unwrap().contains("already running"));
    }

    #[test]
    fn cancel_queues_a_cancel_only_while_building() {
        let idle = reduce(running_android(), Action::CancelBuild);
        let cancelling = reduce(building(), Action::CancelBuild);

        assert!(idle.pending_jobs.is_empty());
        assert_eq!(
            cancelling.pending_jobs.last(),
            Some(&Job::CancelBuild { id: 1 })
        );
        assert_eq!(cancelling.build.phase, BuildPhase::Running);
    }

    // --- output while building ---

    #[test]
    fn output_is_logged_and_the_current_task_followed() {
        let s = reduce(
            building(),
            output(1, &["> Task :a:b", "compiling", "> Task :a:c UP-TO-DATE"]),
        );

        assert_eq!(s.build_log.len(), 3);
        assert_eq!(s.build.task.as_deref(), Some(":a:c"));
    }

    #[test]
    fn compiler_problems_are_collected_once_and_counted() {
        let s = reduce(
            building(),
            output(
                1,
                &[
                    "e: file:///p/A.kt:3:4 Unresolved reference: x",
                    "e: file:///p/A.kt:3:4 Unresolved reference: x",
                    "w: file:///p/B.kt:9:1 unused",
                ],
            ),
        );

        assert_eq!((s.build.errors, s.build.warnings), (1, 1));
        assert_eq!(s.problems.len(), 2);
        assert_eq!(s.problems[0].line, Some(3));
        assert_eq!(s.build_log.len(), 3, "the raw lines stay in the log");
    }

    #[test]
    fn problems_stop_at_the_cap() {
        let many: Vec<String> = (0..MAX_PROBLEMS + 20)
            .map(|n| format!("e: /p/A.kt:{}:1 bad", n + 1))
            .collect();

        let s = reduce(building(), Action::BuildOutput { id: 1, lines: many });

        assert_eq!(s.problems.len(), MAX_PROBLEMS);
    }

    #[test]
    fn events_from_an_older_build_are_ignored() {
        let mut s = building();
        s = reduce(
            s,
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Cancelled,
                millis: 1,
            },
        );
        s = reduce(s, Action::RunApp);

        let late = reduce(s.clone(), output(1, &["stale"]));
        let late_end = reduce(
            s.clone(),
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Succeeded,
                millis: 1,
            },
        );
        let late_step = reduce(
            s.clone(),
            Action::BuildStep {
                id: 1,
                step: "x".into(),
            },
        );

        assert_eq!(late, s);
        assert_eq!(late_end, s);
        assert_eq!(late_step, s);
    }

    #[test]
    fn a_step_replaces_the_task_in_the_header() {
        let mut s = reduce(building(), output(1, &["> Task :a:b"]));

        s = reduce(
            s,
            Action::BuildStep {
                id: 1,
                step: "Installing".into(),
            },
        );

        assert_eq!(s.build.step, "Installing");
        assert_eq!(s.build.task, None);
    }

    // --- how a build ends ---

    #[test]
    fn success_records_the_time_and_follows_the_device_log() {
        let s = reduce(
            building(),
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Succeeded,
                millis: 12_340,
            },
        );

        assert_eq!(s.build.phase, BuildPhase::Succeeded);
        assert_eq!(s.build.millis, Some(12_340));
        assert!(s.status.as_deref().unwrap().contains("12.3s"));
        assert_eq!(s.output_tab, OutputTab::Logcat);
        assert!(matches!(
            s.pending_jobs.last(),
            Some(Job::StartLogs(LogRequest { id: 2, .. }))
        ));
        assert_eq!(s.log_streams[&Platform::Android].id, 2);
        assert!(s.build_log.iter().last().unwrap().contains("done in 12.3s"));
    }

    #[test]
    fn a_failed_build_with_errors_shows_the_problems() {
        let mut s = reduce(building(), output(1, &["e: /p/A.kt:1:1 bad"]));

        s = reduce(
            s,
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Failed(Some("Gradle exited with 1".into())),
                millis: 500,
            },
        );

        assert_eq!(s.build.phase, BuildPhase::Failed);
        assert_eq!(s.output_tab, OutputTab::Problems);
        assert!(
            s.status
                .as_deref()
                .unwrap()
                .contains("Gradle exited with 1")
        );
        assert!(
            s.pending_jobs
                .iter()
                .all(|j| !matches!(j, Job::StartLogs(_)))
        );
    }

    #[test]
    fn a_failed_build_without_compiler_errors_stays_on_the_build_log() {
        let s = reduce(
            building(),
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Failed(None),
                millis: 5,
            },
        );

        assert_eq!(s.output_tab, OutputTab::Build);
        assert!(s.build_log.iter().last().unwrap().contains("build failed"));
    }

    #[test]
    fn cancelling_ends_in_the_cancelled_phase() {
        let s = reduce(
            building(),
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Cancelled,
                millis: 5,
            },
        );

        assert_eq!(s.build.phase, BuildPhase::Cancelled);
        assert_eq!(s.status.as_deref(), Some("Build cancelled"));
    }

    // --- device logs ---

    #[test]
    fn toggling_logs_follows_then_stops_the_chosen_device() {
        let followed = reduce(running_android(), Action::ToggleLogs);
        let stopped = reduce(followed.clone(), Action::ToggleLogs);

        assert_eq!(followed.output_tab, OutputTab::Logcat);
        assert!(followed.log_streams.contains_key(&Platform::Android));
        assert!(stopped.log_streams.is_empty());
        assert_eq!(
            stopped.pending_jobs.last(),
            Some(&Job::StopLogs(Platform::Android))
        );
    }

    #[test]
    fn logs_of_a_stopped_device_cannot_be_followed() {
        let s = with(vec![device(
            Platform::Android,
            "Pixel_8",
            "Pixel_8",
            DeviceState::Stopped,
        )]);

        let after = reduce(s, Action::ToggleLogs);

        assert!(after.pending_jobs.is_empty() && after.log_streams.is_empty());
        assert!(after.status.as_deref().unwrap().contains("not running"));
    }

    #[test]
    fn log_lines_go_to_their_platforms_tab_and_stale_streams_are_ignored() {
        let s = reduce(running_android(), Action::ToggleLogs);
        let id = s.log_streams[&Platform::Android].id;

        let fresh = reduce(
            s.clone(),
            Action::LogOutput {
                platform: Platform::Android,
                id,
                lines: lines(&["a", "b"]),
            },
        );
        let stale = reduce(
            s,
            Action::LogOutput {
                platform: Platform::Android,
                id: id + 7,
                lines: lines(&["x"]),
            },
        );

        assert_eq!(fresh.logcat.len(), 3, "marker line plus two");
        assert_eq!(stale.logcat.len(), 1);
        assert!(fresh.ios_log.is_empty());
    }

    #[test]
    fn a_stream_that_ends_is_forgotten_and_says_why() {
        let s = reduce(running_android(), Action::ToggleLogs);
        let id = s.log_streams[&Platform::Android].id;

        let ended = reduce(
            s,
            Action::LogEnded {
                platform: Platform::Android,
                id,
                reason: Some("adb: device offline".into()),
            },
        );

        assert!(ended.log_streams.is_empty());
        assert!(
            ended
                .logcat
                .iter()
                .last()
                .unwrap()
                .contains("device offline")
        );
    }

    #[test]
    fn the_ios_stream_has_its_own_tab() {
        let s = with(vec![device(
            Platform::Ios,
            "11111111-1111-1111-1111-111111111111",
            "iPhone 17",
            DeviceState::Running,
        )]);
        let s = reduce(s, Action::ToggleTarget);

        let after = reduce(s, Action::ToggleLogs);

        assert_eq!(after.output_tab, OutputTab::IosLog);
        assert!(after.log_streams.contains_key(&Platform::Ios));
    }

    // --- the panel itself ---

    fn with_log(count: usize) -> AppState {
        let mut s = building();
        let many: Vec<String> = (0..count).map(|n| format!("line {n}")).collect();
        s = reduce(s, Action::BuildOutput { id: 1, lines: many });
        s
    }

    #[test]
    fn tabs_change_directly_and_by_stepping() {
        let s = reduce(
            running_android(),
            Action::ShowOutputTab(OutputTab::Problems),
        );
        let next = reduce(s.clone(), Action::OutputTabMove(1));
        let back = reduce(s, Action::OutputTabMove(-1));

        assert_eq!(next.output_tab, OutputTab::Git);
        assert_eq!(back.output_tab, OutputTab::IosLog);
    }

    #[test]
    fn scrolling_goes_back_in_time_and_stops_at_the_ends() {
        let s = with_log(10);

        let up = reduce(s.clone(), Action::OutputScroll(3));
        let too_far = reduce(s.clone(), Action::OutputScroll(500));
        let down = reduce(up.clone(), Action::OutputScroll(-9));
        let short = reduce(with_log(3), Action::OutputScroll(5));
        let followed = reduce(up.clone(), Action::OutputFollow);

        assert_eq!(up.output_scroll[OutputTab::Build.index()], 3);
        assert_eq!(
            too_far.output_scroll[OutputTab::Build.index()],
            10 - OUTPUT_VIEWPORT_ROWS
        );
        assert_eq!(down.output_scroll[OutputTab::Build.index()], 0);
        assert_eq!(
            short.output_scroll[OutputTab::Build.index()],
            0,
            "a log that fits has nothing to scroll back to"
        );
        assert_eq!(followed.output_scroll[OutputTab::Build.index()], 0);
    }

    #[test]
    fn new_output_does_not_move_a_reader_who_scrolled_back() {
        let scrolled = reduce(with_log(10), Action::OutputScroll(3));

        let more = reduce(scrolled, output(1, &["new 1", "new 2"]));

        assert_eq!(more.output_scroll[OutputTab::Build.index()], 5);
    }

    #[test]
    fn a_reader_at_the_end_keeps_following() {
        let more = reduce(with_log(10), output(1, &["new"]));

        assert_eq!(more.output_scroll[OutputTab::Build.index()], 0);
    }

    #[test]
    fn a_tab_switch_forced_by_a_finished_build_ends_filter_typing() {
        let typing = reduce(building(), Action::FilterOpen);

        let failed = reduce(
            reduce(typing.clone(), output(1, &["e: /p/A.kt:1:1 bad"])),
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Failed(None),
                millis: 1,
            },
        );
        let succeeded = reduce(
            typing,
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Succeeded,
                millis: 1,
            },
        );

        assert!(!failed.output_filter.editing && !succeeded.output_filter.editing);
    }

    #[test]
    fn typing_a_filter_collects_text_until_done_and_clear_drops_it() {
        let s = reduce(running_android(), Action::FilterOpen);
        let s = "Er"
            .chars()
            .fold(s, |s, c| reduce(s, Action::FilterInput(c)));
        let s = reduce(s, Action::FilterBackspace);
        let s = reduce(s, Action::FilterInput('\n'));
        assert_eq!(s.output_filter.text, "E");
        assert!(s.output_filter.editing);

        let done = reduce(s.clone(), Action::FilterDone);
        let typed_after = reduce(done.clone(), Action::FilterInput('x'));
        let cleared = reduce(s, Action::FilterClear);

        assert!(!done.output_filter.editing && done.output_filter.text == "E");
        assert_eq!(typed_after.output_filter.text, "E", "not typing any more");
        assert!(cleared.output_filter.text.is_empty() && !cleared.output_filter.editing);
    }

    #[test]
    fn the_filter_text_is_bounded() {
        let mut s = reduce(running_android(), Action::FilterOpen);
        for _ in 0..MAX_QUERY_CHARS + 50 {
            s = reduce(s, Action::FilterInput('a'));
        }

        assert_eq!(s.output_filter.text.chars().count(), MAX_QUERY_CHARS);
    }

    #[test]
    fn filtering_is_only_for_log_tabs_and_levels_only_for_logcat() {
        let problems = reduce(
            running_android(),
            Action::ShowOutputTab(OutputTab::Problems),
        );
        let build = running_android();
        let logcat = reduce(running_android(), Action::ShowOutputTab(OutputTab::Logcat));

        assert!(!reduce(problems, Action::FilterOpen).output_filter.editing);
        assert!(
            reduce(build.clone(), Action::FilterOpen)
                .output_filter
                .editing
        );
        assert_eq!(
            reduce(build, Action::CycleLogLevel).output_filter.min_level,
            crate::output::LogLevel::Verbose
        );
        assert_eq!(
            reduce(logcat, Action::CycleLogLevel)
                .output_filter
                .min_level,
            crate::output::LogLevel::Debug
        );
    }

    #[test]
    fn clearing_empties_only_the_shown_log() {
        let mut s = reduce(running_android(), Action::ToggleLogs);
        s = reduce(s, Action::RunApp);
        s = reduce(s, output(2, &["x"]));
        s = reduce(s, Action::ShowOutputTab(OutputTab::Logcat));

        let cleared = reduce(s.clone(), Action::ClearOutput);

        assert!(cleared.logcat.is_empty());
        assert_eq!(cleared.build_log.len(), s.build_log.len());
    }

    // --- problems ---

    fn problem(path: Option<&str>, line: Option<u32>) -> Problem {
        Problem {
            severity: Severity::Error,
            path: path.map(String::from),
            line,
            column: Some(1),
            message: "bad".into(),
        }
    }

    fn with_problems(problems: Vec<Problem>) -> AppState {
        let mut s = running_android();
        s.problems = Arc::new(problems);
        s.output_tab = OutputTab::Problems;
        s
    }

    #[test]
    fn the_problem_selection_moves_within_the_list() {
        let s = with_problems(vec![problem(None, None), problem(None, None)]);

        let down = reduce(s.clone(), Action::OutputScroll(-1));
        let past = reduce(down.clone(), Action::OutputScroll(-5));
        let up = reduce(down, Action::OutputScroll(1));

        assert_eq!(past.problem_selected, 1);
        assert_eq!(up.problem_selected, 0);
    }

    #[test]
    fn a_problem_inside_the_project_opens_as_a_relative_path() {
        let s = with_problems(vec![problem(Some("/p/shared/src/A.kt"), Some(12))]);

        assert_eq!(
            s.selected_problem_target(),
            Ok((PathBuf::from("shared/src/A.kt"), Some(12)))
        );
    }

    #[test]
    fn a_problem_outside_the_project_or_without_a_path_cannot_be_opened() {
        for (path, expect) in [
            (Some("/etc/passwd"), "outside the project"),
            (Some("/p/../etc/passwd"), "outside the project"),
            (Some("/p"), "outside the project"),
            (None, "no source location"),
        ] {
            let s = with_problems(vec![problem(path, Some(1))]);

            let err = s.selected_problem_target().unwrap_err();

            assert!(err.contains(expect), "{path:?}: {err}");
        }
        let empty = with_problems(Vec::new());
        assert!(empty.selected_problem_target().is_err());
    }
}
