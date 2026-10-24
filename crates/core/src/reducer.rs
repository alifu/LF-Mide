//! Pure state transitions.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::action::Action;
use crate::config::{Config, TerminalKind};
use crate::devices::{Device, DeviceState, Platform};
use crate::doctor::{Report, popup_content_rows};
use crate::editor_reducer;
use crate::git_reducer;
use crate::layout::{devices_viewport_rows, files_viewport_rows};
use crate::nav::{TreeIndex, parent_row};
use crate::output_reducer;
use crate::perf_reducer;
use crate::state::{
    AppState, DeviceAction, DeviceCommand, DoctorPanel, FinderState, GrepState, MAX_QUERY_CHARS,
    MAX_TRANSITION_POLLS, Panel, Transition,
};

/// Consumes `state` and returns the state that results from applying
/// `action`. Taking ownership keeps updates pure without copying the whole
/// state on every action.
pub fn reduce(state: AppState, action: Action) -> AppState {
    match action {
        Action::Quit => editor_reducer::quit(state),
        Action::QuitNow => AppState {
            should_quit: true,
            ..state
        },
        Action::EmbeddedStarted => editor_reducer::started(state),
        Action::EditorScreen(screen) => editor_reducer::screen(state, screen),
        // Sent to Neovim by the app; nothing here changes.
        Action::EditorKeys(_) | Action::EditorPaste(_) => state,
        Action::EditorExited => editor_reducer::exited(state),
        Action::EditorQuitCancelled => editor_reducer::quit_cancelled(state),
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
            branch_known: true,
            status: None,
            ..state
        },
        Action::Status(message) => AppState {
            status: Some(message),
            ..state
        },
        Action::ToggleDoctor => {
            let doctor = match state.doctor {
                DoctorPanel::Closed => DoctorPanel::Running,
                _ => DoctorPanel::Closed,
            };
            AppState {
                doctor,
                doctor_scroll: 0,
                ..state
            }
        }
        Action::RefreshDoctor => match state.doctor {
            DoctorPanel::Ready(_) => AppState {
                doctor: DoctorPanel::Running,
                doctor_scroll: 0,
                ..state
            },
            _ => state,
        },
        // Only a panel still waiting for results accepts them: a run that
        // finishes after the user closed the panel is dropped.
        Action::DoctorFinished(report) => match state.doctor {
            DoctorPanel::Running => AppState {
                doctor: DoctorPanel::Ready(report),
                doctor_scroll: 0,
                ..state
            },
            _ => state,
        },
        Action::DoctorScroll(delta) => {
            let DoctorPanel::Ready(report) = &state.doctor else {
                return state;
            };
            let moved = i32::from(state.doctor_scroll) + i32::from(delta);
            let doctor_scroll = moved.clamp(0, i32::from(max_scroll(&state, report))) as u16;
            AppState {
                doctor_scroll,
                ..state
            }
        }
        Action::TerminalDetected(detected_terminal) => AppState {
            detected_terminal,
            ..state
        },
        Action::OpenTerminalPicker => AppState {
            terminal_picker: Some(picker_index(&state.config)),
            ..state
        },
        Action::TerminalPickerMove(delta) => {
            let Some(current) = state.terminal_picker else {
                return state;
            };
            let rows = TerminalKind::ALL.len() as i32;
            let moved = (current as i32 + i32::from(delta)).rem_euclid(rows);
            AppState {
                terminal_picker: Some(moved as usize),
                ..state
            }
        }
        Action::TerminalPickerSelect => {
            let Some(row) = state.terminal_picker else {
                return state;
            };
            let kind = TerminalKind::ALL[row.min(TerminalKind::ALL.len() - 1)];
            AppState {
                config: Config {
                    terminal: kind,
                    ..state.config
                },
                terminal_picker: None,
                status: Some(format!("Terminal: {}", kind.label())),
                ..state
            }
        }
        Action::TerminalPickerCancel => AppState {
            terminal_picker: None,
            ..state
        },
        Action::OpenEditor if state.embedded => AppState {
            focus: Panel::Editor,
            ..state
        },
        Action::OpenEditor if state.editor_launching => AppState {
            status: Some("Neovim is already opening…".to_string()),
            ..state
        },
        Action::OpenEditor => AppState {
            status: Some("Opening Neovim…".to_string()),
            editor_launching: true,
            editor_launch_wanted: true,
            focus: if editor_reducer::shows_files_here(&state) {
                Panel::Editor
            } else {
                state.focus
            },
            ..state
        },
        Action::EditorLaunchStarted => AppState {
            editor_launch_wanted: false,
            ..state
        },
        Action::EditorLaunchFailed(message) => AppState {
            editor_launching: false,
            editor_launch_wanted: false,
            pending_open: None,
            status: Some(message),
            ..state
        },
        Action::PendingOpenSent => AppState {
            pending_open: None,
            ..state
        },
        Action::EditorOpened(socket) => AppState {
            editor_launching: false,
            editor_socket: Some(socket),
            // A new Neovim starts without signs.
            marks_key: None,
            // A new Neovim: its socket has not been proven yet.
            editor_ready: false,
            status: Some(
                if state.embedded {
                    "Neovim is ready"
                } else {
                    "Neovim opened in a split pane"
                }
                .to_string(),
            ),
            ..state
        },
        Action::FileOpened(path) => git_reducer::file_shown(
            AppState {
                editor_ready: true,
                status: Some(format!("Opened {}", path.display())),
                ..state
            },
            path,
        ),
        // Something found the old Neovim gone. A launch that is already under
        // way (the user pressed `e`, or chose a file) is not affected: it
        // still has to finish, and the file waiting for it still opens.
        // Our own Neovim is known to be gone only when its process ends.
        Action::EditorLost if state.embedded => state,
        Action::EditorLost => AppState {
            editor_socket: None,
            marks_key: None,
            editor_ready: false,
            status: Some("Neovim isn't running: press e to open it".to_string()),
            ..state
        },
        Action::FilesLoaded(files) => rebuild_tree(AppState {
            tree_index: Arc::new(TreeIndex::build(&files)),
            files,
            files_loading: false,
            ..state
        }),
        Action::RefreshFiles => AppState {
            files_loading: true,
            ..state
        },
        Action::TreeMove(delta) => {
            if state.tree.is_empty() {
                return state;
            }
            let moved = state.tree_selected as i64 + i64::from(delta);
            let row = moved.clamp(0, state.tree.len() as i64 - 1) as usize;
            select_row(state, row)
        }
        Action::TreeToggle => match selected_dir(&state) {
            Some((path, _)) => toggle_dir(state, path),
            None => state,
        },
        Action::TreeExpand => match selected_dir(&state) {
            Some((path, false)) => toggle_dir(state, path),
            // Already open: step into it, if it has anything inside.
            Some((path, true)) => {
                let next = state.tree_selected + 1;
                match state.tree.get(next) {
                    Some(child) if child.path.starts_with(&path) => select_row(state, next),
                    _ => state,
                }
            }
            None => state,
        },
        Action::TreeCollapse => match selected_dir(&state) {
            Some((path, true)) => toggle_dir(state, path),
            _ => match parent_row(&state.tree, state.tree_selected) {
                Some(parent) => select_row(state, parent),
                None => state,
            },
        },
        Action::Focus(panel) => AppState {
            focus: panel,
            ..state
        },
        Action::ToggleTarget => show_selected_device(AppState {
            target: state.target.other(),
            device_top: 0,
            ..state
        }),
        Action::SdkLoaded(sdk) => AppState {
            sdk,
            sdk_loaded: true,
            ..state
        },
        Action::DevicesLoaded { devices, errors } => {
            let mut transitions = state.transitions.clone();
            let merged = apply_transitions(&devices, &errors, &mut transitions);
            let status = settled_status(&state, &merged, &transitions);
            reconcile_selection(AppState {
                devices: Arc::new(merged),
                devices_loaded: true,
                device_errors: errors,
                transitions,
                status,
                ..state
            })
        }
        // The event loop starts the listing; the list itself is unchanged.
        Action::RefreshDevices => state,
        Action::DeviceMove(delta) => {
            let ids: Vec<String> = state
                .target_devices()
                .iter()
                .map(|d| d.id.clone())
                .collect();
            if ids.is_empty() {
                return state;
            }
            let current = state
                .selected_devices
                .get(&state.target)
                .and_then(|id| ids.iter().position(|candidate| candidate == id))
                .unwrap_or(0);
            let next = moved(current, delta, ids.len());
            let mut selected_devices = state.selected_devices.clone();
            selected_devices.insert(state.target, ids[next].clone());
            show_selected_device(AppState {
                selected_devices,
                ..state
            })
        }
        Action::BootDevice => start_or_stop(state, DeviceAction::Boot),
        Action::StopDevice => start_or_stop(state, DeviceAction::Stop),
        Action::DeviceActionFailed {
            platform,
            id,
            message,
        } => {
            let mut transitions = state.transitions.clone();
            let undone = transitions.remove(&(platform, id.clone()));
            // Put the device back to what it was before we asked.
            let restored = undone.map(|t| match t.state {
                DeviceState::Starting => DeviceState::Stopped,
                _ => DeviceState::Running,
            });
            AppState {
                devices: Arc::new(with_state(&state.devices, platform, &id, restored)),
                transitions,
                status: Some(message),
                ..state
            }
        }
        Action::ToggleInfo => AppState {
            info_open: !state.info_open,
            ..state
        },
        Action::RunApp => output_reducer::run_app(state),
        Action::CancelBuild => output_reducer::cancel_build(state),
        Action::BuildStep { id, step } => output_reducer::build_step(state, id, step),
        Action::BuildOutput { id, lines } => output_reducer::build_output(state, id, lines),
        Action::BuildFinished {
            id,
            outcome,
            millis,
        } => output_reducer::build_finished(state, id, outcome, millis),
        Action::ToggleLogs => output_reducer::toggle_logs(state),
        Action::LogOutput {
            platform,
            id,
            lines,
        } => output_reducer::log_output(state, platform, id, lines),
        Action::LogEnded {
            platform,
            id,
            reason,
        } => output_reducer::log_ended(state, platform, id, reason),
        Action::ShowOutputTab(tab) => output_reducer::show_tab(state, tab),
        Action::OutputTabMove(by) => {
            let tab = state.output_tab.moved(by);
            output_reducer::show_tab(state, tab)
        }
        Action::OutputScroll(delta) => output_reducer::scroll(state, delta),
        Action::OutputFollow => output_reducer::follow(state),
        Action::ClearOutput => output_reducer::clear(state),
        Action::FilterOpen => output_reducer::filter_open(state),
        Action::FilterInput(ch) => output_reducer::filter_input(state, ch),
        Action::FilterBackspace => output_reducer::filter_backspace(state),
        Action::FilterDone => output_reducer::filter_done(state),
        Action::FilterClear => output_reducer::filter_clear(state),
        Action::CycleLogLevel => output_reducer::cycle_level(state),
        Action::PerfSampled(sample) => perf_reducer::sampled(state, sample),
        Action::TogglePerfPanel => perf_reducer::toggle_panel(state),
        Action::TogglePerfIndicator => perf_reducer::toggle_indicator(state),
        Action::PerfMove(delta) => perf_reducer::move_selection(state, delta),
        Action::PerfStop => perf_reducer::stop_selected(state),
        Action::GitLoaded { head, changes } => git_reducer::git_loaded(state, head, changes),
        Action::RefreshGit => state,
        Action::ShowGit => git_reducer::show_git(state),
        Action::OpenDiffSelected => git_reducer::open_diff_selected(state),
        Action::OpenDiffCurrent => git_reducer::open_diff_current(state),
        Action::DiffLoaded { id, result } => git_reducer::diff_loaded(state, id, result),
        Action::DiffScroll(delta) => git_reducer::diff_scroll(state, delta),
        Action::DiffHunk(delta) => git_reducer::diff_hunk(state, delta),
        Action::DiffLayout => git_reducer::diff_layout(state),
        Action::DiffClose => git_reducer::diff_close(state),
        Action::BasePickerOpen => git_reducer::picker_open(state),
        Action::BasePickerMove(delta) => git_reducer::picker_move(state, delta),
        Action::BasePickerSelect => git_reducer::picker_select(state),
        Action::BasePickerCancel => git_reducer::picker_cancel(state),
        Action::BasesLoaded(bases) => git_reducer::bases_loaded(state, bases),
        Action::ToggleBlame => git_reducer::toggle_blame(state),
        Action::MarksStale => git_reducer::marks_stale(state),
        Action::EditorFileChanged(path) => git_reducer::editor_file_changed(state, path),
        Action::PrefsLoaded(prefs) => {
            let mut selected_devices = state.selected_devices.clone();
            for platform in Platform::ALL {
                if let Some(id) = prefs.device_for(platform) {
                    selected_devices.insert(platform, id.to_string());
                }
            }
            AppState {
                target: prefs.target.unwrap_or(state.target),
                selected_devices,
                ..state
            }
        }
        Action::OpenFinder => AppState {
            finder: Some(FinderState::default()),
            grep: None,
            ..state
        },
        Action::FinderInput(ch) => update_finder(state, |finder| {
            Some(FinderState {
                query: with_char(&finder.query, ch)?,
                selected: 0,
                ..finder.clone()
            })
        }),
        Action::FinderBackspace => update_finder(state, |finder| {
            Some(FinderState {
                query: without_last_char(&finder.query),
                selected: 0,
                ..finder.clone()
            })
        }),
        Action::FinderMove(delta) => update_finder(state, |finder| {
            Some(FinderState {
                selected: moved(finder.selected, delta, finder.matches.len()),
                ..finder.clone()
            })
        }),
        Action::FinderCancel => AppState {
            finder: None,
            ..state
        },
        // Results for an older query (typing outran the ranking) are dropped.
        Action::FinderResults { query, matches } => update_finder(state, |finder| {
            (finder.query == query).then(|| FinderState {
                matches: Arc::new(matches),
                selected: 0,
                ..finder.clone()
            })
        }),
        Action::OpenGrep => AppState {
            grep: Some(GrepState::default()),
            finder: None,
            ..state
        },
        Action::GrepInput(ch) => update_grep(state, |grep| {
            Some(GrepState {
                query: with_char(&grep.query, ch)?,
                ..grep.clone()
            })
        }),
        Action::GrepBackspace => update_grep(state, |grep| {
            Some(GrepState {
                query: without_last_char(&grep.query),
                ..grep.clone()
            })
        }),
        Action::GrepMove(delta) => update_grep(state, |grep| {
            Some(GrepState {
                selected: moved(grep.selected, delta, grep.hits.len()),
                ..grep.clone()
            })
        }),
        Action::GrepCancel => AppState {
            grep: None,
            ..state
        },
        Action::GrepSearch => update_grep(state, |grep| {
            (!grep.query.trim().is_empty()).then(|| GrepState {
                searched: Some(grep.query.clone()),
                hits: Arc::new(Vec::new()),
                selected: 0,
                running: true,
                error: None,
                ..grep.clone()
            })
        }),
        // Only the search that is still the latest one may fill the list.
        Action::GrepResults { query, result } => update_grep(state, |grep| {
            (grep.searched.as_deref() == Some(query.as_str())).then(|| {
                let (hits, error) = match result {
                    Ok(hits) => (hits, None),
                    Err(message) => (Vec::new(), Some(message)),
                };
                GrepState {
                    hits: Arc::new(hits),
                    selected: 0,
                    running: false,
                    error,
                    ..grep.clone()
                }
            })
        }),
        Action::OpenFile { path, line } => {
            let focus = if editor_reducer::shows_files_here(&state) {
                Panel::Editor
            } else {
                state.focus
            };
            // Choosing a file ends whatever overlay it was chosen in.
            if state.editor_socket.is_none() {
                // No Neovim yet: start one (unless one is on its way) and
                // open the file the moment it answers.
                let start = !state.editor_launching;
                return AppState {
                    status: Some(format!("Opening Neovim, then {}…", path.display())),
                    pending_open: Some((path, line)),
                    editor_launching: true,
                    editor_launch_wanted: state.editor_launch_wanted || start,
                    focus,
                    finder: None,
                    grep: None,
                    diff: None,
                    ..state
                };
            }
            AppState {
                status: Some(format!("Opening {}…", path.display())),
                focus,
                finder: None,
                grep: None,
                diff: None,
                ..state
            }
        }
        Action::ViewportChanged(terminal_height) => {
            let selected = state.tree_selected;
            let resized = show_selected_device(select_row(
                AppState {
                    terminal_height,
                    ..state
                },
                selected,
            ));
            let DoctorPanel::Ready(report) = &resized.doctor else {
                return resized;
            };
            let doctor_scroll = resized.doctor_scroll.min(max_scroll(&resized, report));
            AppState {
                doctor_scroll,
                ..resized
            }
        }
    }
}

