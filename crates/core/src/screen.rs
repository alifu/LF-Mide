//! What the embedded Neovim shows: a grid of cells with highlights and a
//! cursor, kept up to date from its `ext_linegrid` redraw events.
//!
//! Rows are shared (`Arc`) and copied only when changed, so handing a
//! snapshot to the view after every `flush` costs a few pointer copies, and
//! a snapshot never changes once it has been handed out.

use std::collections::HashMap;
use std::sync::Arc;

use crate::text::{REPLACEMENT, is_hidden};

/// Larger grids are refused: Neovim only ever gets the size we ask for, so
/// anything bigger is a bug and must not allocate without bound.
pub const MAX_COLUMNS: u16 = 1000;
pub const MAX_ROWS: u16 = 500;

/// One cell: a single character (or grapheme), empty for the right half of
/// a double-width character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub hl: u32,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            text: " ".to_string(),
            hl: 0,
        }
    }
}

/// How a highlight id draws, from `hl_attr_define`. Colors are 0xRRGGBB.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HlAttr {
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub special: Option<u32>,
    pub reverse: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub undercurl: bool,
    pub strikethrough: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    /// A bar, as in insert mode.
    Vertical,
    /// An underline, as in replace mode.
    Horizontal,
}

/// One cell of a `grid_line` event: `repeat` copies of `text`; `hl` absent
/// means "the same as the cell before".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineCell {
    pub text: String,
    pub hl: Option<u32>,
    pub repeat: usize,
}

/// The redraw events the screen understands. Anything else is ignored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedrawEvent {
    Resize {
        width: u16,
        height: u16,
    },
    Clear,
    CursorGoto {
        row: u16,
        col: u16,
    },
    Line {
        row: u16,
        col: u16,
        cells: Vec<LineCell>,
    },
    /// The region `top..bottom` x `left..right` moves up by `rows` (down
    /// when negative).
    Scroll {
        top: u16,
        bottom: u16,
        left: u16,
        right: u16,
        rows: i64,
    },
    HlAttrDefine {
        id: u32,
        attr: HlAttr,
    },
    DefaultColors {
        fg: Option<u32>,
        bg: Option<u32>,
    },
    /// Cursor shape per mode, indexed by the mode index of `ModeChange`.
    ModeInfo(Vec<CursorShape>),
    ModeChange(usize),
    BusyStart,
    BusyStop,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Screen {
    pub width: u16,
    pub height: u16,
    rows: Vec<Arc<Vec<Cell>>>,
    /// Row and column of the cursor.
    pub cursor: (u16, u16),
    highlights: Arc<HashMap<u32, HlAttr>>,
    pub default_fg: Option<u32>,
    pub default_bg: Option<u32>,
    modes: Arc<Vec<CursorShape>>,
    mode: usize,
    /// Neovim is busy and has hidden the cursor.
    pub busy: bool,
}

impl Screen {
    /// The screen after `event`.
    pub fn apply(self, event: RedrawEvent) -> Screen {
        match event {
            RedrawEvent::Resize { width, height } => self.resized(width, height),
            RedrawEvent::Clear => Screen {
                rows: blank_rows(self.width, self.height),
                ..self
            },
            RedrawEvent::CursorGoto { row, col } => Screen {
                cursor: (
                    row.min(self.height.saturating_sub(1)),
                    col.min(self.width.saturating_sub(1)),
                ),
                ..self
            },
            RedrawEvent::Line { row, col, cells } => self.line(row, col, &cells),
            RedrawEvent::Scroll {
                top,
                bottom,
                left,
                right,
                rows,
            } => self.scrolled(top, bottom, left, right, rows),
            RedrawEvent::HlAttrDefine { id, attr } => {
                let mut highlights = self.highlights;
                Arc::make_mut(&mut highlights).insert(id, attr);
                Screen { highlights, ..self }
            }
            RedrawEvent::DefaultColors { fg, bg } => Screen {
                default_fg: fg,
                default_bg: bg,
                ..self
            },
            RedrawEvent::ModeInfo(shapes) => Screen {
                modes: Arc::new(shapes),
                ..self
            },
            RedrawEvent::ModeChange(mode) => Screen { mode, ..self },
            RedrawEvent::BusyStart => Screen { busy: true, ..self },
            RedrawEvent::BusyStop => Screen {
                busy: false,
                ..self
            },
        }
    }

