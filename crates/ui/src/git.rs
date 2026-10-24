//! The Git tab, the diff overlay and the base picker.

use crate::theme;
use lf_mide_core::diff_view::{DiffContent, DiffView, SideRow, hunk_header, side_rows};
use lf_mide_core::layout::diff_viewport_rows;
use lf_mide_core::{AppState, ChangeKind, DiffLine, FileChange, FileDiff, LineOrigin};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use lf_mide_core::text::visible as printable;

use crate::search::{centered, window_top};

const NUMBER_WIDTH: usize = 5;
const HINT: &str = "j/k scroll · n/N change · s layout · b base · Enter open in editor · Esc close";
const PICKER_TITLE: &str = " Compare against · j/k · Enter choose · Esc cancel ";
const PICKER_WIDTH: u16 = 72;
const MARGIN_X: u16 = 2;
const MARGIN_Y: u16 = 1;

fn kind_color(kind: ChangeKind) -> Color {
    match kind {
        ChangeKind::Modified => theme::current().warn,
        ChangeKind::Added => theme::current().ok,
        ChangeKind::Deleted => theme::current().error,
        ChangeKind::Renamed => theme::current().info,
        ChangeKind::Untracked => theme::current().muted,
        ChangeKind::Conflicted => theme::current().changed,
    }
}

/// The content of the Git tab: one line per changed file.
pub(crate) fn change_lines(state: &AppState, rows: usize) -> Vec<Line<'static>> {
    if !state.git_loaded {
        return vec![muted("Reading the repository…")];
    }
    if state.git_changes.is_empty() {
        return vec![muted("Working tree clean: nothing differs from HEAD")];
    }
    let top = window_top(state.git_selected, state.git_changes.len(), rows);
    state
        .git_changes
        .iter()
        .enumerate()
        .skip(top)
        .take(rows)
        .map(|(index, change)| change_line(change, index == state.git_selected))
        .collect()
}

fn change_line(change: &FileChange, selected: bool) -> Line<'static> {
    let pointer = if selected { "▶ " } else { "  " };
    let staged = if change.staged { "+" } else { " " };
    let mut style = Style::default();
    if selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::styled(pointer, Style::new().fg(theme::current().accent)),
        Span::styled(staged, Style::new().fg(theme::current().ok)),
        Span::styled(
            change.kind.letter().to_string(),
            Style::new().fg(kind_color(change.kind)),
        ),
        Span::styled(
            format!(" {}", printable(&change.path.to_string_lossy())),
            style,
        ),
    ])
}

fn muted(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_string(),
        Style::new().fg(theme::current().muted),
    ))
}

pub(crate) fn render_diff(frame: &mut Frame, state: &AppState) {
    let Some(view) = &state.diff else {
        return;
    };
    let screen = frame.area();
    let popup = Rect::new(
        screen.x + MARGIN_X.min(screen.width / 2),
        screen.y + MARGIN_Y.min(screen.height / 2),
        screen.width.saturating_sub(2 * MARGIN_X).max(1),
        screen.height.saturating_sub(2 * MARGIN_Y).max(1),
    );
    let block = Block::bordered()
        .title(format!(
            " Diff · {} vs {} ",
            printable(&view.path.to_string_lossy()),
            printable(&view.base.label)
        ))
        .border_style(Style::new().fg(theme::current().accent));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let rows = usize::from(diff_viewport_rows(screen.height));
    let mut lines = vec![summary_line(view)];
    lines.extend(body_lines(view, rows, usize::from(inner.width)));
    let hint_row = usize::from(inner.height).saturating_sub(1);
    lines.resize(hint_row.max(lines.len().min(hint_row)), Line::raw(""));
    lines.truncate(hint_row);
    lines.push(muted(HINT));
    frame.render_widget(Paragraph::new(lines), inner);
    render_picker(frame, state, view);
}

fn summary_line(view: &DiffView) -> Line<'static> {
    let layout = if view.side_by_side {
        "side by side"
    } else {
        "unified"
    };
    let text = match view.diff() {
        Some(diff) => {
            let (added, removed) = counts(diff);
            let hunks = diff.hunks.len();
            let current = view.current_hunk().map_or(0, |n| n + 1);
            format!("change {current}/{hunks} · +{added} −{removed} · {layout}")
        }
        None => layout.to_string(),
    };
    muted(&text)
}

fn counts(diff: &FileDiff) -> (usize, usize) {
    let all = diff.hunks.iter().flat_map(|h| &h.lines);
    all.fold((0, 0), |(added, removed), line| match line.origin {
        LineOrigin::Added => (added + 1, removed),
        LineOrigin::Removed => (added, removed + 1),
        LineOrigin::Context => (added, removed),
    })
}

