//! Code highlighting with ColorCode 2.0.15 parity for every ColorCode
//! language (gate G1). See REND-2 in `compat/decisions/rendering-differences.md`
//! and HL-1 to HL-4 in `compat/decisions/highlight-differences.md`.
//!
//! This is a port of `ColorCodeSyntaxHighlighter.HighlightCode` and of the
//! ColorCode pieces it runs:
//!
//! * `LanguageRepository.FindById` and Marknexia's `ResolveLanguage`;
//! * `LanguageParser.Parse`: the compiled language regex (one alternation of
//!   every rule, generated into `languages.rs` from the .NET
//!   `LanguageCompiler`) is matched repeatedly; each match's group captures
//!   become scopes, nested languages (`&javascript`) are parsed inside their
//!   capture, and the scopes are arranged into a tree, quirks included;
//! * `HtmlClassFormatter`: `<div class="{css}"><pre>`, `<span class="…">`
//!   insertions sorted stably by position, and `WebUtility.HtmlEncode` text.
//!
//! Anything .NET would throw on (an out-of-range `Substring`, a missing
//! nested language, a scope with two parents) returns the plain
//! `<pre><code>` fallback, as `HighlightCode`'s `catch` does. So does an
//! exhausted regex work budget ([`regex::Budget`]), which is where Rust is
//! deliberately different: .NET would backtrack for minutes or hours (HL-1).
//!
//! C# runs through a linear-time hand-written tokenizer (`csharp.rs`) that
//! produces the same output as the engine for the same rules.

use std::sync::OnceLock;

use regex::{Budget, Match, Regex, Stop};

mod csharp;
#[rustfmt::skip]
mod languages;
pub(crate) mod regex;
#[rustfmt::skip]
mod unicode;

/// One ColorCode language (generated).
pub(crate) struct LanguageDef {
    pub(crate) id: &'static str,
    pub(crate) css: &'static str,
    /// Lower-case aliases that `HasAlias` accepts.
    pub(crate) aliases: &'static [&'static str],
    /// The `LanguageCompiler` regex.
    pub(crate) pattern: &'static str,
    /// Scope name per group number; `&id` names a nested language.
    pub(crate) captures: &'static [Option<&'static str>],
    /// `GetGroupNumbers().Length` of the .NET regex.
    pub(crate) groups: usize,
}

/// A `StyleDictionary` entry (generated).
pub(crate) struct StyleDef {
    scope: &'static str,
    class: &'static str,
}

const PLAIN_OPEN: &str = "<pre><code>";
const PLAIN_CLOSE: &str = "</code></pre>";
const PRE_OPEN: &str = "><pre>\n";
const DIV_OPEN: &str = "<div class=\"";
const BLOCK_CLOSE: &str = "\n</pre></div>";

/// Regex steps that one document may spend on highlighting, before the
/// per-unit credit: about one second of matching, shared by every code
/// block of the document (HL-1).
const BUDGET_BASE: u64 = 1 << 27;
/// Regex steps credited per UTF-16 code unit of each highlighted block. The
/// oracle corpus needs at most 141 per unit for ordinary code (HL-1).
const BUDGET_PER_UNIT: u64 = 512;
/// The most steps one document can be granted, base and credits together:
/// about 16 s of matching, whatever the document size (HL-1).
const BUDGET_CAP: u64 = 1 << 31;

/// The regex budget of one document: every block draws from it, and
/// `cancel` is polled while it is spent.
pub(crate) fn document_budget(cancel: &(dyn Fn() -> bool + Sync)) -> Budget<'_> {
    Budget::new(BUDGET_BASE)
        .with_cap(BUDGET_CAP)
        .with_cancel(cancel)
}

/// The budget of a document that has one block of `units` UTF-16 units.
pub(crate) const fn budget_for(units: u64) -> u64 {
    let steps = BUDGET_BASE.saturating_add(BUDGET_PER_UNIT.saturating_mul(units));
    if steps > BUDGET_CAP {
        BUDGET_CAP
    } else {
        steps
    }
}

/// Why [`highlight`] produced no block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HighlightError {
    /// The output would exceed the byte limit.
    Overflow,
    /// The caller cancelled the render.
    Cancelled,
}

/// `ColorCodeSyntaxHighlighter.HighlightCode`, or an error as soon as the
/// output would exceed `limit` bytes or the render is cancelled. Regex work
/// is drawn from the document's `budget`.
pub(crate) fn highlight(
    code: &str,
    language: &str,
    limit: usize,
    budget: &mut Budget<'_>,
) -> Result<String, HighlightError> {
    highlight_with(code, language, limit, Path::Default, budget).0
}

