//! Maps terminal key events to actions.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use lf_mide_core::Action;

pub fn action_for(key: KeyEvent) -> Option<Action> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if ctrl => Some(Action::Quit),
        KeyCode::Char('q') if !ctrl => Some(Action::Quit),
        KeyCode::Tab => Some(Action::FocusNext),
        KeyCode::BackTab => Some(Action::FocusPrev),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn maps_quit_keys() {
        assert_eq!(
            action_for(press(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Action::Quit)
        );
        assert_eq!(
            action_for(press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
    }

    #[test]
    fn maps_focus_keys() {
        assert_eq!(
            action_for(press(KeyCode::Tab, KeyModifiers::NONE)),
            Some(Action::FocusNext)
        );
        assert_eq!(
            action_for(press(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Action::FocusPrev)
        );
    }

    #[test]
    fn ignores_unmapped_and_release_events() {
        let mut release = press(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;

        assert_eq!(
            action_for(press(KeyCode::Char('x'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            action_for(press(KeyCode::Char('c'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(action_for(release), None);
    }
}