fn body_lines(view: &DiffView, rows: usize, width: usize) -> Vec<Line<'static>> {
    let diff = match &view.content {
        DiffContent::Loading => return vec![muted("Loading the diff…")],
        DiffContent::Failed(message) => {
            return vec![Line::from(Span::styled(
                format!("Could not read the diff: {message}"),
                Style::new().fg(theme::current().error),
            ))];
        }
        DiffContent::Ready(diff) => diff,
    };
    if diff.binary && diff.hunks.is_empty() {
        return vec![muted("Binary or very large file: no text to compare")];
    }
    if diff.is_empty() {
        return vec![muted(&format!(
            "No differences from {}",
            printable(&view.base.label)
        ))];
    }
    let mut lines = if view.side_by_side {
        side_by_side_lines(view, diff, rows, width)
    } else {
        unified_lines(diff, view.scroll, rows)
    };
    if diff.truncated && lines.len() < rows {
        lines.push(muted("… the diff was cut: it is very long"));
    }
    lines
}

fn number(value: Option<u32>) -> String {
    value.map_or_else(
        || " ".repeat(NUMBER_WIDTH),
        |n| format!("{n:>NUMBER_WIDTH$}"),
    )
}

fn origin_style(origin: LineOrigin) -> (&'static str, Style) {
    match origin {
        LineOrigin::Added => ("+", Style::new().fg(theme::current().ok)),
        LineOrigin::Removed => ("-", Style::new().fg(theme::current().error)),
        LineOrigin::Context => (" ", Style::default()),
    }
}

fn unified_lines(diff: &FileDiff, scroll: usize, rows: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(rows);
    let mut row = 0usize;
    'hunks: for hunk in &diff.hunks {
        let span = 1 + hunk.lines.len();
        if row + span <= scroll {
            row += span;
            continue;
        }
        for offset in 0..span {
            if row >= scroll {
                lines.push(if offset == 0 {
                    Line::from(Span::styled(
                        hunk_header(hunk),
                        Style::new().fg(theme::current().info),
                    ))
                } else {
                    unified_line(&hunk.lines[offset - 1])
                });
                if lines.len() == rows {
                    break 'hunks;
                }
            }
            row += 1;
        }
    }
    lines
}

fn unified_line(line: &DiffLine) -> Line<'static> {
    let (mark, style) = origin_style(line.origin);
    Line::from(vec![
        Span::styled(
            format!("{} {} ", number(line.old_no), number(line.new_no)),
            Style::new().fg(theme::current().muted),
        ),
        Span::styled(format!("{mark} {}", line.text), style),
    ])
}

fn side_by_side_lines(
    view: &DiffView,
    diff: &FileDiff,
    rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    // Built once when the layout was chosen, not on every frame.
    let all = view
        .side_cache
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(side_rows(diff)));
    let column = width.saturating_sub(3) / 2;
    all.iter()
        .skip(view.scroll)
        .take(rows)
        .map(|row| match row {
            SideRow::Header(text) => Line::from(Span::styled(
                text.clone(),
                Style::new().fg(theme::current().info),
            )),
            SideRow::Pair { left, right } => {
                let mut spans = side_cell(left.as_ref(), column, LineOrigin::Removed);
                spans.push(Span::styled(" │ ", Style::new().fg(theme::current().muted)));
                spans.extend(side_cell(right.as_ref(), column, LineOrigin::Added));
                Line::from(spans)
            }
        })
        .collect()
}

/// One side of a row, padded to `width` columns. Context shows on both sides.
fn side_cell(line: Option<&DiffLine>, width: usize, changed: LineOrigin) -> Vec<Span<'static>> {
    let Some(line) = line else {
        return vec![Span::raw(" ".repeat(width))];
    };
    let numbered = if changed == LineOrigin::Removed {
        line.old_no
    } else {
        line.new_no
    };
    let style = if line.origin == changed {
        origin_style(changed).1
    } else {
        Style::default()
    };
    let text: String = line
        .text
        .chars()
        .take(width.saturating_sub(NUMBER_WIDTH + 1))
        .collect();
    let used = NUMBER_WIDTH + 1 + text.chars().count();
    vec![
        Span::styled(
            format!("{} ", number(numbered)),
            Style::new().fg(theme::current().muted),
        ),
        Span::styled(
            format!("{text}{}", " ".repeat(width.saturating_sub(used))),
            style,
        ),
    ]
}