/// Which implementation highlights C#.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Path {
    /// The C# fast path for C#, the engine for everything else.
    Default,
    /// The engine for every language (tests compare it with the fast path).
    Engine,
}

/// [`highlight`], plus the regex steps it used.
pub(crate) fn highlight_with(
    code: &str,
    language: &str,
    limit: usize,
    path: Path,
    budget: &mut Budget<'_>,
) -> (Result<String, HighlightError>, u64) {
    if code.is_empty() {
        return (Ok(String::new()), 0);
    }
    let Some(index) = resolve_language(language) else {
        return (plain(code, limit), 0);
    };
    let definition = &languages::LANGUAGES[index];
    let mut out = Sink::new(limit, code.len());
    out.push_str(DIV_OPEN);
    out.push_str(definition.css);
    out.push_str("\"");
    out.push_str(PRE_OPEN);
    let mut steps = 0;
    let result = if definition.id == "c#" && path == Path::Default {
        if csharp::write(code, &mut out.text, limit) {
            Ok(())
        } else {
            out.over = true;
            Ok(())
        }
    } else {
        let text: Vec<u16> = code.encode_utf16().collect();
        budget.credit(BUDGET_PER_UNIT.saturating_mul(text.len() as u64));
        let before = budget.left();
        let result = parse(index, &text, budget, &mut out);
        steps = before - budget.left();
        result
    };
    let html = match result {
        Ok(()) => {
            out.push_str(BLOCK_CLOSE);
            if out.over {
                Err(HighlightError::Overflow)
            } else {
                Ok(out.text)
            }
        }
        Err(Failure::Fallback) => plain(code, limit),
        Err(Failure::Cancelled) => Err(HighlightError::Cancelled),
    };
    (html, steps)
}

/// `<pre><code>{WebUtility.HtmlEncode(code)}</code></pre>`.
fn plain(code: &str, limit: usize) -> Result<String, HighlightError> {
    let mut out = String::with_capacity(PLAIN_OPEN.len() + code.len() + PLAIN_CLOSE.len());
    out.push_str(PLAIN_OPEN);
    web_encode(code, &mut out);
    out.push_str(PLAIN_CLOSE);
    if out.len() <= limit {
        Ok(out)
    } else {
        Err(HighlightError::Overflow)
    }
}

/// Exact lower bound of `highlight(code, language, _)`'s length: exact for
/// an unknown language (the plain fallback). A known language can still
/// fall back to plain, so its bound is the smaller of the two shapes.
pub(crate) fn minimum_len(code: &str, language: &str) -> usize {
    if code.is_empty() {
        return 0;
    }
    let plain = PLAIN_OPEN.len() + web_encoded_len(code) + PLAIN_CLOSE.len();
    match resolve_language(language) {
        None => plain,
        Some(index) => {
            let css = languages::LANGUAGES[index].css;
            // Spans can split a surrogate pair into two U+FFFD (6 bytes).
            let text = code
                .chars()
                .map(|c| if c > '\u{ffff}' { 6 } else { web_char_len(c) })
                .sum::<usize>();
            let highlighted =
                DIV_OPEN.len() + css.len() + 1 + PRE_OPEN.len() + text + BLOCK_CLOSE.len();
            plain.min(highlighted)
        }
    }
}

/// Every language id, in `LanguageRepository` order.
pub(crate) fn language_ids() -> Vec<&'static str> {
    languages::LANGUAGES
        .iter()
        .map(|language| language.id)
        .collect()
}

/// `ResolveLanguage`: the index of the ColorCode language, if any.
fn resolve_language(language: &str) -> Option<usize> {
    let normalized = language.trim().to_lowercase();
    if normalized.is_empty() {
        return None;
    }
    let find = find_by_id;
    match normalized.as_str() {
        "c#" | "cs" => find("csharp"),
        "c++" | "c" => find("cpp"),
        "js" => find("javascript"),
        "ts" => find("typescript").or_else(|| find("javascript")),
        "md" => find("markdown"),
        "ps1" | "posh" => find("powershell"),
        other => find(other),
    }
}

/// `LanguageRepository.FindById`: the first language whose id matches
/// case-insensitively or that has the alias.
fn find_by_id(id: &str) -> Option<usize> {
    let lower = id.to_lowercase();
    languages::LANGUAGES.iter().position(|language| {
        language.id.to_lowercase() == lower || language.aliases.contains(&lower.as_str())
    })
}

