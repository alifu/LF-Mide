//! The Devices panel (target, SDK facts, emulators/simulators) and the
//! environment overlay.

use lf_mide_core::layout::DEVICE_PANEL_HEADER_ROWS;
use lf_mide_core::text::visible as printable;
use std::path::Path;

use lf_mide_core::{AppState, Device, DeviceState, Panel, Platform, ToolInfo, display_path};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::search::centered;
use crate::{ACCENT, MUTED, panel_block};

const DETAIL_GAP: &str = "  ";
const INFO_TITLE: &str = " Environment · Esc close ";
const INFO_WIDTH: u16 = 84;

/// `Android · Medium_Phone_API_36` for the header; `Android · no device`.
pub(crate) fn target_summary(state: &AppState) -> String {
    let device = state
        .selected_device()
        .map_or(std::borrow::Cow::Borrowed("no device"), |d| {
            printable(&d.name)
        });
    format!("{} · {device}", state.target.label())
}

fn version_of(tool: &Option<ToolInfo>) -> String {
    match tool {
        Some(tool) => tool.version.clone().unwrap_or_else(|| "?".to_string()),
        None => "missing".to_string(),
    }
}

/// `/Applications/Xcode.app` from `/Applications/Xcode.app/Contents/Developer`.
fn app_path(developer_dir: &Path) -> &Path {
    developer_dir
        .ancestors()
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        .unwrap_or(developer_dir)
}

/// The two SDK lines at the top of the panel for the current target.
fn sdk_lines(state: &AppState) -> [Line<'static>; 2] {
    let muted = Style::new().fg(MUTED);
    let problem = Style::new().fg(Color::Red);
    if !state.sdk_loaded {
        return [Line::styled("Checking tools…", muted), Line::raw("")];
    }
    let home = state.sdk.home.as_deref();
    match state.target {
        Platform::Android => {
            let android = &state.sdk.android;
            let first = match &android.root {
                Some(root) => Line::raw(format!("SDK {}", display_path(root, home))),
                None => Line::styled("Android SDK not found", problem),
            };
            let second = if android.adb.is_some() || android.emulator.is_some() {
                Line::styled(
                    format!(
                        "adb {} · emulator {}",
                        version_of(&android.adb),
                        version_of(&android.emulator)
                    ),
                    muted,
                )
            } else {
                Line::raw("")
            };
            [first, second]
        }
        Platform::Ios => {
            let ios = &state.sdk.ios;
            match &ios.developer_dir {
                Some(dir) => {
                    let version = ios.version.as_deref().unwrap_or("Xcode");
                    let simctl = if ios.simctl { "ok" } else { "missing" };
                    [
                        Line::raw(format!("{version} · simctl {simctl}")),
                        Line::styled(display_path(app_path(dir), home), muted),
                    ]
                }
                None => [Line::styled("Xcode not found", problem), Line::raw("")],
            }
        }
    }
}

fn state_marker(device_state: DeviceState) -> Span<'static> {
    match device_state {
        DeviceState::Running => Span::styled("●", Style::new().fg(Color::Green)),
        DeviceState::Starting => Span::styled("◐", Style::new().fg(Color::Yellow)),
        DeviceState::Stopping => Span::styled("◑", Style::new().fg(Color::Yellow)),
        DeviceState::Stopped => Span::styled("○", Style::new().fg(MUTED)),
    }
}

/// The part of `detail` that fits in `room` columns: all of it, else the
/// first `a · b` segment, else nothing. Never a cut-off fragment.
fn fit_detail(detail: &str, room: usize) -> Option<&str> {
    let fits = |text: &str| !text.is_empty() && text.chars().count() <= room;
    if fits(detail) {
        return Some(detail);
    }
    detail.split(" · ").next().filter(|first| fits(first))
}

fn device_row(device: &Device, selected: bool, width: usize) -> Line<'static> {
    let name_style = if selected {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    let mut spans = vec![
        Span::styled(if selected { "▶" } else { " " }, Style::new().fg(ACCENT)),
        state_marker(device.state),
        Span::styled(format!(" {}", printable(&device.name)), name_style),
    ];
    // Pointer, state marker, a space and the name come first; the detail
    // gets whatever width is left.
    let used = 2 + 1 + device.name.chars().count() + DETAIL_GAP.len();
    if let Some(detail) = fit_detail(&device.detail, width.saturating_sub(used)) {
        spans.push(Span::styled(
            format!("{DETAIL_GAP}{detail}"),
            Style::new().fg(MUTED),
        ));
    }
    Line::from(spans)
}

