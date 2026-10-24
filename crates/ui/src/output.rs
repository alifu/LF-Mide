//! The Output panel: build log, device logs, problems, and the build
//! summary shown in the status bar.

use std::path::Path;

use lf_mide_core::logparse::{BuildLine, logcat_level, parse_build_line};
use lf_mide_core::{
    AppState, BuildPhase, LogLevel, OutputFilter, OutputTab, Panel, Platform, Problem, Severity,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use lf_mide_core::text::visible as printable;

use crate::{ACCENT, MUTED, panel_block};

/// Rows of the panel used by the tab line.
const TAB_ROWS: u16 = 1;
const SEPARATOR: &str = " │ ";

/// One line for the status bar: what the build is doing, or how the last
/// one ended. `None` before the first build.
pub(crate) fn build_summary(state: &AppState) -> Option<(String, Color)> {
    let build = &state.build;
    let seconds = |millis: Option<u64>| millis.map_or(0.0, |ms| ms as f64 / 1000.0);
    match build.phase {
        BuildPhase::Idle => None,
        BuildPhase::Running => {
            let mut text = format!("▶ {}", build.step);
            if let Some(device) = &build.device {
                text.push_str(&format!(" {device}"));
            }
            if let Some(task) = &build.task {
                text.push_str(&format!(" · {task}"));
            }
            Some((text, Color::Cyan))
        }
        BuildPhase::Succeeded => Some((
            format!("✓ built and running · {:.1}s", seconds(build.millis)),
            Color::Green,
        )),
        BuildPhase::Failed => {
            let errors = match build.errors {
                0 => String::new(),
                1 => " · 1 error".to_string(),
                n => format!(" · {n} errors"),
            };
            Some((format!("✗ build failed{errors}"), Color::Red))
        }
        BuildPhase::Cancelled => Some(("■ build cancelled".to_string(), Color::Yellow)),
    }
}

pub(crate) fn render_output(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = panel_block(state, Panel::Bottom, "Output");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let tabs = Rect::new(inner.x, inner.y, inner.width, TAB_ROWS);
    frame.render_widget(Paragraph::new(tab_line(state)), tabs);
    let content = Rect::new(
        inner.x,
        inner.y + TAB_ROWS,
        inner.width,
        inner.height.saturating_sub(TAB_ROWS),
    );
    if content.height == 0 {
        return;
    }
    let lines = match state.output_tab {
        OutputTab::Problems => problem_lines(state, content.height.into(), content.width.into()),
        OutputTab::Git => crate::git::change_lines(state, content.height.into()),
        tab => log_lines(state, tab, content.height.into()),
    };
    frame.render_widget(Paragraph::new(lines), content);
}

fn tab_line(state: &AppState) -> Line<'static> {
    let mut spans = Vec::new();
    for tab in OutputTab::ALL {
        if !spans.is_empty() {
            spans.push(Span::styled(SEPARATOR, Style::new().fg(MUTED)));
        }
        let style = if tab == state.output_tab {
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(MUTED)
        };
        spans.push(Span::styled(tab_label(state, tab), style));
    }
    if let Some(text) = filter_text(state) {
        spans.push(Span::styled(
            format!("   {text}"),
            Style::new().fg(Color::Yellow),
        ));
    }
    let scrolled = state.output_scroll[state.output_tab.index()];
    if scrolled > 0 {
        spans.push(Span::styled(
            format!("   ↑ {scrolled} older · G follows"),
            Style::new().fg(Color::Yellow),
        ));
    }
    Line::from(spans)
}

/// The filter as shown next to the tabs, while it matters for this tab.
fn filter_text(state: &AppState) -> Option<String> {
    let filter = &effective_filter(state, state.output_tab);
    let on_logs = matches!(
        state.output_tab,
        OutputTab::Build | OutputTab::Logcat | OutputTab::IosLog
    );
    if !on_logs || !filter.is_active() {
        return None;
    }
    let mut text = format!("filter: {}", filter.text);
    if filter.editing {
        text.push('▏');
    }
    if state.output_tab == OutputTab::Logcat && filter.min_level > LogLevel::Verbose {
        text.push_str(&format!(" · ≥ {}", filter.min_level.label()));
    }
    Some(text)
}

