//! The performance indicator in the status bar and the Perf panel.

use crate::theme;
use lf_mide_core::perf::SwapLimits;
use lf_mide_core::{
    AppState, Health, PerfSample, Platform, Pressure, ProcessKind, ProcessStat, Trend, format_cpu,
    format_size,
};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use lf_mide_core::text::visible as printable;

use crate::search::{centered, window_top};

const TITLE: &str = " Perf · j/k move · x stop · p/Esc close ";
const PANEL_WIDTH: u16 = 100;
const NAME_WIDTH: usize = 34;
/// Rows of the panel that are not process rows: system line, blank, header,
/// and the footer that carries the question when a stop is being confirmed.
const FIXED_ROWS: usize = 4;

fn limits(state: &AppState) -> SwapLimits {
    SwapLimits::from_mib(state.config.swap_warn_mb, state.config.swap_critical_mb)
}

fn health_color(health: Health) -> Color {
    match health {
        Health::Good => theme::current().ok,
        Health::Warn => theme::current().warn,
        Health::Critical => theme::current().error,
    }
}

fn trend_mark(trend: Trend) -> &'static str {
    match trend {
        Trend::Up => " ▲",
        Trend::Down => " ▼",
        Trend::Flat => "",
    }
}

/// `RAM 11.9G/16.0G · swap 2.0G ▲`, coloured by how bad things are. `None`
/// when the indicator is switched off.
pub(crate) fn indicator(state: &AppState) -> Option<(String, Color)> {
    if !state.config.show_perf {
        return None;
    }
    let Some(sample) = &state.perf else {
        return Some(("RAM …".to_string(), theme::current().muted));
    };
    let system = &sample.system;
    let text = format!(
        "RAM {}/{} · swap {}{}",
        format_size(system.ram_used),
        format_size(system.ram_total),
        format_size(system.swap_used),
        trend_mark(state.swap_trend)
    );
    Some((text, health_color(system.health(limits(state)))))
}

pub(crate) fn render_perf(frame: &mut Frame, state: &AppState) {
    if !state.perf_open {
        return;
    }
    let area = frame.area();
    let rows = state.perf.as_ref().map_or(1, |p| p.processes.len().max(1));
    let wanted = u16::try_from(rows + FIXED_ROWS + 2).unwrap_or(u16::MAX);
    let width = u16::try_from(u32::from(area.width) * 9 / 10)
        .unwrap_or(u16::MAX)
        .min(PANEL_WIDTH);
    let popup = centered(area, width, wanted);
    let block = Block::bordered()
        .title(TITLE)
        .border_style(Style::new().fg(theme::current().accent));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(panel_lines(state, inner)), inner);
}

fn panel_lines(state: &AppState, inner: Rect) -> Vec<Line<'static>> {
    let Some(sample) = &state.perf else {
        return vec![Line::from(Span::styled(
            "Waiting for the first sample…",
            Style::new().fg(theme::current().muted),
        ))];
    };
    let mut lines = vec![system_line(state, sample), Line::raw("")];
    lines.push(Line::from(Span::styled(
        format!(
            "  {:<NAME_WIDTH$} {:>7} {:>8} {:>7}",
            "Process", "PID", "Memory", "CPU"
        ),
        Style::new().fg(theme::current().muted),
    )));
    let visible = usize::from(inner.height).saturating_sub(FIXED_ROWS).max(1);
    let footer = footer_line(state, sample);
    let top = window_top(state.perf_selected, sample.processes.len(), visible);
    lines.extend(
        sample
            .processes
            .iter()
            .enumerate()
            .skip(top)
            .take(visible)
            .map(|(index, process)| process_line(state, process, index == state.perf_selected)),
    );
    if sample.processes.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Nothing to watch is running",
            Style::new().fg(theme::current().muted),
        )));
    }
    lines.push(footer);
    lines
}

/// Where the confirmation question is asked: its own line, so no column of
/// the table can push it out of sight.
fn footer_line(state: &AppState, sample: &PerfSample) -> Line<'static> {
    let asked = state.perf_confirm.as_ref().and_then(|asked| {
        sample
            .processes
            .iter()
            .find(|process| asked.is_for(process))
    });
    match asked {
        Some(process) => Line::from(Span::styled(
            format!(
                "Press x again to stop {}. Any other key cancels.",
                lf_mide_core::perf_reducer::describe(process)
            ),
            Style::new()
                .fg(theme::current().warn)
                .add_modifier(Modifier::BOLD),
        )),
        None => Line::from(Span::styled(
            "x stops the highlighted helper (asks twice)",
            Style::new().fg(theme::current().muted),
        )),
    }
}