fn compiled(index: usize) -> Option<&'static Regex> {
    static COMPILED: OnceLock<Vec<OnceLock<Option<Regex>>>> = OnceLock::new();
    let cells = COMPILED.get_or_init(|| {
        languages::LANGUAGES
            .iter()
            .map(|_| OnceLock::new())
            .collect()
    });
    cells[index]
        .get_or_init(|| {
            let definition = &languages::LANGUAGES[index];
            // Only groups with a scope need capture history.
            let observed = |group: usize| {
                definition
                    .captures
                    .get(group)
                    .is_some_and(|scope| scope.is_some_and(|name| !name.is_empty()))
            };
            let regex = Regex::with_observed(definition.pattern, &observed).ok()?;
            // The capture-to-scope list indexes .NET group numbers.
            debug_assert_eq!(regex.group_count(), definition.groups);
            Some(regex)
        })
        .as_ref()
}

fn style_class(scope: &str) -> Option<&'static str> {
    languages::STYLES
        .iter()
        .find(|style| style.scope == scope)
        .map(|style| style.class)
}

/// Highlighting failed where .NET throws (or the budget ran out): the
/// caller falls back to plain text.
#[derive(Debug)]
enum Failure {
    /// Return the plain `<pre><code>` fallback.
    Fallback,
    /// Stop: the render was cancelled.
    Cancelled,
}

impl From<Stop> for Failure {
    fn from(stop: Stop) -> Self {
        match stop {
            Stop::Exhausted => Self::Fallback,
            Stop::Cancelled => Self::Cancelled,
        }
    }
}

/// Output that stops growing past the limit but keeps the parse going, so
/// a later failure still selects the plain fallback exactly as in .NET.
struct Sink {
    text: String,
    limit: usize,
    over: bool,
}

impl Sink {
    fn new(limit: usize, code_len: usize) -> Self {
        Self {
            text: String::with_capacity((code_len * 2 + 64).min(limit)),
            limit,
            over: false,
        }
    }

    fn push_str(&mut self, text: &str) {
        if self.over {
            return;
        }
        if self.text.len() + text.len() > self.limit {
            self.over = true;
            self.text = String::new();
        } else {
            self.text.push_str(text);
        }
    }

    fn push_encoded(&mut self, units: &[u16]) {
        if self.over {
            return;
        }
        web_encode_units(units, &mut self.text);
        if self.text.len() > self.limit {
            self.over = true;
            self.text = String::new();
        }
    }
}

/// A `ColorCode.Parsing.Scope` in an arena.
struct Scope {
    name: &'static str,
    index: i64,
    length: i64,
    parent: Option<usize>,
    children: Vec<usize>,
}

/// `LanguageParser.Parse` with `HtmlClassFormatter.Write` as the handler.
fn parse(
    language: usize,
    text: &[u16],
    budget: &mut Budget<'_>,
    out: &mut Sink,
) -> Result<(), Failure> {
    let regex = compiled(language).ok_or(Failure::Fallback)?;
    let mut current = 0;
    let mut from = 0;
    while from <= text.len() {
        let Some(found) = regex.find_at(text, from, budget)? else {
            break;
        };
        if found.start > current {
            write(&text[current..found.start], &[], &[], out)?;
        }
        if found.end > found.start {
            let mut arena = Vec::new();
            let styles = captured_styles(&found, found.start, language, text, budget, &mut arena)?;
            let tree = style_tree(styles, &mut arena)?;
            write(&text[found.start..found.end], &tree, &arena, out)?;
        }
        current = found.end;
        // NextMatch: after an empty match the search moves one unit on.
        from = if found.end == found.start {
            found.end + 1
        } else {
            found.end
        };
    }
    if current < text.len() {
        write(&text[current..], &[], &[], out)?;
    }
    Ok(())
}

/// `GetCapturedStyles`: one scope per capture of every group that is
/// non-empty (by its last capture) and has a scope name.
fn captured_styles(
    found: &Match,
    current_index: usize,
    language: usize,
    text: &[u16],
    budget: &mut Budget<'_>,
    arena: &mut Vec<Scope>,
) -> Result<Vec<usize>, Failure> {
    let captures = languages::LANGUAGES[language].captures;
    let mut styles = Vec::new();
    for (group, history) in found.groups.iter().enumerate() {
        let Some(&(start, end)) = history.last() else {
            continue;
        };
        if end <= start || group >= captures.len() {
            continue;
        }
        let Some(name) = captures[group].filter(|name| !name.is_empty()) else {
            continue;
        };
        for &(start, end) in history {
            if end < start {
                // Captures are normalized, so this cannot happen; skip
                // rather than slice out of order.
                continue;
            }
            let offset = start as i64 - current_index as i64;
            if let Some(nested) = name.strip_prefix('&') {
                nested_styles(
                    &text[start..end],
                    offset,
                    nested,
                    budget,
                    arena,
                    &mut styles,
                )?;
            } else {
                arena.push(Scope {
                    name,
                    index: offset,
                    length: (end - start) as i64,
                    parent: None,
                    children: Vec::new(),
                });
                styles.push(arena.len() - 1);
            }
        }
    }
    Ok(styles)
}

