//! SVG strokes and the text layer of a classified grid.
//!
//! The SVG holds only `<path>` elements with a fixed class and a `d`
//! attribute: lines, rounded corners and diagonals are stroked, arrowheads
//! are filled. Characters that are not drawn stay as HTML text in a layer
//! over the SVG, on the same grid, so they remain selectable and findable.

use std::fmt::Write as _;

use super::grid::{EAST, Grid, Mark, Marks, NORTH, SOUTH, Stroke, WEST};
use crate::text::html_encode;

/// SVG units per cell. The page draws one cell as `1ch` by `2ch`.
pub(super) const CELL_WIDTH: usize = 8;
pub(super) const CELL_HEIGHT: usize = 16;

/// Offset of each line of a double stroke from the cell centre line.
const DOUBLE_OFFSET: usize = 2;

pub(super) struct Drawing {
    pub svg: String,
    /// The text layer: one line per grid row, drawn cells blanked, already
    /// entity-encoded.
    pub text: String,
}

const STYLES: [(Stroke, &str); 4] = [
    (Stroke::Light, "tdg-line"),
    (Stroke::Heavy, "tdg-heavy"),
    (Stroke::Double, "tdg-double"),
    (Stroke::Dashed, "tdg-dashed"),
];

fn style_index(stroke: Stroke) -> u8 {
    match stroke {
        Stroke::Light => 1,
        Stroke::Heavy => 2,
        Stroke::Double => 3,
        Stroke::Dashed => 4,
    }
}

/// Half-cell arm coverage: a horizontal half per side of each cell in each
/// row, and a vertical half per side of each cell in each column. Each half
/// holds a style index (0: none). Contiguous halves become one segment.
struct Halves {
    columns: usize,
    rows: usize,
    horizontal: Vec<u8>,
    vertical: Vec<u8>,
}

impl Halves {
    fn new(rows: usize, columns: usize) -> Self {
        Self {
            columns,
            rows,
            horizontal: vec![0; rows * columns * 2],
            vertical: vec![0; rows * columns * 2],
        }
    }

    fn set(&mut self, row: usize, column: usize, arms: u8, style: u8) {
        let h = row * self.columns * 2 + column * 2;
        let v = column * self.rows * 2 + row * 2;
        if arms & WEST != 0 {
            self.horizontal[h] = style;
        }
        if arms & EAST != 0 {
            self.horizontal[h + 1] = style;
        }
        if arms & NORTH != 0 {
            self.vertical[v] = style;
        }
        if arms & SOUTH != 0 {
            self.vertical[v + 1] = style;
        }
    }
}

/// Runs of equal non-zero values in `halves`: `(style, start, end)`.
fn runs(halves: &[u8]) -> impl Iterator<Item = (u8, usize, usize)> + '_ {
    let mut index = 0;
    std::iter::from_fn(move || {
        while index < halves.len() && halves[index] == 0 {
            index += 1;
        }
        if index == halves.len() {
            return None;
        }
        let style = halves[index];
        let start = index;
        while index < halves.len() && halves[index] == style {
            index += 1;
        }
        Some((style, start, index))
    })
}

