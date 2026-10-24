//! Mouse input. A click focuses its panel, picks a file-tree row (a second
//! click on the same row opens or folds it) and reaches Neovim in the editor
//! panel. The wheel scrolls the panel under the pointer.

use crossterm::event::{MouseButton as TermButton, MouseEvent, MouseEventKind};
use lf_mide_core::{Action, AppState, EditorMouse, MouseAction, MouseButton, Panel};
use ratatui::layout::{Position, Rect};

/// File-tree rows and output lines one wheel notch moves.
const WHEEL_LINES: i16 = 3;

/// What a mouse event means for the app. Nothing happens while an overlay
/// covers the screen, or on a border or a part of the screen with no panel.
pub fn actions_for(state: &AppState, event: MouseEvent, screen: Rect) -> Vec<Action> {
    if state.overlay_open() {
        return Vec::new();
    }
    if let MouseEventKind::Down(TermButton::Left) = event.kind {
        return click(state, event, screen);
    }
    let Some(action) = mouse_action(event.kind) else {
        return Vec::new();
    };
    match (
        action,
        lf_mide_ui::panel_at(screen, event.column, event.row),
    ) {
        (MouseAction::ScrollUp, Some(panel)) => wheel(state, event, screen, panel, true),
        (MouseAction::ScrollDown, Some(panel)) => wheel(state, event, screen, panel, false),
        // A drag or release can leave the editor panel; Neovim still gets
        // it, as long as it lands inside the grid.
        _ => editor_mouse(state, event, screen, action),
    }
}

fn click(state: &AppState, event: MouseEvent, screen: Rect) -> Vec<Action> {
    let Some(panel) = lf_mide_ui::panel_at(screen, event.column, event.row) else {
        return Vec::new();
    };
    let mut actions = vec![Action::Focus(panel)];
    match panel {
        Panel::Files => actions.extend(tree_click(state, event, screen)),
        Panel::Editor => actions.extend(editor_mouse(
            state,
            event,
            screen,
            MouseAction::Press(MouseButton::Left),
        )),
        Panel::Devices | Panel::Bottom => {}
    }
    actions
}

/// Selects the file-tree row under the pointer; clicking the row that is
/// already selected opens it (a file) or folds it (a folder), as Enter does.
fn tree_click(state: &AppState, event: MouseEvent, screen: Rect) -> Vec<Action> {
    let inner = lf_mide_ui::panel_content(screen, Panel::Files);
    if !inner.contains(Position::new(event.column, event.row)) {
        return Vec::new();
    }
    let index = state.tree_top + usize::from(event.row - inner.y);
    let Some(row) = state.tree.get(index) else {
        return Vec::new();
    };
    if index != state.tree_selected {
        return vec![Action::TreeSelect(index)];
    }
    if row.is_dir {
        vec![Action::TreeToggle]
    } else {
        vec![Action::OpenFile {
            path: row.path.clone(),
            line: None,
        }]
    }
}

fn wheel(state: &AppState, event: MouseEvent, screen: Rect, panel: Panel, up: bool) -> Vec<Action> {
    // Positive steps move down the list.
    let step = if up { -1 } else { 1 };
    match panel {
        Panel::Files => vec![Action::TreeMove(step * WHEEL_LINES)],
        // The output counts lines back in time, so up is positive there.
        Panel::Bottom => vec![Action::OutputScroll(-i32::from(step * WHEEL_LINES))],
        Panel::Devices => vec![Action::DeviceMove(step)],
        Panel::Editor => {
            let action = if up {
                MouseAction::ScrollUp
            } else {
                MouseAction::ScrollDown
            };
            editor_mouse(state, event, screen, action)
        }
    }
}

/// The Neovim event for a mouse event inside its grid, when Neovim runs in
/// the editor panel.
fn editor_mouse(
    state: &AppState,
    event: MouseEvent,
    screen: Rect,
    action: MouseAction,
) -> Vec<Action> {
    if !state.embedded {
        return Vec::new();
    }
    let grid = lf_mide_ui::panel_content(screen, Panel::Editor);
    if !grid.contains(Position::new(event.column, event.row)) {
        return Vec::new();
    }
    vec![Action::EditorMouse(EditorMouse {
        action,
        row: event.row - grid.y,
        col: event.column - grid.x,
    })]
}

fn mouse_action(kind: MouseEventKind) -> Option<MouseAction> {
    Some(match kind {
        MouseEventKind::Down(button) => MouseAction::Press(button_of(button)),
        MouseEventKind::Drag(button) => MouseAction::Drag(button_of(button)),
        MouseEventKind::Up(button) => MouseAction::Release(button_of(button)),
        MouseEventKind::ScrollUp => MouseAction::ScrollUp,
        MouseEventKind::ScrollDown => MouseAction::ScrollDown,
        _ => return None,
    })
}

