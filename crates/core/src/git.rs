//! Git facts shown in the cockpit: changed files, diffs, blame. Plain data
//! and the rules that turn one into another; reading the repository lives in
//! `lf-mide-vcs`.

use std::path::PathBuf;

/// Longest blame/diff text kept per line; repository data is not trusted.
pub const MAX_TEXT_CHARS: usize = 300;
/// Changed files listed.
pub const MAX_CHANGES: usize = 2000;
/// Diff lines kept for one file.
pub const MAX_DIFF_LINES: usize = 20_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl ChangeKind {
    /// One letter, as `git status --short` would.
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Modified => 'M',
            ChangeKind::Added => 'A',
            ChangeKind::Deleted => 'D',
            ChangeKind::Renamed => 'R',
            ChangeKind::Untracked => '?',
            ChangeKind::Conflicted => 'U',
        }
    }
}

/// A file that differs from HEAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Relative to the project root.
    pub path: PathBuf,
    pub kind: ChangeKind,
    pub staged: bool,
    /// Changes whenever the file's size or modification time does, so a
    /// further edit of an already modified file is noticed.
    pub stamp: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOrigin {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub origin: LineOrigin,
    pub text: String,
    /// Line number in the base version (not for added lines).
    pub old_no: Option<u32>,
    /// Line number in the working file (not for removed lines).
    pub new_no: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileDiff {
    pub hunks: Vec<Hunk>,
    pub binary: bool,
    /// More lines existed than [`MAX_DIFF_LINES`].
    pub truncated: bool,
}

impl FileDiff {
    pub fn is_empty(&self) -> bool {
        self.hunks.is_empty() && !self.binary
    }

    /// Rows the unified view takes: a header per hunk plus its lines.
    pub fn unified_rows(&self) -> usize {
        self.hunks.iter().map(|h| 1 + h.lines.len()).sum()
    }
}

/// How a line of the working file differs from the base, for the gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    Added,
    Changed,
    /// Lines were removed just below this one.
    DeletedBelow,
    /// Lines were removed above the first line.
    DeletedAbove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineMark {
    /// 1-based line of the working file.
    pub line: u32,
    pub kind: MarkKind,
}

/// Gutter marks for a diff: lines that are new, lines that replace others,
/// and where lines were taken away.
pub fn marks_from_diff(diff: &FileDiff) -> Vec<LineMark> {
    let mut marks = Vec::new();
    for hunk in &diff.hunks {
        let mut index = 0;
        while index < hunk.lines.len() {
            if hunk.lines[index].origin == LineOrigin::Context {
                index += 1;
                continue;
            }
            // One run of removed and/or added lines.
            let start = index;
            while index < hunk.lines.len() && hunk.lines[index].origin != LineOrigin::Context {
                index += 1;
            }
            marks.extend(marks_for_run(
                &hunk.lines[start..index],
                &hunk.lines[..start],
            ));
        }
    }
    marks
}

fn marks_for_run(run: &[DiffLine], before: &[DiffLine]) -> Vec<LineMark> {
    let removed = run
        .iter()
        .filter(|l| l.origin == LineOrigin::Removed)
        .count();
    let added: Vec<u32> = run.iter().filter_map(|l| l.new_no).collect();
    if added.is_empty() {
        // Only removals: mark the line above, or the top of the file.
        let above = before.iter().rev().find_map(|l| l.new_no);
        return vec![match above {
            Some(line) => LineMark {
                line,
                kind: MarkKind::DeletedBelow,
            },
            None => LineMark {
                line: 1,
                kind: MarkKind::DeletedAbove,
            },
        }];
    }
    let kind = if removed > 0 {
        MarkKind::Changed
    } else {
        MarkKind::Added
    };
    added
        .into_iter()
        .map(|line| LineMark { line, kind })
        .collect()
}

/// Where each hunk starts in the working file (at least 1), for jumping
/// between changes: its first added line, or for a pure removal the line
/// above it (where the gutter marks it).
pub fn hunk_lines(diff: &FileDiff) -> Vec<u32> {
    diff.hunks.iter().map(hunk_line).collect()
}

fn hunk_line(hunk: &Hunk) -> u32 {
    let mut above = None;
    let mut index = 0;
    while index < hunk.lines.len() {
        if hunk.lines[index].origin == LineOrigin::Context {
            above = hunk.lines[index].new_no;
            index += 1;
            continue;
        }
        let run_end = hunk.lines[index..]
            .iter()
            .position(|l| l.origin == LineOrigin::Context)
            .map_or(hunk.lines.len(), |n| index + n);
        let first_added = hunk.lines[index..run_end].iter().find_map(|l| l.new_no);
        return first_added.or(above).unwrap_or(hunk.new_start).max(1);
    }
    hunk.new_start.max(1)
}

/// Who last changed a line of the working file, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    /// 1-based line of the working file.
    pub line: u32,
    /// Short commit id; `None` for lines not committed yet.
    pub commit: Option<String>,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
    pub summary: String,
}

