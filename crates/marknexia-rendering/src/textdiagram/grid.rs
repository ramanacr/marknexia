//! The character grid of a text diagram and the classification of each
//! cell: drawn as a line, a corner or an arrowhead, or kept as text.
//!
//! Every decision looks only at a cell and its eight neighbours, so the
//! work is linear in the number of cells.

/// One cell of the grid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Cell {
    /// The character in the cell. `'\0'` marks the second cell of a wide
    /// character; tab stops are expanded to spaces.
    pub ch: char,
    /// Zero-width characters (combining marks, joiners, variation selectors)
    /// that follow `ch` in the source. They are kept for the text layer.
    pub extra: Option<Box<str>>,
}

/// Why a block cannot become a grid.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GridError {
    TooManyLines,
    TooManyColumns,
}

/// Tab stops in the source are expanded to this width.
const TAB_WIDTH: usize = 4;

/// The source as rows of cells. Rows are not padded.
#[derive(Debug)]
pub(super) struct Grid {
    pub rows: Vec<Vec<Cell>>,
    pub columns: usize,
}

impl Grid {
    /// Split `code` into rows of cells. Trailing white space (blank lines
    /// included) and `\r` before a line feed are dropped.
    pub(super) fn new(code: &str, max_lines: usize, max_columns: usize) -> Result<Self, GridError> {
        let code = code.trim_end();
        let mut rows = Vec::new();
        let mut columns = 0;
        for line in code.split('\n') {
            if rows.len() == max_lines {
                return Err(GridError::TooManyLines);
            }
            let line = line.strip_suffix('\r').unwrap_or(line);
            let mut row: Vec<Cell> = Vec::new();
            for ch in line.chars() {
                match width(ch) {
                    _ if ch == '\t' => {
                        let spaces = TAB_WIDTH - row.len() % TAB_WIDTH;
                        row.extend((0..spaces).map(|_| Cell {
                            ch: ' ',
                            extra: None,
                        }));
                    }
                    0 => match row.iter_mut().rev().find(|cell| cell.ch != '\0') {
                        Some(cell) => {
                            let mut extra = cell.extra.take().map(String::from).unwrap_or_default();
                            extra.push(ch);
                            cell.extra = Some(extra.into_boxed_str());
                        }
                        // A zero-width character at the start of a line
                        // gets a cell of its own.
                        None => row.push(Cell { ch, extra: None }),
                    },
                    2 => {
                        row.push(Cell { ch, extra: None });
                        row.push(Cell {
                            ch: '\0',
                            extra: None,
                        });
                    }
                    _ => row.push(Cell { ch, extra: None }),
                }
                if row.len() > max_columns {
                    return Err(GridError::TooManyColumns);
                }
            }
            columns = columns.max(row.len());
            rows.push(row);
        }
        Ok(Self { rows, columns })
    }

    /// The character at a signed position; a space outside the grid.
    pub(super) fn at(&self, row: isize, column: isize) -> char {
        if row < 0 || column < 0 {
            return ' ';
        }
        self.rows
            .get(row as usize)
            .and_then(|cells| cells.get(column as usize))
            .map_or(' ', |cell| cell.ch)
    }
}

/// Terminal cell width: 0 for combining marks and format characters, 2 for
/// East Asian wide and fullwidth characters and the common emoji blocks, 1
/// otherwise. An approximation of UAX #11 (TDG-6).
pub(super) fn width(ch: char) -> usize {
    let code = u32::from(ch);
    const ZERO: &[(u32, u32)] = &[
        (0x0300, 0x036F),
        (0x0483, 0x0489),
        (0x0591, 0x05BD),
        (0x0610, 0x061A),
        (0x064B, 0x065F),
        (0x0E31, 0x0E31),
        (0x0E34, 0x0E3A),
        (0x0E47, 0x0E4E),
        (0x1AB0, 0x1AFF),
        (0x1DC0, 0x1DFF),
        (0x200B, 0x200F),
        (0x2028, 0x202E),
        (0x2060, 0x2064),
        (0x20D0, 0x20FF),
        (0xFE00, 0xFE0F),
        (0xFE20, 0xFE2F),
        (0xFEFF, 0xFEFF),
        (0xE0100, 0xE01EF),
    ];
    const WIDE: &[(u32, u32)] = &[
        (0x1100, 0x115F),
        (0x2E80, 0x303E),
        (0x3041, 0x33FF),
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xA000, 0xA4CF),
        (0xAC00, 0xD7A3),
        (0xF900, 0xFAFF),
        (0xFE30, 0xFE4F),
        (0xFF00, 0xFF60),
        (0xFFE0, 0xFFE6),
        (0x1F300, 0x1F64F),
        (0x1F900, 0x1F9FF),
        (0x20000, 0x3FFFD),
    ];
    let within = |ranges: &[(u32, u32)]| {
        ranges
            .iter()
            .any(|&(low, high)| (low..=high).contains(&code))
    };
    if within(ZERO) {
        0
    } else if within(WIDE) {
        2
    } else {
        1
    }
}

