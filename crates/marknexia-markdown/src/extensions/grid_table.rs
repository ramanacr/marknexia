//! Markdig grid tables for the subset the frozen fixtures exercise: a
//! top-level paragraph whose lines form `+---+` separated rows of `|` cells,
//! with an optional `+===+` header separator. Row and column spans are not
//! supported; such paragraphs stay paragraphs.

use marknexia_core::contracts::Diagnostic;

use super::model::{Alignment, Block, Document, Table, TableRow};

pub(crate) fn apply(document: &mut Document, lines: &[&str], parse: &dyn Fn(&str) -> Document) {
    for block in &mut document.blocks {
        let Block::Paragraph(paragraph) = block else {
            continue;
        };
        let Some(source) = lines.get(paragraph.line..=paragraph.end_line) else {
            continue;
        };
        if let Some((table, diagnostics)) = parse_grid(source, paragraph.line, parse) {
            *block = Block::Table(table);
            // Cell fragments are parsed separately; keep their diagnostics
            // (for example depth flattening), at most one per message.
            for diagnostic in diagnostics {
                if !document
                    .diagnostics
                    .iter()
                    .any(|existing| existing.message == diagnostic.message)
                {
                    document.diagnostics.push(diagnostic);
                }
            }
        }
    }
}

fn parse_grid(
    lines: &[&str],
    first_line: usize,
    parse: &dyn Fn(&str) -> Document,
) -> Option<(Table, Vec<Diagnostic>)> {
    let first = lines.first()?.trim_end();
    if !is_separator(first) || !first.contains('-') {
        return None;
    }
    let columns: Vec<usize> = first
        .bytes()
        .enumerate()
        .filter(|(_, byte)| *byte == b'+')
        .map(|(index, _)| index)
        .collect();
    if columns.len() < 2 || !is_separator(lines.last()?.trim_end()) {
        return None;
    }
    let segment_widths: Vec<usize> = columns
        .windows(2)
        .map(|pair| pair[1] - pair[0] - 1)
        .collect();
    let total: usize = segment_widths.iter().sum();
    let widths = segment_widths
        .iter()
        .map(|width| *width as f32 * 100.0 / total as f32)
        .collect();

    let mut rows: Vec<(usize, Vec<Vec<String>>)> = Vec::new();
    let mut current: Option<(usize, Vec<Vec<String>>)> = None;
    let mut header_rows = 0;
    for (offset, raw) in lines.iter().enumerate().skip(1) {
        let line = raw.trim_end();
        if is_separator(line) {
            if !columns
                .iter()
                .all(|column| line.as_bytes().get(*column) == Some(&b'+'))
            {
                return None;
            }
            rows.extend(current.take());
            if line.contains('=') {
                if header_rows != 0 {
                    return None;
                }
                header_rows = rows.len();
            }
            continue;
        }
        let boundaries = column_bytes(line, &columns)?;
        let (_, cells) = current
            .get_or_insert_with(|| (first_line + offset, vec![Vec::new(); columns.len() - 1]));
        for (cell, pair) in cells.iter_mut().zip(boundaries.windows(2)) {
            cell.push(line[pair[0] + 1..pair[1]].trim().to_owned());
        }
    }
    if current.is_some() || rows.is_empty() {
        return None;
    }
    let mut diagnostics = Vec::new();
    let rows = rows
        .into_iter()
        .enumerate()
        .map(|(index, (line, cells))| TableRow {
            header: index < header_rows,
            cells: cells
                .into_iter()
                .map(|cell| {
                    let fragment = parse(cell.join("\n").trim());
                    let mut blocks = fragment.blocks;
                    relocate(&mut blocks, line);
                    diagnostics.extend(fragment.diagnostics.into_iter().map(|mut diagnostic| {
                        diagnostic.source_line = Some(line);
                        diagnostic
                    }));
                    blocks
                })
                .collect(),
        })
        .collect();
    Some((
        Table {
            alignments: vec![Alignment::None; columns.len() - 1],
            widths,
            rows,
        },
        diagnostics,
    ))
}

