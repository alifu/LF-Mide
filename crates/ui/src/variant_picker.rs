//! The list for choosing the build variant (Android) or scheme (iOS).

use lf_mide_core::{AppState, choice_noun};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::theme;

const WIDTH: u16 = 56;
/// Rows of the list that stay on screen at once.
const MAX_ROWS: u16 = 14;

pub(crate) fn render_variant_picker(frame: &mut Frame, state: &AppState) {
    let Some(picker) = &state.variant_picker else {
        return;
    };
    let noun = choice_noun(picker.platform);
    let title = format!(" Build {noun} · Enter select · Esc cancel ");
    let listed = picker.options.as_ref().map_or(1, Vec::len) as u16;
    let screen = frame.area();
    let width = WIDTH.min(screen.width);
    let height = (listed.min(MAX_ROWS) + 2).min(screen.height);
    let popup = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(theme::current().accent));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let Some(options) = &picker.options else {
        let note = Line::styled(
            format!(" Reading the {noun}s of the project…"),
            Style::new().fg(theme::current().muted),
        );
        frame.render_widget(Paragraph::new(note), inner);
        return;
    };
    let visible = usize::from(inner.height);
    let first = picker.highlight.saturating_sub(visible.saturating_sub(1));
    let current = state.variants.get(&picker.platform);
    let lines: Vec<Line> = options
        .iter()
        .enumerate()
        .skip(first)
        .take(visible)
        .map(|(index, option)| {
            let pointer = if index == picker.highlight {
                "▶"
            } else {
                " "
            };
            let chosen = if Some(option) == current { "●" } else { " " };
            let style = if index == picker.highlight {
                Style::new()
                    .fg(theme::current().accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            Line::styled(format!(" {pointer} {chosen} {option}"), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use crate::testing::{screen, state};
    use lf_mide_core::{Action, Platform, reduce};

    fn open(names: &[&str]) -> lf_mide_core::AppState {
        let s = reduce(state(), Action::OpenVariantPicker);
        reduce(
            s,
            Action::VariantsLoaded {
                platform: Platform::Android,
                result: Ok(names.iter().map(ToString::to_string).collect()),
            },
        )
    }

    #[test]
    fn lists_the_choices_and_marks_the_highlight_and_the_current_one() {
        let mut s = open(&["debug", "freeDebug"]);
        s.variants.insert(Platform::Android, "freeDebug".into());

        let text = screen(&s);

        assert!(text.contains("Build variant"), "{text}");
        assert!(text.contains("▶   debug"), "{text}");
        assert!(text.contains("  ● freeDebug"), "{text}");
    }

    #[test]
    fn says_it_is_reading_while_the_choices_load() {
        let s = reduce(state(), Action::OpenVariantPicker);

        assert!(screen(&s).contains("Reading the variants"));
    }

    #[test]
    fn a_long_list_scrolls_to_keep_the_highlight_visible() {
        let names: Vec<String> = (0..40).map(|n| format!("variant{n:02}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let s = reduce(open(&refs), Action::VariantPickerMove(-1));

        let text = screen(&s);

        assert!(text.contains("variant39"), "{text}");
    }
}
