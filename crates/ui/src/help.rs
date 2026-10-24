//! The help overlay: the keys of the panels and a short Vim cheat sheet.

use crate::theme;
use lf_mide_core::AppState;
use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::search::centered;

const TITLE: &str = " Help · j/k scroll · ? or Esc close ";
const WIDTH: u16 = 76;
const KEY_COLUMN: usize = 14;

/// Every line of the overlay, headings included.
fn lines() -> Vec<Line<'static>> {
    let heading =
        |text: &str| Line::styled(text.to_string(), Style::new().add_modifier(Modifier::BOLD));
    let key = |keys: &str, what: &str| Line::raw(format!("  {keys:<KEY_COLUMN$} {what}"));
    let note =
        |text: &str| Line::styled(format!("  {text}"), Style::new().fg(theme::current().muted));
    vec![
        heading("Panels"),
        key("Tab / S-Tab", "move focus between panels"),
        key("j k h l", "move, collapse or expand in the file tree"),
        key("Enter", "open the file, or open or close a folder"),
        key("f  /", "find a file  ·  search the project"),
        key("e", "open the editor (Neovim) in its panel"),
        key("C-Space", "leave the editor for the panels"),
        key("B  s", "run the app  ·  cancel the build"),
        key("t  d", "switch Android/iOS  ·  focus the devices"),
        key("b x r", "in Devices: boot, stop, refresh"),
        key("o  L", "focus the output  ·  show or hide the logs"),
        key("1-5  F  c", "in the output: tab, filter, clear"),
        key("C  =  A", "git tab  ·  diff of this file  ·  blame"),
        key(
            "p P D i",
            "performance  ·  its indicator  ·  doctor  ·  environment",
        ),
        key("T  R", "choose the terminal  ·  reload the file tree"),
        key(
            "mouse",
            "click to focus or pick, wheel to scroll (Option-drag selects text)",
        ),
        key("q", "quit (Neovim asks first about unsaved changes)"),
        Line::raw(""),
        heading("Vim basics (inside the editor every key goes to Neovim)"),
        key(
            "i  a  o",
            "insert before the cursor, after it, on a new line",
        ),
        key("Esc", "back to normal mode"),
        key("h j k l", "left, down, up, right"),
        key(
            "w  b  0  $",
            "next word, previous word, line start, line end",
        ),
        key("gg  G", "top of the file, end of the file"),
        key("/text", "search the file (n next, N previous)"),
        key(
            "x  dd  yy",
            "delete a character, delete a line, copy a line",
        ),
        key("p  u  C-r", "paste, undo, redo"),
        key(":w  :q", "save, quit Neovim"),
        key(
            ":wq  :qa!",
            "save and quit, or quit everything without saving",
        ),
        note("Type :Tutor in the editor for Neovim's interactive lessons."),
    ]
}

pub(crate) fn render_help(frame: &mut Frame, state: &AppState) {
    if !state.help_open {
        return;
    }
    let lines = lines();
    let area = frame.area();
    let height = u16::try_from(lines.len() + 2)
        .unwrap_or(u16::MAX)
        .min(area.height);
    let popup = centered(area, WIDTH, height);
    let block = Block::bordered()
        .title(TITLE)
        .border_style(Style::new().fg(theme::current().accent));
    let inner = block.inner(popup);
    let last_top = lines.len().saturating_sub(usize::from(inner.height));
    let top = u16::try_from(usize::from(state.help_scroll).min(last_top)).unwrap_or(u16::MAX);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines).scroll((top, 0)), inner);
}

#[cfg(test)]
mod tests {
    use crate::testing::{screen_sized, state};
    use lf_mide_core::{Action, reduce};

    #[test]
    fn help_is_hidden_until_asked_for() {
        let text = screen_sized(&state(), 100, 40);

        assert!(!text.contains("Vim basics"), "{text}");
    }

    #[test]
    fn help_lists_the_panel_keys_and_a_vim_cheat_sheet() {
        let open = reduce(state(), Action::ToggleHelp);

        let text = screen_sized(&open, 100, 40);

        assert!(text.contains("find a file"), "{text}");
        assert!(text.contains("Vim basics"), "{text}");
    }

    #[test]
    fn a_short_terminal_scrolls_help_down_to_its_last_line() {
        let open = reduce(state(), Action::ToggleHelp);
        let last_line = ":Tutor";
        assert!(!screen_sized(&open, 100, 20).contains(last_line));

        let scrolled = reduce(open, Action::HelpScroll(i16::MAX));

        assert!(screen_sized(&scrolled, 100, 20).contains(last_line));
    }
}