/// `AppendCapturedStylesForNestedLanguage`, including its quirk: the whole
/// nested tree is shifted by `offset` once per top-level scope.
fn nested_styles(
    value: &[u16],
    offset: i64,
    id: &str,
    budget: &mut Budget<'_>,
    arena: &mut Vec<Scope>,
    styles: &mut Vec<usize>,
) -> Result<(), Failure> {
    let language = find_by_id(id).ok_or(Failure::Fallback)?;
    let regex = compiled(language).ok_or(Failure::Fallback)?;
    let mut from = 0;
    while from <= value.len() {
        let Some(found) = regex.find_at(value, from, budget)? else {
            break;
        };
        let nested = captured_styles(&found, 0, language, value, budget, arena)?;
        let tree = style_tree(nested, arena)?;
        for &top in &tree {
            shift(&tree, offset, arena);
            styles.push(top);
        }
        from = if found.end == found.start {
            found.end + 1
        } else {
            found.end
        };
    }
    Ok(())
}

fn shift(scopes: &[usize], offset: i64, arena: &mut Vec<Scope>) {
    for &scope in scopes {
        arena[scope].index += offset;
        let children = std::mem::take(&mut arena[scope].children);
        shift(&children, offset, arena);
        arena[scope].children = children;
    }
}

/// `CreateCapturedStyleTree` with `AddScopeToNestedScopes`.
fn style_tree(mut styles: Vec<usize>, arena: &mut [Scope]) -> Result<Vec<usize>, Failure> {
    styles.sort_by_key(|&scope| arena[scope].index);
    let mut tree = Vec::with_capacity(styles.len());
    let mut current: Option<usize> = None;
    for scope in styles {
        let Some(mut candidate) = current else {
            tree.push(scope);
            current = Some(scope);
            continue;
        };
        loop {
            let (index, end) = (arena[scope].index, arena[scope].index + arena[scope].length);
            let parent = &arena[candidate];
            if index >= parent.index && end <= parent.index + parent.length {
                if arena[scope].parent.is_some() {
                    // "The child scope already has a parent."
                    return Err(Failure::Fallback);
                }
                arena[scope].parent = Some(candidate);
                arena[candidate].children.push(scope);
                current = Some(scope);
                break;
            }
            match arena[candidate].parent {
                Some(next) => candidate = next,
                None => {
                    tree.push(scope);
                    current = None;
                    break;
                }
            }
        }
    }
    Ok(tree)
}

/// `HtmlClassFormatter.Write`.
fn write(parsed: &[u16], tree: &[usize], arena: &[Scope], out: &mut Sink) -> Result<(), Failure> {
    enum Insert {
        Open(usize),
        Close,
    }
    fn collect(scope: usize, arena: &[Scope], inserts: &mut Vec<(i64, Insert)>) {
        inserts.push((arena[scope].index, Insert::Open(scope)));
        for &child in &arena[scope].children {
            collect(child, arena, inserts);
        }
        inserts.push((arena[scope].index + arena[scope].length, Insert::Close));
    }
    let mut inserts = Vec::new();
    for &scope in tree {
        collect(scope, arena, &mut inserts);
    }
    inserts.sort_by_key(|&(index, _)| index);
    let len = parsed.len() as i64;
    let mut offset: i64 = 0;
    for (index, insert) in inserts {
        // String.Substring(offset, index - offset) throws out of range.
        if index < offset || index > len {
            return Err(Failure::Fallback);
        }
        out.push_encoded(&parsed[offset as usize..index as usize]);
        match insert {
            Insert::Open(scope) => match style_class(arena[scope].name) {
                Some(class) if !class.is_empty() => {
                    out.push_str("<span class=\"");
                    out.push_str(class);
                    out.push_str("\">");
                }
                _ => out.push_str("<span>"),
            },
            Insert::Close => out.push_str("</span>"),
        }
        offset = index;
    }
    out.push_encoded(&parsed[offset as usize..]);
    Ok(())
}

