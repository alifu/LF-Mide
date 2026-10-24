//! Maps terminal key events to actions. What a key means depends on what is
//! on screen, so the current state is an input.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lf_mide_core::{Action, AppState, DoctorPanel, OutputTab, Panel};

const PAGE_LINES: i16 = 10;

pub fn action_for(state: &AppState, key: KeyEvent) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    // Our Neovim gets every key, Ctrl-C and q included, except the leader.
    if state.editor_has_keys() {
        return editor_key(state, key);
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if matches!(key.code, KeyCode::Char('c')) && ctrl {
        return Some(Action::Quit);
    }
    if state.finder.is_some() {
        return finder_key(state, key);
    }
    if state.grep.is_some() {
        return grep_key(state, key);
    }
    if state.output_filter.editing {
        return filter_key(key);
    }
    if let Some(view) = &state.diff {
        return if view.picker.is_some() {
            base_picker_key(key.code)
        } else {
            diff_key(state, key.code)
        };
    }
    if state.terminal_picker.is_some() {
        return picker_key(key.code);
    }
    if state.info_open {
        return info_key(key.code);
    }
    if state.perf_open {
        return perf_key(key.code);
    }
    match state.doctor {
        DoctorPanel::Closed => main_key(state, key.code, ctrl),
        DoctorPanel::Running | DoctorPanel::Ready(_) => doctor_key(key.code),
    }
}

/// A key for the embedded Neovim; the leader hands focus back to the panels.
fn editor_key(state: &AppState, key: KeyEvent) -> Option<Action> {
    let keys = lf_mide_editor::keys::notation(key)?;
    if lf_mide_editor::keys::leader(&state.config.leader).as_ref() == Some(&keys) {
        return Some(Action::Focus(Panel::Files));
    }
    Some(Action::EditorKeys(keys))
}

fn picker_key(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Esc => Some(Action::TerminalPickerCancel),
        KeyCode::Enter => Some(Action::TerminalPickerSelect),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::TerminalPickerMove(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::TerminalPickerMove(-1)),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

fn main_key(state: &AppState, code: KeyCode, ctrl: bool) -> Option<Action> {
    if state.focus == Panel::Files
        && let Some(action) = tree_key(state, code)
    {
        return Some(action);
    }
    if state.focus == Panel::Devices
        && let Some(action) = device_key(code)
    {
        return Some(action);
    }
    if state.focus == Panel::Bottom
        && let Some(action) = output_key(state, code)
    {
        return Some(action);
    }
    match code {
        KeyCode::Char('q') if !ctrl => Some(Action::Quit),
        KeyCode::Char('B') => Some(Action::RunApp),
        KeyCode::Char('s') => Some(Action::CancelBuild),
        KeyCode::Char('L') => Some(Action::ToggleLogs),
        KeyCode::Char('o') => Some(Action::Focus(Panel::Bottom)),
        KeyCode::Char('t') => Some(Action::ToggleTarget),
        KeyCode::Char('d') => Some(Action::Focus(Panel::Devices)),
        KeyCode::Char('i') => Some(Action::ToggleInfo),
        KeyCode::Char('C') => Some(Action::ShowGit),
        KeyCode::Char('=') => Some(Action::OpenDiffCurrent),
        KeyCode::Char('A') => Some(Action::ToggleBlame),
        KeyCode::Char('p') => Some(Action::TogglePerfPanel),
        KeyCode::Char('P') => Some(Action::TogglePerfIndicator),
        KeyCode::Char('D') => Some(Action::ToggleDoctor),
        KeyCode::Char('T') => Some(Action::OpenTerminalPicker),
        KeyCode::Char('e') => Some(Action::OpenEditor),
        KeyCode::Char('f') => Some(Action::OpenFinder),
        KeyCode::Char('/') => Some(Action::OpenGrep),
        KeyCode::Char('R') => Some(Action::RefreshFiles),
        KeyCode::Tab => Some(Action::FocusNext),
        KeyCode::BackTab => Some(Action::FocusPrev),
        _ => None,
    }
}

/// Keys that act on the device list while it has focus.
fn device_key(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Char('j') | KeyCode::Down => Some(Action::DeviceMove(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::DeviceMove(-1)),
        KeyCode::Char('b') => Some(Action::BootDevice),
        KeyCode::Char('x') => Some(Action::StopDevice),
        KeyCode::Char('r') => Some(Action::RefreshDevices),
        _ => None,
    }
}

/// Typing a log filter: printable characters, including `q`, are text.
fn filter_key(key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => Some(Action::FilterClear),
        KeyCode::Enter => Some(Action::FilterDone),
        KeyCode::Backspace => Some(Action::FilterBackspace),
        KeyCode::Char(ch) if !ctrl && !alt => Some(Action::FilterInput(ch)),
        _ => None,
    }
}