fn tab_label(state: &AppState, tab: OutputTab) -> String {
    match tab {
        OutputTab::Problems if !state.problems.is_empty() => {
            format!("Problems ({})", state.problems.len())
        }
        OutputTab::Logcat if state.log_streams.contains_key(&Platform::Android) => {
            "Logcat ●".to_string()
        }
        OutputTab::IosLog if state.log_streams.contains_key(&Platform::Ios) => {
            "iOS Log ●".to_string()
        }
        other => other.label().to_string(),
    }
}

fn muted(text: &str) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), Style::new().fg(MUTED)))
}

fn empty_hint(tab: OutputTab) -> &'static str {
    match tab {
        OutputTab::Build => "Press B to build the app and run it on the chosen device",
        OutputTab::Logcat => "Press L to follow the log of the chosen emulator",
        OutputTab::IosLog => "Press L to follow the log of the chosen simulator",
        OutputTab::Problems | OutputTab::Git => "",
    }
}

/// The newest `rows` lines, or the window `scroll` lines further back.
fn log_lines(state: &AppState, tab: OutputTab, rows: usize) -> Vec<Line<'static>> {
    let Some(log) = state.log(tab) else {
        return Vec::new();
    };
    if log.is_empty() {
        return vec![muted(empty_hint(tab))];
    }
    let scroll = state.output_scroll[tab.index()];
    let filter = &effective_filter(state, tab);
    if !filter.is_active() {
        let (start, end) = window(log.len(), scroll, rows);
        return (start..end)
            .filter_map(|index| log.get(index))
            .map(|line| styled_log_line(tab, line))
            .collect();
    }
    let shown: Vec<&str> = log
        .iter()
        .filter(|line| filter.allows(line, level_of(tab, line)))
        .collect();
    let nothing_matches = shown.iter().all(|line| line.starts_with("— "));
    let (start, end) = window(shown.len(), scroll, rows);
    let mut lines: Vec<Line<'static>> = shown[start..end]
        .iter()
        .map(|line| styled_log_line(tab, line))
        .collect();
    if nothing_matches {
        lines.push(muted("No lines match the filter (Esc clears it)"));
    }
    lines
}

/// The filter as it applies to `tab`: only logcat lines carry a level, so
/// the minimum level must not hide the other tabs' lines.
fn effective_filter(state: &AppState, tab: OutputTab) -> OutputFilter {
    let mut filter = state.output_filter.clone();
    if tab != OutputTab::Logcat {
        filter.min_level = LogLevel::Verbose;
    }
    filter
}

/// Only logcat lines carry a level.
fn level_of(tab: OutputTab, line: &str) -> Option<LogLevel> {
    (tab == OutputTab::Logcat)
        .then(|| logcat_level(line))
        .flatten()
}

/// The visible part of `len` lines when scrolled back by `scroll`.
fn window(len: usize, scroll: usize, rows: usize) -> (usize, usize) {
    let end = len.saturating_sub(scroll.min(len.saturating_sub(rows)));
    (end.saturating_sub(rows), end)
}

fn styled_log_line(tab: OutputTab, line: &str) -> Line<'static> {
    let color = match tab {
        OutputTab::Build => build_color(line),
        OutputTab::Logcat => logcat_color(line),
        _ => None,
    };
    let style = color.map_or(Style::default(), |color| Style::new().fg(color));
    Line::from(Span::styled(printable(line).into_owned(), style))
}

fn build_color(line: &str) -> Option<Color> {
    if line.starts_with("$ ") || line.starts_with("— ") {
        return Some(ACCENT);
    }
    match parse_build_line(line) {
        BuildLine::Problem(Problem {
            severity: Severity::Error,
            ..
        }) => Some(Color::Red),
        BuildLine::Problem(_) => Some(Color::Yellow),
        BuildLine::Task(_) => Some(MUTED),
        BuildLine::Finished { succeeded: true } => Some(Color::Green),
        BuildLine::Finished { succeeded: false } => Some(Color::Red),
        BuildLine::Other => None,
    }
}

