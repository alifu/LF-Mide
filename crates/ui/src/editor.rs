//! The editor panel: what our Neovim shows, or a note on where Neovim is.

use crate::theme;
use lf_mide_core::screen::Screen;
use lf_mide_core::text::visible as printable;
use lf_mide_core::{AppState, EditorMode, Panel};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::Paragraph;

use crate::panel_block;

pub(crate) fn render_editor(frame: &mut Frame, area: Rect, state: &AppState) {
    let title = if state.quit_pending {
        "Editor · answer Neovim's question to quit".to_string()
    } else if state.editor_has_keys() {
        format!(
            "Editor · {} back to panels",
            printable(&state.config.leader)
        )
    } else {
        "Editor".to_string()
    };
    let block = panel_block(state, Panel::Editor, &title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let screen = state.editor_screen.as_deref().filter(|_| state.embedded);
    let Some(screen) = screen else {
        let note = Span::styled(placeholder(state), Style::new().fg(theme::current().muted));
        frame.render_widget(Paragraph::new(note), inner);
        return;
    };
    draw_screen(frame.buffer_mut(), inner, screen);
    if state.editor_has_keys() && !screen.busy {
        let (row, col) = screen.cursor;
        if row < inner.height && col < inner.width {
            frame.set_cursor_position((inner.x + col, inner.y + row));
        }
    }
}

fn placeholder(state: &AppState) -> &'static str {
    if state.embedded || state.editor_launching {
        return "Starting Neovim…";
    }
    if state.editor_socket.is_some() {
        return "Neovim runs in another pane";
    }
    match state.config.editor {
        EditorMode::Split => "Neovim runs in a split pane: press e to open it",
        EditorMode::Embedded => "Press e, or choose a file, to start Neovim here",
    }
}

/// Copies the grid into `area`, clipped to whichever is smaller.
fn draw_screen(buf: &mut Buffer, area: Rect, screen: &Screen) {
    let rows = area.height.min(screen.height);
    let cols = area.width.min(screen.width);
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            // The right half of a double-width character: drawn by its left half.
            if cell.text.is_empty() {
                continue;
            }
            let wide = screen
                .cell(row, col + 1)
                .is_some_and(|next| next.text.is_empty());
            // A wide character cut off by the panel edge would spill onto the border.
            let text = if wide && col + 1 >= cols {
                " "
            } else {
                cell.text.as_str()
            };
            buf[(area.x + col, area.y + row)]
                .set_symbol(text)
                .set_style(style(screen, cell.hl));
        }
    }
}

fn style(screen: &Screen, hl: u32) -> Style {
    let attr = screen.highlight(hl).cloned().unwrap_or_default();
    let mut style = Style::new()
        .fg(color(attr.fg.or(screen.default_fg)))
        .bg(color(attr.bg.or(screen.default_bg)));
    for (on, modifier) in [
        (attr.bold, Modifier::BOLD),
        (attr.italic, Modifier::ITALIC),
        (attr.underline || attr.undercurl, Modifier::UNDERLINED),
        (attr.strikethrough, Modifier::CROSSED_OUT),
        (attr.reverse, Modifier::REVERSED),
    ] {
        if on {
            style = style.add_modifier(modifier);
        }
    }
    if let Some(special) = attr.special.filter(|_| attr.undercurl || attr.underline) {
        style = style.underline_color(rgb(special));
    }
    style
}

fn color(value: Option<u32>) -> Color {
    value.map_or(Color::Reset, rgb)
}

