//! `==mark==` (Markdig EmphasisExtras `Marked`) for parsers without native
//! support. Uses CommonMark delimiter-run flanking rules for a run of exactly
//! two `=` characters, pairing each closer with the nearest opener.

use super::model::{Block, Document, Inline, MAX_INLINE_DEPTH};

pub(crate) fn apply(document: &mut Document) {
    blocks(&mut document.blocks);
    for footnote in &mut document.footnotes {
        blocks(&mut footnote.blocks);
    }
}

fn blocks(items: &mut [Block]) {
    for block in items {
        match block {
            Block::Paragraph(paragraph) => inlines(&mut paragraph.inlines),
            Block::Heading { inlines: items, .. } => inlines(items),
            Block::Quote(children) => blocks(children),
            Block::List(list) => {
                for item in &mut list.items {
                    blocks(&mut item.blocks);
                }
            }
            Block::Table(table) => {
                for row in &mut table.rows {
                    for cell in &mut row.cells {
                        blocks(cell);
                    }
                }
            }
            Block::ThematicBreak | Block::Code { .. } | Block::Html { .. } => {}
        }
    }
}

enum Token {
    Node(Inline),
    Delimiter,
}

fn inlines(items: &mut Vec<Inline>) {
    apply_at_depth(items, 0);
}

/// `depth` counts inline containers above `items`. New marks may nest at most
/// `MAX_INLINE_DEPTH - depth` deep, so the pass can at most double the
/// adapter-bounded inline depth.
fn apply_at_depth(items: &mut Vec<Inline>, depth: usize) {
    for item in items.iter_mut() {
        if let Some(children) = item.children_mut() {
            apply_at_depth(children, depth + 1);
        }
    }
    if !items
        .iter()
        .any(|item| matches!(item, Inline::Text(text) if text.contains("==")))
    {
        return;
    }
    let merged = merge_text(std::mem::take(items));
    let bounds: Vec<(char, char)> = (0..merged.len())
        .map(|index| {
            (
                boundary(
                    index
                        .checked_sub(1)
                        .and_then(|previous| merged.get(previous)),
                    true,
                ),
                boundary(merged.get(index + 1), false),
            )
        })
        .collect();
    let nesting_budget = MAX_INLINE_DEPTH.saturating_sub(depth);
    let mut output: Vec<Token> = Vec::with_capacity(merged.len());
    let mut openers: Vec<usize> = Vec::new();
    for (item, (before, after)) in merged.into_iter().zip(bounds) {
        let Inline::Text(text) = item else {
            output.push(Token::Node(item));
            continue;
        };
        let mut literal_start = 0;
        for run in delimiter_runs(&text) {
            let previous = text[..run].chars().next_back().unwrap_or(before);
            let next = text[run + 2..].chars().next().unwrap_or(after);
            let left = !next.is_whitespace()
                && (!is_punctuation(next) || previous.is_whitespace() || is_punctuation(previous));
            let right = !previous.is_whitespace()
                && (!is_punctuation(previous) || next.is_whitespace() || is_punctuation(next));
            if !left && !right {
                continue;
            }
            if run > literal_start {
                output.push(Token::Node(Inline::Text(
                    text[literal_start..run].to_owned(),
                )));
            }
            literal_start = run + 2;
            if right && let Some(opener) = openers.pop() {
                let children = output.drain(opener + 1..).map(token_inline).collect();
                output.pop();
                output.push(Token::Node(Inline::Mark(merge_text(children))));
                continue;
            }
            if left && openers.len() < nesting_budget {
                openers.push(output.len());
                output.push(Token::Delimiter);
            } else {
                output.push(Token::Node(Inline::Text("==".to_owned())));
            }
        }
        if literal_start < text.len() {
            output.push(Token::Node(Inline::Text(text[literal_start..].to_owned())));
        }
    }
    *items = merge_text(output.into_iter().map(token_inline).collect());
}

fn token_inline(token: Token) -> Inline {
    match token {
        Token::Node(inline) => inline,
        Token::Delimiter => Inline::Text("==".to_owned()),
    }
}

/// Byte offsets of `=` runs whose length is exactly two.
fn delimiter_runs(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut runs = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'=' {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index] == b'=' {
            index += 1;
        }
        if index - start == 2 {
            runs.push(start);
        }
    }
    runs
}

/// Neighboring character for flanking checks at a text-node boundary: line
/// boundaries count as whitespace, other inline nodes as ordinary characters.
fn boundary(neighbor: Option<&Inline>, before: bool) -> char {
    match neighbor {
        None | Some(Inline::SoftBreak | Inline::HardBreak) => ' ',
        Some(Inline::Text(text) | Inline::Escaped(text)) => {
            let ch = if before {
                text.chars().next_back()
            } else {
                text.chars().next()
            };
            ch.unwrap_or(' ')
        }
        Some(_) => 'a',
    }
}

fn is_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation() || (!ch.is_ascii() && !ch.is_alphanumeric() && !ch.is_whitespace())
}

fn merge_text(items: Vec<Inline>) -> Vec<Inline> {
    let mut merged: Vec<Inline> = Vec::with_capacity(items.len());
    for item in items {
        if let (Inline::Text(text), Some(Inline::Text(previous))) = (&item, merged.last_mut()) {
            previous.push_str(text);
        } else {
            merged.push(item);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Inline {
        Inline::Text(value.to_owned())
    }

    fn run(items: Vec<Inline>) -> Vec<Inline> {
        let mut items = items;
        inlines(&mut items);
        items
    }

    #[test]
    fn pairs_simple_and_split_text() {
        assert_eq!(
            run(vec![text(" "), text("==marked=="), text(" x")]),
            [text(" "), Inline::Mark(vec![text("marked")]), text(" x")]
        );
        assert_eq!(
            run(vec![
                text("==a "),
                Inline::Emphasis(vec![text("b")]),
                text(" c==")
            ]),
            [Inline::Mark(vec![
                text("a "),
                Inline::Emphasis(vec![text("b")]),
                text(" c")
            ])]
        );
    }

    #[test]
    fn rejects_non_flanking_and_wrong_lengths() {
        assert_eq!(run(vec![text("a == b")]), [text("a == b")]);
        assert_eq!(run(vec![text("===x===")]), [text("===x===")]);
        assert_eq!(run(vec![text("==open")]), [text("==open")]);
        assert_eq!(
            run(vec![Inline::Code("==x==".to_owned())]),
            [Inline::Code("==x==".to_owned())]
        );
    }
}