/// The status line after a poll: a "Starting X…" / "Stopping X…" message
/// whose request the listing has just settled becomes "X is running" /
/// "X stopped". Any other message is kept.
fn settled_status(
    state: &AppState,
    merged: &[Device],
    transitions: &BTreeMap<(Platform, String), Transition>,
) -> Option<String> {
    let settled = state
        .transitions
        .iter()
        .filter(|(key, _)| !transitions.contains_key(*key))
        .find_map(|((platform, id), pending)| {
            let device = merged
                .iter()
                .find(|d| d.platform == *platform && &d.id == id)?;
            let (verb, done, gave_up, expected) = match pending.state {
                DeviceState::Starting => (
                    "Starting",
                    "is running",
                    "did not start",
                    DeviceState::Running,
                ),
                _ => ("Stopping", "stopped", "did not stop", DeviceState::Stopped),
            };
            let outcome = if device.state == expected {
                done
            } else {
                gave_up
            };
            // The start message may carry a note after it (another emulator
            // is running): it is still the message to replace.
            let asked = format!("{verb} {}…", device.name);
            state
                .status
                .as_deref()
                .is_some_and(|message| message.starts_with(&asked))
                .then(|| format!("{} {outcome}", device.name))
        });
    settled.or_else(|| state.status.clone())
}