/// Arm directions of a drawn cell.
pub(super) const NORTH: u8 = 1;
pub(super) const EAST: u8 = 2;
pub(super) const SOUTH: u8 = 4;
pub(super) const WEST: u8 = 8;

/// The stroke style of a drawn cell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Stroke {
    Light,
    Heavy,
    Double,
    Dashed,
}

/// How a cell is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mark {
    /// Kept as text.
    Text,
    /// Straight arms from the cell centre to the given edges.
    Arms(u8, Stroke),
    /// A rounded corner joining the given arms (two or three of them).
    Rounded(u8),
    /// Diagonals: `1` rising (`╱`), `2` falling (`╲`), `3` both.
    Diagonal(u8),
    /// An arrowhead pointing in one direction, with a stub arm on the
    /// opposite side that joins the line it ends.
    Head(u8),
}

impl Mark {
    /// The arms this cell offers to its neighbours.
    pub(super) const fn arms(self) -> u8 {
        match self {
            Self::Arms(arms, _) | Self::Rounded(arms) => arms,
            Self::Head(direction) => opposite(direction),
            Self::Text | Self::Diagonal(_) => 0,
        }
    }

    pub(super) const fn is_drawn(self) -> bool {
        !matches!(self, Self::Text)
    }
}

const fn opposite(direction: u8) -> u8 {
    match direction {
        NORTH => SOUTH,
        SOUTH => NORTH,
        EAST => WEST,
        _ => EAST,
    }
}

