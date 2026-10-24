//! Turning Neovim's `redraw` notifications into [`RedrawEvent`]s.
//!
//! A notification carries batches `[name, args, args, ...]`, one `args` per
//! call. Only grid 1 exists without `ext_multigrid`; events for any other
//! grid, unknown events and malformed arguments are skipped.

use lf_mide_core::screen::{CursorShape, HlAttr, LineCell, RedrawEvent};
use nvim_rs::Value;

/// The only grid Neovim draws on when attached with `ext_linegrid` alone.
const MAIN_GRID: u64 = 1;

/// What one call in a redraw notification means for the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Event(RedrawEvent),
    /// Neovim finished a redraw: the screen is consistent now.
    Flush,
}

/// Every update in the arguments of one `redraw` notification, in order.
pub fn parse(batches: &[Value]) -> Vec<Update> {
    batches
        .iter()
        .filter_map(Value::as_array)
        .flat_map(|batch| {
            let name = batch.first().and_then(Value::as_str).unwrap_or_default();
            batch
                .iter()
                .skip(1)
                .filter_map(Value::as_array)
                .filter_map(move |args| parse_call(name, args))
        })
        .collect()
}

fn parse_call(name: &str, args: &[Value]) -> Option<Update> {
    let on_main_grid = || args.first().and_then(Value::as_u64) == Some(MAIN_GRID);
    let event = match name {
        "flush" => return Some(Update::Flush),
        "grid_resize" if on_main_grid() => RedrawEvent::Resize {
            width: small(args.get(1)?)?,
            height: small(args.get(2)?)?,
        },
        "grid_clear" if on_main_grid() => RedrawEvent::Clear,
        "grid_cursor_goto" if on_main_grid() => RedrawEvent::CursorGoto {
            row: small(args.get(1)?)?,
            col: small(args.get(2)?)?,
        },
        "grid_line" if on_main_grid() => RedrawEvent::Line {
            row: small(args.get(1)?)?,
            col: small(args.get(2)?)?,
            cells: args
                .get(3)?
                .as_array()?
                .iter()
                .filter_map(line_cell)
                .collect(),
        },
        "grid_scroll" if on_main_grid() => RedrawEvent::Scroll {
            top: small(args.get(1)?)?,
            bottom: small(args.get(2)?)?,
            left: small(args.get(3)?)?,
            right: small(args.get(4)?)?,
            rows: args.get(5)?.as_i64()?,
        },
        "hl_attr_define" => RedrawEvent::HlAttrDefine {
            id: u32::try_from(args.first()?.as_u64()?).ok()?,
            attr: hl_attr(args.get(1)?),
        },
        "default_colors_set" => RedrawEvent::DefaultColors {
            fg: args.first().and_then(color),
            bg: args.get(1).and_then(color),
        },
        "mode_info_set" => {
            RedrawEvent::ModeInfo(args.get(1)?.as_array()?.iter().map(cursor_shape).collect())
        }
        "mode_change" => RedrawEvent::ModeChange(usize::try_from(args.get(1)?.as_u64()?).ok()?),
        "busy_start" => RedrawEvent::BusyStart,
        "busy_stop" => RedrawEvent::BusyStop,
        _ => return None,
    };
    Some(Update::Event(event))
}

/// A grid coordinate or size; larger values saturate (the screen clamps).
fn small(value: &Value) -> Option<u16> {
    Some(u16::try_from(value.as_u64()?).unwrap_or(u16::MAX))
}

/// `[text, hl_id?, repeat?]`.
fn line_cell(value: &Value) -> Option<LineCell> {
    let parts = value.as_array()?;
    let text = parts.first()?.as_str()?.to_string();
    let hl = parts
        .get(1)
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok());
    let repeat = parts
        .get(2)
        .and_then(Value::as_u64)
        .map_or(1, |n| usize::try_from(n).unwrap_or(usize::MAX));
    Some(LineCell { text, hl, repeat })
}

/// A 24-bit color; Neovim sends -1 for "not set".
fn color(value: &Value) -> Option<u32> {
    u32::try_from(value.as_i64()?)
        .ok()
        .filter(|rgb| *rgb <= 0xff_ffff)
}

fn entry<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(name, _)| name.as_str() == Some(key))
        .map(|(_, value)| value)
}

fn hl_attr(value: &Value) -> HlAttr {
    let map = value.as_map().map(Vec::as_slice).unwrap_or_default();
    let flag = |key| entry(map, key).and_then(Value::as_bool).unwrap_or(false);
    HlAttr {
        fg: entry(map, "foreground").and_then(color),
        bg: entry(map, "background").and_then(color),
        special: entry(map, "special").and_then(color),
        reverse: flag("reverse"),
        bold: flag("bold"),
        italic: flag("italic"),
        underline: flag("underline"),
        undercurl: flag("undercurl"),
        strikethrough: flag("strikethrough"),
    }
}