/// The device list with pending starts/stops applied: a device we asked to
/// start (or stop) keeps showing as starting (stopping) until the listing
/// confirms it, or [`MAX_TRANSITION_POLLS`] polls pass.
fn apply_transitions(
    listed: &[Device],
    errors: &BTreeMap<Platform, String>,
    transitions: &mut BTreeMap<(Platform, String), Transition>,
) -> Vec<Device> {
    // Forget requests for devices that no longer exist. A platform that
    // failed to list says nothing about its devices, so its requests stay.
    transitions.retain(|(platform, id), _| {
        errors.contains_key(platform)
            || listed
                .iter()
                .any(|d| d.platform == *platform && &d.id == id)
    });
    listed
        .iter()
        .map(|device| {
            let key = (device.platform, device.id.clone());
            let Some(pending) = transitions.get(&key).copied() else {
                return device.clone();
            };
            let confirmed = match pending.state {
                DeviceState::Starting => device.state == DeviceState::Running,
                _ => device.state == DeviceState::Stopped,
            };
            let polls = pending.polls.saturating_add(1);
            if confirmed || polls > MAX_TRANSITION_POLLS {
                transitions.remove(&key);
                return device.clone();
            }
            transitions.insert(key, Transition { polls, ..pending });
            Device {
                state: pending.state,
                ..device.clone()
            }
        })
        .collect()
}

/// Keeps each platform's chosen device if it is still listed, otherwise
/// picks a default. A platform that could not be listed at all keeps its
/// remembered choice: a missing tool today is not a reason to forget it.
fn reconcile_selection(state: AppState) -> AppState {
    let mut selected_devices = state.selected_devices.clone();
    for platform in Platform::ALL {
        let listed: Vec<&Device> = state
            .devices
            .iter()
            .filter(|d| d.platform == platform)
            .collect();
        if listed.is_empty() && state.device_errors.contains_key(&platform) {
            continue;
        }
        let still_there = selected_devices
            .get(&platform)
            .is_some_and(|id| listed.iter().any(|d| &d.id == id));
        if still_there {
            continue;
        }
        match default_device(platform, &listed) {
            Some(device) => selected_devices.insert(platform, device.id.clone()),
            None => selected_devices.remove(&platform),
        };
    }
    show_selected_device(AppState {
        selected_devices,
        ..state
    })
}

/// A running device if there is one; on iOS the first iPhone (iPads sort
/// first alphabetically); else the first.
fn default_device<'a>(platform: Platform, listed: &[&'a Device]) -> Option<&'a Device> {
    listed
        .iter()
        .find(|d| d.is_running())
        .or_else(|| {
            (platform == Platform::Ios)
                .then(|| listed.iter().find(|d| d.name.starts_with("iPhone")))
                .flatten()
        })
        .or_else(|| listed.first())
        .copied()
}

/// Scrolls the device list just enough to show the chosen device.
fn show_selected_device(state: AppState) -> AppState {
    let devices = state.target_devices();
    let row = state
        .selected_devices
        .get(&state.target)
        .and_then(|id| devices.iter().position(|d| &d.id == id))
        .unwrap_or(0);
    let len = devices.len();
    let visible = usize::from(devices_viewport_rows(state.terminal_height));
    let top = if row < state.device_top {
        row
    } else if row >= state.device_top + visible {
        row + 1 - visible
    } else {
        state.device_top
    };
    AppState {
        device_top: top.min(len.saturating_sub(visible)),
        ..state
    }
}

/// `devices` with one device's state changed (`None` leaves it as is).
fn with_state(
    devices: &[Device],
    platform: Platform,
    id: &str,
    state: Option<DeviceState>,
) -> Vec<Device> {
    devices
        .iter()
        .map(|d| match state {
            Some(state) if d.platform == platform && d.id == id => Device { state, ..d.clone() },
            _ => d.clone(),
        })
        .collect()
}

/// Starts or stops the chosen device: records that we asked, shows it as
/// starting/stopping at once, and queues the command for the event loop.
fn start_or_stop(state: AppState, action: DeviceAction) -> AppState {
    let Some(device) = state.selected_device().cloned() else {
        return AppState {
            status: Some("No device selected".to_string()),
            ..state
        };
    };
    start_or_stop_device(state, device, action)
}

/// Starts or stops `device` (not necessarily the selected one).
pub(crate) fn start_or_stop_device(
    state: AppState,
    device: Device,
    action: DeviceAction,
) -> AppState {
    let refusal = match (action, device.state) {
        (DeviceAction::Boot, DeviceState::Stopped) | (DeviceAction::Stop, DeviceState::Running) => {
            None
        }
        (DeviceAction::Boot, DeviceState::Running) => Some("is already running"),
        (DeviceAction::Boot, DeviceState::Starting) => Some("is already starting"),
        (_, DeviceState::Stopping) => Some("is shutting down"),
        (DeviceAction::Stop, _) => Some("is not running"),
    };
    if let Some(reason) = refusal {
        return AppState {
            status: Some(format!("{} {reason}", device.name)),
            ..state
        };
    }

    let (pending, verb) = match action {
        DeviceAction::Boot => (DeviceState::Starting, "Starting"),
        DeviceAction::Stop => (DeviceState::Stopping, "Stopping"),
    };
    let mut transitions = state.transitions.clone();
    transitions.insert(
        (device.platform, device.id.clone()),
        Transition {
            state: pending,
            polls: 0,
        },
    );
    let mut pending_commands = state.pending_commands.clone();
    pending_commands.push(DeviceCommand {
        action,
        device: device.clone(),
    });
    AppState {
        devices: Arc::new(with_state(
            &state.devices,
            device.platform,
            &device.id,
            Some(pending),
        )),
        transitions,
        pending_commands,
        status: Some(format!(
            "{verb} {}…{}",
            device.name,
            crowding_note(&state, &device, action)
        )),
        ..state
    }
}

/// A second emulator or simulator of the same kind costs gigabytes: say so
/// when one is already up.
fn crowding_note(state: &AppState, device: &Device, action: DeviceAction) -> String {
    if action != DeviceAction::Boot {
        return String::new();
    }
    let other = state.devices.iter().find(|d| {
        d.platform == device.platform
            && d.id != device.id
            && matches!(d.state, DeviceState::Running | DeviceState::Starting)
    });
    match other {
        Some(other) => format!(
            " ({} is also running: each one holds several GB, stop it if you can)",
            other.name
        ),
        None => String::new(),
    }
}

/// Applies `change` to the open finder; `None` from it (or no finder) leaves
/// the state as it is.
fn update_finder(
    state: AppState,
    change: impl FnOnce(&FinderState) -> Option<FinderState>,
) -> AppState {
    match state.finder.as_ref().and_then(change) {
        Some(finder) => AppState {
            finder: Some(finder),
            ..state
        },
        None => state,
    }
}

/// Like [`update_finder`], for the project search.
fn update_grep(state: AppState, change: impl FnOnce(&GrepState) -> Option<GrepState>) -> AppState {
    match state.grep.as_ref().and_then(change) {
        Some(grep) => AppState {
            grep: Some(grep),
            ..state
        },
        None => state,
    }
}

/// `query` plus `ch`, unless `ch` is a control character or the query is
/// already as long as allowed.
fn with_char(query: &str, ch: char) -> Option<String> {
    (!ch.is_control() && query.chars().count() < MAX_QUERY_CHARS).then(|| format!("{query}{ch}"))
}

fn without_last_char(query: &str) -> String {
    let mut chars = query.chars();
    chars.next_back();
    chars.as_str().to_string()
}

/// `selected` moved by `delta` within a list of `len` entries.
fn moved(selected: usize, delta: i16, len: usize) -> usize {
    let last = len.saturating_sub(1) as i64;
    (selected as i64 + i64::from(delta)).clamp(0, last) as usize
}

/// The selected tree row as `(path, is_expanded)` if it is a directory.
fn selected_dir(state: &AppState) -> Option<(PathBuf, bool)> {
    let row = state.tree.get(state.tree_selected)?;
    row.is_dir.then(|| (row.path.clone(), row.expanded))
}

/// Opens `dir` if closed, closes it if open.
fn toggle_dir(state: AppState, dir: PathBuf) -> AppState {
    let mut expanded = state.expanded.clone();
    if !expanded.remove(&dir) {
        expanded.insert(dir);
    }
    rebuild_tree(AppState { expanded, ..state })
}

/// Recomputes the visible rows, keeping the selection on the same path when
/// it is still there.
fn rebuild_tree(state: AppState) -> AppState {
    let previous = state
        .tree
        .get(state.tree_selected)
        .map(|row| row.path.clone());
    let rows = state.tree_index.rows(&state.expanded);
    let selected = previous
        .and_then(|path| rows.iter().position(|row| row.path == path))
        .unwrap_or_else(|| state.tree_selected.min(rows.len().saturating_sub(1)));
    select_row(
        AppState {
            tree: Arc::new(rows),
            ..state
        },
        selected,
    )
}

/// Selects `row` (clamped to the tree) and scrolls just enough to show it.
fn select_row(state: AppState, row: usize) -> AppState {
    let len = state.tree.len();
    let row = row.min(len.saturating_sub(1));
    let visible = usize::from(files_viewport_rows(state.terminal_height));
    let top = if row < state.tree_top {
        row
    } else if row >= state.tree_top + visible {
        row + 1 - visible
    } else {
        state.tree_top
    };
    AppState {
        tree_selected: row,
        tree_top: top.min(len.saturating_sub(visible)),
        ..state
    }
}

fn picker_index(config: &Config) -> usize {
    TerminalKind::ALL
        .iter()
        .position(|kind| *kind == config.terminal)
        .unwrap_or(0)
}

