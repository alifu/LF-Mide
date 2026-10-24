//! The diff overlay: what is shown, in which layout, and how scrolling and
//! hunk jumping map onto its rows. All pure.

use std::path::PathBuf;
use std::sync::Arc;

use crate::git::{BaseRef, DiffLine, FileDiff, Hunk, LineOrigin};

/// What the overlay has to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffContent {
    Loading,
    Ready(Arc<FileDiff>),
    Failed(String),
}

/// The open diff overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffView {
    /// Relative to the project root.
    pub path: PathBuf,
    pub base: BaseRef,
    /// Which request this belongs to; older results are ignored.
    pub id: u64,
    pub content: DiffContent,
    /// First visible row of the current layout.
    pub scroll: usize,
    pub side_by_side: bool,
    /// Highlighted entry of the base picker while it is open.
    pub picker: Option<usize>,
    /// The side-by-side rows, built once per diff and layout instead of on
    /// every frame and key press (a diff can have 20,000 lines).
    pub side_cache: Option<Arc<Vec<SideRow>>>,
}

/// One row of the side-by-side layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SideRow {
    /// `@@ -10,6 +10,8 @@`
    Header(String),
    /// The old and new text of a line; one side is empty for lines that were
    /// only added or only removed.
    Pair {
        left: Option<DiffLine>,
        right: Option<DiffLine>,
    },
}

pub fn hunk_header(hunk: &Hunk) -> String {
    format!(
        "@@ -{},{} +{},{} @@",
        hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
    )
}

/// The side-by-side rows of a diff. Within a run of changes, removed lines
/// are set against added ones in order; leftovers stand alone.
pub fn side_rows(diff: &FileDiff) -> Vec<SideRow> {
    let mut rows = Vec::new();
    for hunk in &diff.hunks {
        rows.push(SideRow::Header(hunk_header(hunk)));
        let mut index = 0;
        while index < hunk.lines.len() {
            let line = &hunk.lines[index];
            if line.origin == LineOrigin::Context {
                rows.push(SideRow::Pair {
                    left: Some(line.clone()),
                    right: Some(line.clone()),
                });
                index += 1;
                continue;
            }
            let end = hunk.lines[index..]
                .iter()
                .position(|l| l.origin == LineOrigin::Context)
                .map_or(hunk.lines.len(), |n| index + n);
            let run = &hunk.lines[index..end];
            let removed: Vec<&DiffLine> = run
                .iter()
                .filter(|l| l.origin == LineOrigin::Removed)
                .collect();
            let added: Vec<&DiffLine> = run
                .iter()
                .filter(|l| l.origin == LineOrigin::Added)
                .collect();
            for n in 0..removed.len().max(added.len()) {
                rows.push(SideRow::Pair {
                    left: removed.get(n).map(|l| (*l).clone()),
                    right: added.get(n).map(|l| (*l).clone()),
                });
            }
            index = end;
        }
    }
    rows
}

/// Rows in the chosen layout.
pub fn row_count(diff: &FileDiff, side_by_side: bool) -> usize {
    if side_by_side {
        side_rows(diff).len()
    } else {
        diff.unified_rows()
    }
}

/// The row where each hunk's header sits.
pub fn hunk_rows(diff: &FileDiff, side_by_side: bool) -> Vec<usize> {
    let mut rows = Vec::with_capacity(diff.hunks.len());
    if side_by_side {
        rows.extend(
            side_rows(diff)
                .iter()
                .enumerate()
                .filter(|(_, row)| matches!(row, SideRow::Header(_)))
                .map(|(index, _)| index),
        );
    } else {
        let mut at = 0;
        for hunk in &diff.hunks {
            rows.push(at);
            at += 1 + hunk.lines.len();
        }
    }
    rows
}

/// Which hunk the row `row` belongs to (the last header at or above it).
pub fn hunk_at_row(diff: &FileDiff, side_by_side: bool, row: usize) -> Option<usize> {
    hunk_rows(diff, side_by_side)
        .iter()
        .rposition(|start| *start <= row)
}

impl DiffView {
    pub fn new(path: PathBuf, base: BaseRef, id: u64) -> Self {
        Self {
            path,
            base,
            id,
            content: DiffContent::Loading,
            scroll: 0,
            side_by_side: false,
            picker: None,
            side_cache: None,
        }
    }

    /// This view with the side-by-side rows built if that layout is shown.
    pub fn with_cache(self) -> Self {
        let side_cache = match (self.side_by_side, self.diff()) {
            (true, Some(diff)) => Some(Arc::new(side_rows(diff))),
            _ => None,
        };
        Self { side_cache, ..self }
    }

    /// The row where each hunk's header sits in the current layout.
    pub fn hunk_starts(&self) -> Vec<usize> {
        match (&self.side_cache, self.diff()) {
            (Some(rows), _) if self.side_by_side => rows
                .iter()
                .enumerate()
                .filter(|(_, row)| matches!(row, SideRow::Header(_)))
                .map(|(index, _)| index)
                .collect(),
            (_, Some(diff)) => hunk_rows(diff, self.side_by_side),
            _ => Vec::new(),
        }
    }