/// Markdig slices .NET strings, so grid column positions are UTF-16 code-unit
/// offsets (a supplementary character such as 😀 is two units). Separator
/// lines are ASCII, so their byte offsets equal their unit offsets; content
/// lines are measured in units here. Returns the byte offset of each column's
/// `|`, or `None` unless every column has a `|` at exactly its unit offset and
/// the line ends at the last column.
fn column_bytes(line: &str, columns: &[usize]) -> Option<Vec<usize>> {
    let mut boundaries = Vec::with_capacity(columns.len());
    let mut unit = 0;
    for (byte, ch) in line.char_indices() {
        if let Some(&column) = columns.get(boundaries.len()) {
            if unit > column {
                // The column falls inside a surrogate pair.
                return None;
            }
            if unit == column {
                if ch != '|' {
                    return None;
                }
                boundaries.push(byte);
            }
        }
        unit += ch.len_utf16();
    }
    (boundaries.len() == columns.len() && unit == columns[columns.len() - 1] + 1)
        .then_some(boundaries)
}

fn is_separator(line: &str) -> bool {
    line.len() >= 3
        && line.starts_with('+')
        && line.ends_with('+')
        && line.bytes().all(|byte| matches!(byte, b'+' | b'-' | b'='))
}

/// Cell content is parsed separately; attribute its blocks to the row line.
fn relocate(blocks: &mut [Block], line: usize) {
    for block in blocks {
        match block {
            Block::Paragraph(paragraph) => {
                paragraph.line = line;
                paragraph.end_line = line;
            }
            Block::Heading { line: at, .. }
            | Block::Code { line: at, .. }
            | Block::Html { line: at, .. } => {
                *at = line;
            }
            Block::Quote(children) => relocate(children, line),
            Block::List(list) => {
                for item in &mut list.items {
                    relocate(&mut item.blocks, line);
                }
            }
            Block::Table(_) | Block::ThematicBreak => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::model::{Inline, Paragraph};

    fn cell(text: &str) -> Document {
        Document {
            blocks: vec![Block::Paragraph(Paragraph {
                line: 0,
                end_line: 0,
                inlines: vec![Inline::Text(text.to_owned())],
            })],
            ..Document::default()
        }
    }

    fn cell_texts(table: &Table) -> Vec<Vec<String>> {
        table
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| match cell.as_slice() {
                        [Block::Paragraph(paragraph)] => match paragraph.inlines.as_slice() {
                            [Inline::Text(text)] => text.clone(),
                            other => panic!("{other:?}"),
                        },
                        other => panic!("{other:?}"),
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn columns_are_utf16_code_units() {
        // `é` is one UTF-16 unit and `😀` two, so this grid is aligned for .NET
        // although its byte offsets differ.
        let lines = ["+--+--+", "|é |😀|", "+==+==+", "|a |b |", "+--+--+"];
        let (table, _) = parse_grid(&lines, 0, &cell).unwrap();
        assert_eq!(cell_texts(&table), [["é", "😀"], ["a", "b"]]);
        assert!(table.rows[0].header);
        // Four units wide in .NET terms, so `|😀|` (three units) does not fit.
        assert!(parse_grid(&["+----+", "|😀|", "+----+"], 0, &cell).is_none());
        assert!(parse_grid(&["+---+", "|😀 |", "+---+"], 0, &cell).is_some());
        // A column that lands inside a surrogate pair is rejected.
        assert!(parse_grid(&["+--+-+", "|😀|a|", "+--+-+"], 0, &cell).is_some());
        assert!(parse_grid(&["+-+--+", "|😀|a|", "+-+--+"], 0, &cell).is_none());
    }

    #[test]
    fn parses_header_and_widths() {
        let lines = [
            "+---+------+",
            "| A | B    |",
            "+===+======+",
            "| 1 | 2    |",
            "+---+------+",
        ];
        let (table, _) = parse_grid(&lines, 5, &cell).unwrap();
        assert_eq!(table.widths.len(), 2);
        assert!((table.widths[0] - 100.0 / 3.0).abs() < 0.001);
        assert_eq!(table.rows.len(), 2);
        assert!(table.rows[0].header && !table.rows[1].header);
        let Block::Paragraph(first) = &table.rows[1].cells[0][0] else {
            panic!("paragraph cell");
        };
        assert_eq!(first.line, 8);
    }

    #[test]
    fn rejects_malformed_grids() {
        assert!(parse_grid(&["+---+", "| A |"], 0, &cell).is_none());
        assert!(parse_grid(&["+---+", "| A  |", "+---+"], 0, &cell).is_none());
        assert!(parse_grid(&["+---+", "+---+"], 0, &cell).is_none());
        assert!(parse_grid(&["plain text"], 0, &cell).is_none());
    }
}