/// `System.Net.WebUtility.HtmlEncode`.
pub(crate) fn web_encode(text: &str, out: &mut String) {
    let mut last = 0;
    for (index, c) in text.char_indices() {
        if web_char_len(c) == c.len_utf8() && !matches!(c, '<' | '>' | '&' | '"' | '\'') {
            continue;
        }
        out.push_str(&text[last..index]);
        push_entity(c, out);
        last = index + c.len_utf8();
    }
    out.push_str(&text[last..]);
}

fn push_entity(c: char, out: &mut String) {
    use std::fmt::Write as _;
    match c {
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '&' => out.push_str("&amp;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#39;"),
        _ => {
            let _ = write!(out, "&#{};", u32::from(c));
        }
    }
}

/// The encoded length of one character.
const fn web_char_len(c: char) -> usize {
    match c {
        '<' | '>' => 4,
        '&' | '\'' => 5,
        '"' => 6,
        '\u{a0}'..='\u{ff}' => 6,
        '\u{10000}'..='\u{1869f}' => 8,
        '\u{186a0}'..='\u{10ffff}' => 9,
        _ => c.len_utf8(),
    }
}

/// Length of `web_encode(text)`.
pub(crate) fn web_encoded_len(text: &str) -> usize {
    text.chars().map(web_char_len).sum()
}

/// `WebUtility.HtmlEncode` on UTF-16: a lone surrogate (a span boundary can
/// split a pair) becomes U+FFFD.
fn web_encode_units(units: &[u16], out: &mut String) {
    for c in char::decode_utf16(units.iter().copied()) {
        match c {
            Ok(c)
                if c.len_utf8() == web_char_len(c)
                    && !matches!(c, '<' | '>' | '&' | '"' | '\'') =>
            {
                out.push(c);
            }
            Ok(c) => push_entity(c, out),
            Err(_) => out.push('\u{fffd}'),
        }
    }
}

/// [`highlight`] for a document with one code block.
#[cfg(test)]
pub(crate) fn highlight_one(code: &str, language: &str, limit: usize) -> Option<String> {
    highlight(code, language, limit, &mut document_budget(&|| false)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn highlight(code: &str, language: &str, limit: usize) -> Option<String> {
        highlight_one(code, language, limit)
    }

    #[test]
    fn every_language_compiles_with_the_dotnet_group_count() {
        for (index, language) in languages::LANGUAGES.iter().enumerate() {
            let regex = compiled(index).unwrap_or_else(|| {
                panic!("{}: {:?}", language.id, Regex::new(language.pattern).err())
            });
            assert_eq!(regex.group_count(), language.groups, "{}", language.id);
            assert!(
                language.captures.len() <= language.groups,
                "{}",
                language.id
            );
        }
    }

    #[test]
    fn language_resolution() {
        let css = |language: &str| resolve_language(language).map(|i| languages::LANGUAGES[i].css);
        assert_eq!(css(" CSharp "), Some("csharp"));
        assert_eq!(css("c"), Some("cplusplus"));
        assert_eq!(css("ts"), Some("typescript"));
        assert_eq!(css("PS1"), Some("powershell"));
        assert_eq!(css("vbnet"), Some("vb-net"));
        assert_eq!(css("aspx-cs"), Some("aspx-cs"));
        assert_eq!(css("python"), Some("python"));
        assert_eq!(css(""), None);
        assert_eq!(css("text"), None);
        let plain = Some("<pre><code>a&lt;b</code></pre>".to_owned());
        assert_eq!(highlight("a<b", "", usize::MAX), plain);
        assert_eq!(highlight("a<b", "nope", 10), None);
    }

    #[test]
    fn web_encoding_matches_dotnet() {
        let mut out = String::new();
        web_encode("<a href='x'>é & ü\u{100}😀</a>", &mut out);
        assert_eq!(
            out,
            "&lt;a href=&#39;x&#39;&gt;&#233; &amp; &#252;\u{100}&#128512;&lt;/a&gt;"
        );
        assert_eq!(web_encoded_len("<a href='x'>é & ü\u{100}😀</a>"), out.len());
        let mut split = String::new();
        web_encode_units(&[0xd83d], &mut split);
        assert_eq!(split, "\u{fffd}");
    }

    #[test]
    fn output_limit_and_minimum() {
        for (code, language) in [("def f(): pass\n", "python"), ("x", "nope"), ("é😀", "sql")] {
            let html = highlight(code, language, usize::MAX).unwrap();
            assert!(minimum_len(code, language) <= html.len(), "{language}");
            assert_eq!(highlight(code, language, html.len()), Some(html.clone()));
            assert_eq!(highlight(code, language, html.len() - 1), None);
        }
        assert_eq!(minimum_len("x", "nope"), "<pre><code>x</code></pre>".len());
    }
}
