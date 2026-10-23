//! Phase 0 spike: embed Neovim (`nvim --embed`), attach as an `ext_linegrid`
//! UI, apply redraw events to a cell grid, and render that grid into a
//! ratatui `Buffer`. Runs headless and prints the result so it can be checked
//! without an interactive terminal.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nvim_rs::{Handler, Neovim, UiAttachOptions, Value, compat::tokio::Compat};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use tokio::process::{ChildStdin, Command};
use tokio::sync::Notify;

const WIDTH: u16 = 60;
const HEIGHT: u16 = 8;
const FLUSH_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
struct Highlight {
    fg: Option<u32>,
    bg: Option<u32>,
}

#[derive(Default)]
struct Grid {
    width: usize,
    height: usize,
    cells: Vec<(String, u64)>,
    cursor: (usize, usize),
    highlights: HashMap<u64, Highlight>,
    default_fg: u32,
    default_bg: u32,
}

impl Grid {
    fn resize(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.cells = vec![(" ".to_string(), 0); width * height];
    }

    fn clear(&mut self) {
        self.cells
            .iter_mut()
            .for_each(|c| *c = (" ".to_string(), 0));
    }

    /// `grid_line`: cells are `[text, hl_id?, repeat?]`; an omitted hl_id
    /// reuses the previous cell's.
    fn line(&mut self, row: usize, col_start: usize, cells: &[Value]) {
        let mut col = col_start;
        let mut hl = 0;
        for cell in cells {
            let Some(parts) = cell.as_array() else {
                continue;
            };
            let text = parts.first().and_then(Value::as_str).unwrap_or(" ");
            if let Some(id) = parts.get(1).and_then(Value::as_u64) {
                hl = id;
            }
            let repeat = parts.get(2).and_then(Value::as_u64).unwrap_or(1);
            for _ in 0..repeat {
                if row < self.height && col < self.width {
                    self.cells[row * self.width + col] = (text.to_string(), hl);
                }
                col += 1;
            }
        }
    }

    /// `grid_scroll` with rows > 0 moves content up, rows < 0 moves it down.
    fn scroll(&mut self, top: usize, bot: usize, left: usize, right: usize, rows: i64) {
        let copy_row = |grid: &mut Grid, dst: usize, src: usize| {
            for col in left..right {
                grid.cells[dst * grid.width + col] = grid.cells[src * grid.width + col].clone();
            }
        };
        if rows > 0 {
            for row in top..(bot as i64 - rows) as usize {
                copy_row(self, row, row + rows as usize);
            }
        } else {
            for row in ((top as i64 - rows) as usize..bot).rev() {
                copy_row(self, row, (row as i64 + rows) as usize);
            }
        }
    }

    fn render(&self, area: Rect) -> Buffer {
        let mut buf = Buffer::empty(area);
        for row in 0..self.height.min(area.height as usize) {
            for col in 0..self.width.min(area.width as usize) {
                let (text, hl_id) = &self.cells[row * self.width + col];
                let hl = self.highlights.get(hl_id).cloned().unwrap_or_default();
                let style = Style::default()
                    .fg(rgb(hl.fg.unwrap_or(self.default_fg)))
                    .bg(rgb(hl.bg.unwrap_or(self.default_bg)));
                if let Some(cell) = buf.cell_mut((col as u16, row as u16)) {
                    cell.set_symbol(text).set_style(style);
                }
            }
        }
        buf
    }
}

fn rgb(color: u32) -> Color {
    Color::Rgb((color >> 16) as u8, (color >> 8) as u8, color as u8)
}

fn arg(args: &[Value], i: usize) -> usize {
    args.get(i).and_then(Value::as_i64).unwrap_or(0) as usize
}

#[derive(Clone)]
struct UiHandler {
    grid: Arc<Mutex<Grid>>,
    flushed: Arc<Notify>,
}

