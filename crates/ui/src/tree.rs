//! The Files panel: the project's file tree.

use crate::theme;
use lf_mide_core::text::visible as printable;
use lf_mide_core::{AppState, Panel};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::panel_block;

pub(crate) fn render_tree(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = panel_block(state, Panel::Files, "Files");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.tree.is_empty() {
        let note = if state.files_loading {
            "Scanning…"
        } else {
            "No files"
        };
        frame.render_widget(
            Paragraph::new(Line::styled(note, Style::new().fg(theme::current().muted))),
            inner,
        );
        return;
    }

    let visible = usize::from(inner.height);
    // The reducer keeps the selection in view; this only guards against a
    // stale scroll position after the list got shorter.
    let top = state.tree_top.min(state.tree.len().saturating_sub(visible));
    let focused = state.focus == Panel::Files;
    let lines: Vec<Line> = state
        .tree
        .iter()
        .enumerate()
        .skip(top)
        .take(visible)
        .map(|(index, row)| {
            let marker = match (row.is_dir, row.expanded) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                (false, _) => "  ",
            };
            let text = format!("{}{marker}{}", "  ".repeat(row.depth), printable(&row.name));
            let style = if index == state.tree_selected && focused {
                Style::new().add_modifier(Modifier::REVERSED)
            } else if index == state.tree_selected {
                Style::new().add_modifier(Modifier::BOLD)
            } else if row.is_dir {
                Style::new().fg(theme::current().accent)
            } else {
                Style::new()
            };
            Line::styled(text, style)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{buffer, find_on_screen, screen, screen_sized, state, with_files};
    use lf_mide_core::{Action, reduce};

    #[test]
    fn shows_a_scanning_note_then_an_empty_note() {
        let loading = state();
        let empty = with_files(state(), &[]);

        assert!(screen(&loading).contains("Scanning…"));
        assert!(screen(&empty).contains("No files"));
        assert!(!screen(&empty).contains("Scanning"));
    }

    #[test]
    fn shows_directories_with_open_and_closed_markers() {
        let closed = with_files(state(), &["src/App.kt", "README.md"]);
        let open = reduce(closed.clone(), Action::TreeToggle);

        let closed_screen = screen(&closed);
        let open_screen = screen(&open);

        assert!(closed_screen.contains("▸ src") && closed_screen.contains("README.md"));
        assert!(!closed_screen.contains("App.kt"));
        assert!(open_screen.contains("▾ src") && open_screen.contains("App.kt"));
    }

    #[test]
    fn nested_entries_are_indented() {
        let s = reduce(with_files(state(), &["src/App.kt"]), Action::TreeToggle);

        let text = screen(&s);

        let (dir_col, _) = find_on_screen(&text, "▾ src").unwrap();
        let (file_col, _) = find_on_screen(&text, "App.kt").unwrap();
        assert!(
            file_col > dir_col + 2,
            "file at {file_col}, dir at {dir_col}"
        );
    }

    #[test]
    fn scrolling_starts_the_list_at_the_top_row() {
        let names: Vec<String> = (0..60).map(|i| format!("f{i:02}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut s = with_files(state(), &refs);
        s.tree_top = 5;

        let text = screen(&s);

        assert!(text.contains("f05.txt") && !text.contains("f04.txt"));
    }

    #[test]
    fn the_selected_row_is_highlighted_only_while_the_panel_has_focus() {
        let focused = with_files(state(), &["a.txt", "b.txt"]);
        let mut unfocused = focused.clone();
        unfocused.focus = lf_mide_core::Panel::Editor;
        let text = screen(&focused);
        let (col, row) = find_on_screen(&text, "a.txt").unwrap();

        let reversed = |s: &AppState| {
            buffer(s, 100, 24)[(col, row)]
                .modifier
                .contains(Modifier::REVERSED)
        };

        assert!(reversed(&focused));
        assert!(!reversed(&unfocused));
        let (col_b, row_b) = find_on_screen(&text, "b.txt").unwrap();
        assert!(
            !buffer(&focused, 100, 24)[(col_b, row_b)]
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn the_reducers_row_count_matches_what_is_really_drawn() {
        // The reducer scrolls using lf_mide_core::layout; if the drawing and
        // that arithmetic ever disagree the selection would scroll out of view.
        let names: Vec<String> = (0..200).map(|i| format!("f{i:03}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        for height in [12u16, 24, 30, 50] {
            let s = with_files(state(), &refs);

            let text = screen_sized(&s, 100, height);

            let drawn = text.lines().filter(|l| l.contains(".txt")).count();
            assert_eq!(
                drawn,
                usize::from(lf_mide_core::layout::files_viewport_rows(height)),
                "height {height}"
            );
        }
    }

    #[test]
    fn a_stale_scroll_position_cannot_hide_the_whole_list() {
        let mut s = with_files(state(), &["a.txt", "b.txt"]);
        s.tree_top = 500;

        assert!(screen(&s).contains("a.txt"));
    }

    #[test]
    fn survives_tiny_terminals() {
        let s = with_files(state(), &["a/b.txt", "c.txt"]);

        for (w, h) in [(10, 3), (30, 6), (200, 5), (40, 12)] {
            let _ = screen_sized(&s, w, h);
        }
    }
}
