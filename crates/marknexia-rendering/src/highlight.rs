//! Code highlighting: a Marknexia-owned, linear-time reproduction of the
//! ColorCode 2.0.15 `HtmlClassFormatter` output for C#, the only ColorCode
//! language the frozen corpus exercises. See the Rendering/Highlighting
//! section of `docs/superpowers/specs/2026-09-12-rust-content-dependency-decision.md`
//! and REND-2 in `compat/decisions/rendering-differences.md`.
//!
//! ColorCode compiles a language's rules into one alternation, scans for the
//! leftmost match (earlier rules win at the same position), and wraps each
//! non-empty scoped capture in `<span class="{style}">`. Each C# rule below is
//! hand-translated from its regex (read from the ColorCode.Core 2.0.15
//! string heap) into a direct matcher with the same backtracking outcome.
//! Memoized failure bounds keep the scan linear where the regex is quadratic.

use crate::text::{html_encode, is_space, is_word};

/// `ColorCodeSyntaxHighlighter.HighlightCode`.
pub(crate) fn highlight(code: &str, language: &str) -> String {
    if code.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(code.len() * 2 + 64);
    if is_csharp(language) {
        out.push_str("<div class=\"csharp\"><pre>\n");
        CSharp::new(code).write(&mut out);
        out.push_str("\n</pre></div>");
    } else {
        out.push_str("<pre><code>");
        html_encode(code, &mut out);
        out.push_str("</code></pre>");
    }
    out
}

/// `ResolveLanguage` for C#: `c#`/`cs` map to `csharp`, and ColorCode's C#
/// language also answers to its id and the `cake` alias.
fn is_csharp(language: &str) -> bool {
    let normalized = language.trim().to_lowercase();
    matches!(normalized.as_str(), "c#" | "cs" | "csharp" | "cake")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Style {
    Comment,
    XmlDocTag,
    XmlDocComment,
    String,
    Verbatim,
    Keyword,
    Preprocessor,
    Number,
}

impl Style {
    const fn class(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::XmlDocTag => "xmlDocTag",
            Self::XmlDocComment => "xmlDocComment",
            Self::String => "string",
            Self::Verbatim => "stringCSharpVerbatim",
            Self::Keyword => "keyword",
            Self::Preprocessor => "preprocessorKeyword",
            Self::Number => "number",
        }
    }
}

const KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "ascending",
    "base",
    "bool",
    "break",
    "by",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "descending",
    "do",
    "double",
    "dynamic",
    "else",
    "enum",
    "equals",
    "event",
    "explicit",
    "extern",
    "false",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "from",
    "get",
    "goto",
    "group",
    "if",
    "implicit",
    "in",
    "int",
    "into",
    "interface",
    "internal",
    "is",
    "join",
    "let",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "on",
    "operator",
    "orderby",
    "out",
    "override",
    "params",
    "partial",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "select",
    "set",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "var",
    "virtual",
    "void",
    "volatile",
    "where",
    "while",
    "yield",
    "async",
    "await",
    "warning",
    "disable",
];

const ATTRIBUTE_TARGETS: &[&str] = &[
    "assembly", "module", "type", "return", "param", "method", "field", "property", "event",
];

const DIRECTIVES: &[&str] = &[
    "#define",
    "#elif",
    "#else",
    "#endif",
    "#endregion",
    "#error",
    "#if",
    "#line",
    "#pragma",
    "#region",
    "#undef",
    "#warning",
];

/// One rule match: its end and its scoped, ordered, disjoint captures.
struct Match {
    end: usize,
    scopes: Vec<(usize, usize, Style)>,
}

impl Match {
    fn whole(start: usize, end: usize, style: Style) -> Self {
        Self {
            end,
            scopes: vec![(start, end, style)],
        }
    }
}

struct CSharp<'a> {
    code: &'a str,
    bytes: &'a [u8],
    /// No `*/` at or after this offset.
    no_comment_end_from: usize,
    /// Quote-string searches (`'…'` and `"…"`) starting before these offsets
    /// already failed to the end of their line.
    char_dead_until: usize,
    string_dead_until: usize,
    /// No `]` at or after this offset.
    no_bracket_from: usize,
    /// A preprocessor attempt whose leading `\s*` reached this offset failed.
    directive_dead_until: usize,
}

impl<'a> CSharp<'a> {
    fn new(code: &'a str) -> Self {
        Self {
            code,
            bytes: code.as_bytes(),
            no_comment_end_from: usize::MAX,
            char_dead_until: 0,
            string_dead_until: 0,
            no_bracket_from: usize::MAX,
            directive_dead_until: 0,
        }
    }