impl UiHandler {
    fn apply(&self, event: &str, args: &[Value]) {
        let mut grid = self.grid.lock().expect("grid lock poisoned");
        match event {
            "grid_resize" => grid.resize(arg(args, 1), arg(args, 2)),
            "grid_clear" => grid.clear(),
            "grid_cursor_goto" => grid.cursor = (arg(args, 1), arg(args, 2)),
            "grid_line" => {
                let cells = args
                    .get(3)
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                grid.line(arg(args, 1), arg(args, 2), &cells);
            }
            "grid_scroll" => {
                let rows = args.get(5).and_then(Value::as_i64).unwrap_or(0);
                grid.scroll(arg(args, 1), arg(args, 2), arg(args, 3), arg(args, 4), rows);
            }
            "default_colors_set" => {
                grid.default_fg = arg(args, 0) as u32;
                grid.default_bg = arg(args, 1) as u32;
            }
            "hl_attr_define" => {
                let id = arg(args, 0) as u64;
                let attrs = args
                    .get(1)
                    .and_then(Value::as_map)
                    .cloned()
                    .unwrap_or_default();
                let color = |key: &str| {
                    attrs
                        .iter()
                        .find(|(k, _)| k.as_str() == Some(key))
                        .and_then(|(_, v)| v.as_u64())
                        .map(|v| v as u32)
                };
                let hl = Highlight {
                    fg: color("foreground"),
                    bg: color("background"),
                };
                grid.highlights.insert(id, hl);
            }
            _ => {}
        }
    }
}

#[async_trait]
impl Handler for UiHandler {
    type Writer = Compat<ChildStdin>;

    async fn handle_notify(&self, name: String, args: Vec<Value>, _nvim: Neovim<Self::Writer>) {
        if name != "redraw" {
            return;
        }
        // Each redraw arg is `[event_name, call_args_1, call_args_2, ...]`.
        for batch in args.iter().filter_map(Value::as_array) {
            let Some(event) = batch.first().and_then(Value::as_str) else {
                continue;
            };
            for call in batch[1..].iter().filter_map(Value::as_array) {
                self.apply(event, call);
            }
            if event == "flush" {
                self.flushed.notify_one();
            }
        }
    }
}

async fn wait_flush(flushed: &Notify) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(FLUSH_TIMEOUT, flushed.notified()).await?;
    // Let any trailing batches land before reading the grid.
    tokio::time::sleep(Duration::from_millis(200)).await;
    Ok(())
}

fn print_buffer(title: &str, buf: &Buffer, grid: &Grid) {
    println!(
        "--- {title} (cursor row {}, col {}) ---",
        grid.cursor.0, grid.cursor.1
    );
    for y in 0..buf.area.height {
        let line: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
        println!("|{line}|");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let grid = Arc::new(Mutex::new(Grid::default()));
    let flushed = Arc::new(Notify::new());
    let handler = UiHandler {
        grid: grid.clone(),
        flushed: flushed.clone(),
    };

    let mut cmd = Command::new("nvim");
    cmd.args(["--embed", "--clean"]);
    let (nvim, _io, mut child) = nvim_rs::create::tokio::new_child_cmd(&mut cmd, handler).await?;

    let mut opts = UiAttachOptions::new();
    opts.set_rgb(true).set_linegrid_external(true);
    nvim.ui_attach(WIDTH.into(), HEIGHT.into(), &opts).await?;
    wait_flush(&flushed).await?;

    nvim.input("ihello from LF-Mide<CR>embedded neovim works<Esc>")
        .await?;
    wait_flush(&flushed).await?;
    {
        let grid = grid.lock().expect("grid lock poisoned");
        let buf = grid.render(Rect::new(0, 0, WIDTH, HEIGHT));
        print_buffer("after insert", &buf, &grid);
        println!("highlight groups received: {}", grid.highlights.len());
    }

    nvim.ui_try_resize(40, 5).await?;
    wait_flush(&flushed).await?;
    {
        let grid = grid.lock().expect("grid lock poisoned");
        let buf = grid.render(Rect::new(0, 0, 40, 5));
        print_buffer("after resize 40x5", &buf, &grid);
    }

    let _ = nvim.command("qa!").await;
    child.kill().await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_line_applies_repeat_and_carries_highlight() {
        let mut grid = Grid::default();
        grid.resize(5, 1);
        let cells = vec![
            Value::from(vec![Value::from("a"), Value::from(7)]),
            Value::from(vec![Value::from("b"), Value::from(7), Value::from(2)]),
            Value::from(vec![Value::from("c")]),
        ];

        grid.line(0, 0, &cells);

        let row: Vec<_> = grid.cells.iter().map(|(t, h)| (t.as_str(), *h)).collect();
        assert_eq!(row, vec![("a", 7), ("b", 7), ("b", 7), ("c", 7), (" ", 0)]);
    }

    #[test]
    fn grid_scroll_up_moves_rows() {
        let mut grid = Grid::default();
        grid.resize(1, 3);
        for (row, text) in ["1", "2", "3"].iter().enumerate() {
            grid.line(row, 0, &[Value::from(vec![Value::from(*text)])]);
        }

        grid.scroll(0, 3, 0, 1, 1);

        let col: Vec<_> = grid.cells.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(col, vec!["2", "3", "3"]);
    }
}
