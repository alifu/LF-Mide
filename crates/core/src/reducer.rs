//! Pure state transitions.

use crate::action::Action;
use crate::config::{Config, TerminalKind};
use crate::doctor::{Report, popup_content_rows};
use crate::state::{AppState, DoctorPanel};

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
        Action::OpenEditor => AppState {
            status: Some("Opening Neovim…".to_string()),
            ..state
        },
        Action::EditorOpened(socket) => AppState {
            editor_socket: Some(socket),
            status: Some("Neovim opened in a split pane".to_string()),
            ..state
        },
        Action::ViewportChanged(terminal_height) => {
            let resized = AppState {
                terminal_height,
                ..state
            };
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
    use crate::project::{Project, ProjectLayout};
    use crate::state::{BranchInfo, DoctorPanel, Panel};
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
}
