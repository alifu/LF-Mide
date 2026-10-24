//! Reducer logic for the performance indicator and panel.

use crate::config::Config;
use crate::devices::Platform;
use crate::output::Job;
use crate::perf::{
    PerfSample, ProcessKind, ProcessStat, StopRequest, SwapLimits, Trend, format_size, swap_trend,
};
use crate::reducer::start_or_stop_device;
use crate::state::{AppState, DeviceAction};

/// The panel needs this many rows to show the question being asked.
pub const MIN_ROWS_TO_CONFIRM: u16 = 12;
const ASKED_PREFIX: &str = "Press x again to stop ";

pub(crate) fn swap_limits(config: &Config) -> SwapLimits {
    SwapLimits::from_mib(config.swap_warn_mb, config.swap_critical_mb)
}

fn say(state: AppState, message: impl Into<String>) -> AppState {
    AppState {
        status: Some(message.into()),
        ..state
    }
}

pub(crate) fn sampled(state: AppState, sample: PerfSample) -> AppState {
    let trend = state.perf.as_ref().map_or(Trend::Flat, |before| {
        swap_trend(before.system.swap_used, sample.system.swap_used)
    });
    let limits = swap_limits(&state.config);
    let swap_high = sample.system.swap_used >= limits.warn;
    let alert = (swap_high && !state.swap_alerted).then(|| swap_alert(&sample));
    let (perf_selected, perf_selected_pid) = follow_highlight(&state, &sample);
    let still_there = state
        .perf_confirm
        .clone()
        .filter(|asked| sample.processes.iter().any(|process| asked.is_for(process)));
    AppState {
        swap_trend: trend,
        swap_alerted: swap_high,
        perf_selected,
        perf_selected_pid,
        perf_confirm: still_there,
        status: alert.or(state.status),
        perf: Some(sample),
        ..state
    }
}

/// Where the highlight goes in a new, re-sorted list: on the same process
/// if it is still there, else at the same row (kept inside the list).
fn follow_highlight(state: &AppState, sample: &PerfSample) -> (usize, Option<u32>) {
    let moved = state
        .perf_selected_pid
        .and_then(|pid| sample.processes.iter().position(|p| p.pid == pid));
    let index = moved.unwrap_or_else(|| {
        state
            .perf_selected
            .min(sample.processes.len().saturating_sub(1))
    });
    (index, sample.processes.get(index).map(|p| p.pid))
}

/// What to say when swap first gets high: the cheapest memory to give back.
fn swap_alert(sample: &PerfSample) -> String {
    let swap = format_size(sample.system.swap_used);
    let daemon = sample
        .processes
        .iter()
        .filter(|p| {
            matches!(
                p.kind,
                ProcessKind::GradleDaemon | ProcessKind::KotlinDaemon
            )
        })
        .max_by_key(|p| p.rss);
    match daemon {
        Some(p) => format!(
            "Swap is {swap}: stop the {} (p, then x) to free {}",
            p.kind.label(),
            format_size(p.rss)
        ),
        None => format!("Swap is {swap}: press p to see what uses the memory"),
    }
}

/// Drops a pending confirmation and the question it left in the status bar.
fn without_question(state: AppState) -> AppState {
    let stale = state.perf_confirm.is_some()
        && state
            .status
            .as_deref()
            .is_some_and(|message| message.starts_with(ASKED_PREFIX));
    AppState {
        perf_confirm: None,
        status: if stale { None } else { state.status },
        ..state
    }
}

pub(crate) fn toggle_panel(state: AppState) -> AppState {
    let state = without_question(state);
    AppState {
        perf_open: !state.perf_open,
        ..state
    }
}

pub(crate) fn toggle_indicator(state: AppState) -> AppState {
    let config = Config {
        show_perf: !state.config.show_perf,
        ..state.config.clone()
    };
    AppState { config, ..state }
}

pub(crate) fn move_selection(state: AppState, delta: i16) -> AppState {
    let count = state.perf.as_ref().map_or(0, |p| p.processes.len());
    if count == 0 {
        return state;
    }
    let state = without_question(state);
    let next = (state.perf_selected as i64 + i64::from(delta)).clamp(0, count as i64 - 1);
    let perf_selected_pid = state
        .perf
        .as_ref()
        .and_then(|p| p.processes.get(next as usize))
        .map(|p| p.pid);
    AppState {
        perf_selected: next as usize,
        perf_selected_pid,
        ..state
    }
}

