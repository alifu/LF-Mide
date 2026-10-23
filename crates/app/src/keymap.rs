//! Maps terminal key events to actions. What a key means depends on what is
//! on screen, so the current state is an input.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lf_mide_core::{Action, AppState, DoctorPanel, Panel};

const PAGE_LINES: i16 = 10;

pub fn action_for(state: &AppState, key: KeyEvent) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
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
    if state.terminal_picker.is_some() {
        return picker_key(key.code);
    }
    match state.doctor {
        DoctorPanel::Closed => main_key(state, key.code, ctrl),
        DoctorPanel::Running | DoctorPanel::Ready(_) => doctor_key(key.code),
    }
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
    match code {
        KeyCode::Char('q') if !ctrl => Some(Action::Quit),
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

    #[test]
    fn maps_quit_keys() {
        assert_eq!(action(&closed(), KeyCode::Char('q')), Some(Action::Quit));
        assert_eq!(
            action_for(&closed(), press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
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
}