/// `Ada · 3 days ago · a1b2c3d`, or `Not committed yet`.
pub fn blame_text(line: &BlameLine, now: i64) -> String {
    let Some(commit) = &line.commit else {
        return "Not committed yet".to_string();
    };
    let text = format!(
        "{} · {} · {commit}",
        line.author,
        relative_time(now, line.time)
    );
    clean_text(&text)
}

/// Spaces a tab stands for in shown text.
const TAB_WIDTH: usize = 4;

/// Repository text made safe to show: tabs become spaces (indentation must
/// survive), control characters and invisible direction/format
/// characters (which can make text read differently from what it is, as in
/// "Trojan Source" attacks) become a visible `�`, and the length is bounded.
pub fn clean_text(text: &str) -> String {
    let mut out = String::new();
    let mut shown = 0;
    for ch in text.chars() {
        if shown >= MAX_TEXT_CHARS {
            break;
        }
        if ch == '\t' {
            let spaces = TAB_WIDTH.min(MAX_TEXT_CHARS - shown);
            out.extend(std::iter::repeat_n(' ', spaces));
            shown += spaces;
        } else if crate::text::is_hidden(ch) {
            out.push(crate::text::REPLACEMENT);
            shown += 1;
        } else {
            out.push(if ch.is_control() {
                crate::text::REPLACEMENT
            } else {
                ch
            });
            shown += 1;
        }
    }
    out
}

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;
const MONTH: i64 = 30 * DAY;
const YEAR: i64 = 365 * DAY;

