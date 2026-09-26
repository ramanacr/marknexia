//! Mermaid block isolation. Mermaid blocks render as ordinary
//! `<pre><code class="language-mermaid">` so the rendering layer can replace
//! them; the parser only extracts their source and reports limit violations.

use crate::MarkdownOptions;
use marknexia_core::contracts::{Diagnostic, DiagnosticSeverity};

/// .NET: `FencedCodeBlock.Info.Trim()` equals `mermaid`, ignoring case.
pub(crate) fn is_mermaid_language(language: &str) -> bool {
    language.eq_ignore_ascii_case("mermaid")
}

/// .NET `GetCodeBlockText`: skip empty lines, append `Environment.NewLine`
/// (`\r\n` on Windows) to each remaining line, then `TrimEnd()`.
pub(crate) fn diagram_source(literal: &str) -> String {
    let mut source = String::with_capacity(literal.len() + literal.len() / 16);
    for line in literal.split('\n').filter(|line| !line.is_empty()) {
        source.push_str(line);
        source.push_str("\r\n");
    }
    source.truncate(source.trim_end().len());
    source
}

/// Mirrors the .NET renderer's per-diagram limit order: count first, then the
/// UTF-8 size of the decoded rendered code text (the code lines with `\n`).
pub(crate) fn limit_diagnostic(
    number: usize,
    literal: &str,
    line: usize,
    options: &MarkdownOptions,
) -> Option<Diagnostic> {
    let rendered_len = literal.len() + usize::from(!literal.is_empty() && !literal.ends_with('\n'));
    let message = if number > options.max_diagram_count {
        "diagram limit exceeded"
    } else if rendered_len > options.max_diagram_source_bytes {
        "diagram source limit exceeded"
    } else {
        return None;
    };
    Some(Diagnostic {
        severity: DiagnosticSeverity::Warning,
        message: format!("mermaid-{number}: {message}"),
        source_line: Some(line),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_matches_dotnet_line_handling() {
        assert_eq!(diagram_source("graph TD\nA-->B\n"), "graph TD\r\nA-->B");
        assert_eq!(diagram_source("a\n\n  \nb  \n\n"), "a\r\n  \r\nb");
        assert_eq!(diagram_source(""), "");
    }

    #[test]
    fn limits_follow_renderer_order() {
        let options = MarkdownOptions {
            max_diagram_count: 1,
            max_diagram_source_bytes: 4,
            ..MarkdownOptions::default()
        };
        assert_eq!(limit_diagnostic(1, "abc\n", 7, &options), None);
        let size = limit_diagnostic(1, "abcd\n", 7, &options).unwrap();
        assert_eq!(size.message, "mermaid-1: diagram source limit exceeded");
        assert_eq!(size.source_line, Some(7));
        let count = limit_diagnostic(2, "abcd\n", 9, &options).unwrap();
        assert_eq!(count.message, "mermaid-2: diagram limit exceeded");
        assert!(is_mermaid_language("Mermaid"));
        assert!(!is_mermaid_language("mermaidjs"));
    }
}