fn system_line(state: &AppState, sample: &PerfSample) -> Line<'static> {
    let system = &sample.system;
    let health = system.health(limits(state));
    let pressure_color = match system.pressure {
        Pressure::Normal => theme::current().ok,
        Pressure::Warn => theme::current().warn,
        Pressure::Critical => theme::current().error,
    };
    Line::from(vec![
        Span::raw(format!(
            "RAM {} / {}   ",
            format_size(system.ram_used),
            format_size(system.ram_total)
        )),
        Span::raw("Pressure: "),
        Span::styled(system.pressure.label(), Style::new().fg(pressure_color)),
        Span::raw("   Swap "),
        Span::styled(
            format!(
                "{}{}",
                format_size(system.swap_used),
                trend_mark(state.swap_trend)
            ),
            Style::new().fg(health_color(health)),
        ),
        Span::raw(format!("   Disk {} free", format_size(system.disk_free))),
    ])
}

/// `Android emulator Pixel_8`, `iOS simulator iPhone 17 Pro`, `Gradle daemon`.
fn name_of(state: &AppState, process: &ProcessStat) -> String {
    let detail = match process.kind {
        ProcessKind::Simulator => state
            .devices
            .iter()
            .find(|d| d.platform == Platform::Ios && d.id.eq_ignore_ascii_case(&process.detail))
            .map_or_else(|| short_id(&process.detail), |d| d.name.clone()),
        _ => process.detail.clone(),
    };
    let detail = printable(&detail);
    if detail.is_empty() {
        process.kind.label().to_string()
    } else {
        format!("{} {detail}", process.kind.label())
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn process_line(state: &AppState, process: &ProcessStat, selected: bool) -> Line<'static> {
    let pointer = if selected { "▶ " } else { "  " };
    let name: String = name_of(state, process).chars().take(NAME_WIDTH).collect();
    let note = if selected && process.kind.stoppable() {
        "  [x]"
    } else {
        ""
    };
    let mut style = Style::default();
    if selected {
        style = style.add_modifier(Modifier::BOLD);
    }
    let confirming = state
        .perf_confirm
        .as_ref()
        .is_some_and(|asked| asked.is_for(process));
    Line::from(vec![
        Span::styled(pointer, Style::new().fg(theme::current().accent)),
        Span::styled(
            format!(
                "{name:<NAME_WIDTH$} {:>7} {:>8} {:>7}",
                process.pid,
                format_size(process.rss),
                format_cpu(process.cpu_tenths)
            ),
            style,
        ),
        Span::styled(
            note,
            Style::new().fg(if confirming {
                theme::current().warn
            } else {
                theme::current().muted
            }),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use lf_mide_core::{Action, Device, DeviceState, SystemStats, reduce};

    use super::*;
    use crate::testing::{buffer, find_on_screen, screen_sized, state};

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    fn process(kind: ProcessKind, pid: u32, rss: u64, detail: &str) -> ProcessStat {
        ProcessStat {
            kind,
            pid,
            rss,
            cpu_tenths: 123,
            detail: detail.into(),
        }
    }

    fn sampled(swap: u64, pressure: Pressure, processes: Vec<ProcessStat>) -> AppState {
        reduce(
            state(),
            Action::PerfSampled(PerfSample {
                system: SystemStats {
                    ram_used: 12 * GIB,
                    ram_total: 16 * GIB,
                    swap_used: swap,
                    pressure,
                    disk_free: 48 * GIB,
                },
                processes,
            }),
        )
    }

    fn screen(s: &AppState) -> String {
        screen_sized(s, 120, 30)
    }

    #[test]
    fn the_indicator_shows_ram_and_swap_and_can_be_switched_off() {
        let s = sampled(2 * GIB, Pressure::Normal, vec![]);

        let on = indicator(&s).unwrap().0;
        let off = indicator(&reduce(s, Action::TogglePerfIndicator));

        assert_eq!(on, "RAM 12.0G/16.0G · swap 2.0G");
        assert_eq!(off, None);
    }

    #[test]
    fn before_the_first_sample_the_indicator_says_so() {
        assert_eq!(
            indicator(&state()),
            Some(("RAM …".to_string(), theme::current().muted))
        );
    }

    #[test]
    fn the_indicator_colour_follows_pressure_and_swap() {
        let colour = |swap, pressure| indicator(&sampled(swap, pressure, vec![])).unwrap().1;

        assert_eq!(colour(GIB, Pressure::Normal), theme::current().ok);
        assert_eq!(colour(GIB, Pressure::Warn), theme::current().warn);
        assert_eq!(colour(2 * GIB, Pressure::Normal), theme::current().warn);
        assert_eq!(colour(GIB, Pressure::Critical), theme::current().error);
        assert_eq!(colour(5 * GIB, Pressure::Normal), theme::current().error);
    }

    #[test]
    fn a_growing_swap_gets_an_arrow() {
        let s = sampled(GIB, Pressure::Normal, vec![]);
        let grown = reduce(
            s.clone(),
            Action::PerfSampled(PerfSample {
                system: SystemStats {
                    swap_used: 2 * GIB,
                    ..s.perf.as_ref().unwrap().system
                },
                processes: vec![],
            }),
        );

        assert!(indicator(&grown).unwrap().0.ends_with("swap 2.0G ▲"));
    }

    #[test]
    fn the_status_bar_carries_the_indicator() {
        let s = sampled(GIB, Pressure::Normal, vec![]);

        assert!(screen(&s).contains("RAM 12.0G/16.0G · swap 1.0G"));
        assert!(!screen(&reduce(s, Action::TogglePerfIndicator)).contains("RAM 12.0G"));
    }

    #[test]
    fn the_panel_lists_processes_with_memory_and_cpu() {
        let s = sampled(
            GIB,
            Pressure::Warn,
            vec![
                process(ProcessKind::GradleDaemon, 4121, 1800 * MIB, ""),
                process(ProcessKind::Emulator, 5002, 2 * GIB, "Pixel_8"),
            ],
        );
        let s = reduce(s, Action::TogglePerfPanel);

        let text = screen(&s);

        assert!(text.contains("Pressure: warn"), "{text}");
        assert!(text.contains("Disk 48.0G free"));
        assert!(text.contains("Gradle daemon") && text.contains("4121") && text.contains("1.8G"));
        assert!(text.contains("Android emulator Pixel_8") && text.contains("12.3%"));
        assert!(text.contains("[x]"));
    }

    #[test]
    fn a_simulator_is_shown_by_its_device_name() {
        let udid = "23F0A11F-0DD8-4766-853D-987DE9852BAC";
        let device = Device {
            platform: Platform::Ios,
            id: udid.into(),
            name: "iPhone 17 Pro".into(),
            detail: String::new(),
            state: DeviceState::Running,
            serial: None,
        };
        let s = reduce(
            sampled(
                0,
                Pressure::Normal,
                vec![process(ProcessKind::Simulator, 7, GIB, udid)],
            ),
            Action::DevicesLoaded {
                devices: Arc::new(vec![device]),
                errors: BTreeMap::new(),
            },
        );
        let s = reduce(s, Action::TogglePerfPanel);

        assert!(screen(&s).contains("iOS simulator iPhone 17 Pro"));
    }

    #[test]
    fn an_unknown_simulator_shows_a_short_id() {
        let s = sampled(
            0,
            Pressure::Normal,
            vec![process(
                ProcessKind::Simulator,
                7,
                GIB,
                "23F0A11F-0DD8-4766-853D-987DE9852BAC",
            )],
        );

        assert!(screen(&reduce(s, Action::TogglePerfPanel)).contains("iOS simulator 23F0A11F "));
    }

    #[test]
    fn only_the_highlighted_stoppable_row_offers_to_be_stopped_and_asks_for_confirmation() {
        let s = sampled(
            0,
            Pressure::Normal,
            vec![
                process(ProcessKind::Neovim, 1, 3 * GIB, ""),
                process(ProcessKind::GradleDaemon, 2, GIB, ""),
            ],
        );
        let s = reduce(s, Action::TogglePerfPanel);

        let on_nvim = screen(&s);
        let on_gradle = screen(&reduce(s.clone(), Action::PerfMove(1)));
        let asked = screen(&reduce(reduce(s, Action::PerfMove(1)), Action::PerfStop));

        assert!(!on_nvim.contains("[x]"), "an editor cannot be stopped");
        assert!(on_gradle.contains("[x]"));
        assert!(
            asked.contains("Press x again to stop Gradle daemon (pid 2)"),
            "{asked}"
        );
        assert!(asked.contains("Any other key cancels"));
    }

    #[test]
    fn the_highlight_is_marked_and_the_panel_waits_for_the_first_sample() {
        let waiting = reduce(state(), Action::TogglePerfPanel);
        let s = reduce(
            sampled(
                0,
                Pressure::Normal,
                vec![process(ProcessKind::Neovim, 1, GIB, "")],
            ),
            Action::TogglePerfPanel,
        );

        assert!(screen(&waiting).contains("Waiting for the first sample"));
        assert!(screen(&s).contains("▶ nvim"));
    }

    #[test]
    fn a_long_list_scrolls_with_the_highlight_inside_a_small_terminal() {
        let many: Vec<ProcessStat> = (0..40)
            .map(|n| process(ProcessKind::Neovim, 100 + n, (100 - u64::from(n)) * MIB, ""))
            .collect();
        let mut s = reduce(sampled(0, Pressure::Normal, many), Action::TogglePerfPanel);
        s = reduce(s, Action::PerfMove(35));

        let text = screen_sized(&s, 100, 16);

        assert!(text.contains("▶ nvim"), "{text}");
        assert!(text.contains(" 135 "), "the highlighted row is on screen");
        assert!(buffer(&s, 100, 16).area.height == 16);
        assert!(find_on_screen(&text, "Perf").is_some());
    }
}