fn cursor_shape(mode: &Value) -> CursorShape {
    let map = mode.as_map().map(Vec::as_slice).unwrap_or_default();
    match entry(map, "cursor_shape").and_then(Value::as_str) {
        Some("vertical") => CursorShape::Vertical,
        Some("horizontal") => CursorShape::Horizontal,
        _ => CursorShape::Block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v<T: Into<Value>>(x: T) -> Value {
        x.into()
    }

    fn batch(name: &str, calls: Vec<Vec<Value>>) -> Value {
        let mut items = vec![v(name)];
        items.extend(calls.into_iter().map(Value::Array));
        Value::Array(items)
    }

    fn map(pairs: Vec<(&str, Value)>) -> Value {
        Value::Map(pairs.into_iter().map(|(k, val)| (v(k), val)).collect())
    }

    #[test]
    fn grid_events_on_the_main_grid_are_parsed_in_order() {
        let updates = parse(&[
            batch("grid_resize", vec![vec![v(1), v(80), v(24)]]),
            batch(
                "grid_line",
                vec![vec![
                    v(1),
                    v(2),
                    v(3),
                    Value::Array(vec![
                        Value::Array(vec![v("a"), v(5), v(3)]),
                        Value::Array(vec![v("b")]),
                    ]),
                    v(false),
                ]],
            ),
            batch("grid_cursor_goto", vec![vec![v(1), v(4), v(7)]]),
            batch(
                "grid_scroll",
                vec![vec![v(1), v(0), v(10), v(0), v(80), v(-2), v(0)]],
            ),
            batch("grid_clear", vec![vec![v(1)]]),
            batch("flush", vec![vec![]]),
        ]);

        assert_eq!(
            updates,
            vec![
                Update::Event(RedrawEvent::Resize {
                    width: 80,
                    height: 24
                }),
                Update::Event(RedrawEvent::Line {
                    row: 2,
                    col: 3,
                    cells: vec![
                        LineCell {
                            text: "a".into(),
                            hl: Some(5),
                            repeat: 3
                        },
                        LineCell {
                            text: "b".into(),
                            hl: None,
                            repeat: 1
                        },
                    ],
                }),
                Update::Event(RedrawEvent::CursorGoto { row: 4, col: 7 }),
                Update::Event(RedrawEvent::Scroll {
                    top: 0,
                    bottom: 10,
                    left: 0,
                    right: 80,
                    rows: -2
                }),
                Update::Event(RedrawEvent::Clear),
                Update::Flush,
            ]
        );
    }

    #[test]
    fn several_calls_in_one_batch_each_become_an_update() {
        let updates = parse(&[batch(
            "grid_cursor_goto",
            vec![vec![v(1), v(0), v(1)], vec![v(1), v(0), v(2)]],
        )]);

        assert_eq!(updates.len(), 2);
    }

    #[test]
    fn other_grids_unknown_events_and_bad_arguments_are_skipped() {
        let updates = parse(&[
            batch("grid_resize", vec![vec![v(2), v(10), v(10)]]),
            batch("win_viewport", vec![vec![v(1)]]),
            batch("grid_cursor_goto", vec![vec![v(1), v("x"), v(1)]]),
            batch("grid_line", vec![vec![v(1), v(0)]]),
            v("not a batch"),
        ]);

        assert!(updates.is_empty(), "{updates:?}");
    }

    #[test]
    fn huge_coordinates_saturate_instead_of_wrapping() {
        let updates = parse(&[batch(
            "grid_resize",
            vec![vec![v(1), v(u64::MAX), v(70_000u64)]],
        )]);

        assert_eq!(
            updates,
            vec![Update::Event(RedrawEvent::Resize {
                width: u16::MAX,
                height: u16::MAX
            })]
        );
    }

    #[test]
    fn highlights_read_colors_and_flags() {
        let updates = parse(&[batch(
            "hl_attr_define",
            vec![vec![
                v(9),
                map(vec![
                    ("foreground", v(0x112233)),
                    ("background", v(-1)),
                    ("bold", v(true)),
                    ("reverse", v(true)),
                    ("undercurl", v(true)),
                    ("special", v(0xff)),
                ]),
                map(vec![]),
                Value::Array(vec![]),
            ]],
        )]);

        assert_eq!(
            updates,
            vec![Update::Event(RedrawEvent::HlAttrDefine {
                id: 9,
                attr: HlAttr {
                    fg: Some(0x112233),
                    bg: None,
                    special: Some(0xff),
                    reverse: true,
                    bold: true,
                    undercurl: true,
                    ..HlAttr::default()
                },
            })]
        );
    }

    #[test]
    fn default_colors_treat_minus_one_as_unset() {
        let updates = parse(&[batch(
            "default_colors_set",
            vec![vec![v(-1), v(0x000000), v(-1), v(-1), v(-1)]],
        )]);

        assert_eq!(
            updates,
            vec![Update::Event(RedrawEvent::DefaultColors {
                fg: None,
                bg: Some(0)
            })]
        );
    }

    #[test]
    fn modes_give_cursor_shapes_and_mode_change_the_index() {
        let updates = parse(&[
            batch(
                "mode_info_set",
                vec![vec![
                    v(true),
                    Value::Array(vec![
                        map(vec![("cursor_shape", v("block"))]),
                        map(vec![("cursor_shape", v("vertical"))]),
                        map(vec![("cursor_shape", v("horizontal"))]),
                        map(vec![]),
                    ]),
                ]],
            ),
            batch("mode_change", vec![vec![v("insert"), v(1)]]),
            batch("busy_start", vec![vec![]]),
            batch("busy_stop", vec![vec![]]),
        ]);

        assert_eq!(
            updates,
            vec![
                Update::Event(RedrawEvent::ModeInfo(vec![
                    CursorShape::Block,
                    CursorShape::Vertical,
                    CursorShape::Horizontal,
                    CursorShape::Block,
                ])),
                Update::Event(RedrawEvent::ModeChange(1)),
                Update::Event(RedrawEvent::BusyStart),
                Update::Event(RedrawEvent::BusyStop),
            ]
        );
    }
}
