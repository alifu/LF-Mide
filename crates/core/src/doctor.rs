//! Plain data produced by `lf-mide doctor`. The checks themselves live in the
//! `lf-mide-doctor` crate; these types are shared with the in-app panel.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Missing,
}

impl Status {
    pub fn symbol(self) -> &'static str {
        match self {
            Status::Ok => "✓",
            Status::Warn => "⚠",
            Status::Missing => "✗",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Android,
    Ios,
    Editor,
    Terminal,
    Project,
    Resources,
}

impl Category {
    pub const ORDER: [Category; 6] = [
        Category::Android,
        Category::Ios,
        Category::Editor,
        Category::Terminal,
        Category::Project,
        Category::Resources,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Category::Android => "Android",
            Category::Ios => "iOS",
            Category::Editor => "Editor & language servers",
            Category::Terminal => "Terminal",
            Category::Project => "Project",
            Category::Resources => "Resources",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub category: Category,
    pub status: Status,
    /// Version or short finding, e.g. `0.12.6` or `2 AVDs`.
    pub detail: String,
    pub path: Option<String>,
    /// Copy-pasteable fix, shown when the status is not `Ok`.
    pub hint: Option<String>,
}

impl CheckResult {
    pub fn new(category: Category, name: &str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            category,
            status,
            detail: detail.into(),
            path: None,
            hint: None,
        }
    }

    pub fn with_path(self, path: impl Into<String>) -> Self {
        Self {
            path: Some(path.into()),
            ..self
        }
    }

