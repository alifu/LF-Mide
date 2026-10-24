//! The fuzzy file finder and the project search, drawn as overlays.

use lf_mide_core::text::visible as printable;
use lf_mide_core::{AppState, FileMatch, GrepState};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::{ACCENT, MUTED};

const FINDER_TITLE: &str = " Find file · Enter open · Esc cancel ";
const GREP_TITLE: &str = " Search · Enter search / open · Esc cancel ";
const MAX_WIDTH: u16 = 100;
const MAX_HEIGHT: u16 = 24;
const MIN_HEIGHT: u16 = 5;
const CURSOR: char = '▏';

/// A `width` x `height` rectangle centered in `area`, shrunk to fit.
pub(crate) fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

/// First list row to show so that `selected` is visible, centered when the
/// list is longer than the `visible` rows available.
pub(crate) fn window_top(selected: usize, len: usize, visible: usize) -> usize {
    if visible == 0 || len <= visible {
        return 0;
    }
    selected.saturating_sub(visible / 2).min(len - visible)
}

/// `text` as a line where the characters at `indices` stand out.
pub(crate) fn highlighted_line(
    prefix: &str,
    text: &str,
    indices: &[u32],
    base: Style,
) -> Line<'static> {
    let hit_style = base.add_modifier(Modifier::BOLD).fg(Color::Yellow);
    let mut spans = vec![Span::styled(prefix.to_string(), base)];
    let mut run = String::new();
    let mut run_is_hit = false;
    for (position, ch) in text.chars().enumerate() {
        let is_hit = indices.binary_search(&(position as u32)).is_ok();
        if is_hit != run_is_hit && !run.is_empty() {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_is_hit { hit_style } else { base },
            ));
        }
        run_is_hit = is_hit;
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, if run_is_hit { hit_style } else { base }));
    }
    Line::from(spans)
}

/// The overlay rectangle and its bordered block, already drawn; returns the
/// area inside the border.
fn draw_frame(frame: &mut Frame, title: &str) -> Rect {
    let area = frame.area();
    let width = u16::try_from(u32::from(area.width) * 9 / 10)
        .unwrap_or(u16::MAX)
        .min(MAX_WIDTH);
    let height = u16::try_from(u32::from(area.height) * 8 / 10)
        .unwrap_or(u16::MAX)
        .clamp(MIN_HEIGHT, MAX_HEIGHT);
    let popup = centered(area, width, height);
    let block = Block::bordered()
        .title(title.to_string())
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    inner
}

/// `symbol query▏` on one line.
fn prompt_line(symbol: &str, query: &str) -> Line<'static> {
    let accent = Style::new().fg(ACCENT);
    Line::from(vec![
        Span::styled(format!("{symbol} "), accent),
        Span::raw(query.to_string()),
        Span::styled(CURSOR.to_string(), accent),
    ])
}

/// Prompt, status line and list areas inside an overlay.
fn split_inner(inner: Rect) -> (Rect, Rect, Rect) {
    let [prompt, status, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    (prompt, status, list)
}

fn pointer(selected: bool) -> &'static str {
    if selected { "▶" } else { " " }
}

pub(crate) fn render_finder(frame: &mut Frame, state: &AppState) {
    let Some(finder) = &state.finder else { return };
    let inner = draw_frame(frame, FINDER_TITLE);
    let (prompt, status, list) = split_inner(inner);
    frame.render_widget(Paragraph::new(prompt_line(">", &finder.query)), prompt);

    let count = finder.matches.len();
    let note = match (count, finder.query.is_empty()) {
        (0, true) => "Type to search".to_string(),
        (0, false) => "No matches".to_string(),
        (n, true) => format!("{n} files · type to filter"),
        (1, false) => "1 match".to_string(),
        (n, false) => format!("{n} matches"),
    };
    frame.render_widget(
        Paragraph::new(Line::styled(note, Style::new().fg(MUTED))),
        status,
    );

    let visible = usize::from(list.height);
    let top = window_top(finder.selected, count, visible);
    let lines: Vec<Line> = finder
        .matches
        .iter()
        .enumerate()
        .skip(top)
        .take(visible)
        .map(|(index, found)| finder_row(index == finder.selected, found))
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
}

fn finder_row(selected: bool, found: &FileMatch) -> Line<'static> {
    let base = if selected {
        Style::new().fg(ACCENT)
    } else {
        Style::new()
    };
    highlighted_line(
        &format!("{} ", pointer(selected)),
        &printable(&found.path.to_string_lossy()),
        &found.indices,
        base,
    )
}