pub(super) fn draw(grid: &Grid, marks: &Marks) -> Drawing {
    let rows = grid.rows.len();
    let columns = grid.columns.max(1);
    let mut halves = Halves::new(rows, columns);
    // Path data per style (index 1 to 4), plus filled arrowheads.
    let mut paths: [String; 5] = Default::default();
    let mut heads = String::new();
    for (r, row) in marks.marks.iter().enumerate() {
        for (c, mark) in row.iter().enumerate() {
            let x = c * CELL_WIDTH;
            let y = r * CELL_HEIGHT;
            let (cx, cy) = (x + CELL_WIDTH / 2, y + CELL_HEIGHT / 2);
            match *mark {
                Mark::Text => {}
                Mark::Arms(arms, stroke) => halves.set(r, c, arms, style_index(stroke)),
                Mark::Rounded(arms) => {
                    let light = &mut paths[1];
                    for horizontal in [EAST, WEST] {
                        if arms & horizontal == 0 {
                            continue;
                        }
                        let hx = if horizontal == EAST {
                            x + CELL_WIDTH
                        } else {
                            x
                        };
                        for vertical in [NORTH, SOUTH] {
                            if arms & vertical != 0 {
                                let vy = if vertical == SOUTH {
                                    y + CELL_HEIGHT
                                } else {
                                    y
                                };
                                let _ = write!(light, "M{hx} {cy}Q{cx} {cy} {cx} {vy}");
                            }
                        }
                    }
                }
                Mark::Diagonal(kind) => {
                    let light = &mut paths[1];
                    let (right, bottom) = (x + CELL_WIDTH, y + CELL_HEIGHT);
                    if kind & 1 != 0 {
                        let _ = write!(light, "M{x} {bottom}L{right} {y}");
                    }
                    if kind & 2 != 0 {
                        let _ = write!(light, "M{x} {y}L{right} {bottom}");
                    }
                }
                Mark::Head(direction) => {
                    halves.set(r, c, mark.arms(), 1);
                    let (right, bottom) = (x + CELL_WIDTH, y + CELL_HEIGHT);
                    let _ = match direction {
                        EAST => write!(heads, "M{right} {cy}L{x} {}L{x} {}Z", cy - 4, cy + 4),
                        WEST => write!(heads, "M{x} {cy}L{right} {}L{right} {}Z", cy - 4, cy + 4),
                        SOUTH => write!(
                            heads,
                            "M{cx} {}L{} {}L{} {}Z",
                            bottom - 2,
                            cx - 4,
                            y + 6,
                            cx + 4,
                            y + 6
                        ),
                        _ => write!(
                            heads,
                            "M{cx} {}L{} {}L{} {}Z",
                            y + 2,
                            cx - 4,
                            y + 10,
                            cx + 4,
                            y + 10
                        ),
                    };
                }
            }
        }
    }
    let half_width = CELL_WIDTH / 2;
    let half_height = CELL_HEIGHT / 2;
    for r in 0..rows {
        let row = &halves.horizontal[r * columns * 2..(r + 1) * columns * 2];
        let y = r * CELL_HEIGHT + half_height;
        for (style, start, end) in runs(row) {
            let (x0, x1) = (start * half_width, end * half_width);
            let path = &mut paths[usize::from(style)];
            if style == 3 {
                let _ = write!(
                    path,
                    "M{x0} {}H{x1}M{x0} {}H{x1}",
                    y - DOUBLE_OFFSET,
                    y + DOUBLE_OFFSET
                );
            } else {
                let _ = write!(path, "M{x0} {y}H{x1}");
            }
        }
    }
    for c in 0..columns {
        let column = &halves.vertical[c * rows * 2..(c + 1) * rows * 2];
        let x = c * CELL_WIDTH + half_width;
        for (style, start, end) in runs(column) {
            let (y0, y1) = (start * half_height, end * half_height);
            let path = &mut paths[usize::from(style)];
            if style == 3 {
                let _ = write!(
                    path,
                    "M{} {y0}V{y1}M{} {y0}V{y1}",
                    x - DOUBLE_OFFSET,
                    x + DOUBLE_OFFSET
                );
            } else {
                let _ = write!(path, "M{x} {y0}V{y1}");
            }
        }
    }

    let (width, height) = (columns * CELL_WIDTH, rows * CELL_HEIGHT);
    let mut svg = format!(
        "<svg class=\"marknexia-text-diagram-svg\" viewBox=\"0 0 {width} {height}\" width=\"{width}\" height=\"{height}\" aria-hidden=\"true\">"
    );
    for (stroke, class) in STYLES {
        let data = &paths[usize::from(style_index(stroke))];
        if !data.is_empty() {
            let _ = write!(svg, "<path class=\"{class}\" d=\"{data}\"></path>");
        }
    }
    if !heads.is_empty() {
        let _ = write!(svg, "<path class=\"tdg-head\" d=\"{heads}\"></path>");
    }
    svg.push_str("</svg>");

    let mut text = String::new();
    for (r, (cells, row_marks)) in grid.rows.iter().zip(&marks.marks).enumerate() {
        if r > 0 {
            text.push('\n');
        }
        let start = text.len();
        let mut buffer = [0; 4];
        for (cell, mark) in cells.iter().zip(row_marks) {
            if mark.is_drawn() {
                text.push(' ');
            } else if cell.ch != '\0' {
                html_encode(cell.ch.encode_utf8(&mut buffer), &mut text);
                if let Some(extra) = &cell.extra {
                    html_encode(extra, &mut text);
                }
            }
        }
        let kept = text[start..].trim_end_matches(' ').len();
        text.truncate(start + kept);
    }
    Drawing { svg, text }
}

#[cfg(test)]
mod tests {
    use super::super::grid::classify;
    use super::*;

    fn drawing(source: &str) -> Drawing {
        let grid = Grid::new(source, 100, 100).unwrap();
        let marks = classify(&grid);
        draw(&grid, &marks)
    }

    #[test]
    fn merges_runs_and_blanks_drawn_cells() {
        let out = drawing("+--> a<b\n|\nv");
        assert_eq!(
            out.svg,
            "<svg class=\"marknexia-text-diagram-svg\" viewBox=\"0 0 64 48\" width=\"64\" height=\"48\" aria-hidden=\"true\"><path class=\"tdg-line\" d=\"M4 8H28M4 8V40\"></path><path class=\"tdg-head\" d=\"M32 8L24 4L24 12ZM4 46L0 38L8 38Z\"></path></svg>"
        );
        assert_eq!(out.text, "     a&lt;b\n\n");
    }

    #[test]
    fn styles_corners_and_diagonals() {
        let out = drawing("╔═╗\n╰┄╯╱");
        assert!(out.svg.contains("class=\"tdg-double\""));
        assert!(out.svg.contains("class=\"tdg-dashed\" d=\"M8 24H16\""));
        assert!(out.svg.contains("M8 24Q4 24 4 16"));
        assert!(out.svg.contains("M24 32L32 16"));
        assert_eq!(out.text, "\n");
    }
}
