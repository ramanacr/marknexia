//! A small backtracking regex engine with .NET `System.Text.RegularExpressions`
//! semantics, for the ColorCode 2.0.15 language patterns only.
//!
//! The `regex` crate cannot run these patterns: they use lookbehind of
//! variable length (`(?<=<%@.+?language="c\#".*?%>)`), atomic groups,
//! balancing groups (`(?<-comment>…)`), conditionals (`(?(type)…)`) and need
//! every capture of a repeated group (`Group.Captures`), not only the last.
//! `fancy-regex` lacks balancing groups, conditionals and capture history.
//!
//! What this engine reproduces from .NET:
//!
//! * the syntax the ColorCode patterns use, with inline options
//!   (`(?imsx-imsx)` and `(?imsx-imsx:…)`), `IgnorePatternWhitespace`
//!   comments, and .NET group numbering (unnamed groups first, then named
//!   groups in order of appearance);
//! * matching on UTF-16 code units, with the .NET `\w`, `\d`, `\s`, `\b` and
//!   `IgnoreCase` tables measured by the oracle (`unicode.rs`);
//! * leftmost, priority-ordered backtracking; the `RegexInterpreter` loop
//!   rules (an iteration that matches empty ends a loop once its minimum is
//!   met); atomic lookarounds; lookbehind matched right to left;
//! * capture history with undo on backtracking, and balancing pops.
//!
//! **Work budget.** Backtracking can be exponential on hostile input. Every
//! instruction, every backtrack and every character a loop scans costs one
//! step from a [`Budget`]. When the budget runs out the match stops with
//! [`Exhausted`] and the caller falls back to plain text. Recursion depth is
//! bounded by the pattern's lookaround/atomic nesting, never by the input.

use super::unicode::{BOUNDARY_WORD, CASE_EQUIVALENTS, DIGIT, SPACE, WORD};

/// Why a match stopped before it finished.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Stop {
    /// The step budget or the backtracking-state cap ([`MAX_ENTRIES`]) ran
    /// out; the caller falls back to plain text.
    Exhausted,
    /// The caller's cancellation callback returned `true`.
    Cancelled,
}

/// The most backtracking state one match may hold: backtrack entries, undo
/// entries and recorded captures together (16, 12 and 8 bytes each), so a
/// match holds at most about 128 MiB, plus up to the same again while a
/// vector grows. More state stops the match with [`Stop::Exhausted`].
pub(crate) const MAX_ENTRIES: usize = 1 << 23;

/// The budget polls its cancellation callback once per this many steps.
const POLL_INTERVAL: u64 = 1 << 16;

/// Steps left for one document's highlighting (shared by every match it
/// runs), with an optional cancellation callback.
pub(crate) struct Budget<'c> {
    left: u64,
    /// Steps granted so far (start plus credits).
    granted: u64,
    /// Credits stop once `granted` reaches this.
    cap: u64,
    until_poll: u64,
    cancel: Option<&'c (dyn Fn() -> bool + Sync)>,
}

impl std::fmt::Debug for Budget<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budget")
            .field("left", &self.left)
            .field("granted", &self.granted)
            .field("cap", &self.cap)
            .finish_non_exhaustive()
    }
}

impl<'c> Budget<'c> {
    pub(crate) const fn new(steps: u64) -> Self {
        Self {
            left: steps,
            granted: steps,
            cap: u64::MAX,
            until_poll: POLL_INTERVAL,
            cancel: None,
        }
    }

    /// Limits the total that [`Self::credit`] can grant.
    pub(crate) const fn with_cap(mut self, cap: u64) -> Self {
        self.cap = cap;
        self
    }

    /// Polls `cancel` every [`POLL_INTERVAL`] steps.
    pub(crate) fn with_cancel(mut self, cancel: &'c (dyn Fn() -> bool + Sync)) -> Self {
        self.cancel = Some(cancel);
        self
    }

    #[inline]
    fn spend(&mut self, steps: u64) -> Result<(), Stop> {
        if self.left < steps {
            self.left = 0;
            return Err(Stop::Exhausted);
        }
        self.left -= steps;
        if let Some(cancel) = self.cancel {
            if self.until_poll <= steps {
                self.until_poll = POLL_INTERVAL;
                if cancel() {
                    return Err(Stop::Cancelled);
                }
            } else {
                self.until_poll -= steps;
            }
        }
        Ok(())
    }

    /// Adds `steps`, but never past the cap of granted steps.
    pub(crate) fn credit(&mut self, steps: u64) {
        let steps = steps.min(self.cap.saturating_sub(self.granted));
        self.granted += steps;
        self.left = self.left.saturating_add(steps);
    }

    pub(crate) const fn left(&self) -> u64 {
        self.left
    }
}
/// A pattern the parser does not accept.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PatternError(pub(crate) String);

fn error<T>(message: impl Into<String>) -> Result<T, PatternError> {
    Err(PatternError(message.into()))
}

// ---------------------------------------------------------------------------
// Character classes

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Category {
    Word,
    Digit,
    Space,
}

impl Category {
    fn contains(self, unit: u16) -> bool {
        let table = match self {
            Self::Word => WORD,
            Self::Digit => DIGIT,
            Self::Space => SPACE,
        };
        in_ranges(table, unit)
    }
}