    pub fn with_hint(self, hint: impl Into<String>) -> Self {
        Self {
            hint: Some(hint.into()),
            ..self
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    pub results: Vec<CheckResult>,
}

/// What a line of the rendered report is, so each renderer can style it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    Heading,
    Row(Status),
    Path,
    Hint,
    Summary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportLine {
    pub kind: LineKind,
    /// Without indentation or status symbol; renderers add those.
    pub text: String,
}

/// The doctor overlay takes this share of the terminal height.
const POPUP_HEIGHT_PERCENT: u16 = 85;
const MIN_POPUP_HEIGHT: u16 = 6;
/// Top and bottom border.
const POPUP_BORDER_ROWS: u16 = 2;

/// Height of the doctor overlay in a terminal `terminal_height` rows tall.
pub fn popup_height(terminal_height: u16) -> u16 {
    let share = u32::from(terminal_height) * u32::from(POPUP_HEIGHT_PERCENT) / 100;
    let height = u16::try_from(share).unwrap_or(u16::MAX);
    height.max(MIN_POPUP_HEIGHT).min(terminal_height)
}

/// Rows available for report text inside the overlay.
pub fn popup_content_rows(terminal_height: u16) -> u16 {
    popup_height(terminal_height).saturating_sub(POPUP_BORDER_ROWS)
}

/// Width of the check-name column in rendered rows.
const NAME_COLUMN_MIN: usize = 24;

impl Report {
    /// The report as display lines: per category a heading, then each check
    /// as a row (plus its path, plus its hint when not `Ok`), then a summary.
    /// Empty when there are no results.
    pub fn lines(&self) -> Vec<ReportLine> {
        if self.results.is_empty() {
            return Vec::new();
        }
        let width = self
            .results
            .iter()
            .map(|r| r.name.chars().count())
            .max()
            .unwrap_or(0)
            .max(NAME_COLUMN_MIN);
        let line = |kind, text: String| ReportLine { kind, text };

        let mut lines = Vec::new();
        for category in Category::ORDER {
            let in_category: Vec<_> = self
                .results
                .iter()
                .filter(|r| r.category == category)
                .collect();
            if in_category.is_empty() {
                continue;
            }
            lines.push(line(LineKind::Blank, String::new()));
            lines.push(line(LineKind::Heading, category.title().to_string()));
            for result in in_category {
                let row = format!("{:<width$}  {}", result.name, result.detail);
                lines.push(line(LineKind::Row(result.status), row));
                if let Some(path) = &result.path {
                    lines.push(line(LineKind::Path, path.clone()));
                }
                if let (Some(hint), true) = (&result.hint, result.status != Status::Ok) {
                    lines.push(line(LineKind::Hint, hint.clone()));
                }
            }
        }
        lines.push(line(LineKind::Blank, String::new()));
        lines.push(line(LineKind::Summary, self.summary()));
        lines
    }

    /// `25 ok · 4 warnings · 0 missing`
    pub fn summary(&self) -> String {
        let warnings = self.count(Status::Warn);
        format!(
            "{} ok · {warnings} warning{} · {} missing",
            self.count(Status::Ok),
            if warnings == 1 { "" } else { "s" },
            self.count(Status::Missing),
        )
    }

    /// Sorts results into category order (stable within a category).
    pub fn new(mut results: Vec<CheckResult>) -> Self {
        results.sort_by_key(|r| Category::ORDER.iter().position(|c| *c == r.category));
        Self { results }
    }

    pub fn count(&self, status: Status) -> usize {
        self.results.iter().filter(|r| r.status == status).count()
    }

    /// `1` when anything required is missing, `0` otherwise (warnings pass).
    pub fn exit_code(&self) -> i32 {
        i32::from(self.count(Status::Missing) > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(report: &Report) -> Vec<LineKind> {
        report.lines().iter().map(|l| l.kind).collect()
    }

    fn result(category: Category, name: &str, status: Status) -> CheckResult {
        CheckResult::new(category, name, status, "")
    }

    #[test]
    fn status_symbols_are_distinct() {
        assert_eq!(Status::Ok.symbol(), "✓");
        assert_eq!(Status::Warn.symbol(), "⚠");
        assert_eq!(Status::Missing.symbol(), "✗");
    }

    #[test]
    fn report_groups_by_category_order_keeping_check_order() {
        let report = Report::new(vec![
            result(Category::Project, "p1", Status::Ok),
            result(Category::Android, "a1", Status::Ok),
            result(Category::Ios, "i1", Status::Ok),
            result(Category::Android, "a2", Status::Ok),
        ]);

        let names: Vec<_> = report.results.iter().map(|r| r.name.as_str()).collect();

        assert_eq!(names, vec!["a1", "a2", "i1", "p1"]);
    }

    #[test]
    fn exit_code_fails_only_on_missing() {
        let warn_only = Report::new(vec![result(Category::Editor, "x", Status::Warn)]);
        let missing = Report::new(vec![
            result(Category::Editor, "x", Status::Ok),
            result(Category::Editor, "y", Status::Missing),
        ]);

        assert_eq!(warn_only.exit_code(), 0);
        assert_eq!(missing.exit_code(), 1);
        assert_eq!(Report::default().exit_code(), 0);
    }

    #[test]
    fn count_filters_by_status() {
        let report = Report::new(vec![
            result(Category::Editor, "a", Status::Ok),
            result(Category::Editor, "b", Status::Warn),
            result(Category::Editor, "c", Status::Warn),
        ]);

        assert_eq!(report.count(Status::Warn), 2);
        assert_eq!(report.count(Status::Missing), 0);
    }

    #[test]
    fn builders_set_path_and_hint_without_touching_other_fields() {
        let r = CheckResult::new(Category::Ios, "Xcode", Status::Warn, "26.5")
            .with_path("/Applications/Xcode.app")
            .with_hint("sudo xcode-select -s /Applications/Xcode.app");

        assert_eq!(r.detail, "26.5");
        assert_eq!(r.path.as_deref(), Some("/Applications/Xcode.app"));
        assert!(r.hint.unwrap().starts_with("sudo"));
    }

    #[test]
    fn report_serializes_to_json_with_lowercase_enums() {
        let report = Report::new(vec![result(Category::Android, "adb", Status::Missing)]);

        let json = serde_json::to_string(&report).unwrap();

        assert!(json.contains("\"status\":\"missing\""));
        assert!(json.contains("\"category\":\"android\""));
    }

    #[test]
    fn lines_are_grouped_with_blank_separators_and_a_summary() {
        let report = Report::new(vec![
            CheckResult::new(Category::Android, "adb", Status::Missing, "not found")
                .with_hint("sdkmanager"),
            CheckResult::new(Category::Android, "SDK", Status::Ok, "ANDROID_HOME")
                .with_path("/sdk")
                .with_hint("ignored"),
            CheckResult::new(Category::Ios, "Xcode", Status::Warn, "old"),
        ]);

        assert_eq!(
            kinds(&report),
            vec![
                LineKind::Blank,
                LineKind::Heading,
                LineKind::Row(Status::Missing),
                LineKind::Hint,
                LineKind::Row(Status::Ok),
                LineKind::Path,
                LineKind::Blank,
                LineKind::Heading,
                LineKind::Row(Status::Warn),
                LineKind::Blank,
                LineKind::Summary,
            ]
        );
    }

    #[test]
    fn row_text_pads_names_to_a_common_column() {
        let report = Report::new(vec![
            CheckResult::new(Category::Editor, "git", Status::Ok, "2.50"),
            CheckResult::new(
                Category::Editor,
                "a-name-longer-than-the-minimum-column",
                Status::Ok,
                "1.0",
            ),
        ]);

        let rows: Vec<_> = report
            .lines()
            .into_iter()
            .filter(|l| matches!(l.kind, LineKind::Row(_)))
            .collect();

        let detail_at = |l: &ReportLine, d: &str| l.text.find(d).unwrap();
        assert_eq!(detail_at(&rows[0], "2.50"), detail_at(&rows[1], "1.0"));
        assert!(rows[0].text.starts_with("git "));
    }

    #[test]
    fn summary_text_pluralizes_warnings() {
        let one = Report::new(vec![result(Category::Editor, "a", Status::Warn)]);
        let two = Report::new(vec![
            result(Category::Editor, "a", Status::Warn),
            result(Category::Editor, "b", Status::Warn),
        ]);

        assert_eq!(
            one.lines().last().unwrap().text,
            "0 ok · 1 warning · 0 missing"
        );
        assert_eq!(
            two.lines().last().unwrap().text,
            "0 ok · 2 warnings · 0 missing"
        );
    }

    #[test]
    fn popup_is_most_of_the_terminal_but_never_taller_than_it() {
        assert_eq!(popup_height(30), 25);
        assert_eq!(popup_height(10), 8);
        assert_eq!(popup_height(4), 4);
        assert_eq!(popup_content_rows(30), 23);
        assert_eq!(popup_content_rows(4), 2);
        assert_eq!(popup_content_rows(0), 0);
    }

    #[test]
    fn empty_report_has_no_lines() {
        assert!(Report::default().lines().is_empty());
    }
}