    /// `HtmlClassFormatter.Write` for every parsed fragment.
    fn write(mut self, out: &mut String) {
        let code = self.code;
        let mut plain_from = 0;
        let mut index = 0;
        while index < code.len() {
            if let Some(found) = self.match_at(index) {
                html_encode(&code[plain_from..index], out);
                let mut cursor = index;
                for (start, end, style) in found.scopes {
                    if start == end {
                        continue;
                    }
                    html_encode(&code[cursor..start], out);
                    out.push_str("<span class=\"");
                    out.push_str(style.class());
                    out.push_str("\">");
                    html_encode(&code[start..end], out);
                    out.push_str("</span>");
                    cursor = end;
                }
                html_encode(&code[cursor..found.end], out);
                index = found.end;
                plain_from = index;
            } else {
                index += utf8_len(self.bytes[index]);
            }
        }
        html_encode(&code[plain_from..], out);
    }

    /// The combined regex at one position: the first rule that matches.
    fn match_at(&mut self, index: usize) -> Option<Match> {
        match self.bytes[index] {
            b'/' => self
                .block_comment(index)
                .or_else(|| self.xml_doc(index))
                .or_else(|| self.line_comment(index)),
            b'\'' => self.char_literal(index),
            b'@' => self.verbatim(index),
            b'"' => self.string(index),
            b'[' => self.attribute(index),
            _ => None,
        }
        .or_else(|| self.directive(index))
        .or_else(|| self.keyword(index))
        .or_else(|| self.number(index))
    }

    fn line_end(&self, from: usize) -> usize {
        self.code[from..]
            .find('\n')
            .map_or(self.code.len(), |i| from + i)
    }

    fn previous_is_word(&self, index: usize) -> bool {
        self.code[..index].chars().next_back().is_some_and(is_word)
    }

    /// `/\*([^*]|[\r\n]|(\*+([^*/]|[\r\n])))*\*+/`: through the first `*/`.
    fn block_comment(&mut self, index: usize) -> Option<Match> {
        if !self.code[index..].starts_with("/*") || index + 2 >= self.no_comment_end_from {
            return None;
        }
        match self.code[index + 2..].find("*/") {
            Some(offset) => Some(Match::whole(index, index + 2 + offset + 2, Style::Comment)),
            None => {
                self.no_comment_end_from = index + 2;
                None
            }
        }
    }

    /// `(///)(?:\s*?(<[/a-zA-Z0-9\s"=]+>))*([^\r\n]*)`.
    fn xml_doc(&self, index: usize) -> Option<Match> {
        if !self.code[index..].starts_with("///") {
            return None;
        }
        let mut scopes = vec![(index, index + 3, Style::XmlDocTag)];
        let mut cursor = index + 3;
        loop {
            let lt = crate::text::skip_space(self.code, cursor);
            if self.bytes.get(lt) != Some(&b'<') {
                break;
            }
            let body_end = self.code[lt + 1..]
                .char_indices()
                .find(|&(_, c)| {
                    !(c.is_ascii_alphanumeric() || matches!(c, '/' | '"' | '=') || is_space(c))
                })
                .map_or(self.code.len(), |(offset, _)| lt + 1 + offset);
            if body_end == lt + 1 || self.bytes.get(body_end) != Some(&b'>') {
                break;
            }
            scopes.push((lt, body_end + 1, Style::XmlDocTag));
            cursor = body_end + 1;
        }
        let end = self.code[cursor..]
            .find(['\r', '\n'])
            .map_or(self.code.len(), |offset| cursor + offset);
        scopes.push((cursor, end, Style::XmlDocComment));
        Some(Match { end, scopes })
    }

    /// `(//.*?)\r?$` in multiline mode: the capture stops before a `\r` that
    /// ends the line.
    fn line_comment(&self, index: usize) -> Option<Match> {
        if !self.code[index..].starts_with("//") {
            return None;
        }
        let end = self.line_end(index);
        let capture_end = if end >= index + 3 && self.bytes[end - 1] == b'\r' {
            end - 1
        } else {
            end
        };
        Some(Match {
            end,
            scopes: vec![(index, capture_end, Style::Comment)],
        })
    }

    /// The first `quote` after `open` on the same line not preceded by `\`.
    fn closing_quote(&self, open: usize, quote: u8) -> Result<usize, usize> {
        let mut previous = quote;
        for (offset, &byte) in self.bytes[open + 1..].iter().enumerate() {
            match byte {
                b'\n' => return Err(open + 1 + offset),
                _ if byte == quote && previous != b'\\' => return Ok(open + 1 + offset),
                _ => previous = byte,
            }
        }
        Err(self.bytes.len())
    }