fn in_ranges(table: &[(u16, u16)], unit: u16) -> bool {
    table
        .binary_search_by(|&(low, high)| {
            if high < unit {
                std::cmp::Ordering::Less
            } else if low > unit {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// A .NET character class: ranges and categories, optionally negated.
#[derive(Clone, Debug, Default)]
struct ClassDef {
    negate: bool,
    ranges: Vec<(u16, u16)>,
    categories: Vec<(Category, bool)>,
}

impl ClassDef {
    fn single(unit: u16) -> Self {
        Self {
            ranges: vec![(unit, unit)],
            ..Self::default()
        }
    }

    /// `RegexCharClass.AddCaseEquivalences`: every equivalent of every
    /// character in a range joins the class (categories are unchanged).
    fn add_case_equivalences(&mut self) {
        let mut extra = Vec::new();
        for &(unit, set) in CASE_EQUIVALENTS {
            if self
                .ranges
                .iter()
                .any(|&(low, high)| low <= unit && unit <= high)
            {
                extra.extend(set.iter().map(|&u| (u, u)));
            }
        }
        self.ranges.extend(extra);
    }

    fn contains_slow(&self, unit: u16) -> bool {
        let hit = self
            .ranges
            .iter()
            .any(|&(low, high)| low <= unit && unit <= high)
            || self
                .categories
                .iter()
                .any(|&(category, negated)| category.contains(unit) != negated);
        hit != self.negate
    }
}

/// A compiled class with an ASCII bitmap fast path.
#[derive(Clone, Debug)]
struct Class {
    ascii: u128,
    def: ClassDef,
}

impl Class {
    fn new(mut def: ClassDef) -> Self {
        def.ranges.sort_unstable();
        let mut ascii = 0u128;
        for unit in 0..128u16 {
            if def.contains_slow(unit) {
                ascii |= 1 << unit;
            }
        }
        Self { ascii, def }
    }

    #[inline]
    fn contains(&self, unit: u16) -> bool {
        if unit < 128 {
            self.ascii & (1 << unit) != 0
        } else {
            self.def.contains_slow(unit)
        }
    }

    /// Whether any code unit at or above 128 can match.
    fn may_match_non_ascii(&self) -> bool {
        self.def.negate
            || !self.def.categories.is_empty()
            || self.def.ranges.iter().any(|&(_, high)| high >= 128)
    }
}

// ---------------------------------------------------------------------------
// Syntax tree

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Assertion {
    /// `^`; multiline when `true`.
    Bol(bool),
    /// `$`; multiline when `true`.
    Eol(bool),
    /// `\A`.
    Start,
    /// `\Z`.
    EndZ,
    /// `\z`.
    End,
    /// `\b`.
    Boundary,
    /// `\B`.
    NonBoundary,
    /// `\G`.
    StartMatch,
}

#[derive(Clone, Debug)]
enum GroupRef {
    Unnamed(usize),
    Named(String),
}

#[derive(Clone, Debug)]
enum Node {
    Empty,
    /// A class index (literals are single-character classes).
    Char(usize),
    Assert(Assertion),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat {
        node: Box<Node>,
        min: u32,
        max: u32,
        lazy: bool,
    },
    Capture {
        group: Option<GroupRef>,
        uncapture: Option<GroupRef>,
        node: Box<Node>,
    },
    Atomic(Box<Node>),
    Look {
        behind: bool,
        negate: bool,
        node: Box<Node>,
    },
    Conditional {
        group: GroupRef,
        yes: Box<Node>,
        no: Box<Node>,
    },
}

const UNBOUNDED: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Default)]
struct Options {
    ignore_case: bool,
    multiline: bool,
    singleline: bool,
    explicit_capture: bool,
    extended: bool,
}

struct Parser<'a> {
    pattern: &'a [u16],
    pos: usize,
    classes: Vec<Class>,
    unnamed: usize,
    names: Vec<String>,
}

const fn unit(c: char) -> u16 {
    c as u16
}

impl Parser<'_> {
    fn peek(&self) -> Option<u16> {
        self.pattern.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<u16> {
        self.pattern.get(self.pos + offset).copied()
    }

    fn next(&mut self) -> Result<u16, PatternError> {
        let unit = self
            .peek()
            .ok_or_else(|| PatternError("unexpected end".into()))?;
        self.pos += 1;
        Ok(unit)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(unit(c)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn class(&mut self, def: ClassDef) -> Node {
        self.classes.push(Class::new(def));
        Node::Char(self.classes.len() - 1)
    }

    fn literal(&mut self, value: u16, options: Options) -> Node {
        let mut def = ClassDef::single(value);
        if options.ignore_case {
            def.add_case_equivalences();
        }
        self.class(def)
    }

    /// `ScanBlank`: white space and `#` comments under `x`, and `(?#…)`.
    fn skip_blank(&mut self, options: Options) {
        loop {
            if options.extended {
                while matches!(self.peek(), Some(0x09..=0x0D | 0x20)) {
                    self.pos += 1;
                }
                if self.peek() == Some(unit('#')) {
                    while let Some(u) = self.peek() {
                        self.pos += 1;
                        if u == unit('\n') {
                            break;
                        }
                    }
                    continue;
                }
            }
            if self.peek() == Some(unit('('))
                && self.peek_at(1) == Some(unit('?'))
                && self.peek_at(2) == Some(unit('#'))
            {
                while let Some(u) = self.peek() {
                    self.pos += 1;
                    if u == unit(')') {
                        break;
                    }
                }
                continue;
            }
            return;
        }
    }

    /// An alternation up to `)` or the end. `options` changes from inline
    /// `(?i)` stay local to the enclosing group.
    fn alternation(&mut self, mut options: Options) -> Result<Node, PatternError> {
        let mut branches = Vec::new();
        let mut sequence = Vec::new();
        loop {
            self.skip_blank(options);
            match self.peek() {
                None => break,
                Some(u) if u == unit(')') => break,
                Some(u) if u == unit('|') => {
                    self.pos += 1;
                    branches.push(Node::Concat(std::mem::take(&mut sequence)));
                    continue;
                }
                _ => {}
            }
            let Some(atom) = self.atom(&mut options)? else {
                continue;
            };
            self.skip_blank(options);
            let atom = self.quantifier(atom)?;
            sequence.push(atom);
        }
        branches.push(Node::Concat(sequence));
        Ok(if branches.len() == 1 {
            branches.pop().unwrap_or(Node::Empty)
        } else {
            Node::Alt(branches)
        })
    }

    fn quantifier(&mut self, atom: Node) -> Result<Node, PatternError> {
        let (min, max) = match self.peek() {
            Some(u) if u == unit('*') => {
                self.pos += 1;
                (0, UNBOUNDED)
            }
            Some(u) if u == unit('+') => {
                self.pos += 1;
                (1, UNBOUNDED)
            }
            Some(u) if u == unit('?') => {
                self.pos += 1;
                (0, 1)
            }
            Some(u) if u == unit('{') => match self.braces() {
                Some(bounds) => bounds,
                None => return Ok(atom),
            },
            _ => return Ok(atom),
        };
        if matches!(self.peek(), Some(u) if [unit('*'), unit('+'), unit('{')].contains(&u)) {
            return error("nested quantifier");
        }
        let lazy = self.eat('?');
        if min > max {
            return error("illegal {x,y} with x > y");
        }
        Ok(Node::Repeat {
            node: Box::new(atom),
            min,
            max,
            lazy,
        })
    }

    /// `{n}`, `{n,}` or `{n,m}`; otherwise `{` is a literal and nothing moves.
    fn braces(&mut self) -> Option<(u32, u32)> {
        let start = self.pos;
        self.pos += 1;
        let number = |parser: &mut Self| {
            let begin = parser.pos;
            let mut value: u32 = 0;
            while let Some(u) = parser.peek().filter(u16::is_ascii_digit_unit) {
                value = value
                    .saturating_mul(10)
                    .saturating_add(u32::from(u - unit('0')));
                parser.pos += 1;
            }
            (parser.pos > begin).then_some(value)
        };
        let result = (|| {
            let min = number(self)?;
            let max = if self.eat(',') {
                number(self).unwrap_or(UNBOUNDED)
            } else {
                min
            };
            self.eat('}').then_some((min, max))
        })();
        if result.is_none() {
            self.pos = start;
        }
        result
    }

    /// One atom, or `None` for an inline option change.
    fn atom(&mut self, options: &mut Options) -> Result<Option<Node>, PatternError> {
        let u = self.next()?;
        let node = match u {
            0x28 => return self.group(options),
            0x5B => {
                let def = self.char_class(*options)?;
                self.class(def)
            }
            0x5C => self.escape(*options)?,
            0x2E => {
                let def = if options.singleline {
                    ClassDef {
                        negate: true,
                        ..ClassDef::default()
                    }
                } else {
                    ClassDef {
                        negate: true,
                        ranges: vec![(unit('\n'), unit('\n'))],
                        categories: Vec::new(),
                    }
                };
                self.class(def)
            }
            0x5E => Node::Assert(Assertion::Bol(options.multiline)),
            0x24 => Node::Assert(Assertion::Eol(options.multiline)),
            0x2A | 0x2B | 0x3F => return error("quantifier without an atom"),
            0x7B if self.braces_ahead() => return error("quantifier without an atom"),
            _ => self.literal(u, *options),
        };
        Ok(Some(node))
    }

    fn braces_ahead(&mut self) -> bool {
        self.pos -= 1;
        let found = self.braces().is_some();
        if !found {
            self.pos += 1;
        }
        found
    }

    fn group(&mut self, options: &mut Options) -> Result<Option<Node>, PatternError> {
        if !self.eat('?') {
            let group = (!options.explicit_capture).then(|| {
                self.unnamed += 1;
                GroupRef::Unnamed(self.unnamed)
            });
            let node = self.alternation(*options)?;
            self.close()?;
            return Ok(Some(Node::Capture {
                group,
                uncapture: None,
                node: Box::new(node),
            }));
        }
        let kind = self.next()?;
        let node = match kind {
            0x3A => {
                let node = self.alternation(*options)?;
                Node::Capture {
                    group: None,
                    uncapture: None,
                    node: Box::new(node),
                }
            }
            0x3D | 0x21 => Node::Look {
                behind: false,
                negate: kind == unit('!'),
                node: Box::new(self.alternation(*options)?),
            },
            0x3E => Node::Atomic(Box::new(self.alternation(*options)?)),
            0x3C if matches!(self.peek(), Some(0x3D | 0x21)) => {
                let negate = self.next()? == unit('!');
                Node::Look {
                    behind: true,
                    negate,
                    node: Box::new(self.alternation(*options)?),
                }
            }
            0x3C | 0x27 => {
                let close = if kind == unit('<') { '>' } else { '\'' };
                let name = self.name();
                let uncapture = if self.eat('-') {
                    let other = self.name();
                    if other.is_empty() {
                        return error("balancing group without a name");
                    }
                    Some(self.group_ref(other))
                } else {
                    None
                };
                if !self.eat(close) {
                    return error("bad group name");
                }
                let group = if name.is_empty() {
                    if uncapture.is_none() {
                        return error("empty group name");
                    }
                    None
                } else {
                    Some(self.define(name)?)
                };
                Node::Capture {
                    group,
                    uncapture,
                    node: Box::new(self.alternation(*options)?),
                }
            }
            0x28 => {
                let name = self.name();
                if name.is_empty() || !self.eat(')') {
                    return error("only (?(name)…) conditionals are supported");
                }
                let group = self.group_ref(name);
                let body = self.alternation(*options)?;
                let (yes, no) = match body {
                    Node::Alt(mut branches) if branches.len() == 2 => {
                        let no = branches.pop().unwrap_or(Node::Empty);
                        (branches.pop().unwrap_or(Node::Empty), no)
                    }
                    Node::Alt(_) => return error("too many | in (?()|)"),
                    other => (other, Node::Empty),
                };
                Node::Conditional {
                    group,
                    yes: Box::new(yes),
                    no: Box::new(no),
                }
            }
            _ => {
                // Inline options: (?imnsx-imnsx) or (?imnsx-imnsx:…).
                self.pos -= 1;
                let mut scoped = *options;
                let mut on = true;
                loop {
                    let u = self.next()?;
                    match u {
                        0x2D => on = false,
                        0x69 => scoped.ignore_case = on,
                        0x6D => scoped.multiline = on,
                        0x73 => scoped.singleline = on,
                        0x6E => scoped.explicit_capture = on,
                        0x78 => scoped.extended = on,
                        0x29 => {
                            *options = scoped;
                            return Ok(None);
                        }
                        0x3A => {
                            let node = self.alternation(scoped)?;
                            self.close()?;
                            return Ok(Some(Node::Capture {
                                group: None,
                                uncapture: None,
                                node: Box::new(node),
                            }));
                        }
                        _ => return error("unrecognized grouping construct"),
                    }
                }
            }
        };
        self.close()?;
        Ok(Some(node))
    }

    fn close(&mut self) -> Result<(), PatternError> {
        if self.eat(')') {
            Ok(())
        } else {
            error("missing )")
        }
    }

    fn name(&mut self) -> String {
        let begin = self.pos;
        while let Some(u) = self.peek() {
            let word =
                char::from_u32(u32::from(u)).is_some_and(|c| c == '_' || c.is_alphanumeric());
            if !word {
                break;
            }
            self.pos += 1;
        }
        String::from_utf16_lossy(&self.pattern[begin..self.pos])
    }

    fn define(&mut self, name: String) -> Result<GroupRef, PatternError> {
        if name.bytes().all(|b| b.is_ascii_digit()) {
            return error("numbered named groups are not supported");
        }
        if !self.names.contains(&name) {
            self.names.push(name.clone());
        }
        Ok(GroupRef::Named(name))
    }

    fn group_ref(&self, name: String) -> GroupRef {
        GroupRef::Named(name)
    }

    fn escape(&mut self, options: Options) -> Result<Node, PatternError> {
        let u = self.next()?;
        let category = |category, negated| ClassDef {
            categories: vec![(category, negated)],
            ..ClassDef::default()
        };
        Ok(match u {
            0x62 => Node::Assert(Assertion::Boundary),
            0x42 => Node::Assert(Assertion::NonBoundary),
            0x41 => Node::Assert(Assertion::Start),
            0x5A => Node::Assert(Assertion::EndZ),
            0x7A => Node::Assert(Assertion::End),
            0x47 => Node::Assert(Assertion::StartMatch),
            0x77 => self.class(category(Category::Word, false)),
            0x57 => self.class(category(Category::Word, true)),
            0x64 => self.class(category(Category::Digit, false)),
            0x44 => self.class(category(Category::Digit, true)),
            0x73 => self.class(category(Category::Space, false)),
            0x53 => self.class(category(Category::Space, true)),
            0x31..=0x39 | 0x6B | 0x70 | 0x50 => return error("unsupported escape"),
            _ => {
                let value = self.char_escape(u)?;
                self.literal(value, options)
            }
        })
    }

    /// `ScanCharEscape` after the backslash and `u`.
    fn char_escape(&mut self, u: u16) -> Result<u16, PatternError> {
        let hex = |parser: &mut Self, digits: usize| -> Result<u16, PatternError> {
            let mut value = 0u16;
            for _ in 0..digits {
                let d = char::from_u32(u32::from(parser.next()?))
                    .and_then(|c| c.to_digit(16))
                    .ok_or_else(|| PatternError("bad hex escape".into()))?;
                value = value * 16 + d as u16;
            }
            Ok(value)
        };
        Ok(match u {
            0x78 => hex(self, 2)?,
            0x75 => hex(self, 4)?,
            0x61 => 7,
            0x62 => 8,
            0x65 => 27,
            0x66 => 12,
            0x6E => 10,
            0x72 => 13,
            0x74 => 9,
            0x76 => 11,
            0x30 => {
                let mut value = 0u16;
                let mut count = 0;
                while count < 3 {
                    match self.peek() {
                        Some(d @ 0x30..=0x37) => {
                            value = value * 8 + (d - 0x30);
                            self.pos += 1;
                            count += 1;
                        }
                        _ => break,
                    }
                }
                value
            }
            0x63 => {
                let c = self.next()?;
                let upper = if (0x61..=0x7A).contains(&c) {
                    c - 32
                } else {
                    c
                };
                if !(0x40..=0x5F).contains(&upper) {
                    return error("bad control escape");
                }
                upper - 0x40
            }
            _ if char::from_u32(u32::from(u))
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                return error("unrecognized escape");
            }
            _ => u,
        })
    }

    /// `ScanCharClass` after `[`.
    fn char_class(&mut self, options: Options) -> Result<ClassDef, PatternError> {
        let mut def = ClassDef::default();
        if self.eat('^') {
            def.negate = true;
        }
        let mut first = true;
        let mut range_start: Option<u16> = None;
        loop {
            let u = self.next()?;
            if u == unit(']') && !first {
                break;
            }
            first = false;
            let mut value = u;
            if u == unit('\\') {
                let e = self.next()?;
                let category = match e {
                    0x77 => Some((Category::Word, false)),
                    0x57 => Some((Category::Word, true)),
                    0x64 => Some((Category::Digit, false)),
                    0x44 => Some((Category::Digit, true)),
                    0x73 => Some((Category::Space, false)),
                    0x53 => Some((Category::Space, true)),
                    0x70 | 0x50 => return error("\\p is not supported"),
                    _ => None,
                };
                if let Some(category) = category {
                    if range_start.is_some() {
                        return error("a category cannot end a range");
                    }
                    def.categories.push(category);
                    continue;
                }
                value = self.char_escape(e)?;
            } else if u == unit('[') && range_start.is_none() && self.peek() == Some(unit(':')) {
                return error("POSIX classes are not supported");
            }
            if let Some(start) = range_start.take() {
                if value == unit('[') && u == unit('[') {
                    return error("class subtraction is not supported");
                }
                if start > value {
                    return error("reversed range");
                }
                def.ranges.push((start, value));
            } else if self.peek() == Some(unit('-'))
                && self.peek_at(1).is_some_and(|next| next != unit(']'))
            {
                self.pos += 1;
                range_start = Some(value);
            } else if value == unit('-') && u == unit('-') && self.peek() == Some(unit('[')) {
                return error("class subtraction is not supported");
            } else {
                def.ranges.push((value, value));
            }
        }
        if options.ignore_case {
            def.add_case_equivalences();
        }
        Ok(def)
    }
}

trait AsciiDigitUnit {
    fn is_ascii_digit_unit(&self) -> bool;
}

impl AsciiDigitUnit for u16 {
    fn is_ascii_digit_unit(&self) -> bool {
        (0x30..=0x39).contains(self)
    }
}

// ---------------------------------------------------------------------------
// Program

#[derive(Clone, Copy, Debug)]
enum Inst {
    Char {
        class: usize,
        rtl: bool,
    },
    /// A repeated single class: no captures and no empty iterations.
    CharLoop {
        class: usize,
        min: u32,
        max: u32,
        lazy: bool,
        rtl: bool,
    },
    Assert(Assertion),
    Split {
        first: usize,
        second: usize,
    },
    Jmp(usize),
    /// Records the group start in a register.
    Mark(usize),
    Capture {
        mark: usize,
        group: Option<usize>,
        uncapture: Option<usize>,
    },
    LoopInit(usize),
    /// Starts an iteration: count + 1, mark = position.
    LoopEnter(usize),
    LoopCheck {
        id: usize,
        enter: usize,
        exit: usize,
        min: u32,
        max: u32,
        lazy: bool,
    },
    Sub {
        kind: SubKind,
        start: usize,
    },
    IfCaptured {
        group: usize,
        no: usize,
    },
    Match,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubKind {
    Atomic,
    Look { negate: bool },
}

/// The first code units a top-level alternative can start with.
#[derive(Clone, Copy, Debug)]
struct First {
    ascii: u128,
    non_ascii: bool,
    nullable: bool,
}

impl First {
    const EMPTY: Self = Self {
        ascii: 0,
        non_ascii: false,
        nullable: true,
    };

    fn union(self, other: Self) -> Self {
        Self {
            ascii: self.ascii | other.ascii,
            non_ascii: self.non_ascii || other.non_ascii,
            nullable: self.nullable || other.nullable,
        }
    }

    fn may_start(&self, next: Option<u16>) -> bool {
        self.nullable
            || match next {
                None => false,
                Some(u) if u < 128 => self.ascii & (1 << u) != 0,
                Some(_) => self.non_ascii,
            }
    }
}

#[derive(Debug)]
struct Alternative {
    start: usize,
    first: First,
}

/// A compiled pattern.
#[derive(Debug)]
pub(crate) struct Regex {
    classes: Vec<Class>,
    program: Vec<Inst>,
    alternatives: Vec<Alternative>,
    marks: usize,
    loops: usize,
    groups: usize,
}

/// One match: every group's captures as `(start, end)` code-unit offsets,
/// in capture order (`Group.Captures`); group 0 is the whole match.
#[derive(Debug)]
pub(crate) struct Match {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) groups: Vec<Vec<(usize, usize)>>,
}

struct Compiler<'a> {
    program: Vec<Inst>,
    marks: usize,
    loops: usize,
    numbers: &'a dyn Fn(&GroupRef) -> Result<usize, PatternError>,
    /// Groups whose captures are recorded (observed by the caller, or used
    /// by a conditional or a balancing group).
    record: &'a [bool],
    /// Sub-programs (lookarounds and atomic groups) still to emit.
    pending: Vec<(usize, Node, bool)>,
}

impl Compiler<'_> {
    fn emit(&mut self, inst: Inst) -> usize {
        self.program.push(inst);
        self.program.len() - 1
    }

    fn patch(&mut self, at: usize, target: usize) {
        match &mut self.program[at] {
            Inst::Jmp(t) => *t = target,
            Inst::Split { second, .. } => *second = target,
            Inst::IfCaptured { no, .. } => *no = target,
            _ => unreachable!("patching a non-jump"),
        }
    }

    fn node(&mut self, node: &Node, rtl: bool) -> Result<(), PatternError> {
        match node {
            Node::Empty => {}
            &Node::Char(class) => {
                self.emit(Inst::Char { class, rtl });
            }
            &Node::Assert(assertion) => {
                self.emit(Inst::Assert(assertion));
            }
            Node::Concat(items) => {
                if rtl {
                    for item in items.iter().rev() {
                        self.node(item, rtl)?;
                    }
                } else {
                    for item in items {
                        self.node(item, rtl)?;
                    }
                }
            }
            Node::Alt(branches) => {
                let mut jumps = Vec::new();
                for (index, branch) in branches.iter().enumerate() {
                    if index + 1 < branches.len() {
                        let split = self.emit(Inst::Split {
                            first: self.program.len() + 1,
                            second: 0,
                        });
                        self.node(branch, rtl)?;
                        jumps.push(self.emit(Inst::Jmp(0)));
                        let next = self.program.len();
                        self.patch(split, next);
                    } else {
                        self.node(branch, rtl)?;
                    }
                }
                let end = self.program.len();
                for jump in jumps {
                    self.patch(jump, end);
                }
            }
            Node::Repeat {
                node,
                min,
                max,
                lazy,
            } => {
                if let Node::Char(class) = **node {
                    self.emit(Inst::CharLoop {
                        class,
                        min: *min,
                        max: *max,
                        lazy: *lazy,
                        rtl,
                    });
                    return Ok(());
                }
                let id = self.loops;
                self.loops += 1;
                self.emit(Inst::LoopInit(id));
                let jump = self.emit(Inst::Jmp(0));
                let enter = self.emit(Inst::LoopEnter(id));
                self.node(node, rtl)?;
                let check = self.program.len();
                self.patch(jump, check);
                self.emit(Inst::LoopCheck {
                    id,
                    enter,
                    exit: check + 1,
                    min: *min,
                    max: *max,
                    lazy: *lazy,
                });
            }
            Node::Capture {
                group,
                uncapture,
                node,
            } => {
                let group = group
                    .as_ref()
                    .map(self.numbers)
                    .transpose()?
                    .filter(|&group| self.record[group]);
                let uncapture = uncapture.as_ref().map(self.numbers).transpose()?;
                // A group nobody observes needs no capture history.
                if group.is_none() && uncapture.is_none() {
                    return self.node(node, rtl);
                }
                let mark = self.marks;
                self.marks += 1;
                self.emit(Inst::Mark(mark));
                self.node(node, rtl)?;
                self.emit(Inst::Capture {
                    mark,
                    group,
                    uncapture,
                });
            }
            Node::Atomic(node) => {
                let at = self.emit(Inst::Sub {
                    kind: SubKind::Atomic,
                    start: 0,
                });
                self.pending.push((at, (**node).clone(), rtl));
            }
            Node::Look {
                behind,
                negate,
                node,
            } => {
                let at = self.emit(Inst::Sub {
                    kind: SubKind::Look { negate: *negate },
                    start: 0,
                });
                self.pending.push((at, (**node).clone(), *behind));
            }
            Node::Conditional { group, yes, no } => {
                let group = (self.numbers)(group)?;
                let test = self.emit(Inst::IfCaptured { group, no: 0 });
                self.node(yes, rtl)?;
                let jump = self.emit(Inst::Jmp(0));
                let no_start = self.program.len();
                self.patch(test, no_start);
                self.node(no, rtl)?;
                let end = self.program.len();
                self.patch(jump, end);
            }
        }
        Ok(())
    }

    /// Emits every pending sub-program; each ends with `Match`.
    fn flush(&mut self) -> Result<(), PatternError> {
        while let Some((at, node, rtl)) = self.pending.pop() {
            let start = self.program.len();
            self.node(&node, rtl)?;
            self.emit(Inst::Match);
            if let Inst::Sub { start: s, .. } = &mut self.program[at] {
                *s = start;
            }
        }
        Ok(())
    }
}

