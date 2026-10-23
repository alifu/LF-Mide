//! Pure state transitions.

use std::path::PathBuf;
use std::sync::Arc;

use crate::action::Action;
use crate::config::{Config, TerminalKind};
use crate::doctor::{Report, popup_content_rows};
use crate::layout::files_viewport_rows;
use crate::nav::{parent_row, tree_rows};
use crate::state::{AppState, DoctorPanel, FinderState, GrepState, MAX_QUERY_CHARS};

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
        Action::OpenEditor => AppState {
            status: Some("Opening Neovim…".to_string()),
            ..state
        },
        Action::EditorOpened(socket) => AppState {
            editor_socket: Some(socket),
            // A new Neovim: its socket has not been proven yet.
            editor_ready: false,
            status: Some("Neovim opened in a split pane".to_string()),
            ..state
        },
        Action::FileOpened(path) => AppState {
            editor_ready: true,
            status: Some(format!("Opened {}", path.display())),
            ..state
        },
        Action::EditorLost => AppState {
            editor_socket: None,
            editor_ready: false,
            status: Some("Neovim isn't running: press e to open it".to_string()),
            ..state
        },
        Action::FilesLoaded(files) => rebuild_tree(AppState {
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
        Action::OpenFile { path, .. } => {
            if state.editor_socket.is_none() {
                // Keep the overlay and its query: press e, then choose again.
                return AppState {
                    status: Some("Press e to open the editor first".to_string()),
                    ..state
                };
            }
            // Choosing a file ends whatever overlay it was chosen in.
            AppState {
                status: Some(format!("Opening {}…", path.display())),
                finder: None,
                grep: None,
                ..state
            }
        }
        Action::ViewportChanged(terminal_height) => {
            let selected = state.tree_selected;
            let resized = select_row(
                AppState {
                    terminal_height,
                    ..state
                },
                selected,
            );
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
    let rows = tree_rows(&state.files, &state.expanded);
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
        let s = reduce(loaded(&refs), Action::ViewportChanged(24)); // 12 rows fit
        assert_eq!(files_viewport_rows(24), 12);

        let at_bottom = reduce(s.clone(), Action::TreeMove(11));
        let one_past = reduce(s.clone(), Action::TreeMove(12));
        let far = reduce(s, Action::TreeMove(40));
        let back_up = reduce(far.clone(), Action::TreeMove(-40));

        assert_eq!((at_bottom.tree_top, one_past.tree_top), (0, 1));
        assert_eq!(far.tree_top, 29, "40 - 12 + 1");
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

        assert!(without.status.unwrap().contains("Press e"));
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

    #[test]
    fn choosing_a_file_without_an_editor_keeps_the_overlay_and_its_query() {
        let finder = finder_with("itemdet", &["a.kt"]);

        let after = reduce(
            finder,
            Action::OpenFile {
                path: PathBuf::from("a.kt"),
                line: None,
            },
        );

        assert!(after.status.as_deref().unwrap().contains("Press e"));
        assert_eq!(
            after.finder.unwrap().query,
            "itemdet",
            "the user can press e, then choose again"
        );
    }

    #[test]
    fn the_search_overlay_also_survives_a_missing_editor() {
        let s = reduce(grep_typed("setActive"), Action::GrepSearch);

        let after = reduce(
            s,
            Action::OpenFile {
                path: PathBuf::from("a.kt"),
                line: Some(3),
            },
        );

        assert_eq!(after.grep.unwrap().query, "setActive");
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
}
