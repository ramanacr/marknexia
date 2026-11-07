//! Which code blocks become text diagrams (TDG-2, TDG-3).
//!
//! Tagged fences (`bob`, `svgbob`, `ascii`, `diagram`) always do. Untagged
//! and `text` fences do only when a conservative heuristic finds a diagram:
//! the decision errs towards keeping a block as text, because a false
//! positive turns ordinary code or output into a drawing.

use super::grid::{Grid, Mark, Marks, NORTH, SOUTH, arrow_direction, box_mark};

/// How a fence's info string makes a block eligible.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Eligibility {
    /// Always drawn (within the limits).
    Tagged,
    /// Drawn only when [`is_diagram`] accepts it.
    Heuristic,
    /// Never drawn.
    No,
}

pub(super) fn eligibility(language: &str) -> Eligibility {
    const TAGS: [&str; 4] = ["bob", "svgbob", "ascii", "diagram"];
    if TAGS.iter().any(|tag| language.eq_ignore_ascii_case(tag)) {
        Eligibility::Tagged
    } else if language.is_empty() || language.eq_ignore_ascii_case("text") {
        Eligibility::Heuristic
    } else {
        Eligibility::No
    }
}

/// At least this many rows must carry a drawn cell or an arrow.
pub(super) const MIN_DIAGRAM_ROWS: usize = 3;
/// ... and at least this share of the non-blank rows, in percent.
pub(super) const MIN_DIAGRAM_ROW_PERCENT: usize = 30;
/// Drawn cells per thousand non-space cells.
pub(super) const MIN_DRAWN_PER_MILLE: usize = 30;

/// What the classification found in a block.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Features {
    pub nonblank_rows: usize,
    pub diagram_rows: usize,
    pub nonspace_cells: usize,
    pub drawn_cells: usize,
    /// Drawn arrowheads (ASCII and Unicode).
    pub heads: usize,
    /// Box-drawing characters.
    pub box_cells: usize,
    /// Drawn cells with a vertical stroke (`|`, `v`, `^`, corners, box
    /// characters with a north or south arm).
    pub vertical_cells: usize,
    /// Every box-drawing character comes before the first letter or digit
    /// of its row (a `tree` listing).
    pub box_prefix_only: bool,
}

pub(super) fn features(grid: &Grid, marks: &Marks) -> Features {
    let mut features = Features {
        box_prefix_only: true,
        ..Features::default()
    };
    for (cells, row_marks) in grid.rows.iter().zip(&marks.marks) {
        let mut nonblank = false;
        let mut signal = false;
        let mut seen_word = false;
        for (cell, mark) in cells.iter().zip(row_marks) {
            if cell.ch.is_whitespace() || cell.ch == '\0' {
                continue;
            }
            nonblank = true;
            features.nonspace_cells += 1;
            if cell.ch.is_alphanumeric() {
                seen_word = true;
            }
            if box_mark(cell.ch).is_some() {
                features.box_cells += 1;
                if seen_word {
                    features.box_prefix_only = false;
                }
            }
            if mark.is_drawn() {
                features.drawn_cells += 1;
                signal = true;
            }
            if matches!(mark, Mark::Head(_)) {
                features.heads += 1;
            }
            if mark.arms() & (NORTH | SOUTH) != 0 {
                features.vertical_cells += 1;
            }
            if arrow_direction(cell.ch).is_some() {
                signal = true;
            }
        }
        features.nonblank_rows += usize::from(nonblank);
        features.diagram_rows += usize::from(signal);
    }
    features
}

/// The heuristic for untagged and `text` blocks.
pub(super) fn is_diagram(code: &str, grid: &Grid, marks: &Marks) -> bool {
    if looks_like_code_or_output(code) {
        return false;
    }
    accepts(&features(grid, marks))
}

