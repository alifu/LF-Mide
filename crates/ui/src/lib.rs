//! Rendering for LF-Mide. Every function here is a pure view of
//! [`AppState`]; nothing in this crate changes state.

use lf_mide_core::{AppState, BranchInfo, Panel, ProjectLayout};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Tabs};

const FILES_WIDTH_PCT: u16 = 20;
const DEVICES_WIDTH_PCT: u16 = 22;
const BOTTOM_HEIGHT: u16 = 8;
const BOTTOM_TABS: [&str; 5] = ["Build", "Logcat", "iOS Log", "Problems", "Git"];
const HELP_HINT: &str = "Tab/S-Tab: focus · q: quit";

const ACCENT: Color = Color::Cyan;
const MUTED: Color = Color::DarkGray;
const BAR_BG: Color = Color::Rgb(40, 44, 52);

pub fn render(frame: &mut Frame, state: &AppState) {
    let [header, body, bottom, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(BOTTOM_HEIGHT),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [files, editor, devices] = Layout::horizontal([
        Constraint::Percentage(FILES_WIDTH_PCT),
        Constraint::Min(10),
        Constraint::Percentage(DEVICES_WIDTH_PCT),
    ])
    .areas(body);

    render_header(frame, header, state);
    render_placeholder(frame, files, state, Panel::Files, "File tree (Phase 2)");
    render_placeholder(
        frame,
        editor,
        state,
        Panel::Editor,
        "Neovim runs in a split pane (mode D)",
    );
    render_placeholder(
        frame,
        devices,
        state,
        Panel::Devices,
        "Emulators & simulators (Phase 3)",
    );
    render_bottom(frame, bottom, state);
    render_status(frame, status, state);
}

/// Text for the branch segment, e.g. `⎇ main*` or `⎇ a1b2c3d (detached)`.
pub fn branch_label(branch: Option<&BranchInfo>) -> String {
    match branch {
        None => "no git repo".to_string(),
        Some(b) if b.detached => format!("⎇ {} (detached)", b.name),
        Some(b) => format!("⎇ {}{}", b.name, if b.dirty { "*" } else { "" }),
    }
}

fn layout_label(layout: ProjectLayout) -> &'static str {
    match layout {
        ProjectLayout::ComposeApp => "composeApp",
        ProjectLayout::AndroidAppShared => "androidApp + shared",
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
            format!(" ({})", layout_label(state.project.layout)),
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
        format!(" {} ", branch_label(state.branch.as_ref())),
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
    use lf_mide_core::{Action, Config, Project, reduce};
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

    #[test]
    fn marks_focused_panel_title() {
        let files_focused = reduce(state(), Action::FocusPrev);
        assert_eq!(files_focused.focus, Panel::Files);

        let default_screen = draw(&state());
        let files_screen = draw(&files_focused);

        assert!(
            default_screen.contains("▶ Editor"),
            "default focus is the editor:\n{default_screen}"
        );
        assert!(!default_screen.contains("▶ Files"));
        assert!(files_screen.contains("▶ Files"));
        assert!(!files_screen.contains("▶ Editor"));
    }

    #[test]
    fn does_not_panic_on_tiny_terminal() {
        let mut terminal = Terminal::new(TestBackend::new(10, 3)).unwrap();

        terminal.draw(|frame| render(frame, &state())).unwrap();
    }
}