fn rgb(value: u32) -> Color {
    let [_, r, g, b] = value.to_be_bytes();
    Color::Rgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{buffer, screen, state};
    use lf_mide_core::screen::{HlAttr, LineCell, RedrawEvent};
    use lf_mide_core::{Action, reduce};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn running(grid: Screen) -> AppState {
        let s = [
            Action::OpenEditor,
            Action::EditorLaunchStarted,
            Action::EmbeddedStarted,
            Action::EditorOpened(PathBuf::from("/tmp/s")),
            Action::EditorScreen(Arc::new(grid)),
        ]
        .into_iter()
        .fold(state(), reduce);
        AppState {
            focus: Panel::Editor,
            ..s
        }
    }

    fn grid(width: u16, height: u16, rows: &[(&str, u32)]) -> Screen {
        let events = rows
            .iter()
            .enumerate()
            .map(|(row, (text, hl))| RedrawEvent::Line {
                row: u16::try_from(row).unwrap(),
                col: 0,
                cells: text
                    .chars()
                    .map(|ch| LineCell {
                        text: ch.to_string(),
                        hl: Some(*hl),
                        repeat: 1,
                    })
                    .collect(),
            });
        Screen::default()
            .apply(RedrawEvent::Resize { width, height })
            .apply_all(events.collect::<Vec<_>>())
    }

    #[test]
    fn the_grid_is_drawn_inside_the_editor_panel() {
        let s = running(grid(40, 5, &[("fun main() {}", 0), ("~", 0)]));

        let text = screen(&s);

        assert!(text.contains("│fun main() {}"), "{text}");
        assert!(text.contains("Editor · C-Space back to panels"), "{text}");
    }

    #[test]
    fn the_cursor_is_placed_on_its_cell_only_when_the_editor_has_the_keys() {
        let g = grid(40, 5, &[("abc", 0)]).apply(RedrawEvent::CursorGoto { row: 0, col: 2 });
        let focused = running(g.clone());
        let unfocused = AppState {
            focus: Panel::Files,
            ..running(g)
        };
        let cursor = |s: &AppState| {
            let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
            terminal.draw(|frame| crate::render(frame, s)).unwrap();
            let backend = terminal.backend();
            let position = backend.cursor_visible().then(|| backend.cursor_position());
            (position, backend.buffer().clone())
        };

        let (position, buf) = cursor(&focused);
        let position = position.expect("cursor shown");
        assert_eq!(buf[(position.x, position.y)].symbol(), "c");
        assert_eq!(cursor(&unfocused).0, None);
    }

    #[test]
    fn highlights_become_colors_and_modifiers() {
        let attr = HlAttr {
            fg: Some(0x102030),
            bold: true,
            undercurl: true,
            special: Some(0xff0000),
            ..HlAttr::default()
        };
        let g = grid(40, 5, &[("x", 3)])
            .apply(RedrawEvent::HlAttrDefine { id: 3, attr })
            .apply(RedrawEvent::DefaultColors {
                fg: Some(0xffffff),
                bg: Some(0x000001),
            });
        let s = running(g);
        let text = screen(&s);
        let (x, y) = crate::testing::find_on_screen(&text, "│x").unwrap();

        let cell = buffer(&s, 100, 24)[(x + 1, y)].clone();

        assert_eq!(cell.fg, Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(cell.bg, Color::Rgb(0, 0, 1), "the default background");
        assert!(
            cell.modifier
                .contains(Modifier::BOLD | Modifier::UNDERLINED)
        );
        assert_eq!(cell.underline_color, Color::Rgb(0xff, 0, 0));
    }

    #[test]
    fn a_grid_larger_than_the_panel_is_clipped_not_spilled() {
        let wide = "x".repeat(200);
        let s = running(grid(200, 50, &[(&wide, 0)]));

        let text = screen(&s);

        let line = text.lines().find(|l| l.contains("xxx")).unwrap();
        assert!(line.contains("x│"), "stops at the panel border: {line}");
    }

    #[test]
    fn a_wide_character_cut_by_the_edge_is_not_drawn_half() {
        let mut g = grid(2, 1, &[]);
        g = g.apply(RedrawEvent::Line {
            row: 0,
            col: 0,
            cells: vec![
                LineCell {
                    text: "界".into(),
                    hl: None,
                    repeat: 1,
                },
                LineCell {
                    text: String::new(),
                    hl: None,
                    repeat: 1,
                },
            ],
        });
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));

        draw_screen(&mut buf, Rect::new(0, 0, 1, 1), &g);

        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn notes_say_where_neovim_is() {
        let starting = AppState {
            editor_launching: true,
            ..state()
        };
        let split = AppState {
            config: lf_mide_core::Config {
                editor: EditorMode::Split,
                ..lf_mide_core::Config::default()
            },
            ..state()
        };

        assert!(screen(&state()).contains("Press e, or choose a file"));
        assert!(screen(&starting).contains("Starting Neovim"));
        assert!(screen(&split).contains("split pane"));
    }
}