pub(super) fn accepts(features: &Features) -> bool {
    let Features {
        nonblank_rows,
        diagram_rows,
        nonspace_cells,
        drawn_cells,
        heads,
        box_cells,
        box_prefix_only,
        vertical_cells,
    } = *features;
    // A drawing needs an arrow or box-drawing characters: grids of `|`,
    // `-` and `+` alone are tables (psql, Markdown, ASCII tables) and
    // version-control graphs.
    if heads == 0 && box_cells == 0 {
        return false;
    }
    // Box-drawing characters only in front of the text, with no arrow: a
    // `tree`, `cargo tree` or `npm ls` listing.
    if heads == 0 && box_prefix_only {
        return false;
    }
    // ASCII arrows with no vertical stroke are prose or code (`a -> b` on
    // several lines), not a drawing.
    if box_cells == 0 && vertical_cells < 2 {
        return false;
    }
    diagram_rows >= MIN_DIAGRAM_ROWS
        && diagram_rows * 100 >= nonblank_rows * MIN_DIAGRAM_ROW_PERCENT
        && drawn_cells * 1000 >= nonspace_cells * MIN_DRAWN_PER_MILLE
}

/// Source code, terminal sessions, logs, diffs, JSON and compiler
/// diagnostics, recognized from their lines (TDG-3).
pub(super) fn looks_like_code_or_output(code: &str) -> bool {
    let trimmed = code.trim();
    if (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
    {
        return true;
    }
    let mut keywords = 0;
    let mut semicolons = 0;
    let mut braces = 0;
    let mut logs = 0;
    let mut signatures = 0;
    let mut listings = 0;
    for line in code.lines() {
        let line = line.trim_end();
        let content = line.trim_start();
        if content.is_empty() {
            continue;
        }
        if is_prompt(content) || is_diff_or_diagnostic(content) {
            return true;
        }
        keywords += usize::from(starts_with_keyword(content));
        semicolons += usize::from(content.ends_with(';'));
        braces += usize::from(
            content.ends_with('{') || content.ends_with('}') || content.starts_with('}'),
        );
        logs += usize::from(is_log_line(content));
        signatures += usize::from(content.contains(" :: "));
        listings += usize::from(is_permission_listing(content));
    }
    [keywords, semicolons, braces, logs, signatures, listings]
        .iter()
        .any(|&count| count >= 2)
}

/// A shell, REPL or PowerShell prompt at the start of a line.
fn is_prompt(line: &str) -> bool {
    if ["$ ", "# ", "> ", ">>> ", "% "]
        .iter()
        .any(|prompt| line.starts_with(prompt))
    {
        return true;
    }
    // `PS C:\path> ` and `C:\path> `.
    let drive = line.strip_prefix("PS ").unwrap_or(line);
    let bytes = drive.as_bytes();
    if bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && &bytes[1..3] == b":\\" {
        return drive.contains('>');
    }
    // `user@host:~/dir$ `.
    if let Some((user, rest)) = line.split_once('@') {
        let host_end = rest.find(['$', '#']).unwrap_or(0);
        return !user.is_empty()
            && !user.contains(' ')
            && host_end > 0
            && !rest[..host_end].contains(' ')
            && rest[host_end + 1..].starts_with(' ');
    }
    false
}

/// Unified diffs and compiler or interpreter diagnostics.
fn is_diff_or_diagnostic(line: &str) -> bool {
    if [
        "@@ ",
        "diff --git ",
        "+++ ",
        "--- a/",
        "Traceback (most recent call last)",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
    {
        return true;
    }
    // rustc: ` --> src/main.rs:2:5`.
    if line.strip_prefix("--> ").is_some_and(|location| {
        location
            .split(':')
            .nth(1)
            .is_some_and(|part| part.starts_with(|ch: char| ch.is_ascii_digit()))
    }) {
        return true;
    }
    ["error", "warning", "note", "help"].iter().any(|word| {
        line.strip_prefix(word)
            .is_some_and(|rest| rest.starts_with(':') || rest.starts_with('['))
    })
}

fn starts_with_keyword(line: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "fn",
        "pub",
        "let",
        "const",
        "var",
        "function",
        "def",
        "class",
        "import",
        "from",
        "return",
        "if",
        "else",
        "elif",
        "for",
        "while",
        "using",
        "namespace",
        "package",
        "struct",
        "enum",
        "impl",
        "use",
        "mod",
        "public",
        "private",
        "protected",
        "static",
        "async",
        "await",
        "func",
        "type",
        "interface",
        "export",
        "try",
        "catch",
        "switch",
        "case",
        "SELECT",
        "INSERT",
        "UPDATE",
        "DELETE",
        "CREATE",
        "ALTER",
        "int",
        "void",
        "#include",
        "#define",
        "#import",
        "#!",
    ];
    let word_end = line
        .find(|ch: char| ch.is_whitespace() || ch == '(' || ch == '{')
        .unwrap_or(line.len());
    let word = &line[..word_end];
    word_end < line.len() && KEYWORDS.contains(&word) || (word.starts_with("#!") && line.len() > 2)
}

/// `2026-11-07 ...`, `12:30:01 ...`, `[1] ...`, `INFO ...`, `[WARN] ...`.
fn is_log_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes
            .get(range)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    if digits(0..4) && bytes.get(4) == Some(&b'-') && digits(5..7) && bytes.get(7) == Some(&b'-') {
        return true;
    }
    if digits(0..2) && bytes.get(2) == Some(&b':') && digits(3..5) && bytes.get(5) == Some(&b':') {
        return true;
    }
    if bytes.first() == Some(&b'[') && bytes.get(1).is_some_and(u8::is_ascii_digit) {
        return true;
    }
    let word = line
        .trim_start_matches('[')
        .split(|ch: char| !ch.is_ascii_alphabetic())
        .next()
        .unwrap_or("");
    matches!(
        word,
        "INFO" | "WARN" | "WARNING" | "ERROR" | "DEBUG" | "TRACE" | "FATAL"
    )
}