fn selected_process(state: &AppState) -> Option<ProcessStat> {
    state
        .perf
        .as_ref()?
        .processes
        .get(state.perf_selected)
        .cloned()
}

/// The first press asks for confirmation; the second, on the same process,
/// stops it.
pub(crate) fn stop_selected(state: AppState) -> AppState {
    let Some(process) = selected_process(&state) else {
        return say(state, "Nothing selected");
    };
    let name = describe(&process);
    if !process.kind.stoppable() {
        return say(state, format!("{name} cannot be stopped from here"));
    }
    if !state
        .perf_confirm
        .as_ref()
        .is_some_and(|asked| asked.is_for(&process))
    {
        // The question is shown inside the panel; on a terminal too short to
        // show it, a blind second press must not be possible.
        if state.terminal_height < MIN_ROWS_TO_CONFIRM {
            return say(state, "Make the terminal taller to confirm a stop");
        }
        let message = format!("{ASKED_PREFIX}{name}");
        return AppState {
            perf_confirm: Some(StopRequest::of(&process)),
            status: Some(message),
            ..state
        };
    }
    let state = AppState {
        perf_confirm: None,
        ..state
    };
    match device_of(&state, &process) {
        Some(device) => start_or_stop_device(state, device, DeviceAction::Stop),
        None if process.kind == ProcessKind::Simulator => say(
            state,
            format!("{name} is not in the device list: stop it with `xcrun simctl shutdown`"),
        ),
        None => {
            let mut pending_jobs = state.pending_jobs.clone();
            pending_jobs.push(Job::StopProcess {
                pid: process.pid,
                kind: process.kind,
            });
            AppState {
                status: Some(format!("Stopping {name}…")),
                pending_jobs,
                ..state
            }
        }
    }
}

/// The emulator or simulator a process belongs to, if we list it.
fn device_of(state: &AppState, process: &ProcessStat) -> Option<crate::devices::Device> {
    let platform = match process.kind {
        ProcessKind::Emulator => Platform::Android,
        ProcessKind::Simulator => Platform::Ios,
        _ => return None,
    };
    state
        .devices
        .iter()
        .find(|d| d.platform == platform && d.id.eq_ignore_ascii_case(&process.detail))
        .cloned()
}