    /// The screen after every event in `events`, in order.
    pub fn apply_all(self, events: impl IntoIterator<Item = RedrawEvent>) -> Screen {
        events.into_iter().fold(self, Screen::apply)
    }

    pub fn cell(&self, row: u16, col: u16) -> Option<&Cell> {
        self.rows.get(usize::from(row))?.get(usize::from(col))
    }

    pub fn highlight(&self, id: u32) -> Option<&HlAttr> {
        self.highlights.get(&id)
    }

    pub fn cursor_shape(&self) -> CursorShape {
        self.modes.get(self.mode).copied().unwrap_or_default()
    }

    /// One row as plain text (wide characters once), for tests and logs.
    pub fn row_text(&self, row: u16) -> String {
        self.rows
            .get(usize::from(row))
            .map(|cells| cells.iter().map(|cell| cell.text.as_str()).collect())
            .unwrap_or_default()
    }

    fn resized(self, width: u16, height: u16) -> Screen {
        let width = width.min(MAX_COLUMNS);
        let height = height.min(MAX_ROWS);
        // Keep what fits: Neovim redraws after a resize, but not always all of it.
        let rows = (0..height)
            .map(|row| {
                let old = self.rows.get(usize::from(row));
                Arc::new(
                    (0..width)
                        .map(|col| {
                            old.and_then(|cells| cells.get(usize::from(col)))
                                .cloned()
                                .unwrap_or_default()
                        })
                        .collect(),
                )
            })
            .collect();
        Screen {
            width,
            height,
            rows,
            cursor: (
                self.cursor.0.min(height.saturating_sub(1)),
                self.cursor.1.min(width.saturating_sub(1)),
            ),
            ..self
        }
    }

    fn line(self, row: u16, col: u16, cells: &[LineCell]) -> Screen {
        let Some(target) = self.rows.get(usize::from(row)) else {
            return self;
        };
        let mut rows = self.rows.clone();
        let mut updated = Vec::clone(target);
        let width = updated.len();
        let mut at = usize::from(col);
        let mut hl = 0;
        for cell in cells {
            hl = cell.hl.unwrap_or(hl);
            let text = printable_cell(&cell.text);
            // Never write past the row, however large `repeat` claims to be.
            let count = cell.repeat.min(width.saturating_sub(at));
            for slot in updated.iter_mut().skip(at).take(count) {
                *slot = Cell {
                    text: text.clone(),
                    hl,
                };
            }
            at += count;
        }
        rows[usize::from(row)] = Arc::new(updated);
        Screen { rows, ..self }
    }

    fn scrolled(self, top: u16, bottom: u16, left: u16, right: u16, rows: i64) -> Screen {
        let bottom = usize::from(bottom.min(self.height));
        let right = usize::from(right.min(self.width));
        let (top, left) = (usize::from(top), usize::from(left));
        if top >= bottom || left >= right || rows == 0 {
            return self;
        }
        let span = bottom - top;
        let Ok(distance) = usize::try_from(rows.unsigned_abs()) else {
            return self;
        };
        if distance >= span {
            // Everything moved out: Neovim redraws the region itself.
            return self;
        }
        let mut grid = self.rows.clone();
        let moves: Vec<(usize, usize)> = if rows > 0 {
            (top..bottom - distance)
                .map(|dst| (dst, dst + distance))
                .collect()
        } else {
            (top + distance..bottom)
                .rev()
                .map(|dst| (dst, dst - distance))
                .collect()
        };
        for (dst, src) in moves {
            let source = Arc::clone(&self.rows[src]);
            let row = Arc::make_mut(&mut grid[dst]);
            row[left..right].clone_from_slice(&source[left..right]);
        }
        Screen { rows: grid, ..self }
    }
}