fn logcat_color(line: &str) -> Option<Color> {
    if line.starts_with("— ") {
        return Some(ACCENT);
    }
    match logcat_level(line)? {
        LogLevel::Fatal | LogLevel::Error => Some(Color::Red),
        LogLevel::Warn => Some(Color::Yellow),
        LogLevel::Info => Some(Color::Green),
        LogLevel::Debug | LogLevel::Verbose => Some(MUTED),
    }
}

fn problem_lines(state: &AppState, rows: usize, width: usize) -> Vec<Line<'static>> {
    if state.problems.is_empty() {
        let text = match state.build.phase {
            BuildPhase::Idle => "Nothing built yet: press B",
            _ => "No errors or warnings",
        };
        return vec![muted(text)];
    }
    let selected = state.problem_selected.min(state.problems.len() - 1);
    let start = selected.saturating_sub(rows.saturating_sub(1));
    state
        .problems
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .map(|(index, problem)| {
            problem_line(&state.project.root, problem, index == selected, width)
        })
        .collect()
}

fn problem_line(root: &Path, problem: &Problem, selected: bool, width: usize) -> Line<'static> {
    let (glyph, color) = match problem.severity {
        Severity::Error => ("✗", Color::Red),
        Severity::Warning => ("!", Color::Yellow),
    };
    let pointer = if selected { "▶" } else { " " };
    let place = location_text(root, problem);
    let used = 2 + glyph.chars().count() + 1 + place.chars().count() + 2;
    let room = width.saturating_sub(used);
    let message: String = printable(&problem.message).chars().take(room).collect();
    let mut style = Style::default();
    if selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    Line::from(vec![
        Span::styled(format!("{pointer} "), Style::new().fg(ACCENT)),
        Span::styled(glyph, Style::new().fg(color)),
        Span::styled(format!(" {place}"), Style::new().fg(MUTED)),
        Span::styled(format!("  {message}"), style),
    ])
}