fn first_of(node: &Node, classes: &[Class]) -> First {
    match node {
        Node::Empty | Node::Assert(_) | Node::Look { .. } => First::EMPTY,
        &Node::Char(class) => {
            let class = &classes[class];
            First {
                ascii: class.ascii,
                non_ascii: class.may_match_non_ascii(),
                nullable: false,
            }
        }
        Node::Concat(items) => {
            let mut first = First::EMPTY;
            for item in items {
                let next = first_of(item, classes);
                first = First {
                    nullable: next.nullable,
                    ..first.union(next)
                };
                if !next.nullable {
                    return first;
                }
            }
            first
        }
        Node::Alt(branches) => branches.iter().fold(
            First {
                nullable: false,
                ..First::EMPTY
            },
            |acc, branch| acc.union(first_of(branch, classes)),
        ),
        Node::Repeat { node, min, .. } => {
            let first = first_of(node, classes);
            First {
                nullable: first.nullable || *min == 0,
                ..first
            }
        }
        Node::Capture { node, .. } | Node::Atomic(node) => first_of(node, classes),
        Node::Conditional { yes, no, .. } => first_of(yes, classes).union(first_of(no, classes)),
    }
}

/// Marks the groups that conditionals and balancing groups read.
fn referenced(
    node: &Node,
    numbers: &dyn Fn(&GroupRef) -> Result<usize, PatternError>,
    record: &mut [bool],
) -> Result<(), PatternError> {
    match node {
        Node::Empty | Node::Char(_) | Node::Assert(_) => {}
        Node::Concat(items) | Node::Alt(items) => {
            for item in items {
                referenced(item, numbers, record)?;
            }
        }
        Node::Repeat { node, .. } | Node::Atomic(node) | Node::Look { node, .. } => {
            referenced(node, numbers, record)?;
        }
        Node::Capture {
            uncapture, node, ..
        } => {
            if let Some(other) = uncapture {
                record[numbers(other)?] = true;
            }
            referenced(node, numbers, record)?;
        }
        Node::Conditional { group, yes, no } => {
            record[numbers(group)?] = true;
            referenced(yes, numbers, record)?;
            referenced(no, numbers, record)?;
        }
    }
    Ok(())
}