/// Keys that act on the Output panel while it has focus.
fn output_key(state: &AppState, code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Char('[') => Some(Action::OutputTabMove(-1)),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Char(']') => Some(Action::OutputTabMove(1)),
        KeyCode::Char(digit @ '1'..='5') => {
            let index = usize::from(digit as u8 - b'1');
            Some(Action::ShowOutputTab(OutputTab::ALL[index]))
        }
        KeyCode::Char('j') | KeyCode::Down => Some(Action::OutputScroll(-1)),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::OutputScroll(1)),
        KeyCode::PageDown => Some(Action::OutputScroll(-i32::from(PAGE_LINES))),
        KeyCode::PageUp => Some(Action::OutputScroll(i32::from(PAGE_LINES))),
        KeyCode::Char('g') => Some(Action::OutputScroll(i32::MAX)),
        KeyCode::Char('G') => Some(Action::OutputFollow),
        KeyCode::Char('c') => Some(Action::ClearOutput),
        KeyCode::Char('F') => Some(Action::FilterOpen),
        KeyCode::Char('v') => Some(Action::CycleLogLevel),
        KeyCode::Enter if state.output_tab == OutputTab::Git => Some(Action::OpenDiffSelected),
        KeyCode::Char('o') if state.output_tab == OutputTab::Git => {
            Some(match state.git_changes.get(state.git_selected) {
                Some(change) => Action::OpenFile {
                    path: change.path.clone(),
                    line: None,
                },
                None => Action::Status("No changed file selected".to_string()),
            })
        }
        KeyCode::Char('r') if state.output_tab == OutputTab::Git => Some(Action::RefreshGit),
        KeyCode::Enter if state.output_tab == OutputTab::Problems => {
            Some(match state.selected_problem_target() {
                Ok((path, line)) => Action::OpenFile { path, line },
                Err(message) => Action::Status(message),
            })
        }
        _ => None,
    }
}

/// Keys while the diff overlay is open.
fn diff_key(state: &AppState, code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Esc => Some(Action::DiffClose),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::DiffScroll(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::DiffScroll(-1)),
        KeyCode::PageDown | KeyCode::Char(' ') => Some(Action::DiffScroll(i32::from(PAGE_LINES))),
        KeyCode::PageUp => Some(Action::DiffScroll(-i32::from(PAGE_LINES))),
        KeyCode::Char('g') => Some(Action::DiffScroll(i32::MIN)),
        KeyCode::Char('G') => Some(Action::DiffScroll(i32::MAX)),
        KeyCode::Char('n') => Some(Action::DiffHunk(1)),
        KeyCode::Char('N') => Some(Action::DiffHunk(-1)),
        KeyCode::Char('s') => Some(Action::DiffLayout),
        KeyCode::Char('b') => Some(Action::BasePickerOpen),
        KeyCode::Enter | KeyCode::Char('o') => {
            let view = state.diff.as_ref()?;
            Some(Action::OpenFile {
                path: view.path.clone(),
                line: view.current_line(),
            })
        }
        _ => None,
    }
}

/// Keys while choosing what the diff is taken against.
fn base_picker_key(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Esc => Some(Action::BasePickerCancel),
        KeyCode::Enter => Some(Action::BasePickerSelect),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::BasePickerMove(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::BasePickerMove(-1)),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

/// Keys while the performance panel is open.
fn perf_key(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Esc | KeyCode::Char('p') => Some(Action::TogglePerfPanel),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::PerfMove(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::PerfMove(-1)),
        KeyCode::Char('x') => Some(Action::PerfStop),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

/// Keys while the environment overlay is open.
fn info_key(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Esc | KeyCode::Char('i') => Some(Action::ToggleInfo),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

/// Keys that act on the file tree while it has focus.
fn tree_key(state: &AppState, code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Char('j') | KeyCode::Down => Some(Action::TreeMove(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::TreeMove(-1)),
        KeyCode::Char('l') | KeyCode::Right => Some(Action::TreeExpand),
        KeyCode::Char('h') | KeyCode::Left => Some(Action::TreeCollapse),
        KeyCode::Char('g') => Some(Action::TreeMove(i16::MIN)),
        KeyCode::Char('G') => Some(Action::TreeMove(i16::MAX)),
        KeyCode::PageDown => Some(Action::TreeMove(PAGE_LINES)),
        KeyCode::PageUp => Some(Action::TreeMove(-PAGE_LINES)),
        KeyCode::Enter => {
            let row = state.tree.get(state.tree_selected)?;
            Some(if row.is_dir {
                Action::TreeToggle
            } else {
                Action::OpenFile {
                    path: row.path.clone(),
                    line: None,
                }
            })
        }
        _ => None,
    }
}

/// What a key means in a text box with a movable highlight (the finder and
/// the search): printable characters, including `q`, are text.
fn text_entry_key(
    key: KeyEvent,
    input: fn(char) -> Action,
    backspace: Action,
    step: fn(i16) -> Action,
) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('n') if ctrl => Some(step(1)),
        KeyCode::Char('p') if ctrl => Some(step(-1)),
        KeyCode::Char(ch) if !ctrl && !alt => Some(input(ch)),
        KeyCode::Backspace => Some(backspace),
        KeyCode::Down => Some(step(1)),
        KeyCode::Up => Some(step(-1)),
        KeyCode::PageDown => Some(step(PAGE_LINES)),
        KeyCode::PageUp => Some(step(-PAGE_LINES)),
        _ => None,
    }
}

fn finder_key(state: &AppState, key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::FinderCancel),
        KeyCode::Enter => {
            let finder = state.finder.as_ref()?;
            let chosen = finder.matches.get(finder.selected)?;
            Some(Action::OpenFile {
                path: chosen.path.clone(),
                line: None,
            })
        }
        _ => text_entry_key(
            key,
            Action::FinderInput,
            Action::FinderBackspace,
            Action::FinderMove,
        ),
    }
}