pub(crate) fn render_devices(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = panel_block(state, Panel::Devices, "Devices");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let muted = Style::new().fg(MUTED);
    let chip = |platform: Platform| {
        let label = format!(" {} ", platform.label());
        if platform == state.target {
            Span::styled(
                label,
                Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            )
        } else {
            Span::styled(label, muted)
        }
    };
    let chips = Line::from(vec![
        chip(Platform::Android),
        chip(Platform::Ios),
        Span::styled(" t: switch", muted),
    ]);
    let [first, second] = sdk_lines(state);
    let problem = match state.device_errors.get(&state.target) {
        Some(message) => Line::styled(message.clone(), Style::new().fg(Color::Red)),
        None => Line::raw(""),
    };
    debug_assert_eq!(DEVICE_PANEL_HEADER_ROWS, 4);
    let mut lines = vec![chips, first, second, problem];

    let devices = state.target_devices();
    if devices.is_empty() {
        let note = match (
            state.devices_loaded,
            state.device_errors.contains_key(&state.target),
            state.target,
        ) {
            (false, _, _) => "Listing devices…",
            (true, true, _) => "No devices",
            (true, false, Platform::Android) => "No emulators found",
            (true, false, Platform::Ios) => "No simulators found",
        };
        lines.push(Line::styled(note, muted));
    } else {
        let chosen = state.selected_devices.get(&state.target);
        let visible =
            usize::from(inner.height).saturating_sub(usize::from(DEVICE_PANEL_HEADER_ROWS));
        let top = state.device_top.min(devices.len().saturating_sub(visible));
        lines.extend(devices.iter().skip(top).take(visible).map(|device| {
            device_row(device, chosen == Some(&device.id), usize::from(inner.width))
        }));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn info_lines(state: &AppState) -> Vec<Line<'static>> {
    let heading =
        |text: &str| Line::styled(text.to_string(), Style::new().add_modifier(Modifier::BOLD));
    let row = |label: &str, value: String| Line::raw(format!("  {label:<10} {value}"));
    let muted = |text: &str| Line::styled(format!("  {text}"), Style::new().fg(MUTED));
    if !state.sdk_loaded {
        return vec![muted("Checking tools…")];
    }
    let home = state.sdk.home.as_deref();
    let shown = |p: &Path| display_path(p, home);
    let tool_row = |label: &str, tool: &Option<ToolInfo>| match tool {
        Some(tool) => row(
            label,
            format!(
                "{}  {}",
                shown(&tool.path),
                tool.version.as_deref().unwrap_or("?")
            ),
        ),
        None => row(label, "not found".to_string()),
    };

    let android = &state.sdk.android;
    let mut lines = vec![heading("Android SDK")];
    match &android.root {
        Some(root) => {
            let source = android
                .source
                .as_deref()
                .map(|s| format!("  ({s})"))
                .unwrap_or_default();
            lines.push(row("location", format!("{}{source}", shown(root))));
        }
        None => lines.push(row("location", "not found".to_string())),
    }
    lines.push(tool_row("adb", &android.adb));
    lines.push(tool_row("emulator", &android.emulator));

    let ios = &state.sdk.ios;
    lines.push(Line::raw(""));
    lines.push(heading("Xcode"));
    match &ios.developer_dir {
        Some(dir) => {
            lines.push(row("developer", shown(dir)));
            lines.push(row(
                "version",
                ios.version.clone().unwrap_or_else(|| "unknown".to_string()),
            ));
            lines.push(row(
                "simctl",
                if ios.simctl { "ok" } else { "missing" }.to_string(),
            ));
        }
        None => lines.push(row("developer", "not found".to_string())),
    }

    lines.push(Line::raw(""));
    lines.push(heading("Target"));
    let device = state.selected_device().map_or_else(
        || "no device chosen".to_string(),
        |d| format!("{} ({})", d.name, d.id),
    );
    lines.push(row(state.target.label(), device));
    lines
}

pub(crate) fn render_info(frame: &mut Frame, state: &AppState) {
    if !state.info_open {
        return;
    }
    let lines = info_lines(state);
    let area = frame.area();
    let width = u16::try_from(u32::from(area.width) * 9 / 10)
        .unwrap_or(u16::MAX)
        .min(INFO_WIDTH);
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let popup = centered(area, width, height);
    let block = Block::bordered()
        .title(INFO_TITLE)
        .border_style(Style::new().fg(ACCENT));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{buffer, screen_sized, state};
    use lf_mide_core::{Action, AndroidSdk, SdkInfo, ToolInfo, XcodeSdk, reduce};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A realistic terminal: at 140 columns the Devices panel is 30 wide.
    fn screen(s: &AppState) -> String {
        screen_sized(s, 140, 30)
    }

    fn sdk() -> SdkInfo {
        SdkInfo {
            android: AndroidSdk {
                root: Some(PathBuf::from("/Users/test/Library/Android/sdk")),
                source: Some("default location".into()),
                adb: Some(ToolInfo {
                    path: PathBuf::from("/Users/test/Library/Android/sdk/platform-tools/adb"),
                    version: Some("1.0.41".into()),
                }),
                emulator: Some(ToolInfo {
                    path: PathBuf::from("/Users/test/Library/Android/sdk/emulator/emulator"),
                    version: Some("36.1.9".into()),
                }),
            },
            ios: XcodeSdk {
                developer_dir: Some(PathBuf::from("/Applications/Xcode.app/Contents/Developer")),
                version: Some("Xcode 26.5".into()),
                simctl: true,
            },
            home: Some(PathBuf::from("/Users/test")),
        }
    }

    fn device(
        platform: Platform,
        id: &str,
        name: &str,
        detail: &str,
        device_state: DeviceState,
    ) -> Device {
        Device {
            platform,
            id: id.into(),
            name: name.into(),
            detail: detail.into(),
            state: device_state,
            serial: None,
        }
    }

    fn with_devices(s: AppState, devices: Vec<Device>) -> AppState {
        reduce(
            s,
            Action::DevicesLoaded {
                devices: Arc::new(devices),
                errors: BTreeMap::new(),
            },
        )
    }

    fn loaded() -> AppState {
        let s = reduce(state(), Action::SdkLoaded(sdk()));
        with_devices(
            s,
            vec![
                device(
                    Platform::Android,
                    "Pixel_8",
                    "Pixel_8",
                    "API 35",
                    DeviceState::Running,
                ),
                device(
                    Platform::Android,
                    "Tablet",
                    "Tablet",
                    "API 34",
                    DeviceState::Stopped,
                ),
                device(
                    Platform::Ios,
                    "11111111-1111-1111-1111-111111111111",
                    "iPhone 17",
                    "iOS 26.0",
                    DeviceState::Stopped,
                ),
                device(
                    Platform::Ios,
                    "22222222-2222-2222-2222-222222222222",
                    "iPhone 17 Pro",
                    "iOS 26.0",
                    DeviceState::Starting,
                ),
            ],
        )
    }

    #[test]
    fn the_panel_names_both_platforms_and_how_to_switch() {
        let text = screen(&state());

        assert!(text.contains("Devices"), "{text}");
        assert!(text.contains("Android") && text.contains("iOS"), "{text}");
        assert!(text.contains("t: switch"), "{text}");
    }

    #[test]
    fn the_active_platform_stands_out() {
        let android = loaded();
        let ios = reduce(loaded(), Action::ToggleTarget);
        let on_screen = |s: &AppState, label: &str| {
            let text = screen(s);
            // The chips row is the first line inside the Devices panel.
            let (col, row) = text
                .lines()
                .enumerate()
                .find_map(|(r, l)| {
                    let c = l.find(&format!(" {label} "))?;
                    (r > 1 && r < 5).then_some((l[..c].chars().count() as u16 + 1, r as u16))
                })
                .unwrap_or_else(|| panic!("no {label} chip in:\\n{text}"));
            buffer(s, 140, 30)[(col, row)]
                .modifier
                .contains(Modifier::REVERSED)
        };

        assert!(on_screen(&android, "Android") && !on_screen(&android, "iOS"));
        assert!(on_screen(&ios, "iOS") && !on_screen(&ios, "Android"));
    }

    #[test]
    fn tools_are_reported_as_being_checked_until_they_are_known() {
        assert!(screen(&state()).contains("Checking tools"));
        assert!(!screen(&loaded()).contains("Checking tools"));
    }

    #[test]
    fn android_shows_the_sdk_location_and_tool_versions() {
        let text = screen(&loaded());

        assert!(text.contains("SDK ~/Library/Android/sdk"), "{text}");
        assert!(text.contains("adb 1.0.41"), "{text}");
        assert!(text.contains("emulator 36.1.9"), "{text}");
    }

    #[test]
    fn a_missing_sdk_is_said_plainly() {
        let mut s = reduce(state(), Action::SdkLoaded(SdkInfo::default()));
        assert!(screen(&s).contains("Android SDK not found"));

        s = reduce(s, Action::ToggleTarget);
        assert!(screen(&s).contains("Xcode not found"));
    }

    #[test]
    fn ios_shows_xcode_and_whether_simctl_works() {
        let text = screen(&reduce(loaded(), Action::ToggleTarget));

        assert!(text.contains("Xcode 26.5"), "{text}");
        assert!(text.contains("/Applications/Xcode.app"), "{text}");
        assert!(text.contains("simctl ok"), "{text}");
        let mut broken = sdk();
        broken.ios.simctl = false;
        let missing = screen(&reduce(
            reduce(state(), Action::SdkLoaded(broken)),
            Action::ToggleTarget,
        ));
        assert!(missing.contains("simctl missing"), "{missing}");
    }

    #[test]
    fn only_the_current_targets_devices_are_listed() {
        let android = screen(&loaded());
        let ios = screen(&reduce(loaded(), Action::ToggleTarget));

        assert!(
            android.contains("Pixel_8")
                && android.contains("Tablet")
                && !android.contains("iPhone")
        );
        assert!(ios.contains("iPhone 17 Pro") && !ios.contains("Pixel_8"));
        assert!(
            android.contains("API 35") && ios.contains("iOS 26.0"),
            "details are shown"
        );
    }

    #[test]
    fn states_have_distinct_markers_and_the_choice_has_a_pointer() {
        let android = screen(&loaded());
        let ios = screen(&reduce(loaded(), Action::ToggleTarget));
        let line = |text: &str, name: &str| {
            text.lines()
                .skip(1)
                .find(|l| l.contains(name))
                .unwrap()
                .to_string()
        };

        assert!(line(&android, "Pixel_8").contains('●'), "running");
        assert!(line(&android, "Tablet").contains('○'), "stopped");
        assert!(line(&ios, "iPhone 17 Pro").contains('◐'), "starting");
        assert!(
            line(&android, "Pixel_8").contains('▶'),
            "the running one is the default choice"
        );
        assert!(!line(&android, "Tablet").contains('▶'));
    }

    #[test]
    fn details_that_do_not_fit_are_shortened_at_a_boundary_never_cut_mid_word() {
        assert_eq!(
            fit_detail("API 36 · arm64-v8a", 30),
            Some("API 36 · arm64-v8a")
        );
        assert_eq!(
            fit_detail("API 36 · arm64-v8a", 18),
            Some("API 36 · arm64-v8a")
        );
        assert_eq!(fit_detail("API 36 · arm64-v8a", 17), Some("API 36"));
        assert_eq!(fit_detail("API 36 · arm64-v8a", 6), Some("API 36"));
        assert_eq!(fit_detail("API 36 · arm64-v8a", 5), None);
        assert_eq!(fit_detail("iOS 26.0", 8), Some("iOS 26.0"));
        assert_eq!(fit_detail("iOS 26.0", 7), None);
        assert_eq!(fit_detail("", 40), None);
        assert_eq!(fit_detail("API 36", 0), None);
    }

    #[test]
    fn a_narrow_panel_shows_the_api_level_but_a_wide_one_shows_everything() {
        let s = with_devices(
            reduce(state(), Action::SdkLoaded(sdk())),
            vec![device(
                Platform::Android,
                "Pixel_8",
                "Pixel_8",
                "API 35 · arm64-v8a",
                DeviceState::Stopped,
            )],
        );

        let narrow = screen_sized(&s, 140, 30);
        let wide = screen_sized(&s, 200, 30);

        let row = |text: &str| {
            text.lines()
                .skip(1)
                .find(|l| l.contains("Pixel_8"))
                .unwrap()
                .to_string()
        };
        assert!(
            row(&narrow).contains("API 35") && !row(&narrow).contains("arm64"),
            "{}",
            row(&narrow)
        );
        assert!(row(&wide).contains("API 35 · arm64-v8a"), "{}", row(&wide));
    }

    #[test]
    fn moving_the_choice_moves_the_pointer() {
        let moved = reduce(loaded(), Action::DeviceMove(1));

        let text = screen(&moved);

        assert!(
            text.lines()
                .skip(1)
                .find(|l| l.contains("Tablet"))
                .unwrap()
                .contains('▶')
        );
        assert!(
            !text
                .lines()
                .find(|l| l.contains("Pixel_8"))
                .unwrap()
                .contains('▶')
        );
    }

    #[test]
    fn the_list_respects_the_scroll_position() {
        let many: Vec<Device> = (0..40)
            .map(|i| {
                device(
                    Platform::Android,
                    &format!("avd{i:02}"),
                    &format!("avd{i:02}"),
                    "",
                    DeviceState::Stopped,
                )
            })
            .collect();
        let mut s = with_devices(state(), many);
        s.device_top = 10;

        let text = screen(&s);

        assert!(text.contains("avd10") && !text.contains("avd09"), "{text}");
    }

    #[test]
    fn empty_and_failed_listings_are_explained() {
        let waiting = screen(&state());
        let none = screen(&with_devices(state(), vec![]));
        let failed = reduce(
            state(),
            Action::DevicesLoaded {
                devices: Arc::new(vec![]),
                errors: BTreeMap::from([(
                    Platform::Android,
                    "`emulator` was not found".to_string(),
                )]),
            },
        );

        assert!(waiting.contains("Listing devices"), "{waiting}");
        assert!(none.contains("No emulators found"), "{none}");
        assert!(screen(&failed).contains("`emulator` was not found"));
        assert!(screen(&reduce(none_ios(), Action::ToggleTarget)).contains("No simulators found"));
    }

    fn none_ios() -> AppState {
        with_devices(state(), vec![])
    }

    #[test]
    fn the_header_shows_the_target_and_chosen_device() {
        let text = screen(&loaded());
        let ios = screen(&reduce(loaded(), Action::ToggleTarget));

        assert!(text.contains("Android · Pixel_8"), "{text}");
        assert!(ios.contains("iOS · iPhone 17"), "{ios}");
        assert_eq!(target_summary(&state()), "Android · no device");
    }

    #[test]
    fn the_environment_overlay_is_hidden_until_asked_for() {
        assert!(!screen(&loaded()).contains("Environment"));
    }

    #[test]
    fn the_environment_overlay_shows_full_locations() {
        let text = screen(&reduce(loaded(), Action::ToggleInfo));

        for expected in [
            "Environment",
            "Android SDK",
            "~/Library/Android/sdk",
            "default location",
            "platform-tools/adb",
            "1.0.41",
            "emulator",
            "36.1.9",
            "Xcode 26.5",
            "/Applications/Xcode.app/Contents/Developer",
            "simctl",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\\n{text}");
        }
    }

    #[test]
    fn the_overlay_reports_missing_tools_and_unloaded_facts() {
        let unloaded = screen(&reduce(state(), Action::ToggleInfo));
        let missing = screen(&reduce(
            reduce(state(), Action::SdkLoaded(SdkInfo::default())),
            Action::ToggleInfo,
        ));

        assert!(unloaded.contains("Checking"), "{unloaded}");
        assert!(missing.contains("not found"), "{missing}");
    }

    #[test]
    fn hostile_device_names_cannot_inject_terminal_sequences() {
        let s = with_devices(
            state(),
            vec![device(
                Platform::Android,
                "evil",
                "Evil\u{1b}[2J\u{7}name",
                "API\u{1b}]0;x\u{7}",
                DeviceState::Stopped,
            )],
        );

        let buf = buffer(&s, 140, 30);

        assert!(
            !buf.content()
                .iter()
                .any(|c| c.symbol().chars().any(char::is_control))
        );
    }

    #[test]
    fn survives_tiny_and_huge_terminals() {
        let states = [
            loaded(),
            reduce(loaded(), Action::ToggleInfo),
            reduce(loaded(), Action::ToggleTarget),
        ];
        for s in &states {
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
}
