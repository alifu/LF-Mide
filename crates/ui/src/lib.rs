//! Rendering for LF-Mide. Every function here is a pure view of
//! [`AppState`]; nothing in this crate changes state.

mod devices;
mod search;
#[cfg(test)]
mod testing;
mod tree;

use lf_mide_core::layout::{BOTTOM_ROWS, HEADER_ROWS, MIN_BODY_ROWS, STATUS_ROWS};
use lf_mide_core::{
    AppState, BranchInfo, DoctorPanel, LineKind, Panel, ReportLine, Status, TerminalKind,
    popup_height,
};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Tabs};

const FILES_WIDTH_PCT: u16 = 20;
const DEVICES_WIDTH_PCT: u16 = 22;
const BOTTOM_TABS: [&str; 5] = ["Build", "Logcat", "iOS Log", "Problems", "Git"];
const HELP_HINT: &str =
    "Tab: focus · e: editor · f: find · /: search · T: terminal · D: doctor · q: quit";
const PICKER_TITLE: &str = " Terminal · Enter select · Esc cancel ";
const PICKER_WIDTH: u16 = 56;
const DOCTOR_TITLE: &str = " Doctor · Esc close · r re-run · j/k scroll ";
const DOCTOR_MAX_WIDTH: u16 = 110;

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const BAR_BG: Color = Color::Rgb(40, 44, 52);

pub fn render(frame: &mut Frame, state: &AppState) {
    let [header, body, bottom, status] = Layout::vertical([
        Constraint::Length(HEADER_ROWS),
        Constraint::Min(MIN_BODY_ROWS),
        Constraint::Length(BOTTOM_ROWS),
        Constraint::Length(STATUS_ROWS),
    ])
    .areas(frame.area());
    let [files, editor, devices] = Layout::horizontal([
        Constraint::Percentage(FILES_WIDTH_PCT),
        Constraint::Min(10),
        Constraint::Percentage(DEVICES_WIDTH_PCT),
    ])
    .areas(body);

    render_header(frame, header, state);
    tree::render_tree(frame, files, state);
    render_placeholder(
        frame,
        editor,
        state,
        Panel::Editor,
        "Neovim runs in a split pane (mode D)",
    );
    devices::render_devices(frame, devices, state);
    render_bottom(frame, bottom, state);
    render_status(frame, status, state);
    render_doctor(frame, state);
    render_terminal_picker(frame, state);
    search::render_finder(frame, state);
    search::render_grep(frame, state);
    devices::render_info(frame, state);
}

/// Who will host Neovim: the configured terminal, or what `Auto` found.
pub fn terminal_summary(state: &AppState) -> String {
    match state.config.terminal {
        TerminalKind::Auto => {
            let found = state
                .detected_terminal
                .map_or("not detected", TerminalKind::label);
            format!("Auto → {found}")
        }
        chosen => chosen.label().to_string(),
    }
}

/// Small centered list for choosing the terminal.
fn render_terminal_picker(frame: &mut Frame, state: &AppState) {
    let Some(highlight) = state.terminal_picker else {
        return;
    };
    let screen = frame.area();
    let rows = TerminalKind::ALL.len() as u16 + 2;
    let width = PICKER_WIDTH.min(screen.width);
    let height = rows.min(screen.height);
    let popup = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    );

    let block = Block::bordered()
        .title(PICKER_TITLE)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let lines: Vec<Line> = TerminalKind::ALL
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let pointer = if index == highlight { "▶" } else { " " };
            let chosen = if *kind == state.config.terminal {
                "●"
            } else {
                " "
            };
            let note = match kind {
                TerminalKind::Auto => {
                    let found = state
                        .detected_terminal
                        .map_or("not detected", TerminalKind::label);
                    format!(" → {found}")
                }
                other => other
                    .launch_note()
                    .map_or(String::new(), |n| format!(" ({n})")),
            };
            let style = if index == highlight {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            Line::styled(format!(" {pointer} {chosen} {}{note}", kind.label()), style)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Centered overlay with the doctor report (or a progress note).
fn render_doctor(frame: &mut Frame, state: &AppState) {
    if state.doctor == DoctorPanel::Closed {
        return;
    }
    let screen = frame.area();
    // Widen to u32 first: `width * 9` overflows u16 on very wide terminals.
    let ninety_percent = u16::try_from(u32::from(screen.width) * 9 / 10).unwrap_or(u16::MAX);
    let width = ninety_percent.min(DOCTOR_MAX_WIDTH).min(screen.width);
    let height = popup_height(screen.height);
    let popup = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (screen.height - height) / 2,
        width,
        height,
    );

    let block = Block::bordered()
        .title(DOCTOR_TITLE)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let body = match &state.doctor {
        DoctorPanel::Closed => return,
        DoctorPanel::Running => vec![Line::styled("Checking your setup…", Style::new().fg(MUTED))],
        DoctorPanel::Ready(report) => {
            let lines = report.lines();
            let visible = usize::from(inner.height);
            // The reducer already clamps; this guards against a stale height.
            let first = usize::from(state.doctor_scroll).min(lines.len().saturating_sub(visible));
            lines
                .iter()
                .skip(first)
                .take(visible)
                .map(report_line)
                .collect()
        }
    };
    frame.render_widget(Paragraph::new(body), inner);
}