/// The arms and stroke of a box-drawing character (U+2500 to U+257F), or
/// `None` for any other character.
pub(super) fn box_mark(ch: char) -> Option<Mark> {
    use Stroke::{Dashed, Double, Heavy, Light};
    const HORIZONTAL: u8 = EAST | WEST;
    const VERTICAL: u8 = NORTH | SOUTH;
    let code = u32::from(ch);
    if !(0x2500..=0x257F).contains(&code) {
        return None;
    }
    // The heavy variants of each family; mixed light and heavy variants are
    // drawn light.
    const HEAVY: &[u32] = &[
        0x2501, 0x2503, 0x2505, 0x2507, 0x2509, 0x250B, 0x250F, 0x2513, 0x2517, 0x251B, 0x2523,
        0x252B, 0x2533, 0x253B, 0x254B, 0x254D, 0x254F, 0x2578, 0x2579, 0x257A, 0x257B,
    ];
    let weight = if HEAVY.contains(&code) { Heavy } else { Light };
    let arms = |arms: u8| Some(Mark::Arms(arms, weight));
    match code {
        0x2500 | 0x2501 => arms(HORIZONTAL),
        0x2502 | 0x2503 => arms(VERTICAL),
        0x2504 | 0x2505 | 0x2508 | 0x2509 | 0x254C | 0x254D => Some(Mark::Arms(HORIZONTAL, Dashed)),
        0x2506 | 0x2507 | 0x250A | 0x250B | 0x254E | 0x254F => Some(Mark::Arms(VERTICAL, Dashed)),
        0x250C..=0x250F => arms(SOUTH | EAST),
        0x2510..=0x2513 => arms(SOUTH | WEST),
        0x2514..=0x2517 => arms(NORTH | EAST),
        0x2518..=0x251B => arms(NORTH | WEST),
        0x251C..=0x2523 => arms(VERTICAL | EAST),
        0x2524..=0x252B => arms(VERTICAL | WEST),
        0x252C..=0x2533 => arms(SOUTH | HORIZONTAL),
        0x2534..=0x253B => arms(NORTH | HORIZONTAL),
        0x253C..=0x254B => arms(VERTICAL | HORIZONTAL),
        0x2550 => Some(Mark::Arms(HORIZONTAL, Double)),
        0x2551 => Some(Mark::Arms(VERTICAL, Double)),
        0x2552..=0x2554 => Some(Mark::Arms(SOUTH | EAST, Double)),
        0x2555..=0x2557 => Some(Mark::Arms(SOUTH | WEST, Double)),
        0x2558..=0x255A => Some(Mark::Arms(NORTH | EAST, Double)),
        0x255B..=0x255D => Some(Mark::Arms(NORTH | WEST, Double)),
        0x255E..=0x2560 => Some(Mark::Arms(VERTICAL | EAST, Double)),
        0x2561..=0x2563 => Some(Mark::Arms(VERTICAL | WEST, Double)),
        0x2564..=0x2566 => Some(Mark::Arms(SOUTH | HORIZONTAL, Double)),
        0x2567..=0x2569 => Some(Mark::Arms(NORTH | HORIZONTAL, Double)),
        0x256A..=0x256C => Some(Mark::Arms(VERTICAL | HORIZONTAL, Double)),
        0x256D => Some(Mark::Rounded(SOUTH | EAST)),
        0x256E => Some(Mark::Rounded(SOUTH | WEST)),
        0x256F => Some(Mark::Rounded(NORTH | WEST)),
        0x2570 => Some(Mark::Rounded(NORTH | EAST)),
        0x2571 => Some(Mark::Diagonal(1)),
        0x2572 => Some(Mark::Diagonal(2)),
        0x2573 => Some(Mark::Diagonal(3)),
        0x2574 | 0x2578 => arms(WEST),
        0x2575 | 0x2579 => arms(NORTH),
        0x2576 | 0x257A => arms(EAST),
        0x2577 | 0x257B => arms(SOUTH),
        0x257C | 0x257E => arms(HORIZONTAL),
        // 0x257D, 0x257F
        _ => arms(VERTICAL),
    }
}

/// Unicode arrows and the direction each one points.
pub(super) fn arrow_direction(ch: char) -> Option<u8> {
    match ch {
        '→' | '⟶' | '▶' | '►' => Some(EAST),
        '←' | '⟵' | '◀' | '◄' => Some(WEST),
        '↓' | '▼' => Some(SOUTH),
        '↑' | '▲' => Some(NORTH),
        _ => None,
    }
}