    pub fn diff(&self) -> Option<&Arc<FileDiff>> {
        match &self.content {
            DiffContent::Ready(diff) => Some(diff),
            _ => None,
        }
    }

    /// Rows in the current layout (0 until the diff has loaded).
    pub fn rows(&self) -> usize {
        match (&self.side_cache, self.diff()) {
            (Some(rows), _) if self.side_by_side => rows.len(),
            (_, Some(diff)) => row_count(diff, self.side_by_side),
            _ => 0,
        }
    }

    /// The hunk at the top of the view.
    pub fn current_hunk(&self) -> Option<usize> {
        self.diff()?;
        self.hunk_starts()
            .iter()
            .rposition(|start| *start <= self.scroll)
    }

    /// Where the change at the top of the view is in the working file.
    pub fn current_line(&self) -> Option<u32> {
        let diff = self.diff()?;
        crate::git::hunk_lines(diff)
            .get(self.current_hunk().unwrap_or(0))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(origin: LineOrigin, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            origin,
            text: text.into(),
            old_no: old,
            new_no: new,
        }
    }

    fn ctx(old: u32, new: u32, text: &str) -> DiffLine {
        line(LineOrigin::Context, Some(old), Some(new), text)
    }
    fn add(new: u32, text: &str) -> DiffLine {
        line(LineOrigin::Added, None, Some(new), text)
    }
    fn del(old: u32, text: &str) -> DiffLine {
        line(LineOrigin::Removed, Some(old), None, text)
    }

    fn hunk(old_start: u32, new_start: u32, lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            old_start,
            old_lines: 0,
            new_start,
            new_lines: 0,
            lines,
        }
    }

    fn two_hunks() -> FileDiff {
        FileDiff {
            hunks: vec![
                hunk(
                    1,
                    1,
                    vec![
                        ctx(1, 1, "a"),
                        del(2, "b"),
                        add(2, "B"),
                        add(3, "B2"),
                        ctx(3, 4, "c"),
                    ],
                ),
                hunk(
                    40,
                    41,
                    vec![ctx(40, 41, "x"), del(41, "y"), ctx(42, 42, "z")],
                ),
            ],
            ..FileDiff::default()
        }
    }

    #[test]
    fn side_by_side_sets_removed_lines_against_added_ones() {
        let rows = side_rows(&two_hunks());

        assert_eq!(rows.len(), 5 + 4);
        assert!(matches!(&rows[0], SideRow::Header(h) if h.starts_with("@@ -1,0 +1,0")));
        let SideRow::Pair { left, right } = &rows[2] else {
            panic!()
        };
        assert_eq!(left.as_ref().map(|l| l.text.as_str()), Some("b"));
        assert_eq!(right.as_ref().map(|l| l.text.as_str()), Some("B"));
        let SideRow::Pair { left, right } = &rows[3] else {
            panic!()
        };
        assert!(left.is_none(), "the extra added line stands alone");
        assert_eq!(right.as_ref().map(|l| l.text.as_str()), Some("B2"));
    }

    #[test]
    fn a_pure_removal_has_nothing_on_the_right() {
        let rows = side_rows(&two_hunks());

        let SideRow::Pair { left, right } = &rows[7] else {
            panic!("{:?}", rows[7])
        };
        assert_eq!(left.as_ref().map(|l| l.text.as_str()), Some("y"));
        assert!(right.is_none());
    }

    #[test]
    fn row_counts_and_hunk_positions_depend_on_the_layout() {
        let diff = two_hunks();

        assert_eq!(row_count(&diff, false), 6 + 4);
        assert_eq!(row_count(&diff, true), 5 + 4);
        assert_eq!(hunk_rows(&diff, false), [0, 6]);
        assert_eq!(hunk_rows(&diff, true), [0, 5]);
    }

    #[test]
    fn the_hunk_at_a_row_is_the_last_header_above_it() {
        let diff = two_hunks();

        assert_eq!(hunk_at_row(&diff, false, 0), Some(0));
        assert_eq!(hunk_at_row(&diff, false, 5), Some(0));
        assert_eq!(hunk_at_row(&diff, false, 6), Some(1));
        assert_eq!(hunk_at_row(&diff, false, 999), Some(1));
        assert_eq!(hunk_at_row(&FileDiff::default(), false, 0), None);
    }

    #[test]
    fn a_view_knows_its_rows_and_where_its_top_change_is_in_the_file() {
        let mut view = DiffView::new(PathBuf::from("a.kt"), BaseRef::head(), 7);
        assert_eq!((view.rows(), view.current_line()), (0, None));

        view.content = DiffContent::Ready(Arc::new(two_hunks()));
        assert_eq!(view.rows(), 10);
        assert_eq!(view.current_line(), Some(2));
        view.scroll = 7;
        assert_eq!(view.current_hunk(), Some(1));
        assert_eq!(view.current_line(), Some(41));
    }

    #[test]
    fn an_empty_diff_has_no_rows_and_no_line() {
        let mut view = DiffView::new(PathBuf::from("a.kt"), BaseRef::head(), 1);
        view.content = DiffContent::Ready(Arc::new(FileDiff::default()));

        assert_eq!(
            (view.rows(), view.current_hunk(), view.current_line()),
            (0, None, None)
        );
    }
}