fn report_line(line: &ReportLine) -> Line<'static> {
    let text = line.text.clone();
    match line.kind {
        LineKind::Blank => Line::raw(""),
        LineKind::Heading | LineKind::Summary => Line::styled(
            format!(" {text}"),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        LineKind::Row(status) => Line::from(vec![
            Span::raw(" "),
            Span::styled(status.symbol(), Style::new().fg(status_color(status))),
            Span::raw(format!(" {text}")),
        ]),
        LineKind::Path => Line::styled(format!("     {text}"), Style::new().fg(MUTED)),
        LineKind::Hint => Line::styled(format!("     → {text}"), Style::new().fg(Color::Yellow)),
    }
}

fn status_color(status: Status) -> Color {
    match status {
        Status::Ok => Color::Green,
        Status::Warn => Color::Yellow,
        Status::Missing => Color::Red,
    }
}

/// The branch segment of the status bar; a placeholder until the first git
/// check has reported, so "no git repo" is never shown merely for not knowing.
fn branch_text(state: &AppState) -> String {
    if state.branch_known {
        branch_label(state.branch.as_ref())
    } else {
        "⎇ …".to_string()
    }
}

/// Text for the branch segment, e.g. `⎇ main*` or `⎇ a1b2c3d (detached)`.
pub fn branch_label(branch: Option<&BranchInfo>) -> String {
    match branch {
        None => "no git repo".to_string(),
        Some(b) if b.detached => format!("⎇ {} (detached)", b.name),
        Some(b) => format!("⎇ {}{}", b.name, if b.dirty { "*" } else { "" }),
    }
}

fn panel_block(state: &AppState, panel: Panel, title: &str) -> Block<'static> {
    let focused = state.focus == panel;
    let (title, style) = if focused {
        (format!(" ▶ {title} "), Style::new().fg(ACCENT))
    } else {
        (format!(" {title} "), Style::new().fg(MUTED))
    };
    Block::bordered().title(title).border_style(style)
}

fn render_header(frame: &mut Frame, area: Rect, state: &AppState) {
    let line = Line::from(vec![
        Span::styled(
            " LF-Mide ",
            Style::new()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            state.project.name.clone(),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" ({})", state.project.layout.label()),
            Style::new().fg(MUTED),
        ),
        Span::styled(
            format!("  terminal: {}", terminal_summary(state)),
            Style::new().fg(MUTED),
        ),
        Span::styled(
            format!("  target: {}", devices::target_summary(state)),
            Style::new().fg(MUTED),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(BAR_BG)), area);
}

fn render_placeholder(frame: &mut Frame, area: Rect, state: &AppState, panel: Panel, hint: &str) {
    let title = match panel {
        Panel::Files => "Files",
        Panel::Editor => "Editor",
        Panel::Devices => "Devices",
        Panel::Bottom => "Output",
    };
    let body = Paragraph::new(Span::styled(hint.to_string(), Style::new().fg(MUTED)))
        .block(panel_block(state, panel, title));
    frame.render_widget(body, area);
}

fn render_bottom(frame: &mut Frame, area: Rect, state: &AppState) {
    let tabs = Tabs::new(BOTTOM_TABS)
        .select(0)
        .highlight_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
        .block(panel_block(state, Panel::Bottom, "Output"));
    frame.render_widget(tabs, area);
}

