//! Turning a [`Report`] into text for the terminal or JSON for scripts.

use lf_mide_core::{LineKind, Report, Status};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

/// Human-readable report grouped by category. `color` adds ANSI styling.
pub fn render_text(report: &Report, color: bool) -> String {
    let lines = report.lines();
    if lines.is_empty() {
        return "No checks ran\n".to_string();
    }
    let style = |code: &str, text: &str| {
        if color {
            format!("{code}{text}{RESET}")
        } else {
            text.to_string()
        }
    };

    let mut out = format!("{}\n", style(BOLD, "LF-Mide doctor"));
    for line in lines {
        let rendered = match line.kind {
            LineKind::Blank => String::new(),
            LineKind::Heading => style(BOLD, &line.text),
            LineKind::Row(status) => {
                format!(
                    "  {} {}",
                    style(status_color(status), status.symbol()),
                    line.text
                )
            }
            LineKind::Path => format!("      {}", style(DIM, &line.text)),
            LineKind::Hint => format!("      → {}", line.text),
            LineKind::Summary => line.text,
        };
        out.push_str(&rendered);
        out.push('\n');
    }
    out
}

fn status_color(status: Status) -> &'static str {
    match status {
        Status::Ok => GREEN,
        Status::Warn => YELLOW,
        Status::Missing => RED,
    }
}

pub fn render_json(report: &Report) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|err| format!("{{\"error\": \"{err}\"}}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_core::{Category, CheckResult};

    fn sample() -> Report {
        Report::new(vec![
            CheckResult::new(Category::Ios, "Xcode", Status::Ok, "Xcode 26.5")
                .with_path("/Applications/Xcode.app"),
            CheckResult::new(Category::Android, "adb", Status::Missing, "not found")
                .with_hint("sdkmanager \"platform-tools\""),
            CheckResult::new(Category::Android, "Android SDK", Status::Ok, "ANDROID_HOME"),
            CheckResult::new(Category::Editor, "kotlin-lsp", Status::Warn, "not found")
                .with_hint("brew install JetBrains/utils/kotlin-lsp"),
        ])
    }

    #[test]
    fn groups_by_category_in_order_with_symbols() {
        let text = render_text(&sample(), false);

        let android = text.find("Android\n").unwrap();
        let ios = text.find("iOS\n").unwrap();
        let editor = text.find("Editor & language servers\n").unwrap();
        assert!(android < ios && ios < editor, "{text}");
        assert!(text.contains("✓ Android SDK"));
        assert!(text.contains("✗ adb"));
        assert!(text.contains("⚠ kotlin-lsp"));
        assert!(
            !text.contains("\nTerminal\n"),
            "empty categories are not printed"
        );
    }

    #[test]
    fn shows_paths_and_hints_only_where_useful() {
        let text = render_text(&sample(), false);

        assert!(text.contains("      /Applications/Xcode.app"));
        assert!(text.contains("      → sdkmanager \"platform-tools\""));
        assert!(text.contains("      → brew install JetBrains/utils/kotlin-lsp"));
    }

    #[test]
    fn ok_results_never_print_a_hint() {
        let report = Report::new(vec![
            CheckResult::new(Category::Editor, "git", Status::Ok, "2.50.1")
                .with_hint("should not show"),
        ]);

        assert!(!render_text(&report, false).contains("should not show"));
    }

    #[test]
    fn summary_counts_each_status() {
        let text = render_text(&sample(), false);

        assert!(
            text.trim_end().ends_with("2 ok · 1 warning · 1 missing"),
            "{text}"
        );
    }

    #[test]
    fn color_is_opt_in() {
        let plain = render_text(&sample(), false);
        let colored = render_text(&sample(), true);

        assert!(!plain.contains('\x1b'));
        assert!(colored.contains("\x1b[32m✓"));
        assert!(colored.contains("\x1b[31m✗"));
        assert!(colored.contains("\x1b[33m⚠"));
    }

    #[test]
    fn details_line_up_in_a_column() {
        let text = render_text(&sample(), false);
        let column = |needle: &str| {
            let line = text.lines().find(|l| l.contains(needle)).unwrap();
            line.find(needle).unwrap()
        };

        assert_eq!(column("ANDROID_HOME"), column("not found"));
    }

    #[test]
    fn empty_report_says_so() {
        assert!(render_text(&Report::default(), false).contains("No checks ran"));
    }

    #[test]
    fn json_is_valid_and_keeps_every_field() {
        let json = render_json(&sample());

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let results = value["results"].as_array().unwrap();
        assert_eq!(results.len(), 4);
        assert_eq!(results[0]["name"], "adb");
        assert_eq!(results[0]["status"], "missing");
        assert_eq!(results[0]["hint"], "sdkmanager \"platform-tools\"");
        assert!(results[1]["path"].is_null());
    }
}