    /// `'[^\n]*?(?<!\\)'`.
    fn char_literal(&mut self, index: usize) -> Option<Match> {
        if index < self.char_dead_until {
            return None;
        }
        match self.closing_quote(index, b'\'') {
            Ok(close) => Some(Match::whole(index, close + 1, Style::String)),
            Err(line_end) => {
                self.char_dead_until = line_end;
                None
            }
        }
    }

    /// `(?s)("[^\n]*?(?<!\\)")`.
    fn string(&mut self, index: usize) -> Option<Match> {
        self.string_span(index)
            .map(|end| Match::whole(index, end, Style::String))
    }

    fn string_span(&mut self, index: usize) -> Option<usize> {
        if index < self.string_dead_until {
            return None;
        }
        match self.closing_quote(index, b'"') {
            Ok(close) => Some(close + 1),
            Err(line_end) => {
                self.string_dead_until = line_end;
                None
            }
        }
    }

    /// `(?s)@"(?:""|.)*?"(?!")`. The lazy loop prefers to stop at a quote not
    /// followed by a quote and otherwise consumes `""` as a unit. If that path
    /// reaches the end, backtracking splits the last `""` pair, so the match
    /// ends at the last quote of the input.
    fn verbatim(&self, index: usize) -> Option<Match> {
        if !self.code[index..].starts_with("@\"") {
            return None;
        }
        let start = index + 2;
        let mut cursor = start;
        while cursor < self.bytes.len() {
            if self.bytes[cursor] == b'"' {
                if self.bytes.get(cursor + 1) != Some(&b'"') {
                    return Some(Match::whole(index, cursor + 1, Style::Verbatim));
                }
                cursor += 2;
            } else {
                cursor += 1;
            }
        }
        let last = self.code[start..].rfind('"')?;
        Some(Match::whole(index, start + last + 1, Style::Verbatim))
    }

    /// `\[(assembly|…|event):[^\]"]*("[^\n]*?(?<!\\)")?[^\]]*\]`.
    fn attribute(&mut self, index: usize) -> Option<Match> {
        let target_start = index + 1;
        let target = ATTRIBUTE_TARGETS.iter().find(|target| {
            self.code[target_start..].starts_with(*target)
                && self.bytes.get(target_start + target.len()) == Some(&b':')
        })?;
        let target_end = target_start + target.len();
        let after_colon = target_end + 1;
        let stop = self.code[after_colon..]
            .find([']', '"'])
            .map(|offset| after_colon + offset)?;
        let mut scopes = vec![(target_start, target_end, Style::Keyword)];
        if self.bytes[stop] == b']' {
            return Some(Match {
                end: stop + 1,
                scopes,
            });
        }
        if let Some(string_end) = self.string_span(stop)
            && let Some(bracket) = self.bracket_from(string_end)
        {
            scopes.push((stop, string_end, Style::String));
            return Some(Match {
                end: bracket + 1,
                scopes,
            });
        }
        let bracket = self.bracket_from(stop)?;
        Some(Match {
            end: bracket + 1,
            scopes,
        })
    }

    fn bracket_from(&mut self, from: usize) -> Option<usize> {
        if from >= self.no_bracket_from {
            return None;
        }
        let found = self.code[from..].find(']').map(|offset| from + offset);
        if found.is_none() {
            self.no_bracket_from = from;
        }
        found
    }

    /// `^\s*(\#define|…|\#warning).*?$` in multiline mode. `\s*` crosses
    /// line breaks, so a directive may follow blank lines.
    fn directive(&mut self, index: usize) -> Option<Match> {
        if index != 0 && self.bytes[index - 1] != b'\n' {
            return None;
        }
        if index < self.directive_dead_until {
            return None;
        }
        let hash = crate::text::skip_space(self.code, index);
        let Some(directive) = DIRECTIVES
            .iter()
            .find(|directive| self.code[hash..].starts_with(*directive))
        else {
            self.directive_dead_until = hash;
            return None;
        };
        let end = self.line_end(hash);
        Some(Match {
            end,
            scopes: vec![(hash, hash + directive.len(), Style::Preprocessor)],
        })
    }

    /// `\b(abstract|…|disable)\b`.
    fn keyword(&self, index: usize) -> Option<Match> {
        let first = self.bytes[index];
        if !first.is_ascii_lowercase() || self.previous_is_word(index) {
            return None;
        }
        let word_end = self.code[index..]
            .char_indices()
            .find(|&(_, c)| !is_word(c))
            .map_or(self.code.len(), |(offset, _)| index + offset);
        let word = &self.code[index..word_end];
        KEYWORDS
            .contains(&word)
            .then(|| Match::whole(index, word_end, Style::Keyword))
    }