fn render_status(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut spans = vec![Span::styled(
        format!(" {} ", branch_text(state)),
        Style::new().fg(ACCENT),
    )];
    if let Some(message) = &state.status {
        spans.push(Span::styled(
            format!("│ {message} "),
            Style::new().fg(Color::Yellow),
        ));
    }
    spans.push(Span::styled(
        format!("│ {HELP_HINT}"),
        Style::new().fg(MUTED),
    ));
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(BAR_BG)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_core::{Action, Config, Project, ProjectLayout, reduce};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn state() -> AppState {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "lf_pos_mobile".into(),
            layout: ProjectLayout::AndroidAppShared,
        };
        AppState::new(project, Config::default())
    }

    fn draw(state: &AppState) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, state)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn branch_label_variants() {
        let clean = BranchInfo {
            name: "main".into(),
            detached: false,
            dirty: false,
        };
        let dirty = BranchInfo {
            dirty: true,
            ..clean.clone()
        };
        let detached = BranchInfo {
            name: "a1b2c3d".into(),
            detached: true,
            dirty: false,
        };

        assert_eq!(branch_label(Some(&clean)), "⎇ main");
        assert_eq!(branch_label(Some(&dirty)), "⎇ main*");
        assert_eq!(branch_label(Some(&detached)), "⎇ a1b2c3d (detached)");
        assert_eq!(branch_label(None), "no git repo");
    }

    #[test]
    fn renders_project_panels_tabs_and_hint() {
        let screen = draw(&state());

        for expected in [
            "lf_pos_mobile",
            "androidApp + shared",
            "Files",
            "Editor",
            "Devices",
            "Logcat",
            "Problems",
            HELP_HINT,
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in:\n{screen}"
            );
        }
    }

    #[test]
    fn the_branch_shows_a_placeholder_until_it_is_known() {
        let unknown = draw(&state());
        let not_a_repo = draw(&reduce(state(), Action::BranchChanged(None)));

        assert!(
            unknown.contains("⎇ …") && !unknown.contains("no git repo"),
            "{unknown}"
        );
        assert!(not_a_repo.contains("no git repo"), "{not_a_repo}");
    }

    #[test]
    fn renders_branch_and_status_message() {
        let branch = BranchInfo {
            name: "feature/login".into(),
            detached: false,
            dirty: true,
        };
        let state = reduce(
            reduce(state(), Action::BranchChanged(Some(branch))),
            Action::Status("git failed".into()),
        );

        let screen = draw(&state);

        assert!(screen.contains("⎇ feature/login*"));
        assert!(screen.contains("git failed"));
    }

    fn report(count: usize) -> lf_mide_core::Report {
        use lf_mide_core::{Category, CheckResult};
        let results = (0..count)
            .map(|i| CheckResult::new(Category::Editor, &format!("tool{i}"), Status::Ok, "1.0"))
            .collect();
        lf_mide_core::Report::new(results)
    }

    fn with_report(report: lf_mide_core::Report, scroll: u16) -> AppState {
        AppState {
            doctor: DoctorPanel::Ready(report),
            doctor_scroll: scroll,
            ..state()
        }
    }

    #[test]
    fn marks_focused_panel_title() {
        let editor_focused = reduce(state(), Action::FocusNext);
        assert_eq!(editor_focused.focus, Panel::Editor);

        let default_screen = draw(&state());
        let editor_screen = draw(&editor_focused);

        assert!(
            default_screen.contains("▶ Files"),
            "the tree starts focused:\n{default_screen}"
        );
        assert!(!default_screen.contains("▶ Editor"));
        assert!(editor_screen.contains("▶ Editor"));
        assert!(!editor_screen.contains("▶ Files"));
    }

    #[test]
    fn control_characters_from_untrusted_text_never_reach_the_screen() {
        // Project names, status messages and tool errors come from repos and
        // programs we don't control; an ESC would let them drive the terminal.
        let mut hostile = state();
        hostile.project.name = "evil\x1b[2J\x1b]0;pwned\x07name".into();
        hostile.status = Some("err\x1b[31m\rred\x1b[0m\nnext".into());

        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|frame| render(frame, &hostile)).unwrap();

        let buffer = terminal.backend().buffer();
        let has_control = buffer
            .content()
            .iter()
            .any(|cell| cell.symbol().chars().any(char::is_control));
        assert!(!has_control, "a control character reached the buffer");
    }

    #[test]
    fn terminal_summary_resolves_auto_with_what_was_detected() {
        let auto_found = AppState {
            detected_terminal: Some(TerminalKind::Iterm2),
            ..state()
        };
        let auto_unknown = state();
        let mut explicit = state();
        explicit.config.terminal = TerminalKind::Tmux;

        assert_eq!(terminal_summary(&auto_found), "Auto → iTerm2");
        assert_eq!(terminal_summary(&auto_unknown), "Auto → not detected");
        assert_eq!(terminal_summary(&explicit), "tmux");
    }

    #[test]
    fn header_shows_the_terminal() {
        let screen = draw(&AppState {
            detected_terminal: Some(TerminalKind::Iterm2),
            ..state()
        });

        assert!(screen.contains("Auto → iTerm2"), "{screen}");
    }

    #[test]
    fn terminal_picker_is_hidden_while_closed() {
        assert!(!draw(&state()).contains("Enter select"));
    }

    #[test]
    fn terminal_picker_lists_every_choice_with_notes() {
        let open = AppState {
            terminal_picker: Some(2),
            detected_terminal: Some(TerminalKind::Iterm2),
            ..state()
        };

        let screen = draw(&open);

        for kind in TerminalKind::ALL {
            assert!(
                screen.contains(kind.label()),
                "missing {} in:\n{screen}",
                kind.label()
            );
        }
        assert!(screen.contains("Enter select"));
        assert!(
            screen.contains("→ iTerm2"),
            "auto shows what it detected:\n{screen}"
        );
        assert!(screen.contains("you split manually"));
        assert!(screen.contains("opens a new window"));
    }

    #[test]
    fn terminal_picker_marks_the_highlight_and_the_current_choice() {
        let mut open = AppState {
            terminal_picker: Some(2),
            ..state()
        };
        open.config.terminal = TerminalKind::Tmux;

        let screen = draw(&open);

        let has = |marker: &str, label: &str| {
            screen
                .lines()
                .any(|l| l.contains(marker) && l.contains(label))
        };
        assert!(has("▶", "Ghostty"), "highlight on row 2:\n{screen}");
        assert!(has("●", "tmux"), "configured choice is marked:\n{screen}");
        assert!(!has("●", "kitty") && !has("▶", "kitty"));
    }

    #[test]
    fn terminal_picker_does_not_panic_on_tiny_terminals() {
        for (w, h) in [(10, 3), (30, 6), (200, 5), (8000, 10)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            let open = AppState {
                terminal_picker: Some(0),
                ..state()
            };
            terminal.draw(|frame| render(frame, &open)).unwrap();
        }
    }

    #[test]
    fn doctor_overlay_is_hidden_while_closed() {
        let screen = draw(&state());

        assert!(!screen.contains("Doctor ·"), "{screen}");
    }

    #[test]
    fn doctor_shows_progress_while_running() {
        let screen = draw(&AppState {
            doctor: DoctorPanel::Running,
            ..state()
        });

        assert!(screen.contains("Doctor ·"));
        assert!(screen.contains("Checking"), "{screen}");
    }

    #[test]
    fn doctor_shows_rows_symbols_and_hints() {
        use lf_mide_core::{Category, CheckResult};
        let report = lf_mide_core::Report::new(vec![
            CheckResult::new(Category::Android, "Android SDK", Status::Ok, "ANDROID_HOME"),
            CheckResult::new(Category::Android, "adb", Status::Missing, "not found")
                .with_hint("sdkmanager \"platform-tools\""),
        ]);

        let screen = draw(&with_report(report, 0));

        for expected in [
            "Android",
            "✓ Android SDK",
            "✗ adb",
            "→ sdkmanager \"platform-tools\"",
            "1 ok · 0 warnings · 1 missing",
        ] {
            assert!(
                screen.contains(expected),
                "missing {expected:?} in:\n{screen}"
            );
        }
    }

    #[test]
    fn scrolling_hides_the_first_lines() {
        let top = draw(&with_report(report(30), 0));
        let scrolled = draw(&with_report(report(30), 10));

        assert!(top.contains("tool0 "), "{top}");
        assert!(!scrolled.contains("tool0 "), "{scrolled}");
        assert!(scrolled.contains("tool9 "), "{scrolled}");
    }

    #[test]
    fn an_overscrolled_state_still_shows_the_end_of_the_report() {
        let screen = draw(&with_report(report(30), 9999));

        assert!(screen.contains("30 ok"), "{screen}");
    }

    #[test]
    fn doctor_overlay_handles_extremely_wide_terminals() {
        let mut terminal = Terminal::new(TestBackend::new(8000, 10)).unwrap();
        let open = with_report(report(3), 0);

        terminal.draw(|frame| render(frame, &open)).unwrap();
    }

    #[test]
    fn doctor_overlay_does_not_panic_on_tiny_terminals() {
        for (w, h) in [(10, 3), (30, 6), (200, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
            let open = with_report(report(30), 4);
            terminal.draw(|frame| render(frame, &open)).unwrap();
            let running = AppState {
                doctor: DoctorPanel::Running,
                ..state()
            };
            terminal.draw(|frame| render(frame, &running)).unwrap();
        }
    }

    #[test]
    fn does_not_panic_on_tiny_terminal() {
        let mut terminal = Terminal::new(TestBackend::new(10, 3)).unwrap();

        terminal.draw(|frame| render(frame, &state())).unwrap();
    }
}