fn grep_key(state: &AppState, key: KeyEvent) -> Option<Action> {
    match key.code {
        KeyCode::Esc => Some(Action::GrepCancel),
        // Enter searches the typed query; once that query has results,
        // Enter opens the highlighted one.
        KeyCode::Enter => {
            let grep = state.grep.as_ref()?;
            if grep.searched.as_deref() != Some(grep.query.as_str()) {
                return Some(Action::GrepSearch);
            }
            let hit = grep.hits.get(grep.selected)?;
            Some(Action::OpenFile {
                path: hit.path.clone(),
                line: Some(hit.line),
            })
        }
        _ => text_entry_key(
            key,
            Action::GrepInput,
            Action::GrepBackspace,
            Action::GrepMove,
        ),
    }
}

fn doctor_key(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Esc | KeyCode::Char('D') => Some(Action::ToggleDoctor),
        KeyCode::Char('r') => Some(Action::RefreshDoctor),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('j') | KeyCode::Down => Some(Action::DoctorScroll(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(Action::DoctorScroll(-1)),
        KeyCode::PageDown => Some(Action::DoctorScroll(PAGE_LINES)),
        KeyCode::PageUp => Some(Action::DoctorScroll(-PAGE_LINES)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_core::{Config, Project, ProjectLayout, Report, reduce};
    use std::path::PathBuf;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn key(code: KeyCode) -> KeyEvent {
        press(code, KeyModifiers::NONE)
    }

    fn closed() -> AppState {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "demo".into(),
            layout: ProjectLayout::ComposeApp,
        };
        AppState::new(project, Config::default())
    }

    fn open() -> AppState {
        let running = reduce(closed(), Action::ToggleDoctor);
        reduce(running, Action::DoctorFinished(Report::default()))
    }

    fn action(state: &AppState, code: KeyCode) -> Option<Action> {
        action_for(state, key(code))
    }

    fn editing() -> AppState {
        let s = [
            Action::OpenEditor,
            Action::EditorLaunchStarted,
            Action::EmbeddedStarted,
        ]
        .into_iter()
        .fold(closed(), reduce);
        AppState {
            focus: Panel::Editor,
            ..s
        }
    }

    #[test]
    fn every_key_goes_to_the_focused_embedded_editor() {
        let s = editing();
        let ctrl = |ch| press(KeyCode::Char(ch), KeyModifiers::CONTROL);

        assert_eq!(
            action(&s, KeyCode::Char('q')),
            Some(Action::EditorKeys("q".into()))
        );
        assert_eq!(
            action_for(&s, ctrl('c')),
            Some(Action::EditorKeys("<C-c>".into()))
        );
        assert_eq!(
            action(&s, KeyCode::Tab),
            Some(Action::EditorKeys("<Tab>".into()))
        );
        assert_eq!(action(&s, KeyCode::CapsLock), None);
    }

    #[test]
    fn the_leader_leaves_the_editor() {
        let s = editing();
        let custom = AppState {
            config: Config {
                leader: "C-\\".into(),
                ..Config::default()
            },
            ..editing()
        };

        assert_eq!(
            action_for(&s, press(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            Some(Action::Focus(Panel::Files))
        );
        assert_eq!(
            action_for(&custom, press(KeyCode::Char('4'), KeyModifiers::CONTROL)),
            Some(Action::Focus(Panel::Files))
        );
    }

    #[test]
    fn panels_keep_their_keys_when_the_editor_is_not_focused_or_not_running() {
        let elsewhere = AppState {
            focus: Panel::Files,
            ..editing()
        };
        let not_running = AppState {
            focus: Panel::Editor,
            ..closed()
        };

        assert_eq!(action(&elsewhere, KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(action(&not_running, KeyCode::Char('q')), Some(Action::Quit));
    }

    #[test]
    fn maps_quit_keys() {
        assert_eq!(action(&closed(), KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(
            action_for(&closed(), press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
    }

    fn bottom() -> AppState {
        reduce(closed(), Action::Focus(Panel::Bottom))
    }

    #[test]
    fn build_and_log_keys_work_from_any_panel() {
        for (code, expected) in [
            ('B', Action::RunApp),
            ('s', Action::CancelBuild),
            ('L', Action::ToggleLogs),
            ('o', Action::Focus(Panel::Bottom)),
        ] {
            assert_eq!(action(&closed(), KeyCode::Char(code)), Some(expected));
        }
    }

    #[test]
    fn the_output_panel_steps_through_tabs_and_scrolls() {
        let s = bottom();

        assert_eq!(
            action(&s, KeyCode::Char('l')),
            Some(Action::OutputTabMove(1))
        );
        assert_eq!(
            action(&s, KeyCode::Char('[')),
            Some(Action::OutputTabMove(-1))
        );
        assert_eq!(
            action(&s, KeyCode::Char('4')),
            Some(Action::ShowOutputTab(OutputTab::Problems))
        );
        assert_eq!(
            action(&s, KeyCode::Char('k')),
            Some(Action::OutputScroll(1))
        );
        assert_eq!(action(&s, KeyCode::Down), Some(Action::OutputScroll(-1)));
        assert_eq!(action(&s, KeyCode::Char('G')), Some(Action::OutputFollow));
        assert_eq!(action(&s, KeyCode::Char('c')), Some(Action::ClearOutput));
    }

    #[test]
    fn typing_a_filter_treats_letters_as_text_and_enter_and_escape_end_it() {
        let editing = reduce(bottom(), Action::FilterOpen);

        assert_eq!(
            action(&editing, KeyCode::Char('q')),
            Some(Action::FilterInput('q'))
        );
        assert_eq!(
            action(&editing, KeyCode::Char('B')),
            Some(Action::FilterInput('B'))
        );
        assert_eq!(
            action(&editing, KeyCode::Backspace),
            Some(Action::FilterBackspace)
        );
        assert_eq!(action(&editing, KeyCode::Enter), Some(Action::FilterDone));
        assert_eq!(action(&editing, KeyCode::Esc), Some(Action::FilterClear));
        assert_eq!(
            action_for(&editing, press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
    }

    #[test]
    fn the_output_panel_opens_the_filter_and_cycles_the_level() {
        assert_eq!(
            action(&bottom(), KeyCode::Char('F')),
            Some(Action::FilterOpen)
        );
        assert_eq!(
            action(&bottom(), KeyCode::Char('v')),
            Some(Action::CycleLogLevel)
        );
        assert_eq!(action(&closed(), KeyCode::Char('v')), None);
    }

    #[test]
    fn output_keys_do_nothing_when_another_panel_has_focus() {
        assert_eq!(action(&closed(), KeyCode::Char('4')), None);
        assert_eq!(action(&closed(), KeyCode::Char('c')), None);
    }

    #[test]
    fn enter_on_a_problem_opens_it_and_explains_when_it_cannot() {
        let mut s = bottom();
        s.output_tab = OutputTab::Problems;
        let issue = |path: Option<&str>| lf_mide_core::Problem {
            severity: lf_mide_core::Severity::Error,
            path: path.map(String::from),
            line: Some(7),
            column: None,
            message: "bad".into(),
        };
        s.problems = std::sync::Arc::new(vec![issue(Some("/p/shared/A.kt")), issue(None)]);

        assert_eq!(
            action(&s, KeyCode::Enter),
            Some(Action::OpenFile {
                path: PathBuf::from("shared/A.kt"),
                line: Some(7)
            })
        );
        s.problem_selected = 1;
        assert!(
            matches!(action(&s, KeyCode::Enter), Some(Action::Status(m)) if m.contains("no source location"))
        );
    }

    fn with_diff() -> AppState {
        let mut s = closed();
        s.current_file = Some(PathBuf::from("src/App.kt"));
        let s = reduce(s, Action::OpenDiffCurrent);
        let id = s.diff.as_ref().unwrap().id;
        reduce(
            s,
            Action::DiffLoaded {
                id,
                result: Ok(std::sync::Arc::new(lf_mide_core::FileDiff {
                    hunks: vec![lf_mide_core::Hunk {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 7,
                        new_lines: 1,
                        lines: vec![lf_mide_core::DiffLine {
                            origin: lf_mide_core::LineOrigin::Added,
                            text: "x".into(),
                            old_no: None,
                            new_no: Some(7),
                        }],
                    }],
                    ..lf_mide_core::FileDiff::default()
                })),
            },
        )
    }

    #[test]
    fn git_keys_work_from_any_panel() {
        assert_eq!(action(&closed(), KeyCode::Char('C')), Some(Action::ShowGit));
        assert_eq!(
            action(&closed(), KeyCode::Char('=')),
            Some(Action::OpenDiffCurrent)
        );
        assert_eq!(
            action(&closed(), KeyCode::Char('A')),
            Some(Action::ToggleBlame)
        );
    }

    #[test]
    fn the_git_tab_opens_a_diff_or_the_file_and_refreshes() {
        let mut s = bottom();
        s.output_tab = OutputTab::Git;
        s.git_changes = std::sync::Arc::new(vec![lf_mide_core::FileChange {
            path: PathBuf::from("a.kt"),
            kind: lf_mide_core::ChangeKind::Modified,
            staged: false,
            stamp: 1,
        }]);

        assert_eq!(action(&s, KeyCode::Enter), Some(Action::OpenDiffSelected));
        assert_eq!(
            action(&s, KeyCode::Char('o')),
            Some(Action::OpenFile {
                path: PathBuf::from("a.kt"),
                line: None
            })
        );
        assert_eq!(action(&s, KeyCode::Char('r')), Some(Action::RefreshGit));
        let empty = AppState {
            git_changes: Default::default(),
            ..s
        };
        assert!(matches!(
            action(&empty, KeyCode::Char('o')),
            Some(Action::Status(_))
        ));
    }

    #[test]
    fn the_diff_overlay_scrolls_jumps_and_opens_the_file_at_the_change() {
        let s = with_diff();

        assert_eq!(action(&s, KeyCode::Char('j')), Some(Action::DiffScroll(1)));
        assert_eq!(action(&s, KeyCode::PageUp), Some(Action::DiffScroll(-10)));
        assert_eq!(action(&s, KeyCode::Char('n')), Some(Action::DiffHunk(1)));
        assert_eq!(action(&s, KeyCode::Char('N')), Some(Action::DiffHunk(-1)));
        assert_eq!(action(&s, KeyCode::Char('s')), Some(Action::DiffLayout));
        assert_eq!(action(&s, KeyCode::Char('b')), Some(Action::BasePickerOpen));
        assert_eq!(action(&s, KeyCode::Esc), Some(Action::DiffClose));
        assert_eq!(
            action(&s, KeyCode::Enter),
            Some(Action::OpenFile {
                path: PathBuf::from("src/App.kt"),
                line: Some(7)
            })
        );
        assert_eq!(
            action(&s, KeyCode::Char('B')),
            None,
            "the overlay swallows other keys"
        );
        assert_eq!(action(&s, KeyCode::Char('q')), Some(Action::Quit));
    }

    #[test]
    fn the_base_picker_has_its_own_keys() {
        let s = reduce(with_diff(), Action::BasePickerOpen);

        assert_eq!(
            action(&s, KeyCode::Char('j')),
            Some(Action::BasePickerMove(1))
        );
        assert_eq!(action(&s, KeyCode::Enter), Some(Action::BasePickerSelect));
        assert_eq!(action(&s, KeyCode::Esc), Some(Action::BasePickerCancel));
        assert_eq!(action(&s, KeyCode::Char('n')), None);
    }

    #[test]
    fn the_performance_panel_has_its_own_keys() {
        let open = reduce(closed(), Action::TogglePerfPanel);

        assert_eq!(
            action(&closed(), KeyCode::Char('p')),
            Some(Action::TogglePerfPanel)
        );
        assert_eq!(
            action(&closed(), KeyCode::Char('P')),
            Some(Action::TogglePerfIndicator)
        );
        assert_eq!(
            action(&open, KeyCode::Char('p')),
            Some(Action::TogglePerfPanel)
        );
        assert_eq!(action(&open, KeyCode::Esc), Some(Action::TogglePerfPanel));
        assert_eq!(action(&open, KeyCode::Char('j')), Some(Action::PerfMove(1)));
        assert_eq!(
            action(&open, KeyCode::Char('k')),
            Some(Action::PerfMove(-1))
        );
        assert_eq!(action(&open, KeyCode::Char('x')), Some(Action::PerfStop));
        assert_eq!(action(&open, KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(
            action(&open, KeyCode::Char('B')),
            None,
            "the panel swallows other keys"
        );
    }

    #[test]
    fn maps_focus_keys() {
        assert_eq!(action(&closed(), KeyCode::Tab), Some(Action::FocusNext));
        assert_eq!(
            action_for(&closed(), press(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Action::FocusPrev)
        );
    }

    #[test]
    fn capital_d_opens_the_doctor() {
        let shift_d = press(KeyCode::Char('D'), KeyModifiers::SHIFT);

        assert_eq!(action_for(&closed(), shift_d), Some(Action::ToggleDoctor));
    }

    #[test]
    fn ignores_unmapped_and_release_events() {
        let mut release = key(KeyCode::Char('q'));
        release.kind = KeyEventKind::Release;

        assert_eq!(action(&closed(), KeyCode::Char('x')), None);
        assert_eq!(action(&closed(), KeyCode::Char('c')), None);
        assert_eq!(action_for(&closed(), release), None);
    }

    #[test]
    fn doctor_keys_do_nothing_while_the_panel_is_closed() {
        // With the tree focused, j/k/PageDown move it; elsewhere they are free.
        let mut elsewhere = closed();
        elsewhere.focus = Panel::Editor;

        for code in [
            KeyCode::Char('r'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Esc,
            KeyCode::PageDown,
        ] {
            assert_eq!(action(&elsewhere, code), None, "{code:?}");
        }
    }

    #[test]
    fn open_panel_closes_with_esc_or_capital_d() {
        assert_eq!(action(&open(), KeyCode::Esc), Some(Action::ToggleDoctor));
        assert_eq!(
            action(&open(), KeyCode::Char('D')),
            Some(Action::ToggleDoctor)
        );
    }

    #[test]
    fn open_panel_reruns_with_r() {
        assert_eq!(
            action(&open(), KeyCode::Char('r')),
            Some(Action::RefreshDoctor)
        );
    }

    #[test]
    fn open_panel_scrolls_with_vim_keys_arrows_and_pages() {
        assert_eq!(
            action(&open(), KeyCode::Char('j')),
            Some(Action::DoctorScroll(1))
        );
        assert_eq!(
            action(&open(), KeyCode::Down),
            Some(Action::DoctorScroll(1))
        );
        assert_eq!(
            action(&open(), KeyCode::Char('k')),
            Some(Action::DoctorScroll(-1))
        );
        assert_eq!(action(&open(), KeyCode::Up), Some(Action::DoctorScroll(-1)));
        assert_eq!(
            action(&open(), KeyCode::PageDown),
            Some(Action::DoctorScroll(PAGE_LINES))
        );
        assert_eq!(
            action(&open(), KeyCode::PageUp),
            Some(Action::DoctorScroll(-PAGE_LINES))
        );
    }

    #[test]
    fn open_panel_still_quits_but_ignores_focus_keys() {
        assert_eq!(action(&open(), KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(
            action_for(&open(), press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(action(&open(), KeyCode::Tab), None);
    }

    #[test]
    fn the_running_panel_accepts_the_same_keys() {
        let running = reduce(closed(), Action::ToggleDoctor);

        assert_eq!(action(&running, KeyCode::Esc), Some(Action::ToggleDoctor));
        assert_eq!(action(&running, KeyCode::Tab), None);
    }

    fn picker_open() -> AppState {
        reduce(closed(), Action::OpenTerminalPicker)
    }

    #[test]
    fn t_opens_the_terminal_picker_and_e_opens_the_editor() {
        let shift_t = press(KeyCode::Char('T'), KeyModifiers::SHIFT);

        assert_eq!(
            action_for(&closed(), shift_t),
            Some(Action::OpenTerminalPicker)
        );
        assert_eq!(
            action(&closed(), KeyCode::Char('e')),
            Some(Action::OpenEditor)
        );
    }

    #[test]
    fn picker_keys() {
        let state = picker_open();

        assert_eq!(
            action(&state, KeyCode::Esc),
            Some(Action::TerminalPickerCancel)
        );
        assert_eq!(
            action(&state, KeyCode::Enter),
            Some(Action::TerminalPickerSelect)
        );
        assert_eq!(
            action(&state, KeyCode::Char('j')),
            Some(Action::TerminalPickerMove(1))
        );
        assert_eq!(
            action(&state, KeyCode::Down),
            Some(Action::TerminalPickerMove(1))
        );
        assert_eq!(
            action(&state, KeyCode::Char('k')),
            Some(Action::TerminalPickerMove(-1))
        );
        assert_eq!(
            action(&state, KeyCode::Up),
            Some(Action::TerminalPickerMove(-1))
        );
    }

    #[test]
    fn picker_swallows_other_keys_but_still_quits() {
        let state = picker_open();

        for code in [
            KeyCode::Tab,
            KeyCode::Char('D'),
            KeyCode::Char('e'),
            KeyCode::Char('T'),
        ] {
            assert_eq!(action(&state, code), None, "{code:?}");
        }
        assert_eq!(action(&state, KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(
            action_for(&state, press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
    }

    #[test]
    fn terminal_and_editor_keys_do_nothing_while_the_doctor_is_open() {
        assert_eq!(action(&open(), KeyCode::Char('T')), None);
        assert_eq!(action(&open(), KeyCode::Char('e')), None);
    }

    // --- file tree, finder and search ---

    use lf_mide_core::{FileMatch, GrepHit};
    use std::sync::Arc;

    fn with_files(paths: &[&str]) -> AppState {
        let list: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        reduce(closed(), Action::FilesLoaded(Arc::new(list)))
    }

    fn open_file(path: &str) -> Action {
        Action::OpenFile {
            path: PathBuf::from(path),
            line: None,
        }
    }

    #[test]
    fn f_slash_and_capital_r_open_the_finder_search_and_rescan() {
        assert_eq!(
            action(&closed(), KeyCode::Char('f')),
            Some(Action::OpenFinder)
        );
        assert_eq!(
            action(&closed(), KeyCode::Char('/')),
            Some(Action::OpenGrep)
        );
        assert_eq!(
            action(&closed(), KeyCode::Char('R')),
            Some(Action::RefreshFiles)
        );
    }

    #[test]
    fn the_tree_keys_work_only_while_the_tree_has_focus() {
        let tree = with_files(&["a.txt", "b.txt"]);
        let mut elsewhere = tree.clone();
        elsewhere.focus = Panel::Editor;

        assert_eq!(action(&tree, KeyCode::Char('j')), Some(Action::TreeMove(1)));
        assert_eq!(action(&tree, KeyCode::Down), Some(Action::TreeMove(1)));
        assert_eq!(
            action(&tree, KeyCode::Char('k')),
            Some(Action::TreeMove(-1))
        );
        assert_eq!(action(&tree, KeyCode::Up), Some(Action::TreeMove(-1)));
        assert_eq!(action(&elsewhere, KeyCode::Char('j')), None);
        assert_eq!(action(&elsewhere, KeyCode::Char('h')), None);
        assert_eq!(action(&elsewhere, KeyCode::Enter), None);
    }

    #[test]
    fn tree_navigation_keys_cover_expand_collapse_jump_and_page() {
        let tree = with_files(&["a/x.txt"]);

        assert_eq!(action(&tree, KeyCode::Char('l')), Some(Action::TreeExpand));
        assert_eq!(action(&tree, KeyCode::Right), Some(Action::TreeExpand));
        assert_eq!(
            action(&tree, KeyCode::Char('h')),
            Some(Action::TreeCollapse)
        );
        assert_eq!(action(&tree, KeyCode::Left), Some(Action::TreeCollapse));
        assert_eq!(
            action(&tree, KeyCode::Char('g')),
            Some(Action::TreeMove(i16::MIN))
        );
        assert_eq!(
            action(&tree, KeyCode::Char('G')),
            Some(Action::TreeMove(i16::MAX))
        );
        assert_eq!(
            action(&tree, KeyCode::PageDown),
            Some(Action::TreeMove(PAGE_LINES))
        );
        assert_eq!(
            action(&tree, KeyCode::PageUp),
            Some(Action::TreeMove(-PAGE_LINES))
        );
    }

    #[test]
    fn enter_toggles_a_directory_and_opens_a_file() {
        let dir_first = with_files(&["a/x.txt", "z.txt"]);
        let on_file = reduce(dir_first.clone(), Action::TreeMove(1));

        assert_eq!(action(&dir_first, KeyCode::Enter), Some(Action::TreeToggle));
        assert_eq!(action(&on_file, KeyCode::Enter), Some(open_file("z.txt")));
        assert_eq!(
            action(&closed(), KeyCode::Enter),
            None,
            "nothing selected in an empty tree"
        );
    }

    #[test]
    fn q_still_quits_from_the_tree_and_other_keys_still_work_there() {
        let tree = with_files(&["a.txt"]);

        assert_eq!(action(&tree, KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(action(&tree, KeyCode::Char('f')), Some(Action::OpenFinder));
        assert_eq!(action(&tree, KeyCode::Tab), Some(Action::FocusNext));
    }

    fn finder_open(matches: &[&str]) -> AppState {
        let s = reduce(closed(), Action::OpenFinder);
        let found: Vec<FileMatch> = matches
            .iter()
            .map(|p| FileMatch {
                path: PathBuf::from(p),
                score: 1,
                indices: vec![],
            })
            .collect();
        reduce(
            s,
            Action::FinderResults {
                query: String::new(),
                matches: found,
            },
        )
    }

    #[test]
    fn finder_typing_includes_q_and_every_other_character() {
        let s = finder_open(&["a.kt"]);

        for ch in ['q', 'e', 'f', '/', 'T', 'D', 'j', ' ', '.', 'é'] {
            assert_eq!(
                action(&s, KeyCode::Char(ch)),
                Some(Action::FinderInput(ch)),
                "{ch:?}"
            );
        }
        assert_eq!(
            action_for(&s, press(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Some(Action::FinderInput('A'))
        );
        assert_eq!(
            action(&s, KeyCode::Backspace),
            Some(Action::FinderBackspace)
        );
    }

    #[test]
    fn finder_navigation_and_choice() {
        let s = finder_open(&["a.kt", "b.kt"]);
        let on_b = reduce(s.clone(), Action::FinderMove(1));

        assert_eq!(action(&s, KeyCode::Down), Some(Action::FinderMove(1)));
        assert_eq!(action(&s, KeyCode::Up), Some(Action::FinderMove(-1)));
        assert_eq!(
            action_for(&s, press(KeyCode::Char('n'), KeyModifiers::CONTROL)),
            Some(Action::FinderMove(1))
        );
        assert_eq!(
            action_for(&s, press(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Some(Action::FinderMove(-1))
        );
        assert_eq!(action(&s, KeyCode::Esc), Some(Action::FinderCancel));
        assert_eq!(action(&s, KeyCode::Enter), Some(open_file("a.kt")));
        assert_eq!(action(&on_b, KeyCode::Enter), Some(open_file("b.kt")));
    }

    #[test]
    fn finder_enter_without_results_does_nothing_and_ctrl_c_still_quits() {
        let empty = finder_open(&[]);

        assert_eq!(action(&empty, KeyCode::Enter), None);
        assert_eq!(
            action_for(&empty, press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(
            action_for(&empty, press(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            None,
            "control chords are not text"
        );
        assert_eq!(action(&empty, KeyCode::Tab), None);
    }

    fn grep_open(query: &str, hits: Vec<GrepHit>, searched: bool) -> AppState {
        let mut s = reduce(closed(), Action::OpenGrep);
        for ch in query.chars() {
            s = reduce(s, Action::GrepInput(ch));
        }
        if searched {
            s = reduce(s, Action::GrepSearch);
            s = reduce(
                s,
                Action::GrepResults {
                    query: query.to_string(),
                    result: Ok(hits),
                },
            );
        }
        s
    }

    fn hit(path: &str, line: u32) -> GrepHit {
        GrepHit {
            path: PathBuf::from(path),
            line,
            column: 0,
            text: "t".into(),
        }
    }

    #[test]
    fn search_typing_includes_q_and_enter_starts_the_search() {
        let s = grep_open("set", vec![], false);

        assert_eq!(action(&s, KeyCode::Char('q')), Some(Action::GrepInput('q')));
        assert_eq!(action(&s, KeyCode::Backspace), Some(Action::GrepBackspace));
        assert_eq!(action(&s, KeyCode::Enter), Some(Action::GrepSearch));
        assert_eq!(action(&s, KeyCode::Esc), Some(Action::GrepCancel));
    }

    #[test]
    fn enter_opens_the_highlighted_hit_once_the_query_was_searched() {
        let s = grep_open("set", vec![hit("a.kt", 4), hit("b.kt", 9)], true);
        let on_b = reduce(s.clone(), Action::GrepMove(1));

        assert_eq!(
            action(&s, KeyCode::Enter),
            Some(Action::OpenFile {
                path: PathBuf::from("a.kt"),
                line: Some(4)
            })
        );
        assert_eq!(
            action(&on_b, KeyCode::Enter),
            Some(Action::OpenFile {
                path: PathBuf::from("b.kt"),
                line: Some(9)
            })
        );
        assert_eq!(action(&s, KeyCode::Down), Some(Action::GrepMove(1)));
        assert_eq!(
            action_for(&s, press(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Some(Action::GrepMove(-1))
        );
    }

    #[test]
    fn editing_the_query_after_a_search_makes_enter_search_again() {
        let searched = grep_open("set", vec![hit("a.kt", 4)], true);
        let edited = reduce(searched, Action::GrepInput('x'));

        assert_eq!(action(&edited, KeyCode::Enter), Some(Action::GrepSearch));
    }

    #[test]
    fn enter_on_an_empty_result_list_does_nothing() {
        let none = grep_open("zzz", vec![], true);

        assert_eq!(action(&none, KeyCode::Enter), None);
    }

    // --- build target, devices, environment ---

    #[test]
    fn t_switches_the_target_d_focuses_the_devices_and_i_shows_the_environment() {
        assert_eq!(
            action(&closed(), KeyCode::Char('t')),
            Some(Action::ToggleTarget)
        );
        assert_eq!(
            action(&closed(), KeyCode::Char('d')),
            Some(Action::Focus(Panel::Devices))
        );
        assert_eq!(
            action(&closed(), KeyCode::Char('i')),
            Some(Action::ToggleInfo)
        );
    }

    fn devices_focused() -> AppState {
        let mut s = closed();
        s.focus = Panel::Devices;
        s
    }

    #[test]
    fn the_device_keys_work_only_while_the_devices_have_focus() {
        let focused = devices_focused();
        let elsewhere = closed();

        assert_eq!(
            action(&focused, KeyCode::Char('j')),
            Some(Action::DeviceMove(1))
        );
        assert_eq!(action(&focused, KeyCode::Down), Some(Action::DeviceMove(1)));
        assert_eq!(
            action(&focused, KeyCode::Char('k')),
            Some(Action::DeviceMove(-1))
        );
        assert_eq!(action(&focused, KeyCode::Up), Some(Action::DeviceMove(-1)));
        assert_eq!(
            action(&focused, KeyCode::Char('b')),
            Some(Action::BootDevice)
        );
        assert_eq!(
            action(&focused, KeyCode::Char('x')),
            Some(Action::StopDevice)
        );
        assert_eq!(
            action(&focused, KeyCode::Char('r')),
            Some(Action::RefreshDevices)
        );
        for code in [KeyCode::Char('b'), KeyCode::Char('x'), KeyCode::Char('r')] {
            assert_eq!(
                action(&elsewhere, code),
                None,
                "{code:?} must not start or stop devices from the tree"
            );
        }
    }

    #[test]
    fn global_keys_still_work_from_the_devices_panel() {
        let focused = devices_focused();

        assert_eq!(action(&focused, KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(
            action(&focused, KeyCode::Char('t')),
            Some(Action::ToggleTarget)
        );
        assert_eq!(
            action(&focused, KeyCode::Char('f')),
            Some(Action::OpenFinder)
        );
        assert_eq!(action(&focused, KeyCode::Tab), Some(Action::FocusNext));
        assert_eq!(
            action(&focused, KeyCode::Char('h')),
            None,
            "the tree's collapse key is not a device key"
        );
    }

    #[test]
    fn the_environment_overlay_closes_with_escape_or_i_and_swallows_other_keys() {
        let open = reduce(closed(), Action::ToggleInfo);

        assert_eq!(action(&open, KeyCode::Esc), Some(Action::ToggleInfo));
        assert_eq!(action(&open, KeyCode::Char('i')), Some(Action::ToggleInfo));
        assert_eq!(action(&open, KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(
            action_for(&open, press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        for code in [
            KeyCode::Char('t'),
            KeyCode::Char('f'),
            KeyCode::Tab,
            KeyCode::Char('j'),
        ] {
            assert_eq!(action(&open, code), None, "{code:?}");
        }
    }
}