pub(crate) fn render_grep(frame: &mut Frame, state: &AppState) {
    let Some(grep) = &state.grep else { return };
    let inner = draw_frame(frame, GREP_TITLE);
    let (prompt, status, list) = split_inner(inner);
    frame.render_widget(Paragraph::new(prompt_line("/", &grep.query)), prompt);
    frame.render_widget(Paragraph::new(grep_status(grep)), status);

    let visible = usize::from(list.height);
    let top = window_top(grep.selected, grep.hits.len(), visible);
    let lines: Vec<Line> = grep
        .hits
        .iter()
        .enumerate()
        .skip(top)
        .take(visible)
        .map(|(index, hit)| {
            let selected = index == grep.selected;
            let style = if selected {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            let text = format!(
                "{} {}:{}  {}",
                pointer(selected),
                printable(&hit.path.to_string_lossy()),
                hit.line,
                printable(&hit.text)
            );
            Line::styled(text, style)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
}

fn grep_status(grep: &GrepState) -> Line<'static> {
    let muted = Style::new().fg(MUTED);
    if grep.running {
        return Line::styled("Searching…", muted);
    }
    if let Some(error) = &grep.error {
        return Line::styled(printable(error).into_owned(), Style::new().fg(Color::Red));
    }
    match (grep.searched.is_some(), grep.hits.len()) {
        (false, _) => Line::styled("Enter to search", muted),
        (true, 0) => Line::styled("No results", muted),
        (true, 1) => Line::styled("1 result", muted),
        (true, n) => Line::styled(format!("{n} results"), muted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{buffer, find_on_screen, screen, screen_sized, state, with_files};
    use lf_mide_core::{Action, GrepHit, reduce};
    use std::path::PathBuf;

    fn fm(path: &str, indices: &[u32]) -> FileMatch {
        FileMatch {
            path: PathBuf::from(path),
            score: 1,
            indices: indices.to_vec(),
        }
    }

    fn finder(query: &str, matches: Vec<FileMatch>) -> AppState {
        let mut s = reduce(state(), Action::OpenFinder);
        for ch in query.chars() {
            s = reduce(s, Action::FinderInput(ch));
        }
        reduce(
            s,
            Action::FinderResults {
                query: query.to_string(),
                matches,
            },
        )
    }

    fn hit(path: &str, line: u32, text: &str) -> GrepHit {
        GrepHit {
            path: PathBuf::from(path),
            line,
            column: 0,
            text: text.into(),
        }
    }

    fn grep(query: &str, hits: Vec<GrepHit>) -> AppState {
        let mut s = reduce(state(), Action::OpenGrep);
        for ch in query.chars() {
            s = reduce(s, Action::GrepInput(ch));
        }
        let s = reduce(s, Action::GrepSearch);
        reduce(
            s,
            Action::GrepResults {
                query: query.to_string(),
                result: Ok(hits),
            },
        )
    }

    // --- helpers ---

    #[test]
    fn centered_shrinks_to_fit_and_never_overflows() {
        let area = Rect::new(0, 0, 100, 40);

        assert_eq!(centered(area, 50, 20), Rect::new(25, 10, 50, 20));
        assert_eq!(centered(area, 500, 500), area);
        assert_eq!(
            centered(Rect::new(2, 3, 10, 4), 6, 2),
            Rect::new(4, 4, 6, 2)
        );
        assert_eq!(centered(Rect::new(0, 0, 0, 0), 5, 5), Rect::new(0, 0, 0, 0));
    }

    #[test]
    fn window_keeps_the_selection_visible_and_centered() {
        assert_eq!(window_top(0, 100, 10), 0);
        assert_eq!(window_top(4, 100, 10), 0);
        assert_eq!(window_top(30, 100, 10), 25);
        assert_eq!(window_top(99, 100, 10), 90, "never scrolls past the end");
        assert_eq!(window_top(3, 5, 10), 0, "short lists are not scrolled");
        assert_eq!(window_top(0, 0, 10), 0);
        assert_eq!(window_top(5, 100, 0), 0);
    }

    #[test]
    fn highlighted_line_marks_only_the_matched_characters() {
        let line = highlighted_line("> ", "ab.kt", &[0, 1, 4], Style::new());

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "> ab.kt");
        let bold: String = line
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::BOLD))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(bold, "abt");
    }

    #[test]
    fn highlight_indices_past_the_text_are_ignored() {
        let line = highlighted_line("", "ab", &[0, 99], Style::new());

        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "ab");
    }

    // --- finder ---

    #[test]
    fn the_finder_is_hidden_until_opened() {
        assert!(!screen(&state()).contains("Find file"));
    }

    #[test]
    fn an_empty_finder_invites_typing() {
        let text = screen(&reduce(state(), Action::OpenFinder));

        assert!(text.contains("Find file"), "{text}");
        assert!(text.contains("Type to search"), "{text}");
        assert!(text.contains(CURSOR), "the prompt shows a cursor: {text}");
    }

    #[test]
    fn the_finder_shows_the_query_matches_and_a_count() {
        let s = finder("kt", vec![fm("src/App.kt", &[]), fm("src/Util.kt", &[])]);

        let text = screen(&s);

        assert!(text.contains("> kt"), "{text}");
        assert!(text.contains("src/App.kt") && text.contains("src/Util.kt"));
        assert!(text.contains("2 matches"), "{text}");
    }

    #[test]
    fn the_finder_says_when_nothing_matches() {
        let text = screen(&finder("zzz", vec![]));

        assert!(text.contains("No matches"), "{text}");
    }

    #[test]
    fn the_finder_marks_the_highlighted_row_and_the_matched_letters() {
        let s = reduce(
            finder("ab", vec![fm("ab.kt", &[0, 1]), fm("cd.kt", &[])]),
            Action::FinderMove(1),
        );
        let text = screen(&s);
        let buf = buffer(&s, 100, 24);

        let (col, row) = find_on_screen(&text, "ab.kt").unwrap();
        assert!(
            buf[(col, row)].modifier.contains(Modifier::BOLD),
            "matched letter is bold"
        );
        assert!(
            !buf[(col + 3, row)].modifier.contains(Modifier::BOLD),
            "unmatched letter is not"
        );
        let (_, cd_row) = find_on_screen(&text, "cd.kt").unwrap();
        let cd_line = text.lines().nth(cd_row as usize).unwrap();
        assert!(
            cd_line.contains('▶'),
            "highlight is on the second row: {cd_line}"
        );
        let ab_line = text.lines().nth(row as usize).unwrap();
        assert!(!ab_line.contains('▶'));
    }

    #[test]
    fn a_long_result_list_scrolls_with_the_highlight() {
        let names: Vec<String> = (0..80).map(|i| format!("dir/file{i:02}.kt")).collect();
        let matches: Vec<FileMatch> = names.iter().map(|n| fm(n, &[])).collect();
        let s = reduce(finder("f", matches), Action::FinderMove(60));

        let text = screen(&s);

        assert!(text.contains("file60.kt"), "{text}");
        assert!(!text.contains("file00.kt"));
    }

    // --- project search ---

    #[test]
    fn the_search_is_hidden_until_opened() {
        assert!(!screen(&state()).contains("Search ·"));
    }

    #[test]
    fn a_fresh_search_explains_what_enter_does() {
        let text = screen(&reduce(state(), Action::OpenGrep));

        assert!(text.contains("Search ·"), "{text}");
        assert!(text.contains("Enter to search"), "{text}");
    }

    #[test]
    fn a_running_search_shows_progress() {
        let mut s = reduce(state(), Action::OpenGrep);
        s = reduce(s, Action::GrepInput('x'));
        s = reduce(s, Action::GrepSearch);

        assert!(screen(&s).contains("Searching…"));
    }

    #[test]
    fn results_show_file_line_and_text() {
        let s = grep(
            "setActive",
            vec![
                hit("src/App.kt", 3, "vm.setActive(true)"),
                hit("notes.txt", 1, "setActive twice"),
            ],
        );

        let text = screen(&s);

        assert!(text.contains("/ setActive"), "{text}");
        assert!(
            text.contains("src/App.kt:3") && text.contains("vm.setActive(true)"),
            "{text}"
        );
        assert!(text.contains("notes.txt:1"));
        assert!(text.contains("2 results"), "{text}");
    }

    #[test]
    fn results_mark_the_highlighted_row() {
        let s = reduce(
            grep("x", vec![hit("a.kt", 1, "one"), hit("b.kt", 2, "two")]),
            Action::GrepMove(1),
        );

        let text = screen(&s);

        let (_, row) = find_on_screen(&text, "b.kt:2").unwrap();
        assert!(text.lines().nth(row as usize).unwrap().contains('▶'));
        let (_, other) = find_on_screen(&text, "a.kt:1").unwrap();
        assert!(!text.lines().nth(other as usize).unwrap().contains('▶'));
    }

    #[test]
    fn a_search_without_results_or_with_an_error_says_so() {
        let none = screen(&grep("zzz", vec![]));
        let mut failing = reduce(state(), Action::OpenGrep);
        failing = reduce(failing, Action::GrepInput('('));
        failing = reduce(failing, Action::GrepSearch);
        failing = reduce(
            failing,
            Action::GrepResults {
                query: "(".into(),
                result: Err("error: unclosed group".into()),
            },
        );

        assert!(none.contains("No results"), "{none}");
        assert!(screen(&failing).contains("error: unclosed group"));
    }

    // --- robustness ---

    #[test]
    fn overlays_survive_tiny_and_huge_terminals() {
        let overlays = [
            finder("kt", vec![fm("a.kt", &[0]), fm("b.kt", &[])]),
            grep("x", vec![hit("a.kt", 1, "one")]),
            reduce(with_files(state(), &["a.kt"]), Action::OpenFinder),
        ];
        for s in &overlays {
            for (w, h) in [
                (0, 0),
                (1, 1),
                (10, 3),
                (30, 6),
                (200, 5),
                (8000, 10),
                (60, 40),
            ] {
                let _ = screen_sized(s, w, h);
            }
        }
    }

    #[test]
    fn control_characters_in_results_never_reach_the_screen() {
        let s = grep("x", vec![hit("evil\x1b[2J.kt", 1, "text\x1b]0;pwned\x07")]);

        let buf = buffer(&s, 100, 24);

        assert!(
            !buf.content()
                .iter()
                .any(|c| c.symbol().chars().any(char::is_control))
        );
    }
}