/// `Gradle daemon (pid 4121)`, `Android emulator Pixel_8 (pid 5002)`.
pub fn describe(process: &ProcessStat) -> String {
    let detail = if process.detail.is_empty() {
        String::new()
    } else {
        format!(" {}", process.detail)
    };
    format!("{}{detail} (pid {})", process.kind.label(), process.pid)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::action::Action;
    use crate::devices::{Device, DeviceState};
    use crate::perf::{Pressure, SystemStats};
    use crate::project::{Project, ProjectLayout};
    use crate::reducer::reduce;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn base() -> AppState {
        AppState::new(
            Project {
                root: PathBuf::from("/p"),
                name: "demo".into(),
                layout: ProjectLayout::ComposeApp,
            },
            Config::default(),
        )
    }

    fn process(kind: ProcessKind, pid: u32, rss: u64, detail: &str) -> ProcessStat {
        ProcessStat {
            kind,
            pid,
            rss,
            cpu_tenths: 0,
            detail: detail.into(),
        }
    }

    fn sample(swap: u64, processes: Vec<ProcessStat>) -> PerfSample {
        PerfSample {
            system: SystemStats {
                ram_used: 12 * GIB,
                ram_total: 16 * GIB,
                swap_used: swap,
                pressure: Pressure::Normal,
                disk_free: 50 * GIB,
            },
            processes,
        }
    }

    fn with_sample(swap: u64, processes: Vec<ProcessStat>) -> AppState {
        reduce(base(), Action::PerfSampled(sample(swap, processes)))
    }

    #[test]
    fn a_sample_is_kept_and_the_first_one_has_no_trend() {
        let s = with_sample(GIB, vec![]);

        assert_eq!(s.perf.unwrap().system.swap_used, GIB);
        assert_eq!(s.swap_trend, Trend::Flat);
    }

    #[test]
    fn swap_growth_and_shrinkage_become_a_trend() {
        let s = with_sample(GIB, vec![]);

        let up = reduce(s.clone(), Action::PerfSampled(sample(2 * GIB, vec![])));
        let down = reduce(up.clone(), Action::PerfSampled(sample(GIB, vec![])));

        assert_eq!(up.swap_trend, Trend::Up);
        assert_eq!(down.swap_trend, Trend::Down);
    }

    #[test]
    fn high_swap_suggests_stopping_the_biggest_daemon_once() {
        let daemons = vec![
            process(ProcessKind::KotlinDaemon, 2, GIB / 2, ""),
            process(ProcessKind::GradleDaemon, 1, 2 * GIB, ""),
        ];
        let first = with_sample(3 * GIB, daemons.clone());
        let mut quiet = first.clone();
        quiet.status = None;

        let again = reduce(quiet, Action::PerfSampled(sample(3 * GIB, daemons)));

        let message = first.status.unwrap();
        assert!(
            message.contains("Gradle daemon") && message.contains("2.0G"),
            "{message}"
        );
        assert_eq!(again.status, None, "the warning is shown once");
    }

    #[test]
    fn the_warning_returns_after_swap_has_dropped_and_risen_again() {
        let high = with_sample(3 * GIB, vec![]);
        let low = reduce(high, Action::PerfSampled(sample(GIB, vec![])));
        assert!(!low.swap_alerted);

        let again = reduce(low, Action::PerfSampled(sample(3 * GIB, vec![])));

        assert!(again.status.unwrap().contains("Swap is 3.0G"));
    }

    #[test]
    fn low_swap_says_nothing() {
        assert_eq!(with_sample(GIB, vec![]).status, None);
    }

    #[test]
    fn the_indicator_and_the_panel_toggle_independently() {
        let s = base();

        let hidden = reduce(s.clone(), Action::TogglePerfIndicator);
        let open = reduce(s, Action::TogglePerfPanel);

        assert!(!hidden.config.show_perf && !hidden.perf_open);
        assert!(open.perf_open && open.config.show_perf);
        assert!(reduce(hidden, Action::TogglePerfIndicator).config.show_perf);
    }

    #[test]
    fn the_highlight_stays_inside_the_list() {
        let s = with_sample(
            0,
            vec![
                process(ProcessKind::Neovim, 1, 3, ""),
                process(ProcessKind::LfMide, 2, 2, ""),
            ],
        );

        let down = reduce(s.clone(), Action::PerfMove(5));
        let up = reduce(down.clone(), Action::PerfMove(-5));
        let empty = reduce(base(), Action::PerfMove(1));

        assert_eq!((down.perf_selected, up.perf_selected), (1, 0));
        assert_eq!(empty.perf_selected, 0);
    }

    #[test]
    fn the_highlight_stays_on_its_process_when_the_list_reorders() {
        let mut s = with_sample(
            0,
            vec![
                process(ProcessKind::Emulator, 10, 5, "Pixel_8"),
                process(ProcessKind::GradleDaemon, 20, 4, ""),
                process(ProcessKind::Neovim, 30, 3, ""),
            ],
        );
        s = reduce(s, Action::PerfMove(1));
        assert_eq!(s.perf_selected_pid, Some(20));

        // The Gradle daemon grows and overtakes the emulator.
        let after = reduce(
            s,
            Action::PerfSampled(sample(
                0,
                vec![
                    process(ProcessKind::GradleDaemon, 20, 9, ""),
                    process(ProcessKind::Emulator, 10, 5, "Pixel_8"),
                    process(ProcessKind::Neovim, 30, 3, ""),
                ],
            )),
        );

        assert_eq!(
            (after.perf_selected, after.perf_selected_pid),
            (0, Some(20))
        );
    }

    #[test]
    fn a_stop_after_a_reorder_still_targets_the_process_that_was_asked_about() {
        let mut s = with_sample(
            0,
            vec![
                process(ProcessKind::Emulator, 10, 5, "Pixel_8"),
                process(ProcessKind::GradleDaemon, 20, 4, ""),
            ],
        );
        s = reduce(reduce(s, Action::PerfMove(1)), Action::PerfStop);
        assert_eq!(s.perf_confirm.as_ref().map(|r| r.pid), Some(20));

        let reordered = reduce(
            s,
            Action::PerfSampled(sample(
                0,
                vec![
                    process(ProcessKind::GradleDaemon, 20, 9, ""),
                    process(ProcessKind::Emulator, 10, 5, "Pixel_8"),
                ],
            )),
        );
        let done = reduce(reordered, Action::PerfStop);

        assert_eq!(
            done.pending_jobs,
            vec![Job::StopProcess {
                pid: 20,
                kind: ProcessKind::GradleDaemon
            }]
        );
        assert!(
            done.pending_commands.is_empty(),
            "the emulator is untouched"
        );
    }

    #[test]
    fn when_the_highlighted_process_ends_the_highlight_stays_in_the_list() {
        let mut s = with_sample(
            0,
            vec![
                process(ProcessKind::Neovim, 1, 3, ""),
                process(ProcessKind::LfMide, 2, 2, ""),
            ],
        );
        s = reduce(s, Action::PerfMove(1));

        let s = reduce(
            s,
            Action::PerfSampled(sample(0, vec![process(ProcessKind::Neovim, 1, 3, "")])),
        );

        assert_eq!((s.perf_selected, s.perf_selected_pid), (0, Some(1)));
    }

    #[test]
    fn a_shrinking_list_pulls_the_highlight_back() {
        let mut s = with_sample(
            0,
            vec![
                process(ProcessKind::Neovim, 1, 3, ""),
                process(ProcessKind::LfMide, 2, 2, ""),
            ],
        );
        s = reduce(s, Action::PerfMove(1));

        let s = reduce(
            s,
            Action::PerfSampled(sample(0, vec![process(ProcessKind::Neovim, 1, 3, "")])),
        );

        assert_eq!(s.perf_selected, 0);
    }

    #[test]
    fn stopping_needs_a_second_press_on_the_same_process() {
        let s = with_sample(0, vec![process(ProcessKind::GradleDaemon, 4121, GIB, "")]);

        let asked = reduce(s, Action::PerfStop);
        let done = reduce(asked.clone(), Action::PerfStop);

        assert_eq!(asked.perf_confirm.as_ref().map(|r| r.pid), Some(4121));
        assert!(asked.status.as_deref().unwrap().contains("Press x again"));
        assert!(asked.pending_jobs.is_empty());
        assert_eq!(
            done.pending_jobs,
            vec![Job::StopProcess {
                pid: 4121,
                kind: ProcessKind::GradleDaemon
            }]
        );
        assert_eq!(done.perf_confirm, None);
    }

    #[test]
    fn cancelling_a_question_also_removes_it_from_the_status_bar() {
        let s = with_sample(
            0,
            vec![
                process(ProcessKind::GradleDaemon, 1, 3, ""),
                process(ProcessKind::KotlinDaemon, 2, 2, ""),
            ],
        );
        let asked = reduce(s, Action::PerfStop);
        assert!(
            asked
                .status
                .as_deref()
                .unwrap()
                .starts_with("Press x again")
        );

        let moved = reduce(asked.clone(), Action::PerfMove(1));
        let closed = reduce(asked, Action::TogglePerfPanel);

        assert_eq!(moved.status, None);
        assert_eq!(closed.status, None);
    }

    #[test]
    fn cancelling_leaves_an_unrelated_status_alone() {
        let mut s = with_sample(0, vec![process(ProcessKind::GradleDaemon, 1, 3, "")]);
        s.status = Some("Opened src/App.kt".into());

        let moved = reduce(s, Action::PerfMove(1));

        assert_eq!(moved.status.as_deref(), Some("Opened src/App.kt"));
    }

    #[test]
    fn a_reused_pid_does_not_inherit_the_confirmation() {
        let s = with_sample(0, vec![process(ProcessKind::GradleDaemon, 4121, GIB, "")]);
        let asked = reduce(s, Action::PerfStop);
        assert!(asked.perf_confirm.is_some());

        // The daemon ended and an unrelated stoppable process got its pid.
        let reused = reduce(
            asked,
            Action::PerfSampled(sample(
                0,
                vec![process(ProcessKind::KotlinLsp, 4121, GIB, "")],
            )),
        );

        assert_eq!(reused.perf_confirm, None);
        let again = reduce(reused, Action::PerfStop);
        assert!(
            again.pending_jobs.is_empty(),
            "it asks afresh instead of stopping"
        );
    }

    #[test]
    fn a_terminal_too_short_to_show_the_question_never_arms_a_stop() {
        let mut s = with_sample(0, vec![process(ProcessKind::GradleDaemon, 4121, GIB, "")]);
        s.terminal_height = MIN_ROWS_TO_CONFIRM - 1;

        let after = reduce(reduce(s, Action::PerfStop), Action::PerfStop);

        assert_eq!(after.perf_confirm, None);
        assert!(after.pending_jobs.is_empty() && after.pending_commands.is_empty());
        assert!(after.status.unwrap().contains("taller"));
    }

    #[test]
    fn moving_or_closing_cancels_the_confirmation() {
        let s = with_sample(
            0,
            vec![
                process(ProcessKind::GradleDaemon, 1, 3, ""),
                process(ProcessKind::KotlinDaemon, 2, 2, ""),
            ],
        );
        let asked = reduce(s, Action::PerfStop);

        let moved = reduce(asked.clone(), Action::PerfMove(1));
        let closed = reduce(asked, Action::TogglePerfPanel);

        assert_eq!(moved.perf_confirm, None);
        assert_eq!(closed.perf_confirm, None);
        assert!(reduce(moved, Action::PerfStop).pending_jobs.is_empty());
    }

    #[test]
    fn editors_ides_and_ourselves_are_never_stopped() {
        for kind in [
            ProcessKind::Neovim,
            ProcessKind::LfMide,
            ProcessKind::AndroidStudio,
            ProcessKind::Xcode,
        ] {
            let s = with_sample(0, vec![process(kind, 9, 1, "")]);

            let after = reduce(reduce(s, Action::PerfStop), Action::PerfStop);

            assert!(
                after.pending_jobs.is_empty() && after.pending_commands.is_empty(),
                "{kind:?}"
            );
            assert!(after.status.unwrap().contains("cannot be stopped"));
        }
    }

    fn running_device(platform: Platform, id: &str, name: &str) -> Device {
        Device {
            platform,
            id: id.into(),
            name: name.into(),
            detail: String::new(),
            state: DeviceState::Running,
            serial: (platform == Platform::Android).then(|| "emulator-5554".to_string()),
        }
    }

    fn with_device(device: Device, processes: Vec<ProcessStat>) -> AppState {
        let s = reduce(
            base(),
            Action::DevicesLoaded {
                devices: Arc::new(vec![device]),
                errors: BTreeMap::new(),
            },
        );
        reduce(s, Action::PerfSampled(sample(0, processes)))
    }

    #[test]
    fn stopping_an_emulator_uses_the_normal_device_stop() {
        let s = with_device(
            running_device(Platform::Android, "Pixel_8", "Pixel_8"),
            vec![process(ProcessKind::Emulator, 5002, 2 * GIB, "Pixel_8")],
        );

        let done = reduce(reduce(s, Action::PerfStop), Action::PerfStop);

        assert_eq!(done.pending_commands.len(), 1);
        assert_eq!(done.pending_commands[0].device.id, "Pixel_8");
        assert!(done.pending_jobs.is_empty());
        assert!(done.status.unwrap().contains("Stopping Pixel_8"));
    }

    #[test]
    fn a_simulator_is_matched_by_its_udid_without_regard_to_case() {
        let udid = "23F0A11F-0DD8-4766-853D-987DE9852BAC";
        let s = with_device(
            running_device(Platform::Ios, udid, "iPhone 17 Pro"),
            vec![process(
                ProcessKind::Simulator,
                48452,
                GIB,
                &udid.to_lowercase(),
            )],
        );

        let done = reduce(reduce(s, Action::PerfStop), Action::PerfStop);

        assert_eq!(done.pending_commands.len(), 1);
    }

    #[test]
    fn an_emulator_we_do_not_list_falls_back_to_stopping_the_process() {
        let s = with_sample(
            0,
            vec![process(ProcessKind::Emulator, 5002, GIB, "Unknown_AVD")],
        );

        let done = reduce(reduce(s, Action::PerfStop), Action::PerfStop);

        assert!(done.pending_commands.is_empty());
        assert_eq!(done.pending_jobs.len(), 1);
    }

    #[test]
    fn an_unlisted_simulator_is_never_signalled() {
        let udid = "23F0A11F-0DD8-4766-853D-987DE9852BAC";
        let s = with_sample(0, vec![process(ProcessKind::Simulator, 48452, GIB, udid)]);

        let done = reduce(reduce(s, Action::PerfStop), Action::PerfStop);

        assert!(done.pending_jobs.is_empty() && done.pending_commands.is_empty());
        assert!(done.status.unwrap().contains("simctl shutdown"));
    }

    #[test]
    fn describe_names_the_process_its_detail_and_its_pid() {
        assert_eq!(
            describe(&process(ProcessKind::Emulator, 7, 0, "Pixel_8")),
            "Android emulator Pixel_8 (pid 7)"
        );
        assert_eq!(
            describe(&process(ProcessKind::GradleDaemon, 8, 0, "")),
            "Gradle daemon (pid 8)"
        );
    }
}