fn is_alphanumeric(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Characters a run of `-` may end at for the run to be a line.
fn joins_horizontal(ch: char) -> bool {
    matches!(ch, '+' | '>' | '<' | '|' | '.' | '\'')
        || box_mark(ch).is_some()
        || arrow_direction(ch).is_some_and(|direction| direction & (EAST | WEST) != 0)
}

/// The classification of every cell.
pub(super) struct Marks {
    pub marks: Vec<Vec<Mark>>,
}

/// Classify every cell of `grid` (TDG-4).
pub(super) fn classify(grid: &Grid) -> Marks {
    let mut marks: Vec<Vec<Mark>> = grid
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| box_mark(cell.ch).unwrap_or(Mark::Text))
                .collect()
        })
        .collect();
    let at = |row: usize, column: usize, dr: isize, dc: isize| {
        grid.at(row as isize + dr, column as isize + dc)
    };

    // 1. Runs of `-`.
    let mut dash = vec![Vec::new(); grid.rows.len()];
    for (r, row) in grid.rows.iter().enumerate() {
        dash[r] = vec![false; row.len()];
        let mut c = 0;
        while c < row.len() {
            if row[c].ch != '-' {
                c += 1;
                continue;
            }
            let start = c;
            while c < row.len() && row[c].ch == '-' {
                c += 1;
            }
            let left = at(r, start, 0, -1);
            let right = at(r, c, 0, 0);
            let joined = joins_horizontal(left) || joins_horizontal(right);
            let words = is_alphanumeric(left) || is_alphanumeric(right);
            if c - start >= 3 || (joined && !words) {
                dash[r][start..c].iter_mut().for_each(|drawn| *drawn = true);
            }
        }
    }
    let is_dash = |row: usize, column: isize| {
        column >= 0
            && dash
                .get(row)
                .and_then(|cells| cells.get(column as usize))
                .copied()
                .unwrap_or(false)
    };
    let box_arms = |row: isize, column: isize| box_mark(grid.at(row, column)).map_or(0, Mark::arms);

    // 2. Vertical bars: `|` with a vertical partner above or below.
    let free = |row: isize, column: isize| {
        !is_alphanumeric(grid.at(row, column - 1)) && !is_alphanumeric(grid.at(row, column + 1))
    };
    let joins_from_above = |row: isize, column: isize| match grid.at(row, column) {
        '|' | '+' | '.' => true,
        '^' => free(row, column),
        _ => box_arms(row, column) & SOUTH != 0,
    };
    let joins_from_below = |row: isize, column: isize| match grid.at(row, column) {
        '|' | '+' | '\'' => true,
        'v' => free(row, column),
        _ => box_arms(row, column) & NORTH != 0,
    };
    let mut bar = vec![Vec::new(); grid.rows.len()];
    for (r, row) in grid.rows.iter().enumerate() {
        bar[r] = row
            .iter()
            .enumerate()
            .map(|(c, cell)| {
                let (ri, ci) = (r as isize, c as isize);
                cell.ch == '|' && (joins_from_above(ri - 1, ci) || joins_from_below(ri + 1, ci))
            })
            .collect();
    }
    let is_bar = |row: isize, column: isize| {
        row >= 0
            && column >= 0
            && bar
                .get(row as usize)
                .and_then(|cells| cells.get(column as usize))
                .copied()
                .unwrap_or(false)
    };
    let vertical_above = |row: isize, column: isize| {
        is_bar(row - 1, column)
            || matches!(grid.at(row - 1, column), '+' | '.')
            || box_arms(row - 1, column) & SOUTH != 0
    };
    let vertical_below = |row: isize, column: isize| {
        is_bar(row + 1, column)
            || matches!(grid.at(row + 1, column), '+' | '\'')
            || box_arms(row + 1, column) & NORTH != 0
    };
    let horizontal_left = |row: usize, column: isize| {
        is_dash(row, column - 1) || box_arms(row as isize, column - 1) & EAST != 0
    };
    let horizontal_right = |row: usize, column: isize| {
        is_dash(row, column + 1) || box_arms(row as isize, column + 1) & WEST != 0
    };

    // 3. Everything else, from the dash and bar decisions.
    for (r, row) in grid.rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let (ri, ci) = (r as isize, c as isize);
            let mark = match cell.ch {
                '-' if dash[r][c] => Mark::Arms(EAST | WEST, Stroke::Light),
                '|' if bar[r][c] => {
                    let mut arms = NORTH | SOUTH;
                    if is_dash(r, ci + 1) {
                        arms |= EAST;
                    }
                    if is_dash(r, ci - 1) {
                        arms |= WEST;
                    }
                    Mark::Arms(arms, Stroke::Light)
                }
                '+' => {
                    let mut arms = 0;
                    if horizontal_right(r, ci) {
                        arms |= EAST;
                    }
                    if horizontal_left(r, ci) {
                        arms |= WEST;
                    }
                    if vertical_above(ri, ci)
                        || joins_from_above(ri - 1, ci) && grid.at(ri - 1, ci) == '^'
                    {
                        arms |= NORTH;
                    }
                    if vertical_below(ri, ci)
                        || joins_from_below(ri + 1, ci) && grid.at(ri + 1, ci) == 'v'
                    {
                        arms |= SOUTH;
                    }
                    if arms == 0 {
                        Mark::Text
                    } else {
                        Mark::Arms(arms, Stroke::Light)
                    }
                }
                '.' | '\'' => {
                    let mut arms = 0;
                    if is_dash(r, ci + 1) || box_arms(ri, ci + 1) & WEST != 0 {
                        arms |= EAST;
                    }
                    if is_dash(r, ci - 1) || box_arms(ri, ci - 1) & EAST != 0 {
                        arms |= WEST;
                    }
                    let vertical = if cell.ch == '.' {
                        (is_bar(ri + 1, ci) || box_arms(ri + 1, ci) & NORTH != 0).then_some(SOUTH)
                    } else {
                        (is_bar(ri - 1, ci) || box_arms(ri - 1, ci) & SOUTH != 0).then_some(NORTH)
                    };
                    match vertical {
                        Some(vertical) if arms != 0 => Mark::Rounded(arms | vertical),
                        _ => Mark::Text,
                    }
                }
                'v' if free(ri, ci) && vertical_above(ri, ci) => Mark::Head(SOUTH),
                '^' if free(ri, ci) && vertical_below(ri, ci) => Mark::Head(NORTH),
                '>' if horizontal_left(r, ci) => Mark::Head(EAST),
                '<' if horizontal_right(r, ci) => Mark::Head(WEST),
                ch => match arrow_direction(ch) {
                    Some(EAST) if horizontal_left(r, ci) => Mark::Head(EAST),
                    Some(WEST) if horizontal_right(r, ci) => Mark::Head(WEST),
                    Some(SOUTH) if vertical_above(ri, ci) => Mark::Head(SOUTH),
                    Some(NORTH) if vertical_below(ri, ci) => Mark::Head(NORTH),
                    _ => marks[r][c],
                },
            };
            marks[r][c] = mark;
        }
    }
    Marks { marks }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(text: &str) -> Vec<String> {
        let grid = Grid::new(text, 100, 100).unwrap();
        let marks = classify(&grid);
        grid.rows
            .iter()
            .zip(&marks.marks)
            .map(|(row, marks)| {
                row.iter()
                    .zip(marks)
                    .map(|(cell, mark)| if mark.is_drawn() { '#' } else { cell.ch })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn ascii_connectors() {
        assert_eq!(
            marks("+--> metrics\n|\nv\nx -> y"),
            ["#### metrics", "#", "#", "x ## y"]
        );
        // Words, options and plus signs stay text.
        assert_eq!(
            marks("short-lived [--lines|--grep] a + b C++ x->y"),
            ["short-lived [--lines|--grep] a + b C++ x->y"]
        );
        // `|` needs a vertical partner.
        assert_eq!(marks("Pre|Post\n a|b"), ["Pre|Post", " a|b"]);
        assert_eq!(marks(" |\n |"), [" #", " #"]);
        // Rounded ASCII corners.
        assert_eq!(marks(".--.\n|  |\n'--'"), ["####", "#  #", "####"]);
        // `v` next to letters is a letter.
        assert_eq!(marks("|\nvia"), ["|", "via"]);
    }

    #[test]
    fn box_drawing_and_arrows() {
        assert_eq!(
            marks("┌─┐\n│ │ → x\n└┬┘\n ├──→ y\n ↓"),
            ["###", "# # → x", "###", " #### y", " #"]
        );
        assert_eq!(box_mark('╳'), Some(Mark::Diagonal(3)));
        assert_eq!(box_mark('═'), Some(Mark::Arms(EAST | WEST, Stroke::Double)));
        assert_eq!(box_mark('┅'), Some(Mark::Arms(EAST | WEST, Stroke::Dashed)));
        assert_eq!(box_mark('┿'), Some(Mark::Arms(15, Stroke::Light)));
        assert_eq!(box_mark('a'), None);
        // Every box-drawing character has a mark with at least one arm or a
        // diagonal.
        for code in 0x2500..=0x257F {
            let mark = box_mark(char::from_u32(code).unwrap()).unwrap();
            assert!(
                mark.arms() != 0 || matches!(mark, Mark::Diagonal(_)),
                "{code:x}"
            );
        }
    }

    #[test]
    fn widths_and_tabs() {
        let grid = Grid::new("a\tb\n漢x\ne\u{301}|", 10, 10).unwrap();
        assert_eq!(grid.rows[0].len(), 5);
        assert_eq!(
            grid.rows[1].iter().map(|cell| cell.ch).collect::<String>(),
            "漢\0x"
        );
        assert_eq!(grid.rows[2].len(), 2);
        assert_eq!(grid.rows[2][0].extra.as_deref(), Some("\u{301}"));
        assert_eq!(grid.columns, 5);
        assert_eq!(
            Grid::new("a\nb\nc", 2, 10).unwrap_err(),
            GridError::TooManyLines
        );
        assert_eq!(
            Grid::new("abc", 2, 2).unwrap_err(),
            GridError::TooManyColumns
        );
        assert_eq!(Grid::new("a\r\nb\n", 2, 2).unwrap().rows.len(), 2);
    }
}