impl Regex {
    /// Parses and compiles a .NET pattern (`RegexOptions.None`), recording
    /// the captures of every group.
    #[cfg(test)]
    pub(crate) fn new(pattern: &str) -> Result<Self, PatternError> {
        Self::with_observed(pattern, &|_| true)
    }

    /// Parses and compiles a .NET pattern (`RegexOptions.None`). Only the
    /// groups for which `observed` returns `true` (plus those that
    /// conditionals and balancing groups read) keep capture history;
    /// [`Match::groups`] is empty for the others.
    pub(crate) fn with_observed(
        pattern: &str,
        observed: &dyn Fn(usize) -> bool,
    ) -> Result<Self, PatternError> {
        let units: Vec<u16> = pattern.encode_utf16().collect();
        let mut parser = Parser {
            pattern: &units,
            pos: 0,
            classes: Vec::new(),
            unnamed: 0,
            names: Vec::new(),
        };
        let root = parser.alternation(Options::default())?;
        if parser.pos != units.len() {
            return error("unbalanced )");
        }
        let unnamed = parser.unnamed;
        let names = parser.names.clone();
        let numbers = move |group: &GroupRef| -> Result<usize, PatternError> {
            match group {
                GroupRef::Unnamed(number) => Ok(*number),
                GroupRef::Named(name) => names
                    .iter()
                    .position(|n| n == name)
                    .map(|index| unnamed + 1 + index)
                    .ok_or_else(|| PatternError(format!("reference to undefined group {name}"))),
            }
        };
        let groups = unnamed + 1 + parser.names.len();
        let mut record: Vec<bool> = (0..groups).map(observed).collect();
        referenced(&root, &numbers, &mut record)?;
        let classes = parser.classes;
        let branches = match root {
            Node::Alt(branches) => branches,
            other => vec![other],
        };
        let mut compiler = Compiler {
            program: Vec::new(),
            marks: 0,
            loops: 0,
            numbers: &numbers,
            record: &record,
            pending: Vec::new(),
        };
        let mut alternatives = Vec::new();
        for branch in &branches {
            let start = compiler.program.len();
            compiler.node(branch, false)?;
            compiler.emit(Inst::Match);
            alternatives.push(Alternative {
                start,
                first: first_of(branch, &classes),
            });
        }
        compiler.flush()?;
        Ok(Self {
            marks: compiler.marks,
            loops: compiler.loops,
            program: compiler.program,
            classes,
            alternatives,
            groups,
        })
    }