/// Furthest the report can scroll while still filling the overlay.
fn max_scroll(state: &AppState, report: &Report) -> u16 {
    let visible = usize::from(popup_content_rows(state.terminal_height));
    let hidden = report.lines().len().saturating_sub(visible);
    u16::try_from(hidden).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::doctor::{Category, CheckResult, Status};
    use crate::layout::files_viewport_rows;
    use crate::nav::{FileMatch, GrepHit};
    use crate::project::{Project, ProjectLayout};
    use crate::state::{BranchInfo, DoctorPanel, MAX_QUERY_CHARS, Panel};
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

        assert_eq!(next.focus, Panel::Editor);
        assert_eq!(back.focus, Panel::Files);
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
    fn the_branch_is_unknown_until_the_first_check_reports() {
        let fresh = state();
        let outside_a_repo = reduce(state(), Action::BranchChanged(None));

        assert!(!fresh.branch_known);
        assert!(outside_a_repo.branch_known && outside_a_repo.branch.is_none());
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

    /// A report with roughly 3 lines per check plus headings: taller than any
    /// test terminal.
    fn long_report() -> Report {
        let results = (0..30)
            .map(|i| CheckResult::new(Category::Editor, &format!("tool{i}"), Status::Ok, "1.0"))
            .collect();
        Report::new(results)
    }

    fn ready() -> AppState {
        let opened = reduce(state(), Action::ToggleDoctor);
        reduce(opened, Action::DoctorFinished(long_report()))
    }

    #[test]
    fn toggle_opens_in_running_state_and_closes_again() {
        let opened = reduce(state(), Action::ToggleDoctor);
        let closed = reduce(opened.clone(), Action::ToggleDoctor);

        assert_eq!(opened.doctor, DoctorPanel::Running);
        assert_eq!(closed.doctor, DoctorPanel::Closed);
    }

    #[test]
    fn finished_run_fills_the_open_panel() {
        let opened = reduce(state(), Action::ToggleDoctor);

        let done = reduce(opened, Action::DoctorFinished(long_report()));

        assert_eq!(done.doctor, DoctorPanel::Ready(long_report()));
    }

    #[test]
    fn a_run_finishing_after_the_panel_was_closed_is_discarded() {
        let closed = reduce(reduce(state(), Action::ToggleDoctor), Action::ToggleDoctor);

        let after = reduce(closed, Action::DoctorFinished(long_report()));

        assert_eq!(after.doctor, DoctorPanel::Closed);
    }

    #[test]
    fn refresh_restarts_only_a_finished_panel() {
        let rerun = reduce(ready(), Action::RefreshDoctor);
        let while_closed = reduce(state(), Action::RefreshDoctor);
        let while_running = reduce(reduce(state(), Action::ToggleDoctor), Action::RefreshDoctor);

        assert_eq!(rerun.doctor, DoctorPanel::Running);
        assert_eq!(while_closed.doctor, DoctorPanel::Closed);
        assert_eq!(while_running.doctor, DoctorPanel::Running);
    }

    #[test]
    fn scrolling_moves_within_the_report_and_stops_at_both_ends() {
        let start = ready();
        let max = long_report().lines().len() as u16 - popup_content_rows(start.terminal_height);

        let down = reduce(start.clone(), Action::DoctorScroll(5));
        let up_past_top = reduce(down.clone(), Action::DoctorScroll(-50));
        let down_past_end = reduce(down, Action::DoctorScroll(500));

        assert_eq!(start.doctor_scroll, 0);
        assert_eq!(up_past_top.doctor_scroll, 0);
        assert_eq!(down_past_end.doctor_scroll, max);
    }

    #[test]
    fn scrolling_is_ignored_unless_a_report_is_showing() {
        let running = reduce(
            reduce(state(), Action::ToggleDoctor),
            Action::DoctorScroll(3),
        );

        assert_eq!(running.doctor_scroll, 0);
        assert_eq!(reduce(state(), Action::DoctorScroll(3)).doctor_scroll, 0);
    }

    #[test]
    fn reopening_starts_at_the_top() {
        let scrolled = reduce(ready(), Action::DoctorScroll(10));
        let reopened = reduce(reduce(scrolled, Action::ToggleDoctor), Action::ToggleDoctor);

        assert_eq!(reopened.doctor_scroll, 0);
    }

    #[test]
    fn growing_the_terminal_pulls_the_scroll_back_in_range() {
        let scrolled = reduce(ready(), Action::DoctorScroll(500));

        let taller = reduce(scrolled.clone(), Action::ViewportChanged(60));

        assert!(taller.doctor_scroll < scrolled.doctor_scroll);
        assert_eq!(taller.terminal_height, 60);
    }

    // --- terminal picker and editor launch ---

    #[test]
    fn opening_the_picker_highlights_the_current_terminal() {
        let mut s = state();
        s.config.terminal = TerminalKind::Wezterm;

        let opened = reduce(s, Action::OpenTerminalPicker);

        let index = TerminalKind::ALL
            .iter()
            .position(|k| *k == TerminalKind::Wezterm);
        assert_eq!(opened.terminal_picker, index);
        assert_eq!(
            reduce(state(), Action::OpenTerminalPicker).terminal_picker,
            Some(0)
        );
    }

    #[test]
    fn picker_moves_and_wraps_around() {
        let last = TerminalKind::ALL.len() - 1;
        let open = reduce(state(), Action::OpenTerminalPicker);

        let down = reduce(open.clone(), Action::TerminalPickerMove(1));
        let wrapped_up = reduce(open.clone(), Action::TerminalPickerMove(-1));
        let wrapped_down = reduce(wrapped_up.clone(), Action::TerminalPickerMove(1));

        assert_eq!(down.terminal_picker, Some(1));
        assert_eq!(wrapped_up.terminal_picker, Some(last));
        assert_eq!(wrapped_down.terminal_picker, Some(0));
    }

    #[test]
    fn picker_moves_are_ignored_while_closed() {
        assert_eq!(
            reduce(state(), Action::TerminalPickerMove(1)).terminal_picker,
            None
        );
    }

    #[test]
    fn selecting_applies_the_highlighted_terminal_and_closes_the_picker() {
        let open = reduce(state(), Action::OpenTerminalPicker);
        let on_iterm = reduce(open, Action::TerminalPickerMove(1));

        let chosen = reduce(on_iterm, Action::TerminalPickerSelect);

        assert_eq!(chosen.config.terminal, TerminalKind::ALL[1]);
        assert_eq!(chosen.terminal_picker, None);
        assert!(
            chosen
                .status
                .unwrap()
                .contains(TerminalKind::ALL[1].label())
        );
    }

    #[test]
    fn cancelling_leaves_the_setting_alone() {
        let open = reduce(state(), Action::OpenTerminalPicker);
        let moved = reduce(open, Action::TerminalPickerMove(2));

        let cancelled = reduce(moved, Action::TerminalPickerCancel);

        assert_eq!(cancelled.config.terminal, Config::default().terminal);
        assert_eq!(cancelled.terminal_picker, None);
    }

    #[test]
    fn select_without_an_open_picker_does_nothing() {
        let after = reduce(state(), Action::TerminalPickerSelect);

        assert_eq!(after, state());
    }

    #[test]
    fn detected_terminal_is_recorded() {
        let after = reduce(
            state(),
            Action::TerminalDetected(Some(TerminalKind::Iterm2)),
        );

        assert_eq!(after.detected_terminal, Some(TerminalKind::Iterm2));
    }

    #[test]
    fn open_editor_shows_progress_and_the_result_records_the_socket() {
        let asked = reduce(state(), Action::OpenEditor);
        let opened = reduce(
            asked.clone(),
            Action::EditorOpened(PathBuf::from("/tmp/lf/nvim.sock")),
        );

        assert!(asked.status.unwrap().contains("Opening Neovim"));
        assert_eq!(
            opened.editor_socket,
            Some(PathBuf::from("/tmp/lf/nvim.sock"))
        );
        assert!(opened.status.unwrap().contains("Neovim"));
    }

    // --- file tree ---

    fn loaded(paths: &[&str]) -> AppState {
        let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        reduce(state(), Action::FilesLoaded(std::sync::Arc::new(list)))
    }

    fn row_names(state: &AppState) -> Vec<String> {
        state
            .tree
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    fn selected_path(state: &AppState) -> PathBuf {
        state.tree[state.tree_selected].path.clone()
    }

    #[test]
    fn the_tree_starts_in_the_loading_state_with_focus_on_files() {
        let fresh = state();

        assert!(fresh.files_loading);
        assert!(fresh.tree.is_empty());
        assert_eq!(fresh.focus, Panel::Files);
    }

    #[test]
    fn loaded_files_become_tree_rows_and_end_loading() {
        let s = loaded(&["src/App.kt", "README.md"]);

        assert!(!s.files_loading);
        assert_eq!(row_names(&s), vec!["src", "README.md"]);
        assert_eq!(s.tree_selected, 0);
    }

    #[test]
    fn refresh_marks_loading_without_dropping_the_tree() {
        let s = reduce(loaded(&["a.txt"]), Action::RefreshFiles);

        assert!(s.files_loading);
        assert_eq!(row_names(&s), vec!["a.txt"]);
    }

    #[test]
    fn moving_clamps_at_both_ends() {
        let s = loaded(&["a", "b", "c"]);

        let down = reduce(s.clone(), Action::TreeMove(10));
        let up = reduce(down.clone(), Action::TreeMove(-10));

        assert_eq!(down.tree_selected, 2);
        assert_eq!(up.tree_selected, 0);
        assert_eq!(
            reduce(state(), Action::TreeMove(1)).tree_selected,
            0,
            "empty tree"
        );
    }

    #[test]
    fn moving_scrolls_to_keep_the_selection_visible() {
        let names: Vec<String> = (0..60).map(|i| format!("f{i:02}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let s = reduce(loaded(&refs), Action::ViewportChanged(24)); // 10 rows fit
        assert_eq!(files_viewport_rows(24), 10);

        let at_bottom = reduce(s.clone(), Action::TreeMove(9));
        let one_past = reduce(s.clone(), Action::TreeMove(10));
        let far = reduce(s, Action::TreeMove(40));
        let back_up = reduce(far.clone(), Action::TreeMove(-40));

        assert_eq!((at_bottom.tree_top, one_past.tree_top), (0, 1));
        assert_eq!(far.tree_top, 31, "40 - 10 + 1");
        assert_eq!((back_up.tree_selected, back_up.tree_top), (0, 0));
    }

    #[test]
    fn toggle_opens_and_closes_a_directory_keeping_the_selection_on_it() {
        let s = loaded(&["a/x.txt", "b/y.txt"]);
        let on_b = reduce(s, Action::TreeMove(1));

        let open = reduce(on_b, Action::TreeToggle);
        let closed = reduce(open.clone(), Action::TreeToggle);

        assert_eq!(row_names(&open), vec!["a", "b", "  y.txt"]);
        assert_eq!(selected_path(&open), PathBuf::from("b"));
        assert_eq!(row_names(&closed), vec!["a", "b"]);
        assert_eq!(selected_path(&closed), PathBuf::from("b"));
    }

    #[test]
    fn toggle_does_nothing_on_a_file() {
        let s = loaded(&["a.txt"]);

        assert_eq!(reduce(s.clone(), Action::TreeToggle), s);
    }

    #[test]
    fn expand_opens_a_closed_directory_then_steps_into_it() {
        let s = loaded(&["a/x.txt", "a/y.txt"]);

        let open = reduce(s, Action::TreeExpand);
        let inside = reduce(open.clone(), Action::TreeExpand);

        assert_eq!(row_names(&open), vec!["a", "  x.txt", "  y.txt"]);
        assert_eq!(open.tree_selected, 0);
        assert_eq!(selected_path(&inside), PathBuf::from("a/x.txt"));
    }

    #[test]
    fn collapse_closes_an_open_directory_or_steps_out_to_the_parent() {
        let s = reduce(loaded(&["a/x.txt"]), Action::TreeExpand);
        let in_file = reduce(s.clone(), Action::TreeMove(1));
        assert_eq!(selected_path(&in_file), PathBuf::from("a/x.txt"));

        let to_parent = reduce(in_file, Action::TreeCollapse);
        let closed = reduce(to_parent.clone(), Action::TreeCollapse);

        assert_eq!(selected_path(&to_parent), PathBuf::from("a"));
        assert_eq!(
            row_names(&to_parent),
            vec!["a", "  x.txt"],
            "stepping out leaves it open"
        );
        assert_eq!(row_names(&closed), vec!["a"]);
    }

    #[test]
    fn collapse_at_the_top_level_does_nothing() {
        let s = loaded(&["a.txt"]);

        assert_eq!(reduce(s.clone(), Action::TreeCollapse), s);
    }

    #[test]
    fn a_rescan_keeps_the_selection_on_the_same_file() {
        let s = reduce(loaded(&["b.txt", "c.txt"]), Action::TreeMove(1));
        assert_eq!(selected_path(&s), PathBuf::from("c.txt"));

        let rescanned = reduce(
            s,
            Action::FilesLoaded(std::sync::Arc::new(vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b.txt"),
                PathBuf::from("c.txt"),
            ])),
        );

        assert_eq!(selected_path(&rescanned), PathBuf::from("c.txt"));
        assert_eq!(rescanned.tree_selected, 2);
    }

    #[test]
    fn a_rescan_that_removes_the_selected_file_clamps_the_selection() {
        let s = reduce(loaded(&["a.txt", "b.txt", "c.txt"]), Action::TreeMove(2));

        let rescanned = reduce(
            s,
            Action::FilesLoaded(std::sync::Arc::new(vec![PathBuf::from("a.txt")])),
        );

        assert_eq!(rescanned.tree_selected, 0);
    }

    #[test]
    fn open_file_needs_a_running_editor() {
        let without = reduce(
            state(),
            Action::OpenFile {
                path: PathBuf::from("a.txt"),
                line: None,
            },
        );
        let mut with_editor = state();
        with_editor.editor_socket = Some(PathBuf::from("/tmp/s"));
        let with = reduce(
            with_editor,
            Action::OpenFile {
                path: PathBuf::from("a.txt"),
                line: Some(3),
            },
        );

        assert!(
            without
                .status
                .unwrap()
                .contains("Opening Neovim, then a.txt")
        );
        assert!(with.status.unwrap().contains("a.txt"));
    }

    #[test]
    fn a_lost_editor_is_forgotten() {
        let mut s = state();
        s.editor_socket = Some(PathBuf::from("/tmp/s"));

        let lost = reduce(s, Action::EditorLost);

        assert_eq!(lost.editor_socket, None);
        assert!(lost.status.unwrap().contains("press e"));
    }

    // --- finder ---

    fn fm(path: &str) -> FileMatch {
        FileMatch {
            path: PathBuf::from(path),
            score: 1,
            indices: vec![],
        }
    }

    fn finder_with(query: &str, paths: &[&str]) -> AppState {
        let mut s = reduce(state(), Action::OpenFinder);
        for ch in query.chars() {
            s = reduce(s, Action::FinderInput(ch));
        }
        reduce(
            s,
            Action::FinderResults {
                query: query.to_string(),
                matches: paths.iter().map(|p| fm(p)).collect(),
            },
        )
    }

    #[test]
    fn opening_the_finder_starts_with_an_empty_query() {
        let s = reduce(state(), Action::OpenFinder);

        let finder = s.finder.unwrap();
        assert!(finder.query.is_empty() && finder.matches.is_empty());
        assert_eq!(finder.selected, 0);
    }

    #[test]
    fn typing_and_backspace_edit_the_query_and_reset_the_highlight() {
        let mut s = finder_with("", &["a", "b", "c"]);
        s = reduce(s, Action::FinderMove(2));
        assert_eq!(s.finder.as_ref().unwrap().selected, 2);

        s = reduce(s, Action::FinderInput('k'));
        s = reduce(s, Action::FinderInput('t'));
        assert_eq!(s.finder.as_ref().unwrap().query, "kt");
        assert_eq!(s.finder.as_ref().unwrap().selected, 0);

        s = reduce(s, Action::FinderBackspace);
        s = reduce(s, Action::FinderBackspace);
        s = reduce(s, Action::FinderBackspace);
        assert_eq!(s.finder.unwrap().query, "");
    }

    #[test]
    fn control_characters_and_over_long_queries_are_not_accepted() {
        let mut s = reduce(state(), Action::OpenFinder);
        s = reduce(s, Action::FinderInput('\x1b'));
        s = reduce(s, Action::FinderInput('\n'));
        assert_eq!(s.finder.as_ref().unwrap().query, "");

        for _ in 0..(MAX_QUERY_CHARS + 50) {
            s = reduce(s, Action::FinderInput('a'));
        }
        assert_eq!(s.finder.unwrap().query.chars().count(), MAX_QUERY_CHARS);
    }

    #[test]
    fn the_finder_highlight_moves_within_the_results() {
        let s = finder_with("x", &["a", "b", "c"]);

        let down = reduce(s.clone(), Action::FinderMove(1));
        let past_end = reduce(s.clone(), Action::FinderMove(50));
        let up_from_top = reduce(s, Action::FinderMove(-5));

        assert_eq!(down.finder.unwrap().selected, 1);
        assert_eq!(past_end.finder.unwrap().selected, 2);
        assert_eq!(up_from_top.finder.unwrap().selected, 0);
    }

    #[test]
    fn results_for_an_older_query_are_ignored() {
        let s = reduce(
            reduce(state(), Action::OpenFinder),
            Action::FinderInput('a'),
        );

        let stale = reduce(
            s.clone(),
            Action::FinderResults {
                query: String::new(),
                matches: vec![fm("old")],
            },
        );
        let fresh = reduce(
            s,
            Action::FinderResults {
                query: "a".into(),
                matches: vec![fm("new")],
            },
        );

        assert!(stale.finder.unwrap().matches.is_empty());
        assert_eq!(fresh.finder.unwrap().matches[0].path, PathBuf::from("new"));
    }

    #[test]
    fn results_arriving_after_the_finder_closed_are_dropped() {
        let closed = reduce(reduce(state(), Action::OpenFinder), Action::FinderCancel);

        let after = reduce(
            closed,
            Action::FinderResults {
                query: String::new(),
                matches: vec![fm("x")],
            },
        );

        assert_eq!(after.finder, None);
    }

    #[test]
    fn finder_input_without_an_open_finder_does_nothing() {
        assert_eq!(reduce(state(), Action::FinderInput('a')), state());
        assert_eq!(reduce(state(), Action::FinderBackspace), state());
        assert_eq!(reduce(state(), Action::FinderMove(1)), state());
    }

    // --- project search ---

    fn hit(path: &str, line: u32) -> GrepHit {
        GrepHit {
            path: PathBuf::from(path),
            line,
            column: 0,
            text: "t".into(),
        }
    }

    fn grep_typed(query: &str) -> AppState {
        query
            .chars()
            .fold(reduce(state(), Action::OpenGrep), |s, ch| {
                reduce(s, Action::GrepInput(ch))
            })
    }

    #[test]
    fn searching_marks_running_and_clears_old_results() {
        let s = reduce(grep_typed("setActive"), Action::GrepSearch);

        let grep = s.grep.unwrap();
        assert!(grep.running);
        assert_eq!(grep.searched.as_deref(), Some("setActive"));
        assert!(grep.hits.is_empty() && grep.error.is_none());
    }

    #[test]
    fn results_fill_the_list_and_stop_the_spinner() {
        let s = reduce(grep_typed("x"), Action::GrepSearch);

        let done = reduce(
            s,
            Action::GrepResults {
                query: "x".into(),
                result: Ok(vec![hit("a.kt", 1), hit("b.kt", 2)]),
            },
        );

        let grep = done.grep.unwrap();
        assert!(!grep.running);
        assert_eq!(grep.hits.len(), 2);
        assert_eq!(grep.selected, 0);
    }

    #[test]
    fn a_failed_search_shows_its_reason() {
        let s = reduce(grep_typed("("), Action::GrepSearch);

        let done = reduce(
            s,
            Action::GrepResults {
                query: "(".into(),
                result: Err("error: unclosed group".into()),
            },
        );

        let grep = done.grep.unwrap();
        assert_eq!(grep.error.as_deref(), Some("error: unclosed group"));
        assert!(!grep.running && grep.hits.is_empty());
    }

    #[test]
    fn results_for_a_superseded_search_are_ignored() {
        let first = reduce(grep_typed("a"), Action::GrepSearch);
        let second = reduce(reduce(first, Action::GrepInput('b')), Action::GrepSearch);

        let stale = reduce(
            second.clone(),
            Action::GrepResults {
                query: "a".into(),
                result: Ok(vec![hit("old.kt", 1)]),
            },
        );

        assert!(stale.grep.as_ref().unwrap().hits.is_empty());
        assert!(
            stale.grep.unwrap().running,
            "still waiting for the 'ab' search"
        );
    }

    #[test]
    fn grep_highlight_moves_and_clamps() {
        let s = reduce(grep_typed("x"), Action::GrepSearch);
        let s = reduce(
            s,
            Action::GrepResults {
                query: "x".into(),
                result: Ok(vec![hit("a", 1), hit("b", 2), hit("c", 3)]),
            },
        );

        assert_eq!(
            reduce(s.clone(), Action::GrepMove(1))
                .grep
                .unwrap()
                .selected,
            1
        );
        assert_eq!(
            reduce(s.clone(), Action::GrepMove(99))
                .grep
                .unwrap()
                .selected,
            2
        );
        assert_eq!(reduce(s, Action::GrepMove(-1)).grep.unwrap().selected, 0);
    }

    #[test]
    fn editing_the_query_keeps_the_old_results_until_the_next_search() {
        let s = reduce(grep_typed("x"), Action::GrepSearch);
        let s = reduce(
            s,
            Action::GrepResults {
                query: "x".into(),
                result: Ok(vec![hit("a", 1)]),
            },
        );

        let edited = reduce(s, Action::GrepInput('y'));

        let grep = edited.grep.unwrap();
        assert_eq!(grep.query, "xy");
        assert_eq!(grep.hits.len(), 1);
        assert_eq!(grep.searched.as_deref(), Some("x"));
    }

    #[test]
    fn a_blank_query_does_not_start_a_search() {
        let s = reduce(grep_typed("   "), Action::GrepSearch);

        assert!(!s.grep.unwrap().running);
    }

    // --- overlays together ---

    #[test]
    fn opening_a_file_closes_the_overlays() {
        let mut with_editor = state();
        with_editor.editor_socket = Some(PathBuf::from("/tmp/s"));
        let both = reduce(reduce(with_editor, Action::OpenFinder), Action::OpenGrep);

        let after = reduce(
            both,
            Action::OpenFile {
                path: PathBuf::from("a.kt"),
                line: Some(2),
            },
        );

        assert!(after.finder.is_none() && after.grep.is_none());
    }

    #[test]
    fn opening_one_overlay_closes_the_other() {
        let finder = reduce(state(), Action::OpenFinder);

        let grep = reduce(finder, Action::OpenGrep);
        assert!(grep.finder.is_none() && grep.grep.is_some());

        let back = reduce(grep, Action::OpenFinder);
        assert!(back.grep.is_none() && back.finder.is_some());
    }

    #[test]
    fn cancel_closes_each_overlay() {
        assert!(
            reduce(reduce(state(), Action::OpenGrep), Action::GrepCancel)
                .grep
                .is_none()
        );
        assert!(
            reduce(reduce(state(), Action::OpenFinder), Action::FinderCancel)
                .finder
                .is_none()
        );
    }

    fn choose(state: AppState, name: &str, line: Option<u32>) -> AppState {
        reduce(
            state,
            Action::OpenFile {
                path: PathBuf::from(name),
                line,
            },
        )
    }

    #[test]
    fn choosing_a_file_without_an_editor_starts_one_and_remembers_the_file() {
        let finder = finder_with("itemdet", &["a.kt"]);

        let after = choose(finder, "a.kt", Some(7));

        assert!(after.finder.is_none(), "the choice ends the overlay");
        assert_eq!(after.pending_open, Some((PathBuf::from("a.kt"), Some(7))));
        assert!(after.editor_launching && after.editor_launch_wanted);
        assert_eq!(after.status.as_deref(), Some("Opening Neovim, then a.kt…"));
    }

    #[test]
    fn a_second_choice_while_neovim_is_starting_only_replaces_the_file() {
        let first = choose(state(), "a.kt", None);
        let started = reduce(first, Action::EditorLaunchStarted);
        assert!(!started.editor_launch_wanted && started.editor_launching);

        let second = choose(started, "b.kt", Some(2));

        assert_eq!(second.pending_open, Some((PathBuf::from("b.kt"), Some(2))));
        assert!(!second.editor_launch_wanted, "no second Neovim");
    }

    #[test]
    fn pressing_e_twice_does_not_start_two_neovims() {
        let first = reduce(state(), Action::OpenEditor);
        assert!(first.editor_launch_wanted && first.editor_launching);
        let started = reduce(first, Action::EditorLaunchStarted);

        let again = reduce(started, Action::OpenEditor);

        assert!(!again.editor_launch_wanted);
        assert!(again.status.unwrap().contains("already opening"));
    }

    #[test]
    fn when_neovim_is_up_the_waiting_file_stays_queued_until_it_is_handed_over() {
        let waiting = reduce(
            choose(state(), "a.kt", Some(3)),
            Action::EditorLaunchStarted,
        );

        let up = reduce(waiting, Action::EditorOpened(PathBuf::from("/tmp/s")));
        assert!(!up.editor_launching);
        assert_eq!(up.pending_open, Some((PathBuf::from("a.kt"), Some(3))));
        let handed = reduce(up, Action::PendingOpenSent);

        assert_eq!(handed.pending_open, None);
        assert!(handed.editor_socket.is_some());
    }

    #[test]
    fn a_launch_that_fails_drops_the_file_and_says_why() {
        let waiting = reduce(choose(state(), "a.kt", None), Action::EditorLaunchStarted);

        let failed = reduce(
            waiting,
            Action::EditorLaunchFailed("tmux is not running".into()),
        );

        assert_eq!(failed.pending_open, None);
        assert!(!failed.editor_launching && !failed.editor_launch_wanted);
        assert_eq!(failed.status.as_deref(), Some("tmux is not running"));
        // The next choice may try again.
        assert!(choose(failed, "b.kt", None).editor_launch_wanted);
    }

    #[test]
    fn a_neovim_found_gone_does_not_cancel_a_launch_that_is_already_under_way() {
        let mut waiting = choose(state(), "a.kt", None);
        waiting.editor_socket = None;
        let started = reduce(waiting, Action::EditorLaunchStarted);

        let lost = reduce(started, Action::EditorLost);

        assert_eq!(lost.pending_open, Some((PathBuf::from("a.kt"), None)));
        assert!(lost.editor_launching, "no second Neovim may be started");
        // And pressing e or choosing again does not start another.
        let again = choose(reduce(lost, Action::OpenEditor), "b.kt", None);
        assert!(!again.editor_launch_wanted);
    }

    #[test]
    fn a_dead_neovim_is_forgotten_but_nothing_waits_for_it() {
        let mut s = state();
        s.editor_socket = Some(PathBuf::from("/tmp/dead.sock"));
        s.editor_ready = true;

        let lost = reduce(s, Action::EditorLost);

        assert_eq!(lost.editor_socket, None);
        assert!(!lost.editor_ready && !lost.editor_launching);
        // The next choice starts a fresh Neovim.
        assert!(choose(lost, "a.kt", None).editor_launch_wanted);
    }

    #[test]
    fn with_a_running_neovim_nothing_waits() {
        let mut s = state();
        s.editor_socket = Some(PathBuf::from("/tmp/s"));

        let after = choose(s, "a.kt", None);

        assert_eq!(
            (after.pending_open, after.editor_launch_wanted),
            (None, false)
        );
    }

    #[test]
    fn a_search_hit_chosen_without_an_editor_opens_neovim_at_that_line() {
        let s = reduce(grep_typed("setActive"), Action::GrepSearch);

        let after = reduce(
            s,
            Action::OpenFile {
                path: PathBuf::from("a.kt"),
                line: Some(3),
            },
        );

        assert!(after.grep.is_none());
        assert_eq!(after.pending_open, Some((PathBuf::from("a.kt"), Some(3))));
        assert!(after.editor_launch_wanted);
    }

    #[test]
    fn a_shown_file_proves_the_editor_is_reachable_until_it_is_lost_or_relaunched() {
        let mut s = state();
        s.editor_socket = Some(PathBuf::from("/tmp/s"));
        assert!(!s.editor_ready);

        let shown = reduce(s, Action::FileOpened(PathBuf::from("src/App.kt")));
        let lost = reduce(shown.clone(), Action::EditorLost);
        let relaunched = reduce(
            shown.clone(),
            Action::EditorOpened(PathBuf::from("/tmp/new")),
        );

        assert!(shown.editor_ready);
        assert_eq!(shown.status.as_deref(), Some("Opened src/App.kt"));
        assert!(!lost.editor_ready);
        assert!(!relaunched.editor_ready, "a new Neovim is unproven");
    }

    // --- build target, devices and their start/stop ---

    use crate::devices::{DeviceState as DState, SdkInfo};
    use crate::prefs::ProjectPrefs;

    fn dev(platform: Platform, id: &str, name: &str, state: DState) -> Device {
        Device {
            platform,
            id: id.into(),
            name: name.into(),
            detail: String::new(),
            state,
            serial: (platform == Platform::Android && state == DState::Running)
                .then(|| "emulator-5554".to_string()),
        }
    }

    fn pixel(state: DState) -> Device {
        dev(Platform::Android, "Pixel_8", "Pixel_8", state)
    }

    fn sample_devices() -> Vec<Device> {
        vec![
            dev(Platform::Android, "Pixel_8", "Pixel_8", DState::Stopped),
            dev(Platform::Android, "Tablet", "Tablet", DState::Stopped),
            dev(
                Platform::Ios,
                "11111111-1111-1111-1111-111111111111",
                "iPad (A16)",
                DState::Stopped,
            ),
            dev(
                Platform::Ios,
                "22222222-2222-2222-2222-222222222222",
                "iPhone 17",
                DState::Stopped,
            ),
            dev(
                Platform::Ios,
                "33333333-3333-3333-3333-333333333333",
                "iPhone 17 Pro",
                DState::Stopped,
            ),
        ]
    }

    fn with_devices(devices: Vec<Device>) -> AppState {
        reduce(
            state(),
            Action::DevicesLoaded {
                devices: Arc::new(devices),
                errors: BTreeMap::new(),
            },
        )
    }

    fn selected_id(s: &AppState, platform: Platform) -> Option<&str> {
        s.selected_devices.get(&platform).map(String::as_str)
    }

    #[test]
    fn a_new_session_targets_android_with_nothing_loaded() {
        let s = state();

        assert_eq!(s.target, Platform::Android);
        assert!(!s.devices_loaded && !s.sdk_loaded && s.devices.is_empty());
        assert!(s.selected_device().is_none() && s.pending_commands.is_empty());
    }

    #[test]
    fn toggling_the_target_flips_between_the_platforms() {
        let ios = reduce(state(), Action::ToggleTarget);
        let android = reduce(ios.clone(), Action::ToggleTarget);

        assert_eq!(ios.target, Platform::Ios);
        assert_eq!(android.target, Platform::Android);
    }

    #[test]
    fn the_environment_overlay_toggles() {
        let open = reduce(state(), Action::ToggleInfo);

        assert!(open.info_open);
        assert!(!reduce(open, Action::ToggleInfo).info_open);
    }

    #[test]
    fn focus_can_be_set_directly() {
        assert_eq!(
            reduce(state(), Action::Focus(Panel::Devices)).focus,
            Panel::Devices
        );
    }

    #[test]
    fn sdk_facts_are_stored() {
        let mut sdk = SdkInfo::default();
        sdk.ios.version = Some("Xcode 26.5".into());

        let s = reduce(state(), Action::SdkLoaded(sdk.clone()));

        assert!(s.sdk_loaded);
        assert_eq!(s.sdk, sdk);
    }

    #[test]
    fn loading_devices_picks_a_sensible_default_per_platform() {
        let s = with_devices(sample_devices());

        assert!(s.devices_loaded);
        assert_eq!(selected_id(&s, Platform::Android), Some("Pixel_8"));
        assert_eq!(
            selected_id(&s, Platform::Ios),
            Some("22222222-2222-2222-2222-222222222222"),
            "the first iPhone, not the iPad that sorts first"
        );
    }

    #[test]
    fn a_running_device_is_preferred_as_the_default() {
        let mut devices = sample_devices();
        devices[1].state = DState::Running;
        devices[4].state = DState::Running;

        let s = with_devices(devices);

        assert_eq!(selected_id(&s, Platform::Android), Some("Tablet"));
        assert_eq!(
            selected_id(&s, Platform::Ios),
            Some("33333333-3333-3333-3333-333333333333")
        );
    }

    #[test]
    fn an_existing_choice_survives_a_refresh_while_the_device_exists() {
        let mut s = with_devices(sample_devices());
        s = reduce(s, Action::DeviceMove(1));
        assert_eq!(selected_id(&s, Platform::Android), Some("Tablet"));

        let refreshed = reduce(
            s,
            Action::DevicesLoaded {
                devices: Arc::new(sample_devices()),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(selected_id(&refreshed, Platform::Android), Some("Tablet"));
    }

    #[test]
    fn a_vanished_device_is_replaced_by_a_default() {
        let mut s = with_devices(sample_devices());
        s = reduce(s, Action::DeviceMove(1));
        let remaining: Vec<Device> = sample_devices()
            .into_iter()
            .filter(|d| d.id != "Tablet")
            .collect();

        let after = reduce(
            s,
            Action::DevicesLoaded {
                devices: Arc::new(remaining),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(selected_id(&after, Platform::Android), Some("Pixel_8"));
    }

    #[test]
    fn remembered_choices_apply_once_the_devices_load() {
        let prefs = ProjectPrefs {
            target: Some(Platform::Ios),
            android_device: Some("Tablet".into()),
            ios_device: Some("33333333-3333-3333-3333-333333333333".into()),
        };

        let loaded = reduce(state(), Action::PrefsLoaded(prefs));
        assert_eq!(loaded.target, Platform::Ios);
        let with_list = reduce(
            loaded,
            Action::DevicesLoaded {
                devices: Arc::new(sample_devices()),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(selected_id(&with_list, Platform::Android), Some("Tablet"));
        assert_eq!(
            selected_id(&with_list, Platform::Ios),
            Some("33333333-3333-3333-3333-333333333333")
        );
    }

    #[test]
    fn a_platform_that_could_not_be_listed_keeps_its_remembered_choice() {
        let prefs = ProjectPrefs {
            android_device: Some("Tablet".into()),
            ..ProjectPrefs::default()
        };
        let loaded = reduce(state(), Action::PrefsLoaded(prefs));
        let only_ios: Vec<Device> = sample_devices()
            .into_iter()
            .filter(|d| d.platform == Platform::Ios)
            .collect();
        let errors = BTreeMap::from([(Platform::Android, "emulator not found".to_string())]);

        let after = reduce(
            loaded,
            Action::DevicesLoaded {
                devices: Arc::new(only_ios),
                errors,
            },
        );

        assert_eq!(
            selected_id(&after, Platform::Android),
            Some("Tablet"),
            "not forgotten just because adb is missing today"
        );
        assert_eq!(
            after.device_errors[&Platform::Android],
            "emulator not found"
        );
    }

    #[test]
    fn the_prefs_to_remember_reflect_the_target_and_choices() {
        let mut s = with_devices(sample_devices());
        s = reduce(s, Action::ToggleTarget);

        let prefs = s.prefs();

        assert_eq!(prefs.target, Some(Platform::Ios));
        assert_eq!(prefs.android_device.as_deref(), Some("Pixel_8"));
        assert_eq!(
            prefs.ios_device.as_deref(),
            Some("22222222-2222-2222-2222-222222222222")
        );
    }

    #[test]
    fn moving_walks_the_current_targets_devices_only() {
        let s = with_devices(sample_devices());

        let down = reduce(s.clone(), Action::DeviceMove(1));
        let past_end = reduce(s.clone(), Action::DeviceMove(50));
        let up = reduce(s.clone(), Action::DeviceMove(-5));
        let ios = reduce(reduce(s, Action::ToggleTarget), Action::DeviceMove(1));

        assert_eq!(selected_id(&down, Platform::Android), Some("Tablet"));
        assert_eq!(
            selected_id(&past_end, Platform::Android),
            Some("Tablet"),
            "clamped to the last Android device"
        );
        assert_eq!(selected_id(&up, Platform::Android), Some("Pixel_8"));
        assert_eq!(
            selected_id(&ios, Platform::Ios),
            Some("33333333-3333-3333-3333-333333333333")
        );
        assert_eq!(
            selected_id(&ios, Platform::Android),
            Some("Pixel_8"),
            "the other platform keeps its choice"
        );
    }

    #[test]
    fn moving_with_no_devices_does_nothing() {
        assert_eq!(reduce(state(), Action::DeviceMove(1)), state());
    }

    #[test]
    fn the_device_list_scrolls_to_keep_the_choice_visible() {
        let many: Vec<Device> = (0..40)
            .map(|i| {
                dev(
                    Platform::Android,
                    &format!("avd{i:02}"),
                    &format!("avd{i:02}"),
                    DState::Stopped,
                )
            })
            .collect();
        let s = reduce(with_devices(many), Action::ViewportChanged(30)); // 12 rows

        let far = reduce(s.clone(), Action::DeviceMove(30));
        let back = reduce(far.clone(), Action::DeviceMove(-30));

        assert_eq!(crate::layout::devices_viewport_rows(30), 12);
        assert_eq!(far.device_top, 30 + 1 - 12);
        assert_eq!(back.device_top, 0);
    }

    // --- start and stop ---

    #[test]
    fn booting_marks_the_device_starting_and_queues_the_command() {
        let s = with_devices(sample_devices());

        let booting = reduce(s, Action::BootDevice);

        assert_eq!(booting.pending_commands.len(), 1);
        assert_eq!(booting.pending_commands[0].action, DeviceAction::Boot);
        assert_eq!(booting.pending_commands[0].device.id, "Pixel_8");
        assert!(booting.status.unwrap().contains("Starting Pixel_8"));
        assert!(
            booting
                .transitions
                .contains_key(&(Platform::Android, "Pixel_8".to_string()))
        );
    }

    #[test]
    fn the_list_shows_a_device_as_starting_while_it_boots() {
        let booting = reduce(with_devices(sample_devices()), Action::BootDevice);

        let polled = reduce(
            booting,
            Action::DevicesLoaded {
                devices: Arc::new(sample_devices()),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(
            polled.selected_device().unwrap().state,
            DState::Starting,
            "still stopped for adb, but we know it was asked to start"
        );
    }

    #[test]
    fn a_running_device_ends_the_wait() {
        let booting = reduce(with_devices(sample_devices()), Action::BootDevice);
        let mut up = sample_devices();
        up[0].state = DState::Running;

        let after = reduce(
            booting,
            Action::DevicesLoaded {
                devices: Arc::new(up),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(after.selected_device().unwrap().state, DState::Running);
        assert!(after.transitions.is_empty());
    }

    #[test]
    fn confirming_a_start_replaces_the_starting_message() {
        let booting = reduce(with_devices(sample_devices()), Action::BootDevice);
        let mut up = sample_devices();
        up[0].state = DState::Running;

        let after = reduce(
            booting,
            Action::DevicesLoaded {
                devices: Arc::new(up),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(after.status.as_deref(), Some("Pixel_8 is running"));
    }

    #[test]
    fn starting_a_second_device_of_the_same_kind_mentions_the_one_already_up() {
        let mut devices = sample_devices();
        devices[1].state = DState::Running; // Tablet
        let mut s = with_devices(devices);
        s.selected_devices
            .insert(Platform::Android, "Pixel_8".into());

        let booting = reduce(s, Action::BootDevice);

        let status = booting.status.unwrap();
        assert!(status.starts_with("Starting Pixel_8…"), "{status}");
        assert!(status.contains("Tablet is also running"), "{status}");
    }

    #[test]
    fn a_device_of_the_other_platform_or_a_stop_adds_no_note() {
        let mut devices = sample_devices();
        devices[2].state = DState::Running; // an iPad, while an Android one is chosen
        let booting = reduce(with_devices(devices), Action::BootDevice);

        assert_eq!(booting.status.as_deref(), Some("Starting Pixel_8…"));
    }

    #[test]
    fn the_start_message_is_still_replaced_when_it_carried_a_note() {
        let mut devices = sample_devices();
        devices[1].state = DState::Running;
        let mut s = with_devices(devices.clone());
        s.selected_devices
            .insert(Platform::Android, "Pixel_8".into());
        let booting = reduce(s, Action::BootDevice);
        devices[0].state = DState::Running;

        let after = reduce(
            booting,
            Action::DevicesLoaded {
                devices: Arc::new(devices),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(after.status.as_deref(), Some("Pixel_8 is running"));
    }

    #[test]
    fn confirming_a_start_leaves_an_unrelated_message_alone() {
        let mut booting = reduce(with_devices(sample_devices()), Action::BootDevice);
        booting.status = Some("Opened src/App.kt".to_string());
        let mut up = sample_devices();
        up[0].state = DState::Running;

        let after = reduce(
            booting,
            Action::DevicesLoaded {
                devices: Arc::new(up),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(after.status.as_deref(), Some("Opened src/App.kt"));
    }

    #[test]
    fn a_device_that_never_comes_up_is_eventually_shown_as_stopped_again() {
        let mut s = reduce(with_devices(sample_devices()), Action::BootDevice);

        for _ in 0..=MAX_TRANSITION_POLLS {
            s = reduce(
                s,
                Action::DevicesLoaded {
                    devices: Arc::new(sample_devices()),
                    errors: BTreeMap::new(),
                },
            );
        }

        assert_eq!(s.selected_device().unwrap().state, DState::Stopped);
        assert!(s.transitions.is_empty());
    }

    #[test]
    fn giving_up_on_a_start_says_so() {
        let mut s = reduce(with_devices(sample_devices()), Action::BootDevice);

        for _ in 0..=MAX_TRANSITION_POLLS {
            s = reduce(
                s,
                Action::DevicesLoaded {
                    devices: Arc::new(sample_devices()),
                    errors: BTreeMap::new(),
                },
            );
        }

        assert_eq!(s.status.as_deref(), Some("Pixel_8 did not start"));
    }

    #[test]
    fn a_failed_listing_keeps_the_pending_start_of_that_platform() {
        let booting = reduce(with_devices(sample_devices()), Action::BootDevice);
        let mut errors = BTreeMap::new();
        errors.insert(Platform::Android, "emulator: timed out".to_string());

        let after = reduce(
            booting,
            Action::DevicesLoaded {
                devices: Arc::new(Vec::new()),
                errors,
            },
        );
        let recovered = reduce(
            after,
            Action::DevicesLoaded {
                devices: Arc::new(sample_devices()),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(recovered.selected_device().unwrap().state, DState::Starting);
    }

    #[test]
    fn stopping_waits_for_the_device_to_actually_stop() {
        let mut devices = sample_devices();
        devices[0].state = DState::Running;
        devices[0].serial = Some("emulator-5554".into());
        let s = with_devices(devices.clone());

        let stopping = reduce(s, Action::StopDevice);
        let still_up = reduce(
            stopping.clone(),
            Action::DevicesLoaded {
                devices: Arc::new(devices),
                errors: BTreeMap::new(),
            },
        );
        let down = reduce(
            still_up.clone(),
            Action::DevicesLoaded {
                devices: Arc::new(sample_devices()),
                errors: BTreeMap::new(),
            },
        );

        assert_eq!(stopping.pending_commands[0].action, DeviceAction::Stop);
        assert_eq!(still_up.selected_device().unwrap().state, DState::Stopping);
        assert_eq!(down.selected_device().unwrap().state, DState::Stopped);
        assert!(down.transitions.is_empty());
    }

    #[test]
    fn boot_and_stop_are_refused_when_pointless() {
        let none = reduce(state(), Action::BootDevice);
        let mut running = sample_devices();
        running[0].state = DState::Running;
        let already = reduce(with_devices(running), Action::BootDevice);
        let not_running = reduce(with_devices(sample_devices()), Action::StopDevice);

        assert!(
            none.pending_commands.is_empty()
                && none.status.as_deref().unwrap().contains("No device")
        );
        assert!(
            already.pending_commands.is_empty()
                && already
                    .status
                    .as_deref()
                    .unwrap()
                    .contains("already running")
        );
        assert!(
            not_running.pending_commands.is_empty()
                && not_running
                    .status
                    .as_deref()
                    .unwrap()
                    .contains("not running")
        );
    }

    #[test]
    fn a_device_already_starting_cannot_be_started_twice() {
        let once = reduce(with_devices(sample_devices()), Action::BootDevice);
        let polled = reduce(
            once,
            Action::DevicesLoaded {
                devices: Arc::new(sample_devices()),
                errors: BTreeMap::new(),
            },
        );

        let twice = reduce(polled, Action::BootDevice);

        assert_eq!(
            twice.pending_commands.len(),
            1,
            "still just the first request"
        );
    }

    #[test]
    fn a_failed_start_clears_the_wait_and_says_why() {
        let booting = reduce(with_devices(sample_devices()), Action::BootDevice);

        let failed = reduce(
            booting,
            Action::DeviceActionFailed {
                platform: Platform::Android,
                id: "Pixel_8".into(),
                message: "emulator: denied".into(),
            },
        );

        assert!(failed.transitions.is_empty());
        assert_eq!(failed.status.as_deref(), Some("emulator: denied"));
    }

    #[test]
    fn refresh_asks_for_a_new_listing_without_changing_the_list() {
        let s = with_devices(sample_devices());

        assert_eq!(reduce(s.clone(), Action::RefreshDevices), s);
    }

    #[test]
    fn the_device_selection_stays_visible_when_the_terminal_shrinks() {
        let many: Vec<Device> = (0..40)
            .map(|i| {
                dev(
                    Platform::Android,
                    &format!("avd{i:02}"),
                    &format!("avd{i:02}"),
                    DState::Stopped,
                )
            })
            .collect();
        let s = reduce(
            reduce(with_devices(many), Action::ViewportChanged(60)),
            Action::DeviceMove(30),
        );

        let smaller = reduce(s, Action::ViewportChanged(24));

        let rows = usize::from(crate::layout::devices_viewport_rows(24));
        assert!(
            30 >= smaller.device_top && 30 < smaller.device_top + rows,
            "top {} rows {rows}",
            smaller.device_top
        );
    }

    #[test]
    fn pixel_helper_is_used() {
        assert_eq!(pixel(DState::Stopped).id, "Pixel_8");
    }
}