fn button_of(button: TermButton) -> MouseButton {
    match button {
        TermButton::Left => MouseButton::Left,
        TermButton::Right => MouseButton::Right,
        TermButton::Middle => MouseButton::Middle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use lf_mide_core::{Config, Project, ProjectLayout, TreeRow, reduce};
    use std::path::PathBuf;
    use std::sync::Arc;

    const SCREEN: Rect = Rect::new(0, 0, 150, 40);

    fn state() -> AppState {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "demo".into(),
            layout: ProjectLayout::ComposeApp,
        };
        AppState::new(project, Config::default())
    }

    /// A state whose file tree lists `names` (files only).
    fn with_files(names: &[&str]) -> AppState {
        let rows = names
            .iter()
            .map(|name| TreeRow {
                path: PathBuf::from(name),
                name: name.to_string(),
                depth: 0,
                is_dir: false,
                expanded: false,
            })
            .collect();
        AppState {
            tree: Arc::new(rows),
            ..state()
        }
    }

    fn at(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// The first row of the file tree's list, on the screen.
    fn first_tree_row() -> u16 {
        lf_mide_ui::panel_content(SCREEN, Panel::Files).y
    }

    fn click_at(state: &AppState, column: u16, row: u16) -> Vec<Action> {
        actions_for(
            state,
            at(MouseEventKind::Down(TermButton::Left), column, row),
            SCREEN,
        )
    }

    #[test]
    fn clicking_a_panel_focuses_it() {
        let bottom = lf_mide_ui::panel_content(SCREEN, Panel::Bottom);

        let actions = click_at(&state(), bottom.x + 1, bottom.y);

        assert_eq!(actions, vec![Action::Focus(Panel::Bottom)]);
    }

    #[test]
    fn a_click_on_a_file_selects_it_and_a_second_click_opens_it() {
        let s = with_files(&["a.kt", "b.kt", "c.kt"]);
        let row = first_tree_row() + 2;
        let column = lf_mide_ui::panel_content(SCREEN, Panel::Files).x + 1;

        let first = click_at(&s, column, row);
        let selected = reduce(s.clone(), Action::TreeSelect(2));
        let second = click_at(&selected, column, row);

        assert_eq!(
            first,
            vec![Action::Focus(Panel::Files), Action::TreeSelect(2)]
        );
        assert_eq!(
            second,
            vec![
                Action::Focus(Panel::Files),
                Action::OpenFile {
                    path: PathBuf::from("c.kt"),
                    line: None
                }
            ]
        );
    }

    #[test]
    fn a_click_on_the_border_or_past_the_last_file_selects_nothing() {
        let s = with_files(&["a.kt"]);
        let inner = lf_mide_ui::panel_content(SCREEN, Panel::Files);

        let on_border = click_at(&s, inner.x, inner.y - 1);
        let past_the_end = click_at(&s, inner.x + 1, inner.y + 5);

        assert!(!on_border.contains(&Action::TreeSelect(0)));
        assert!(
            !past_the_end
                .iter()
                .any(|a| matches!(a, Action::TreeSelect(_)))
        );
    }

    #[test]
    fn clicks_in_the_editor_reach_neovim_only_while_it_runs() {
        let grid = lf_mide_ui::panel_content(SCREEN, Panel::Editor);
        let (column, row) = (grid.x + 4, grid.y + 2);
        let mut running = state();
        running.embedded = true;

        let sent = click_at(&running, column, row);
        let not_running = click_at(&state(), column, row);

        assert!(sent.contains(&Action::EditorMouse(EditorMouse {
            action: MouseAction::Press(MouseButton::Left),
            row: 2,
            col: 4,
        })));
        assert!(
            !not_running
                .iter()
                .any(|a| matches!(a, Action::EditorMouse(_)))
        );
    }

    #[test]
    fn the_wheel_scrolls_the_panel_under_the_pointer() {
        let s = with_files(&["a.kt", "b.kt"]);
        let files = lf_mide_ui::panel_content(SCREEN, Panel::Files);
        let bottom = lf_mide_ui::panel_content(SCREEN, Panel::Bottom);

        let up_in_files = actions_for(&s, at(MouseEventKind::ScrollUp, files.x, files.y), SCREEN);
        let down_in_output = actions_for(
            &s,
            at(MouseEventKind::ScrollDown, bottom.x, bottom.y),
            SCREEN,
        );

        assert_eq!(up_in_files, vec![Action::TreeMove(-WHEEL_LINES)]);
        assert_eq!(
            down_in_output,
            vec![Action::OutputScroll(-i32::from(WHEEL_LINES))]
        );
    }

    #[test]
    fn an_open_overlay_gets_no_mouse_input() {
        let mut covered = state();
        covered.help_open = true;
        let grid = lf_mide_ui::panel_content(SCREEN, Panel::Editor);
        covered.embedded = true;

        assert!(click_at(&covered, grid.x + 1, grid.y + 1).is_empty());
    }
}