    /// Number of groups, group 0 included (`GetGroupNumbers().Length`).
    pub(crate) const fn group_count(&self) -> usize {
        self.groups
    }

    /// `Regex.Match(text, from)` over `text` as the whole input: the
    /// leftmost match starting at or after `from`.
    pub(crate) fn find_at(
        &self,
        text: &[u16],
        from: usize,
        budget: &mut Budget<'_>,
    ) -> Result<Option<Match>, Stop> {
        // Positions are stored as u32; NONE marks an unset register.
        if text.len() >= NONE as usize {
            return Err(Stop::Exhausted);
        }
        let mut runner = Runner {
            regex: self,
            text,
            registers: vec![NONE; self.marks + 2 * self.loops],
            captures: vec![Vec::new(); self.groups],
            captured: 0,
            log: Vec::new(),
            tracked: 0,
            budget,
            match_start: from,
        };
        for start in from..=text.len() {
            runner.budget.spend(1)?;
            let next = text.get(start).copied();
            for alternative in &self.alternatives {
                if !alternative.first.may_start(next) {
                    continue;
                }
                runner.match_start = start;
                if let Some(end) = runner.run(alternative.start, start)? {
                    let mut groups: Vec<Vec<(usize, usize)>> = runner
                        .captures
                        .iter()
                        .map(|history| {
                            history
                                .iter()
                                .map(|&(s, e)| (s as usize, e as usize))
                                .collect()
                        })
                        .collect();
                    groups[0] = vec![(start, end)];
                    return Ok(Some(Match { start, end, groups }));
                }
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Matcher

/// An unset register (no group start yet, or no loop iteration yet).
const NONE: u32 = u32::MAX;

/// One undo entry (12 bytes): `kind` in the top two bits of `head`, an
/// index in the rest.
#[derive(Clone, Copy)]
struct Undo {
    head: u32,
    a: u32,
    b: u32,
}

const UNDO_REGISTER: u32 = 0;
const UNDO_PUSHED: u32 = 1 << 30;
const UNDO_POPPED: u32 = 2 << 30;
/// A loop's two registers (count, mark) in one entry; `head` holds the
/// count register.
const UNDO_LOOP: u32 = 3 << 30;
const KIND_MASK: u32 = 3 << 30;

/// One backtrack entry (16 bytes): `kind` in the top two bits of `head`,
/// the resume instruction in the rest. A single-class loop's class and
/// direction are read from its instruction, just before the resume point.
#[derive(Clone, Copy)]
struct Track {
    head: u32,
    pos: u32,
    log: u32,
    /// Units a loop can still give back or take.
    left: u32,
}

/// Resume at `pc` and `pos`.
const TRACK_BRANCH: u32 = 0;
/// A greedy single-class loop can give back `left` more units.
const TRACK_GIVE_BACK: u32 = 1 << 30;
/// A lazy single-class loop can take `left` more units.
const TRACK_TAKE_MORE: u32 = 2 << 30;

struct Runner<'r, 't, 'b, 'c> {
    regex: &'r Regex,
    text: &'t [u16],
    /// Group marks, then two registers per loop (count, mark).
    registers: Vec<u32>,
    captures: Vec<Vec<(u32, u32)>>,
    /// Total entries in `captures`.
    captured: usize,
    log: Vec<Undo>,
    /// Backtrack entries held by every active `run` frame.
    tracked: usize,
    budget: &'b mut Budget<'c>,
    match_start: usize,
}

impl Runner<'_, '_, '_, '_> {
    fn set(&mut self, register: usize, value: u32) {
        let old = self.registers[register];
        self.log.push(Undo {
            head: UNDO_REGISTER | register as u32,
            a: old,
            b: 0,
        });
        self.registers[register] = value;
    }

    /// Sets a loop's count and mark with one undo entry.
    fn set_loop(&mut self, id: usize, count: u32, mark: u32) {
        let (count_register, mark_register) = self.loop_registers(id);
        self.log.push(Undo {
            head: UNDO_LOOP | count_register as u32,
            a: self.registers[count_register],
            b: self.registers[mark_register],
        });
        self.registers[count_register] = count;
        self.registers[mark_register] = mark;
    }

    fn undo_to(&mut self, length: usize) {
        while self.log.len() > length {
            let Some(undo) = self.log.pop() else { break };
            let index = (undo.head & !KIND_MASK) as usize;
            match undo.head & KIND_MASK {
                UNDO_REGISTER => self.registers[index] = undo.a,
                UNDO_LOOP => {
                    self.registers[index] = undo.a;
                    self.registers[index + 1] = undo.b;
                }
                UNDO_PUSHED => {
                    self.captures[index].pop();
                    self.captured -= 1;
                }
                _ => {
                    self.captures[index].push((undo.a, undo.b));
                    self.captured += 1;
                }
            }
        }
    }

    fn loop_registers(&self, id: usize) -> (usize, usize) {
        let base = self.regex.marks + 2 * id;
        (base, base + 1)
    }

    fn is_word(&self, pos: usize) -> bool {
        self.text
            .get(pos)
            .is_some_and(|&u| in_ranges(BOUNDARY_WORD, u))
    }

    fn assert(&self, assertion: Assertion, pos: usize) -> bool {
        let text = self.text;
        let len = text.len();
        let newline = unit('\n');
        match assertion {
            Assertion::Bol(false) | Assertion::Start => pos == 0,
            Assertion::Bol(true) => pos == 0 || text[pos - 1] == newline,
            Assertion::Eol(false) | Assertion::EndZ => {
                pos == len || (pos + 1 == len && text[pos] == newline)
            }
            Assertion::Eol(true) => pos == len || text[pos] == newline,
            Assertion::End => pos == len,
            Assertion::Boundary => (pos > 0 && self.is_word(pos - 1)) != self.is_word(pos),
            Assertion::NonBoundary => (pos > 0 && self.is_word(pos - 1)) == self.is_word(pos),
            Assertion::StartMatch => pos == self.match_start,
        }
    }

    #[inline]
    fn step(&self, class: usize, pos: usize, rtl: bool) -> Option<usize> {
        let class = &self.regex.classes[class];
        if rtl {
            (pos > 0 && class.contains(self.text[pos - 1])).then(|| pos - 1)
        } else {
            (pos < self.text.len() && class.contains(self.text[pos])).then_some(pos + 1)
        }
    }

    /// The class and direction of the single-class loop just before `pc`.
    fn char_loop_before(&self, pc: usize) -> (usize, bool) {
        match self.regex.program[pc - 1] {
            Inst::CharLoop { class, rtl, .. } => (class, rtl),
            _ => unreachable!("a loop track resumes after its CharLoop"),
        }
    }

    fn push_track(&mut self, track: &mut Vec<Track>, kind: u32, pc: usize, pos: usize, left: u32) {
        track.push(Track {
            head: kind | pc as u32,
            pos: pos as u32,
            log: self.log.len() as u32,
            left,
        });
        self.tracked += 1;
    }

    /// Runs the program from `pc` at `pos`. On success the captures and
    /// registers it set stay (logged); on failure everything is undone.
    fn run(&mut self, pc: usize, pos: usize) -> Result<Option<usize>, Stop> {
        let base_log = self.log.len();
        let mut track: Vec<Track> = Vec::new();
        let result = self.run_frame(pc, pos, base_log, &mut track);
        self.tracked -= track.len();
        result
    }

    fn run_frame(
        &mut self,
        pc: usize,
        pos: usize,
        base_log: usize,
        track: &mut Vec<Track>,
    ) -> Result<Option<usize>, Stop> {
        let mut pc = pc;
        let mut pos = pos;
        loop {
            self.budget.spend(1)?;
            if self.log.len() + self.tracked + self.captured > MAX_ENTRIES {
                return Err(Stop::Exhausted);
            }
            let ok = match self.regex.program[pc] {
                Inst::Match => return Ok(Some(pos)),
                Inst::Char { class, rtl } => match self.step(class, pos, rtl) {
                    Some(next) => {
                        pos = next;
                        pc += 1;
                        true
                    }
                    None => false,
                },
                Inst::CharLoop {
                    class,
                    min,
                    max,
                    lazy,
                    rtl,
                } => {
                    let mut taken = 0u32;
                    let wanted = if lazy { min } else { max };
                    while taken < wanted {
                        match self.step(class, pos, rtl) {
                            Some(next) => {
                                pos = next;
                                taken += 1;
                            }
                            None => break,
                        }
                    }
                    self.budget.spend(u64::from(taken))?;
                    if taken < min {
                        false
                    } else {
                        if lazy {
                            if max > min {
                                self.push_track(track, TRACK_TAKE_MORE, pc + 1, pos, max - min);
                            }
                        } else if taken > min {
                            self.push_track(track, TRACK_GIVE_BACK, pc + 1, pos, taken - min);
                        }
                        pc += 1;
                        true
                    }
                }
                Inst::Assert(assertion) => {
                    pc += 1;
                    self.assert(assertion, pos)
                }
                Inst::Split { first, second } => {
                    self.push_track(track, TRACK_BRANCH, second, pos, 0);
                    pc = first;
                    true
                }
                Inst::Jmp(target) => {
                    pc = target;
                    true
                }
                Inst::Mark(mark) => {
                    self.set(mark, pos as u32);
                    pc += 1;
                    true
                }
                Inst::Capture {
                    mark,
                    group,
                    uncapture,
                } => self
                    .capture(mark, group, uncapture, pos)
                    .inspect(|()| pc += 1)
                    .is_some(),
                Inst::LoopInit(id) => {
                    self.set_loop(id, 0, NONE);
                    pc += 1;
                    true
                }
                Inst::LoopEnter(id) => {
                    let (count, _) = self.loop_registers(id);
                    let next = self.registers[count].saturating_add(1);
                    self.set_loop(id, next, pos as u32);
                    pc += 1;
                    true
                }
                Inst::LoopCheck {
                    id,
                    enter,
                    exit,
                    min,
                    max,
                    lazy,
                } => {
                    let (count, mark) = self.loop_registers(id);
                    let done = self.registers[count];
                    let empty = self.registers[mark] == pos as u32;
                    if lazy {
                        if done < min {
                            pc = enter;
                        } else {
                            if done < max && !empty {
                                self.push_track(track, TRACK_BRANCH, enter, pos, 0);
                            }
                            pc = exit;
                        }
                    } else if done >= max || (empty && done >= min) {
                        pc = exit;
                    } else {
                        if done >= min {
                            self.push_track(track, TRACK_BRANCH, exit, pos, 0);
                        }
                        pc = enter;
                    }
                    true
                }
                Inst::Sub { kind, start } => {
                    let before = self.log.len();
                    let result = self.run(start, pos)?;
                    pc += 1;
                    match (kind, result) {
                        (SubKind::Atomic, Some(end)) => {
                            pos = end;
                            true
                        }
                        (SubKind::Look { negate: false }, Some(_))
                        | (SubKind::Look { negate: true }, None) => true,
                        (SubKind::Look { negate: true }, Some(_)) => {
                            self.undo_to(before);
                            false
                        }
                        (_, None) => false,
                    }
                }
                Inst::IfCaptured { group, no } => {
                    pc = if self.captures[group].is_empty() {
                        no
                    } else {
                        pc + 1
                    };
                    true
                }
            };
            if ok {
                continue;
            }
            // Backtrack.
            loop {
                self.budget.spend(1)?;
                let Some(entry) = track.pop() else {
                    self.undo_to(base_log);
                    return Ok(None);
                };
                self.tracked -= 1;
                self.undo_to(entry.log as usize);
                let target = (entry.head & !KIND_MASK) as usize;
                let at = entry.pos as usize;
                match entry.head & KIND_MASK {
                    TRACK_BRANCH => {
                        pc = target;
                        pos = at;
                        break;
                    }
                    TRACK_GIVE_BACK => {
                        let (_, rtl) = self.char_loop_before(target);
                        let at = if rtl { at + 1 } else { at - 1 };
                        if entry.left > 1 {
                            track.push(Track {
                                pos: at as u32,
                                left: entry.left - 1,
                                ..entry
                            });
                            self.tracked += 1;
                        }
                        pc = target;
                        pos = at;
                        break;
                    }
                    _ => {
                        let (class, rtl) = self.char_loop_before(target);
                        let Some(next) = self.step(class, at, rtl) else {
                            continue;
                        };
                        if entry.left > 1 {
                            track.push(Track {
                                pos: next as u32,
                                left: entry.left - 1,
                                ..entry
                            });
                            self.tracked += 1;
                        }
                        pc = target;
                        pos = next;
                        break;
                    }
                }
            }
        }
    }

    /// `Capturemark`: `Capture` or `TransferCapture` (balancing groups).
    fn capture(
        &mut self,
        mark: usize,
        group: Option<usize>,
        uncapture: Option<usize>,
        pos: usize,
    ) -> Option<()> {
        let marked = self.registers[mark];
        if marked == NONE {
            return None;
        }
        let pos = pos as u32;
        let (mut start, mut end) = if marked <= pos {
            (marked, pos)
        } else {
            (pos, marked)
        };
        if let Some(other) = uncapture {
            let &(start2, end2) = self.captures[other].last()?;
            // TransferCapture: the interval between the two captures.
            if start >= end2 {
                end = start;
                start = end2;
            } else if end <= start2 {
                start = start2;
            } else {
                end = end.min(end2);
                start = start.max(start2);
            }
            let (a, b) = self.captures[other].pop()?;
            self.captured -= 1;
            self.log.push(Undo {
                head: UNDO_POPPED | other as u32,
                a,
                b,
            });
        }
        if let Some(group) = group {
            self.captures[group].push((start, end));
            self.captured += 1;
            self.log.push(Undo {
                head: UNDO_PUSHED | group as u32,
                a: 0,
                b: 0,
            });
        }
        Some(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    type Found = (usize, usize, Vec<Vec<(usize, usize)>>);

    fn find(pattern: &str, text: &str) -> Option<Found> {
        let regex = Regex::new(pattern).unwrap();
        let units: Vec<u16> = text.encode_utf16().collect();
        regex
            .find_at(&units, 0, &mut Budget::new(1_000_000))
            .unwrap()
            .map(|m| (m.start, m.end, m.groups))
    }

    fn span(pattern: &str, text: &str) -> Option<(usize, usize)> {
        find(pattern, text).map(|(s, e, _)| (s, e))
    }

    #[test]
    fn basics() {
        assert_eq!(span("abc", "xxabcx"), Some((2, 5)));
        assert_eq!(span("a|ab", "ab"), Some((0, 1)));
        assert_eq!(span("a*?b", "aaab"), Some((0, 4)));
        assert_eq!(span("x{2,3}", "xxxx"), Some((0, 3)));
        assert_eq!(span("x{2,3}?", "xxxx"), Some((0, 2)));
        assert_eq!(span("a{,2}", "a{,2}"), Some((0, 5)));
        assert_eq!(span("{-+", "{--"), Some((0, 3)));
        assert_eq!(span("(?i)ABC", "xabc"), Some((1, 4)));
        assert_eq!(span("(?i)k", "\u{212a}"), Some((0, 1)));
        assert_eq!(span("(?i:a)b", "AB"), None);
        assert_eq!(span("(?x) a b # c\n c", "abc"), Some((0, 3)));
        assert_eq!(span("^b", "a\nb"), None);
        assert_eq!(span("(?m)^b", "a\nb"), Some((2, 3)));
        assert_eq!(span("a$", "a\n"), Some((0, 1)));
        assert_eq!(span("a$", "a\nb"), None);
        assert_eq!(span("\\bif\\b", "elif if"), Some((5, 7)));
        assert_eq!(span("[a-z0-9-_]+", "Q-a_9"), Some((1, 5)));
        assert_eq!(span("[]a]+", "]a]"), Some((0, 3)));
        assert_eq!(span("\\#\\⋆", "#⋆"), Some((0, 2)));
        assert_eq!(span(".", "\n"), None);
        assert_eq!(span("(?s).", "\n"), Some((0, 1)));
    }

    #[test]
    fn lookaround_and_atomic() {
        assert_eq!(span("(?<!\\\\)\"", "\\\"x\""), Some((3, 4)));
        assert_eq!(span("(?<=:\\s*)x", "a:  x"), Some((4, 5)));
        assert_eq!(span("(?<==\\s*f*b*r*u*)'", "= br'"), Some((4, 5)));
        assert_eq!(span("(?<=a.*?b)c", "a bc"), Some((3, 4)));
        assert_eq!(span("a(?=b)", "acab"), Some((2, 3)));
        assert_eq!(span("a(?!b)", "abac"), Some((2, 3)));
        assert_eq!(span("(?>a+)b", "aaab"), Some((0, 4)));
        assert_eq!(span("(?>a|ab)c", "abc"), None);
        assert_eq!(span("(?:a|ab)c", "abc"), Some((0, 3)));
    }

    #[test]
    fn captures_keep_history() {
        let (_, _, groups) = find("(?:(a)|(b))+", "abab").unwrap();
        assert_eq!(groups[1], vec![(0, 1), (2, 3)]);
        assert_eq!(groups[2], vec![(1, 2), (3, 4)]);
        // Backtracking removes captures.
        let (_, _, groups) = find("(a)*ab", "aaab").unwrap();
        assert_eq!(groups[1], vec![(0, 1), (1, 2)]);
        // An empty iteration ends the loop but its capture stays.
        let (_, _, groups) = find("(a?)*", "a").unwrap();
        assert_eq!(groups[1], vec![(0, 1), (1, 1)]);
        // Named groups are numbered after unnamed ones.
        let regex = Regex::new("(?<n>x)(y)").unwrap();
        assert_eq!(regex.group_count(), 3);
        let (_, _, groups) = find("(?<n>x)(y)", "xy").unwrap();
        assert_eq!(groups[1], vec![(1, 2)]);
        assert_eq!(groups[2], vec![(0, 1)]);
    }

    #[test]
    fn balancing_and_conditionals() {
        let comment = "\\{-(?>(?<c>\\{-)|(?<-c>-\\})|[^{}-]|-(?!\\})|\\{(?!-))*(?(c)(?!))-\\}";
        assert_eq!(span(comment, "{- a {- b -} c -} d -}"), Some((0, 17)));
        assert_eq!(span(&format!("^{comment}"), "{- a {- b -}"), None);
        assert_eq!(span("(?<t>a)?(?(t)b|c)", "c"), Some((0, 1)));
        assert_eq!(span("(?<t>a)?(?(t)b|c)", "ab"), Some((0, 2)));
    }

    #[test]
    fn budget_stops_catastrophic_backtracking() {
        let regex = Regex::new("(a*)*b").unwrap();
        let text: Vec<u16> = "a".repeat(5_000).encode_utf16().collect();
        let mut budget = Budget::new(1_000_000);
        assert_eq!(
            regex.find_at(&text, 0, &mut budget).err(),
            Some(Stop::Exhausted)
        );
        assert_eq!(budget.left(), 0);
        let regex = Regex::new("(x+x+)+y").unwrap();
        let text: Vec<u16> = "x".repeat(64).encode_utf16().collect();
        assert!(
            regex
                .find_at(&text, 0, &mut Budget::new(1_000_000))
                .is_err()
        );
    }

    #[test]
    fn budget_polls_cancellation_and_caps_credit() {
        let regex = Regex::new("(a*)*b").unwrap();
        let text: Vec<u16> = "a".repeat(5_000).encode_utf16().collect();
        let cancel = || true;
        let mut budget = Budget::new(u64::MAX).with_cancel(&cancel);
        assert_eq!(
            regex.find_at(&text, 0, &mut budget).err(),
            Some(Stop::Cancelled)
        );
        let mut capped = Budget::new(10).with_cap(25);
        capped.credit(10);
        capped.credit(10);
        assert_eq!(capped.left(), 25);
    }

    #[test]
    fn state_cap_stops_the_match() {
        // One loop iteration per unit keeps backtracking state per unit.
        let regex = Regex::new("(?:x|y)*z").unwrap();
        let text: Vec<u16> = "x".repeat(MAX_ENTRIES).encode_utf16().collect();
        let mut budget = Budget::new(u64::MAX);
        assert_eq!(
            regex.find_at(&text, 0, &mut budget).err(),
            Some(Stop::Exhausted)
        );
        // Unobserved groups keep no history.
        let regex = Regex::with_observed("(a)(b)", &|group| group == 2).unwrap();
        let units: Vec<u16> = "ab".encode_utf16().collect();
        let found = regex
            .find_at(&units, 0, &mut Budget::new(1_000))
            .unwrap()
            .unwrap();
        assert!(found.groups[1].is_empty());
        assert_eq!(found.groups[2], vec![(1, 2)]);
    }

    #[test]
    fn rejects_unsupported_syntax() {
        for pattern in ["(", ")", "a**", "\\1", "[a-[b]]", "(?<1>a)", "\\p{L}", "*"] {
            assert!(Regex::new(pattern).is_err(), "{pattern}");
        }
    }
}