/// `path:line:col`, relative to the project when it is inside it.
fn location_text(root: &Path, problem: &Problem) -> String {
    let Some(path) = &problem.path else {
        return "(no location)".to_string();
    };
    let shown = Path::new(path)
        .strip_prefix(root)
        .map_or_else(|_| path.clone(), |relative| relative.display().to_string());
    let shown = printable(&shown).into_owned();
    match (problem.line, problem.column) {
        (Some(line), Some(column)) => format!("{shown}:{line}:{column}"),
        (Some(line), None) => format!("{shown}:{line}"),
        _ => shown,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lf_mide_core::{Action, BuildOutcome, Device, DeviceState, reduce};

    use super::*;
    use crate::testing::{buffer, screen_sized, state};

    fn running_android() -> AppState {
        let device = Device {
            platform: Platform::Android,
            id: "Pixel_8".into(),
            name: "Pixel_8".into(),
            detail: String::new(),
            state: DeviceState::Running,
            serial: Some("emulator-5554".into()),
        };
        reduce(
            state(),
            Action::DevicesLoaded {
                devices: Arc::new(vec![device]),
                errors: Default::default(),
            },
        )
    }

    fn building(lines: &[&str]) -> AppState {
        let s = reduce(running_android(), Action::RunApp);
        reduce(
            s,
            Action::BuildOutput {
                id: 1,
                lines: lines.iter().map(|l| l.to_string()).collect(),
            },
        )
    }

    fn screen(s: &AppState) -> String {
        screen_sized(s, 120, 30)
    }

    #[test]
    fn an_empty_build_tab_says_how_to_start() {
        assert!(screen(&state()).contains("Press B to build"));
    }

    #[test]
    fn the_tabs_are_listed_with_the_shown_one_marked_by_style() {
        let text = screen(&state());

        for tab in ["Build", "Logcat", "iOS Log", "Problems", "Git"] {
            assert!(text.contains(tab), "{tab}");
        }
    }

    #[test]
    fn build_output_is_shown_newest_at_the_bottom() {
        let many: Vec<String> = (0..200).map(|n| format!("compiling file {n}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();

        let text = screen(&building(&refs));

        assert!(text.contains("compiling file 199"));
        assert!(!text.contains("compiling file 10\n"));
    }

    #[test]
    fn scrolling_back_shows_older_lines_and_says_so() {
        let many: Vec<String> = (0..200).map(|n| format!("compiling file {n}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let s = reduce(building(&refs), Action::OutputScroll(50));

        let text = screen(&s);

        assert!(text.contains("↑ 50 older"));
        assert!(!text.contains("compiling file 199"));
        assert!(text.contains("compiling file 149"));
    }

    #[test]
    fn errors_and_tasks_get_their_own_colours() {
        let s = building(&[
            "e: file:///p/A.kt:3:4 Unresolved reference: x",
            "> Task :a:b",
            "plain",
        ]);
        let buf = buffer(&s, 120, 30);
        let text = screen(&s);
        let color_of = |needle: &str| {
            let (x, y) = crate::testing::find_on_screen(&text, needle).unwrap();
            buf[(x, y)].fg
        };

        assert_eq!(color_of("e: file"), Color::Red);
        assert_eq!(color_of("> Task"), MUTED);
        assert_eq!(color_of("plain"), Color::Reset);
    }

    #[test]
    fn the_status_bar_follows_the_build_from_start_to_finish() {
        let started = building(&["> Task :androidApp:compileDebugKotlin"]);
        let done = reduce(
            started.clone(),
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Succeeded,
                millis: 12_300,
            },
        );

        assert!(screen(&started).contains("▶ Building Pixel_8 · :androidApp:compileDebugKotlin"));
        assert!(screen(&done).contains("✓ built and running · 12.3s"));
    }

    #[test]
    fn a_failed_build_shows_the_error_count_and_the_problems() {
        let failed = reduce(
            building(&["e: file:///p/shared/A.kt:3:4 Unresolved reference: x"]),
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Failed(None),
                millis: 100,
            },
        );

        let text = screen(&failed);

        assert!(text.contains("✗ build failed · 1 error"));
        assert!(text.contains("Problems (1)"));
        assert!(
            text.contains("✗ shared/A.kt:3:4  Unresolved reference: x"),
            "{text}"
        );
    }

    #[test]
    fn a_cancelled_build_is_shown_as_cancelled() {
        let s = reduce(
            building(&[]),
            Action::BuildFinished {
                id: 1,
                outcome: BuildOutcome::Cancelled,
                millis: 1,
            },
        );

        assert!(screen(&s).contains("■ build cancelled"));
    }

    #[test]
    fn the_log_tabs_mark_the_streams_being_followed() {
        let s = reduce(running_android(), Action::ToggleLogs);

        let text = screen(&s);

        assert!(text.contains("Logcat ●"));
        assert!(text.contains("— following Pixel_8 —"));
    }

    #[test]
    fn logcat_lines_are_coloured_by_level() {
        let s = reduce(running_android(), Action::ToggleLogs);
        let s = reduce(
            s,
            Action::LogOutput {
                platform: Platform::Android,
                id: 1,
                lines: vec![
                    "10-24 09:15:02.123  1  2 E Tag: crashed".to_string(),
                    "10-24 09:15:02.124  1  2 D Tag: chatter".to_string(),
                ],
            },
        );
        let text = screen(&s);
        let buf = buffer(&s, 120, 30);
        let at = |needle: &str| {
            let (x, y) = crate::testing::find_on_screen(&text, needle).unwrap();
            buf[(x, y)].fg
        };

        assert_eq!(at("crashed"), Color::Red);
        assert_eq!(at("chatter"), MUTED);
    }

    #[test]
    fn the_problems_tab_marks_the_selection_and_explains_when_empty() {
        let empty = reduce(state(), Action::ShowOutputTab(OutputTab::Problems));
        let s = building(&[
            "e: file:///p/A.kt:1:1 first",
            "e: file:///p/B.kt:2:2 second",
        ]);
        let s = reduce(s, Action::ShowOutputTab(OutputTab::Problems));
        let s = reduce(s, Action::OutputScroll(-1));

        let text = screen(&s);

        assert!(screen(&empty).contains("Nothing built yet"));
        assert!(text.contains("▶ ✗ B.kt:2:2  second"), "{text}");
        assert!(text.contains("  ✗ A.kt:1:1  first"));
    }

    fn logcat_with(lines: &[&str]) -> AppState {
        let s = reduce(running_android(), Action::ToggleLogs);
        reduce(
            s,
            Action::LogOutput {
                platform: Platform::Android,
                id: 1,
                lines: lines.iter().map(|l| l.to_string()).collect(),
            },
        )
    }

    const NOISY: [&str; 3] = [
        "10-24 09:15:02.100  1  2 D Net: ping",
        "10-24 09:15:02.200  1  2 W Net: slow reply",
        "10-24 09:15:02.300  1  2 E App: crashed badly",
    ];

    #[test]
    fn a_text_filter_shows_only_matching_lines_and_the_bar_says_so() {
        let mut s = logcat_with(&NOISY);
        s = reduce(s, Action::FilterOpen);
        for c in "NET".chars() {
            s = reduce(s, Action::FilterInput(c));
        }

        let text = screen(&s);

        assert!(text.contains("ping") && text.contains("slow reply"));
        assert!(!text.contains("crashed badly"));
        assert!(text.contains("filter: NET▏"));
    }

    #[test]
    fn the_level_filter_hides_quieter_lines_and_is_named() {
        let mut s = logcat_with(&NOISY);
        s = reduce(s, Action::ShowOutputTab(OutputTab::Logcat));
        for _ in 0..3 {
            s = reduce(s, Action::CycleLogLevel);
        }

        let text = screen(&s);

        assert!(text.contains("≥ Warn"));
        assert!(text.contains("slow reply") && text.contains("crashed badly"));
        assert!(!text.contains("ping"));
    }

    #[test]
    fn a_logcat_level_does_not_hide_the_build_log() {
        let mut s = logcat_with(&NOISY);
        for _ in 0..3 {
            s = reduce(s, Action::CycleLogLevel);
        }
        let s = reduce(s, Action::RunApp);
        let s = reduce(
            s,
            Action::BuildOutput {
                id: 2,
                lines: vec!["compiling things".into()],
            },
        );

        let text = screen(&s);

        assert!(text.contains("compiling things"), "{text}");
        assert!(!text.contains("≥ Warn"), "the level belongs to Logcat");
    }

    #[test]
    fn a_filter_that_matches_nothing_says_how_to_undo_it() {
        let mut s = logcat_with(&NOISY);
        s = reduce(s, Action::FilterOpen);
        s = reduce(s, Action::FilterInput('z'));

        assert!(screen(&s).contains("No lines match the filter"));
    }

    #[test]
    fn scrolling_through_a_filtered_log_never_goes_past_its_start() {
        let mut s = logcat_with(&NOISY);
        s = reduce(s, Action::FilterOpen);
        s = reduce(s, Action::FilterInput('e'));
        s = reduce(s, Action::OutputScroll(1_000));

        let text = screen(&s);

        assert!(text.contains("slow reply") && text.contains("crashed badly"));
    }

    #[test]
    fn a_long_message_is_cut_to_the_panel_width() {
        let long = format!("e: file:///p/A.kt:1:1 {}", "word ".repeat(100));
        let s = reduce(
            building(&[&long]),
            Action::ShowOutputTab(OutputTab::Problems),
        );

        let text = screen_sized(&s, 80, 24);

        assert!(text.lines().all(|l| l.chars().count() <= 80));
    }
}
