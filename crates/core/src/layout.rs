//! Screen geometry shared by the reducer (scrolling) and the ui (drawing).

/// Rows used by the header line.
pub const HEADER_ROWS: u16 = 1;
/// Rows used by the output tabs panel under the main area.
pub const BOTTOM_ROWS: u16 = 8;
/// Rows used by the status line.
pub const STATUS_ROWS: u16 = 1;
/// The main area never shrinks below this.
pub const MIN_BODY_ROWS: u16 = 3;
/// Top and bottom border of a panel.
const BORDER_ROWS: u16 = 2;

/// Height of the row of panels (files, editor, devices).
pub fn body_rows(terminal_height: u16) -> u16 {
    terminal_height
        .saturating_sub(HEADER_ROWS + BOTTOM_ROWS + STATUS_ROWS)
        .max(MIN_BODY_ROWS)
}

/// Rows of the file tree that fit inside its panel.
pub fn files_viewport_rows(terminal_height: u16) -> u16 {
    body_rows(terminal_height)
        .saturating_sub(BORDER_ROWS)
        .max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_takes_what_the_header_output_and_status_leave() {
        assert_eq!(body_rows(30), 20);
        assert_eq!(body_rows(24), 14);
    }

    #[test]
    fn body_never_drops_below_the_minimum() {
        assert_eq!(body_rows(5), MIN_BODY_ROWS);
        assert_eq!(body_rows(0), MIN_BODY_ROWS);
    }

    #[test]
    fn the_tree_viewport_excludes_the_panel_borders() {
        assert_eq!(files_viewport_rows(30), 18);
        assert_eq!(files_viewport_rows(0), 1);
    }
}