/// `just now`, `5 minutes ago`, `3 days ago`, `2 years ago`.
pub fn relative_time(now: i64, then: i64) -> String {
    let age = now.saturating_sub(then);
    if age < 0 || age < MINUTE {
        return "just now".to_string();
    }
    let (count, unit) = if age < HOUR {
        (age / MINUTE, "minute")
    } else if age < DAY {
        (age / HOUR, "hour")
    } else if age < MONTH {
        (age / DAY, "day")
    } else if age < YEAR {
        (age / MONTH, "month")
    } else {
        (age / YEAR, "year")
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseKind {
    Head,
    Branch,
    Commit,
}

/// What a diff is taken against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseRef {
    pub kind: BaseKind,
    /// Shown to the user: `HEAD`, a branch name, `a1b2c3d Fix the thing`.
    pub label: String,
    /// What to resolve in the repository: `HEAD`, a branch, a commit id.
    pub spec: String,
}

impl BaseRef {
    pub fn head() -> Self {
        Self {
            kind: BaseKind::Head,
            label: "HEAD".to_string(),
            spec: "HEAD".to_string(),
        }
    }
}

impl Default for BaseRef {
    fn default() -> Self {
        Self::head()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(origin: LineOrigin, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            origin,
            text: String::new(),
            old_no: old,
            new_no: new,
        }
    }

    fn ctx(old: u32, new: u32) -> DiffLine {
        line(LineOrigin::Context, Some(old), Some(new))
    }
    fn add(new: u32) -> DiffLine {
        line(LineOrigin::Added, None, Some(new))
    }
    fn del(old: u32) -> DiffLine {
        line(LineOrigin::Removed, Some(old), None)
    }

    fn diff(lines: Vec<DiffLine>) -> FileDiff {
        FileDiff {
            hunks: vec![Hunk {
                old_start: 1,
                old_lines: 0,
                new_start: 1,
                new_lines: 0,
                lines,
            }],
            ..FileDiff::default()
        }
    }

    fn marks(lines: Vec<DiffLine>) -> Vec<(u32, MarkKind)> {
        marks_from_diff(&diff(lines))
            .into_iter()
            .map(|m| (m.line, m.kind))
            .collect()
    }

    #[test]
    fn new_lines_are_marked_added() {
        assert_eq!(
            marks(vec![ctx(1, 1), add(2), add(3), ctx(2, 4)]),
            [(2, MarkKind::Added), (3, MarkKind::Added)]
        );
    }

    #[test]
    fn lines_that_replace_others_are_marked_changed() {
        assert_eq!(
            marks(vec![ctx(1, 1), del(2), add(2), add(3), ctx(3, 4)]),
            [(2, MarkKind::Changed), (3, MarkKind::Changed)]
        );
    }

    #[test]
    fn removed_lines_mark_the_line_above() {
        assert_eq!(
            marks(vec![ctx(1, 1), del(2), del(3), ctx(4, 2)]),
            [(1, MarkKind::DeletedBelow)]
        );
    }

    #[test]
    fn removed_lines_at_the_top_mark_the_first_line() {
        assert_eq!(
            marks(vec![del(1), ctx(2, 1)]),
            [(1, MarkKind::DeletedAbove)]
        );
    }

    #[test]
    fn separate_runs_in_one_hunk_are_marked_separately() {
        assert_eq!(
            marks(vec![add(1), ctx(1, 2), del(2), ctx(3, 3), add(4)]),
            [
                (1, MarkKind::Added),
                (2, MarkKind::DeletedBelow),
                (4, MarkKind::Added)
            ]
        );
    }

    #[test]
    fn a_new_file_is_all_added_and_an_unchanged_one_has_no_marks() {
        assert_eq!(
            marks(vec![add(1), add(2)]),
            [(1, MarkKind::Added), (2, MarkKind::Added)]
        );
        assert!(marks_from_diff(&FileDiff::default()).is_empty());
    }

    #[test]
    fn jumping_points_are_the_first_changed_line_of_each_hunk() {
        let mut d = diff(vec![ctx(1, 1), ctx(2, 2), add(3), ctx(3, 4)]);
        d.hunks.push(Hunk {
            old_start: 40,
            old_lines: 1,
            new_start: 41,
            new_lines: 0,
            lines: vec![ctx(40, 41), del(41), ctx(42, 42)],
        });

        // A pure removal points at the line above it.
        assert_eq!(hunk_lines(&d), [3, 40 + 1]);
    }

    #[test]
    fn a_diff_counts_its_rows_and_knows_when_it_is_empty() {
        let d = diff(vec![ctx(1, 1), add(2)]);

        assert_eq!(d.unified_rows(), 3);
        assert!(!d.is_empty());
        assert!(FileDiff::default().is_empty());
        assert!(
            !FileDiff {
                binary: true,
                ..FileDiff::default()
            }
            .is_empty()
        );
    }

    #[test]
    fn ages_are_written_the_way_people_say_them() {
        let now = 10_000_000_000;
        let ago = |secs: i64| relative_time(now, now - secs);

        assert_eq!(ago(10), "just now");
        assert_eq!(ago(-500), "just now", "a commit dated in the future");
        assert_eq!(ago(60), "1 minute ago");
        assert_eq!(ago(5 * 60), "5 minutes ago");
        assert_eq!(ago(2 * HOUR), "2 hours ago");
        assert_eq!(ago(DAY), "1 day ago");
        assert_eq!(ago(3 * DAY), "3 days ago");
        assert_eq!(ago(2 * MONTH), "2 months ago");
        assert_eq!(ago(3 * YEAR), "3 years ago");
    }

    #[test]
    fn blame_reads_author_age_and_commit() {
        let blamed = BlameLine {
            line: 4,
            commit: Some("a1b2c3d".into()),
            author: "Ada".into(),
            time: 1_000_000 - 3 * DAY,
            summary: "Fix it".into(),
        };
        let fresh = BlameLine {
            commit: None,
            ..blamed.clone()
        };

        assert_eq!(blame_text(&blamed, 1_000_000), "Ada · 3 days ago · a1b2c3d");
        assert_eq!(blame_text(&fresh, 1_000_000), "Not committed yet");
    }

    #[test]
    fn repository_text_loses_control_characters_and_is_bounded() {
        let hostile = BlameLine {
            line: 1,
            commit: Some("abc".into()),
            author: "Eve\u{1b}[31m\nBob".into(),
            time: 0,
            summary: String::new(),
        };

        let text = blame_text(&hostile, 0);

        assert!(!text.chars().any(char::is_control), "{text:?}");
        assert_eq!(
            clean_text(&"x".repeat(1000)).chars().count(),
            MAX_TEXT_CHARS
        );
    }

    #[test]
    fn tabs_become_spaces_so_indentation_survives() {
        assert_eq!(clean_text("\tif (x) {"), "    if (x) {");
        assert_eq!(
            clean_text("a\nb"),
            "a\u{FFFD}b",
            "a stray line break is shown, not hidden"
        );
        assert_eq!(clean_text("a\tb"), "a    b");
        assert_eq!(
            clean_text("\t".repeat(1000).as_str()).chars().count(),
            MAX_TEXT_CHARS
        );
    }

    #[test]
    fn direction_tricks_and_zero_width_characters_are_removed() {
        let trick = "access\u{202E}fdp.exe \u{200B}x\u{2066}y\u{FEFF}";

        assert_eq!(
            clean_text(trick),
            "access\u{FFFD}fdp.exe \u{FFFD}x\u{FFFD}y\u{FFFD}"
        );
        assert_eq!(clean_text("naïve 日本語 ✓"), "naïve 日本語 ✓");
    }

    #[test]
    fn the_default_base_is_head() {
        assert_eq!(BaseRef::default(), BaseRef::head());
        assert_eq!(BaseRef::head().spec, "HEAD");
        assert_eq!(ChangeKind::Untracked.letter(), '?');
    }
}