    /// `\b[0-9]{1,}\b`.
    fn number(&self, index: usize) -> Option<Match> {
        if !self.bytes[index].is_ascii_digit() || self.previous_is_word(index) {
            return None;
        }
        let end = self.bytes[index..]
            .iter()
            .position(|byte| !byte.is_ascii_digit())
            .map_or(self.bytes.len(), |offset| index + offset);
        if self.code[end..].chars().next().is_some_and(is_word) {
            return None;
        }
        Some(Match::whole(index, end, Style::Number))
    }
}

const fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs(code: &str) -> String {
        let html = highlight(code, "c#");
        html.strip_prefix("<div class=\"csharp\"><pre>\n")
            .and_then(|rest| rest.strip_suffix("\n</pre></div>"))
            .expect("C# wrapper")
            .to_owned()
    }

    #[test]
    fn fixture_line() {
        assert_eq!(
            cs("var value = 1;\n"),
            "<span class=\"keyword\">var</span> value = <span class=\"number\">1</span>;\n"
        );
    }

    #[test]
    fn language_resolution() {
        for language in ["c#", "C#", "cs", " CSharp ", "cake"] {
            assert!(highlight("x", language).starts_with("<div class=\"csharp\">"));
        }
        assert_eq!(highlight("a<b", "python"), "<pre><code>a&lt;b</code></pre>");
        assert_eq!(highlight("a<b", ""), "<pre><code>a&lt;b</code></pre>");
        assert_eq!(highlight("", "c#"), "");
    }

    #[test]
    fn words_numbers_and_boundaries() {
        assert_eq!(
            cs("avar var_ x1 1x 1.5 yield"),
            "avar var_ x1 1x <span class=\"number\">1</span>.<span class=\"number\">5</span> <span class=\"keyword\">yield</span>"
        );
        assert_eq!(
            cs("ascending as"),
            "<span class=\"keyword\">ascending</span> <span class=\"keyword\">as</span>"
        );
        assert_eq!(cs("Var éif"), "Var éif");
    }

    #[test]
    fn comments() {
        assert_eq!(
            cs("a /* x\n*/ b // c\r\n// d"),
            "a <span class=\"comment\">/* x\n*/</span> b <span class=\"comment\">// c</span>\r\n<span class=\"comment\">// d</span>"
        );
        assert_eq!(cs("/*/ int"), "/*/ <span class=\"keyword\">int</span>");
        assert_eq!(
            cs("/// <summary>Hi</summary>\n"),
            "<span class=\"xmlDocTag\">///</span> <span class=\"xmlDocTag\">&lt;summary&gt;</span><span class=\"xmlDocComment\">Hi&lt;/summary&gt;</span>\n"
        );
        assert_eq!(cs("///"), "<span class=\"xmlDocTag\">///</span>");
    }

    #[test]
    fn strings() {
        assert_eq!(
            cs(r#""a\"b" 'c' '\'' "x"#),
            r#"<span class="string">&quot;a\&quot;b&quot;</span> <span class="string">&#39;c&#39;</span> <span class="string">&#39;\&#39;&#39;</span> &quot;x"#
        );
        assert_eq!(
            cs("@\"a\"\"b\" c"),
            "<span class=\"stringCSharpVerbatim\">@&quot;a&quot;&quot;b&quot;</span> c"
        );
        // Unterminated: backtracking splits the last pair.
        assert_eq!(
            cs("@\"a\"\"b"),
            "<span class=\"stringCSharpVerbatim\">@&quot;a&quot;&quot;</span>b"
        );
        assert_eq!(cs("@\"ab"), "@&quot;ab");
    }

    #[test]
    fn attributes_and_directives() {
        assert_eq!(
            cs("[assembly: Guid(\"x\")]"),
            "[<span class=\"keyword\">assembly</span>: Guid(<span class=\"string\">&quot;x&quot;</span>)]"
        );
        assert_eq!(
            cs("  #region Name\nint"),
            "  <span class=\"preprocessorKeyword\">#region</span> Name\n<span class=\"keyword\">int</span>"
        );
        assert_eq!(cs("x #if"), "x #<span class=\"keyword\">if</span>");
    }

    #[test]
    fn hostile_shapes_stay_linear() {
        let started = std::time::Instant::now();
        for sample in ["\\'", "\\\"", "/*", "[type:\"", "\n \n", "@\"\"", "///<a"] {
            let code = sample.repeat(200_000);
            let _ = highlight(&code, "cs");
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(20));
    }
}