fn render_picker(frame: &mut Frame, state: &AppState, view: &DiffView) {
    let Some(highlight) = view.picker else {
        return;
    };
    let screen = frame.area();
    let wanted = u16::try_from(state.bases.len().max(1) + 2).unwrap_or(u16::MAX);
    let popup = centered(
        screen,
        PICKER_WIDTH.min(screen.width),
        wanted.min(screen.height),
    );
    let block = Block::bordered()
        .title(PICKER_TITLE)
        .border_style(Style::new().fg(theme::current().accent));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let lines: Vec<Line> = if state.bases.is_empty() {
        vec![muted("Reading branches and commits…")]
    } else {
        let visible = usize::from(inner.height).max(1);
        let top = window_top(highlight, state.bases.len(), visible);
        state
            .bases
            .iter()
            .enumerate()
            .skip(top)
            .take(visible)
            .map(|(index, base)| {
                let pointer = if index == highlight { "▶ " } else { "  " };
                let chosen = if *base == view.base { "● " } else { "  " };
                Line::from(vec![
                    Span::styled(pointer, Style::new().fg(theme::current().accent)),
                    Span::raw(format!("{chosen}{}", printable(&base.label))),
                ])
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use lf_mide_core::{Action, BaseKind, BaseRef, Hunk, reduce};

    use super::*;
    use crate::testing::{buffer, find_on_screen, screen_sized, state};

    fn change(path: &str, kind: ChangeKind, staged: bool) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            kind,
            staged,
            stamp: 1,
        }
    }

    fn with_changes(changes: Vec<FileChange>) -> AppState {
        let s = reduce(
            state(),
            Action::GitLoaded {
                head: Some("abc".into()),
                changes: Arc::new(changes),
            },
        );
        reduce(s, Action::ShowGit)
    }

    fn screen(s: &AppState) -> String {
        screen_sized(s, 120, 34)
    }

    fn dl(origin: LineOrigin, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            origin,
            text: text.into(),
            old_no: old,
            new_no: new,
        }
    }

    fn sample_diff() -> FileDiff {
        FileDiff {
            hunks: vec![Hunk {
                old_start: 10,
                old_lines: 3,
                new_start: 10,
                new_lines: 4,
                lines: vec![
                    dl(LineOrigin::Context, Some(10), Some(10), "fun main() {"),
                    dl(LineOrigin::Removed, Some(11), None, "    println(\"old\")"),
                    dl(LineOrigin::Added, None, Some(11), "    println(\"new\")"),
                    dl(LineOrigin::Added, None, Some(12), "    println(\"more\")"),
                    dl(LineOrigin::Context, Some(12), Some(13), "}"),
                ],
            }],
            ..FileDiff::default()
        }
    }

    fn open_diff(diff: FileDiff) -> AppState {
        let mut s = state();
        s.terminal_height = 34;
        s.current_file = Some(PathBuf::from("src/App.kt"));
        let s = reduce(s, Action::OpenDiffCurrent);
        let id = s.diff.as_ref().unwrap().id;
        reduce(
            s,
            Action::DiffLoaded {
                id,
                result: Ok(Arc::new(diff)),
            },
        )
    }

    // --- the Git tab ---

    #[test]
    fn the_git_tab_lists_changed_files_with_their_kind_and_staging() {
        let s = with_changes(vec![
            change("a/Main.kt", ChangeKind::Modified, false),
            change("b/New.kt", ChangeKind::Added, true),
            change("c/notes.txt", ChangeKind::Untracked, false),
        ]);

        let text = screen(&s);

        assert!(text.contains("▶  M a/Main.kt"), "{text}");
        assert!(text.contains("  +A b/New.kt"));
        assert!(text.contains("   ? c/notes.txt"));
    }

    #[test]
    fn the_git_tab_says_when_the_tree_is_clean_or_not_read_yet() {
        let clean = with_changes(vec![]);
        let unread = reduce(state(), Action::ShowGit);

        assert!(screen(&clean).contains("Working tree clean"));
        assert!(screen(&unread).contains("Reading the repository"));
    }

    #[test]
    fn a_long_list_keeps_the_highlight_on_screen() {
        let many: Vec<FileChange> = (0..40)
            .map(|n| change(&format!("src/file{n:02}.kt"), ChangeKind::Modified, false))
            .collect();
        let mut s = with_changes(many);
        s = reduce(s, Action::OutputScroll(-30));

        assert!(screen(&s).contains("▶  M src/file30.kt"));
    }

    // --- the diff overlay ---

    #[test]
    fn the_unified_view_shows_numbers_marks_and_a_summary() {
        let text = screen(&open_diff(sample_diff()));

        assert!(text.contains("Diff · src/App.kt vs HEAD"), "{text}");
        assert!(text.contains("change 1/1 · +2 −1 · unified"));
        assert!(text.contains("@@ -10,3 +10,4 @@"));
        assert!(text.contains("   10    10   fun main() {"));
        assert!(text.contains("   11       - "));
        assert!(text.contains("       11 +     println(\"new\")"));
        assert!(text.contains(HINT));
    }

    #[test]
    fn added_lines_are_green_removed_red_and_headers_cyan() {
        let s = open_diff(sample_diff());
        let text = screen(&s);
        let buf = buffer(&s, 120, 34);
        let colour_of = |needle: &str| {
            let (x, y) = find_on_screen(&text, needle).unwrap();
            buf[(x, y)].fg
        };

        assert_eq!(colour_of("println(\"new\")"), theme::current().ok);
        assert_eq!(colour_of("println(\"old\")"), theme::current().error);
        assert_eq!(colour_of("@@ -10"), theme::current().info);
    }

    #[test]
    fn the_side_by_side_view_puts_old_text_on_the_left_and_new_on_the_right() {
        let s = reduce(open_diff(sample_diff()), Action::DiffLayout);

        let text = screen(&s);

        assert!(text.contains("side by side"));
        let row = text
            .lines()
            .find(|l| l.contains("println(\"old\")"))
            .unwrap();
        let (old, new) = (row.find("old").unwrap(), row.find("new").unwrap());
        assert!(old < new && row[old..new].contains('│'), "{row}");
        assert!(
            text.lines()
                .any(|l| l.contains("println(\"more\")") && !l.contains("println(\"old\")"))
        );
    }

    #[test]
    fn scrolling_moves_the_window_over_the_rows() {
        let many: Vec<DiffLine> = (1..=80)
            .map(|n| dl(LineOrigin::Added, None, Some(n), &format!("row number {n}")))
            .collect();
        let diff = FileDiff {
            hunks: vec![Hunk {
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 80,
                lines: many,
            }],
            ..FileDiff::default()
        };
        let top = open_diff(diff);
        let scrolled = reduce(top.clone(), Action::DiffScroll(40));

        assert!(screen(&top).contains("row number 1\n") || screen(&top).contains("row number 1 "));
        assert!(!screen(&top).contains("row number 60"));
        let moved = screen(&scrolled);
        assert!(moved.contains("row number 45") && !moved.contains("row number 2 "));
    }

    #[test]
    fn loading_failure_empty_and_binary_each_have_their_own_message() {
        let mut loading = state();
        loading.current_file = Some(PathBuf::from("a.kt"));
        let loading = reduce(loading, Action::OpenDiffCurrent);
        let failed = reduce(
            loading.clone(),
            Action::DiffLoaded {
                id: 1,
                result: Err("boom".into()),
            },
        );
        let empty = open_diff(FileDiff::default());
        let binary = open_diff(FileDiff {
            binary: true,
            ..FileDiff::default()
        });

        assert!(screen(&loading).contains("Loading the diff"));
        assert!(screen(&failed).contains("Could not read the diff: boom"));
        assert!(screen(&empty).contains("No differences from HEAD"));
        assert!(screen(&binary).contains("Binary or very large file"));
    }

    #[test]
    fn a_truncated_diff_says_so() {
        let mut diff = sample_diff();
        diff.truncated = true;

        assert!(screen(&open_diff(diff)).contains("the diff was cut"));
    }

    #[test]
    fn the_base_picker_lists_branches_and_commits_and_marks_the_one_in_use() {
        let s = open_diff(sample_diff());
        let bases = vec![
            BaseRef::head(),
            BaseRef {
                kind: BaseKind::Branch,
                label: "feature/login".into(),
                spec: "refs/heads/feature/login".into(),
            },
            BaseRef {
                kind: BaseKind::Commit,
                label: "a1b2c3d Add the screen".into(),
                spec: "a1b2c3d".into(),
            },
        ];
        let s = reduce(
            reduce(s, Action::BasesLoaded(Arc::new(bases))),
            Action::BasePickerOpen,
        );
        let s = reduce(s, Action::BasePickerMove(1));

        let text = screen(&s);

        assert!(text.contains("Compare against"));
        assert!(text.contains("▶   feature/login"), "{text}");
        assert!(text.contains("  ● HEAD"));
        assert!(text.contains("a1b2c3d Add the screen"));
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let s = open_diff(sample_diff());
        for (w, h) in [(1, 1), (5, 3), (10, 6), (20, 8), (60, 12)] {
            let _ = screen_sized(&s, w, h);
            let _ = screen_sized(&reduce(s.clone(), Action::DiffLayout), w, h);
            let _ = screen_sized(&reduce(s.clone(), Action::BasePickerOpen), w, h);
        }
    }

    #[test]
    fn repository_text_cannot_smuggle_control_characters_onto_the_screen() {
        let diff = FileDiff {
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 0,
                new_start: 1,
                new_lines: 1,
                lines: vec![dl(LineOrigin::Added, None, Some(1), "plain\u{1b}[2Jtext")],
            }],
            ..FileDiff::default()
        };

        let text = screen(&open_diff(diff));

        assert!(!text.chars().any(|c| c == '\u{1b}'));
    }
}