/// `ls -l` lines: `drwxr-xr-x`, `-rw-r--r--`, `lrwxrwxrwx`.
fn is_permission_listing(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.len() > 10
        && matches!(bytes[0], b'-' | b'd' | b'l' | b'c' | b'b' | b'p' | b's')
        && bytes[1..10]
            .iter()
            .all(|byte| matches!(byte, b'-' | b'r' | b'w' | b'x' | b's' | b'S' | b't' | b'T'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        for tag in ["bob", "SVGBOB", "ascii", "Diagram"] {
            assert_eq!(eligibility(tag), Eligibility::Tagged, "{tag}");
        }
        for tag in ["", "text", "TEXT"] {
            assert_eq!(eligibility(tag), Eligibility::Heuristic, "{tag}");
        }
        for tag in ["rust", "txt", "mermaid", "diagrams"] {
            assert_eq!(eligibility(tag), Eligibility::No, "{tag}");
        }
    }

    #[test]
    fn line_recognizers() {
        for prompt in [
            "$ cargo build",
            "PS C:\\repo> dir",
            "C:\\repo>dir",
            "user@host:~/src$ ls",
            ">>> print(1)",
        ] {
            assert!(is_prompt(prompt), "{prompt}");
        }
        for text in ["a@b c", "PS2 thing", "Client --> Server"] {
            assert!(!is_prompt(text), "{text}");
        }
        assert!(is_diff_or_diagnostic("--> src/main.rs:2:5"));
        assert!(is_diff_or_diagnostic("error[E0308]: mismatched types"));
        assert!(!is_diff_or_diagnostic("--> next step"));
        assert!(!is_diff_or_diagnostic("errors are retried"));
        assert!(starts_with_keyword("fn main() {"));
        assert!(starts_with_keyword("#!/bin/sh"));
        assert!(!starts_with_keyword("function"));
        assert!(!starts_with_keyword("Adaptive policy controller"));
        assert!(is_log_line("2026-11-07 12:00:00 started"));
        assert!(is_log_line("[INFO] ready"));
        assert!(is_log_line("12:30:01 tick"));
        assert!(!is_log_line("Information flow"));
        assert!(is_permission_listing("drwxr-xr-x 2 u g 4096 Nov 7 dir"));
        assert!(!is_permission_listing("-- --> x"));
    }
}
