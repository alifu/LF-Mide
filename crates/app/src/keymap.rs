//! Maps terminal key events to actions. What a key means depends on what is
//! on screen, so the current state is an input.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lf_mide_core::{Action, AppState, DoctorPanel};

const PAGE_LINES: i16 = 10;

pub fn action_for(state: &AppState, key: KeyEvent) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if matches!(key.code, KeyCode::Char('c')) && ctrl {
        return Some(Action::Quit);
    }
    if state.terminal_picker.is_some() {
        return picker_key(key.code);
    }
    match state.doctor {
        DoctorPanel::Closed => main_key(key.code, ctrl),
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

fn main_key(code: KeyCode, ctrl: bool) -> Option<Action> {
    match code {
        KeyCode::Char('q') if !ctrl => Some(Action::Quit),
        KeyCode::Char('D') => Some(Action::ToggleDoctor),
        KeyCode::Char('T') => Some(Action::OpenTerminalPicker),
        KeyCode::Char('e') => Some(Action::OpenEditor),
        KeyCode::Tab => Some(Action::FocusNext),
        KeyCode::BackTab => Some(Action::FocusPrev),
        _ => None,
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
        for code in [
            KeyCode::Char('r'),
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Esc,
            KeyCode::PageDown,
        ] {
            assert_eq!(action(&closed(), code), None, "{code:?}");
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
}