fn blank_rows(width: u16, height: u16) -> Vec<Arc<Vec<Cell>>> {
    (0..height)
        .map(|_| Arc::new(vec![Cell::default(); usize::from(width)]))
        .collect()
}

/// A cell's text, or [`REPLACEMENT`] if it holds anything that must not
/// reach the terminal as it is (an escape sequence, a direction override).
/// The zero-width joiner stays: it is what glues an emoji sequence together.
fn printable_cell(text: &str) -> String {
    let unsafe_char = |ch: char| ch.is_control() || (is_hidden(ch) && ch != '\u{200D}');
    if text.chars().any(unsafe_char) {
        REPLACEMENT.to_string()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str, hl: Option<u32>, repeat: usize) -> LineCell {
        LineCell {
            text: text.to_string(),
            hl,
            repeat,
        }
    }

    fn screen(width: u16, height: u16) -> Screen {
        Screen::default().apply(RedrawEvent::Resize { width, height })
    }

    fn line(row: u16, col: u16, cells: Vec<LineCell>) -> RedrawEvent {
        RedrawEvent::Line { row, col, cells }
    }

    fn with_rows(width: u16, rows: &[&str]) -> Screen {
        let height = u16::try_from(rows.len()).unwrap();
        let events = rows.iter().enumerate().map(|(row, text)| {
            let cells = text
                .chars()
                .map(|ch| cell(&ch.to_string(), None, 1))
                .collect();
            line(u16::try_from(row).unwrap(), 0, cells)
        });
        screen(width, height).apply_all(events.collect::<Vec<_>>())
    }

    #[test]
    fn a_resized_screen_is_blank_and_of_that_size() {
        let s = screen(4, 2);

        assert_eq!((s.width, s.height), (4, 2));
        assert_eq!(s.row_text(0), "    ");
        assert_eq!(s.row_text(1), "    ");
        assert_eq!(s.cell(2, 0), None);
    }

    #[test]
    fn a_line_repeats_cells_and_carries_the_highlight_forward() {
        let s = screen(6, 1).apply(line(
            0,
            1,
            vec![
                cell("a", Some(7), 1),
                cell("b", None, 2),
                cell("c", Some(3), 1),
            ],
        ));

        assert_eq!(s.row_text(0), " abbc ");
        let hls: Vec<u32> = (0..6).map(|col| s.cell(0, col).unwrap().hl).collect();
        assert_eq!(hls, vec![0, 7, 7, 7, 3, 0]);
    }

    #[test]
    fn a_line_never_writes_past_the_row_or_outside_the_grid() {
        let s = screen(3, 1)
            .apply(line(0, 2, vec![cell("x", None, usize::MAX)]))
            .apply(line(5, 0, vec![cell("y", None, 1)]));

        assert_eq!(s.row_text(0), "  x");
    }

    #[test]
    fn scrolling_up_and_down_moves_only_the_region() {
        let up = with_rows(2, &["1a", "2b", "3c", "4d"]).apply(RedrawEvent::Scroll {
            top: 0,
            bottom: 3,
            left: 0,
            right: 1,
            rows: 1,
        });
        let down = with_rows(2, &["1a", "2b", "3c", "4d"]).apply(RedrawEvent::Scroll {
            top: 1,
            bottom: 4,
            left: 0,
            right: 2,
            rows: -2,
        });

        let text = |s: &Screen| (0..4).map(|r| s.row_text(r)).collect::<Vec<_>>();
        // Column 1 and row 3 are outside the region and stay.
        assert_eq!(text(&up), vec!["2a", "3b", "3c", "4d"]);
        assert_eq!(text(&down), vec!["1a", "2b", "3c", "2b"]);
    }

    #[test]
    fn out_of_range_scrolls_change_nothing() {
        let before = with_rows(2, &["ab", "cd"]);
        for (top, bottom, rows) in [(0, 2, 2), (0, 2, i64::MIN), (5, 9, 1), (1, 1, 1)] {
            let after = before.clone().apply(RedrawEvent::Scroll {
                top,
                bottom,
                left: 0,
                right: 2,
                rows,
            });
            assert_eq!(after, before, "top {top} bottom {bottom} rows {rows}");
        }
    }

    #[test]
    fn a_snapshot_does_not_change_when_the_screen_does() {
        let snapshot = with_rows(2, &["ab", "cd"]);

        let next = snapshot.clone().apply(line(0, 0, vec![cell("z", None, 1)]));

        assert_eq!(snapshot.row_text(0), "ab");
        assert_eq!(next.row_text(0), "zb");
        assert!(
            Arc::ptr_eq(&snapshot.rows[1], &next.rows[1]),
            "untouched rows are shared, not copied"
        );
    }

    #[test]
    fn resizing_keeps_what_fits_and_clamps_the_cursor() {
        let s = with_rows(3, &["abc", "def"])
            .apply(RedrawEvent::CursorGoto { row: 1, col: 2 })
            .apply(RedrawEvent::Resize {
                width: 2,
                height: 1,
            });

        assert_eq!(s.row_text(0), "ab");
        assert_eq!(s.cursor, (0, 1));
    }

    #[test]
    fn absurd_sizes_are_clamped() {
        let s = screen(u16::MAX, u16::MAX);

        assert_eq!((s.width, s.height), (MAX_COLUMNS, MAX_ROWS));
    }

    #[test]
    fn clear_blanks_every_cell() {
        let s = with_rows(2, &["ab"]).apply(RedrawEvent::Clear);

        assert_eq!(s.row_text(0), "  ");
    }

    #[test]
    fn the_cursor_stays_on_the_grid() {
        let s = screen(4, 2).apply(RedrawEvent::CursorGoto { row: 9, col: 9 });

        assert_eq!(s.cursor, (1, 3));
    }

    #[test]
    fn escape_sequences_and_direction_overrides_never_reach_a_cell() {
        let s = screen(4, 1).apply(line(
            0,
            0,
            vec![
                cell("\u{1b}[2J", None, 1),
                cell("\u{202E}", None, 1),
                cell("👨\u{200D}👩", None, 1),
                cell("é", None, 1),
            ],
        ));

        assert_eq!(s.row_text(0), "\u{FFFD}\u{FFFD}👨\u{200D}👩é");
    }

    #[test]
    fn highlights_colors_and_cursor_shape_are_remembered() {
        let attr = HlAttr {
            fg: Some(0xff0000),
            bold: true,
            ..HlAttr::default()
        };
        let s = screen(1, 1).apply_all([
            RedrawEvent::HlAttrDefine {
                id: 4,
                attr: attr.clone(),
            },
            RedrawEvent::DefaultColors {
                fg: Some(1),
                bg: Some(2),
            },
            RedrawEvent::ModeInfo(vec![CursorShape::Block, CursorShape::Vertical]),
            RedrawEvent::ModeChange(1),
            RedrawEvent::BusyStart,
        ]);

        assert_eq!(s.highlight(4), Some(&attr));
        assert_eq!((s.default_fg, s.default_bg), (Some(1), Some(2)));
        assert_eq!(s.cursor_shape(), CursorShape::Vertical);
        assert!(s.busy);
        assert!(!s.apply(RedrawEvent::BusyStop).busy);
    }

    #[test]
    fn an_unknown_mode_index_has_a_block_cursor() {
        let s = screen(1, 1).apply(RedrawEvent::ModeChange(42));

        assert_eq!(s.cursor_shape(), CursorShape::Block);
    }
}
