//! Nesting-depth and tree-construction work probe, run before sanitization.
//!
//! html5ever's tree builder scans the stack of open elements for many start
//! and end tags (for example "has a `p` element in button scope" on every
//! `<div>`), so deeply nested input costs time quadratic in its depth. That
//! makes it a CPU denial-of-service vector well inside the byte limits:
//! 20,000 nested `<div>`s is 100 KiB. This probe runs the same html5ever
//! fragment parse (same options and `<div>` context as ammonia) into a sink
//! that keeps only the tree shape: parent, children and depth of every node.
//!
//! * Depth is exact. Whenever a node is attached, moved before a sibling, or
//!   has its children reparented (the adoption agency), the depths of the
//!   whole moved subtree are recomputed with an explicit stack. Detached
//!   subtrees, such as a formatting-element clone the adoption agency fills
//!   before inserting it, are re-measured when they are attached.
//! * Work is budgeted. Every node placement is charged its depth, which
//!   stands in for the tree builder's own open-stack scans. Every re-depth
//!   visit and child-list scan is charged too. The budget is linear in the
//!   input length. Adoption-agency reparenting is capped separately, also
//!   linearly in the input length.
//!
//! The probe also **plans the chunks** that the sanitizer parses
//! independently. A chunk may end only at a split candidate from the
//! attribute pre-scan (the tokenizer is then certainly in the data state with
//! no token in progress) at which the tree builder holds no handle except the
//! document, the `<html>` root and the context element
//! (`TreeBuilder::trace_handles`). Then the stack of open elements is just the
//! root, the list of active formatting elements holds no element, and neither
//! a form nor a head element pointer is set, so the insertion mode is "in
//! body" again and no later token can reach a node before the split. Parsing
//! the rest from scratch therefore builds the same top-level nodes, except
//! that text on both sides of the split is not merged into one text node,
//! which serializes to the same bytes. At every chunk boundary the sink
//! forgets the finished top-level subtrees, exactly as a fresh parse would,
//! so the charged work models the chunked parses and the probe's memory stays
//! proportional to one chunk.
//!
//! The input is fed in pieces and the probe stops at the first piece that
//! breaks a limit, so rejected work stays linear in the input length.

use std::{
    borrow::Cow,
    cell::{Cell, Ref, RefCell},
    ops::Range,
    rc::Rc,
};

use html5ever::{
    Attribute, ParseOpts, QualName, local_name, ns, parse_fragment,
    tendril::{StrTendril, TendrilSink},
    tokenizer::TokenizerOpts,
    tree_builder::{ElementFlags, NodeOrText, QuirksMode, Tracer, TreeSink},
};

/// Maximum element nesting accepted by the sanitizer.
pub const MAX_NESTING_DEPTH: usize = 256;

/// Work units allowed per input byte, on top of [`BASE_WORK`]. Ordinary
/// Markdown output uses well under 3 units per byte (a 20-level nested list is
/// about 2). Flat content at maximum depth uses about 23.
pub(crate) const WORK_PER_INPUT_BYTE: usize = 8;
pub(crate) const BASE_WORK: usize = 256 * 1024;

/// Adoption-agency `reparent_children` calls allowed regardless of size.
pub(crate) const BASE_REPARENTS: usize = 4_096;
/// One more reparent call is allowed per this many input bytes.
pub(crate) const BYTES_PER_REPARENT: usize = 256;

/// Largest piece fed to the parser between limit and cancellation checks.
const FEED_BYTES: usize = 16 * 1024;

/// Node ids the fragment parser creates first: the context `<div>` and the
/// `<html>` root (the document is 0).
const CONTEXT: usize = 1;
const ROOT: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Verdict {
    Within,
    TooDeep,
    TooMuchWork,
    Cancelled,
}

/// Reparent calls allowed for an input of `input_bytes`.
pub(crate) const fn max_reparents(input_bytes: usize) -> usize {
    BASE_REPARENTS + input_bytes / BYTES_PER_REPARENT
}

/// Work units allowed for an input of `input_bytes`.
pub(crate) const fn max_work(input_bytes: usize) -> usize {
    input_bytes
        .saturating_mul(WORK_PER_INPUT_BYTE)
        .saturating_add(BASE_WORK)
}

/// Measure `input` against the depth `limit` and the linear work budget.
#[cfg(test)]
pub(crate) fn probe(input: &str, limit: usize) -> Verdict {
    let mut planner = Planner::new(
        input,
        &[],
        0,
        usize::MAX,
        limit,
        Budget::for_input(input.len()),
    );
    match planner.advance(input.len(), &|| false) {
        Verdict::Within => planner.finish(),
        verdict => verdict,
    }
}

/// Work and reparent allowances for one probe run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Budget {
    pub(crate) work: usize,
    pub(crate) reparents: usize,
}

impl Budget {
    pub(crate) const ZERO: Self = Self {
        work: 0,
        reparents: 0,
    };

    /// What was used after `earlier`, a reading of the same counters.
    pub(crate) const fn since(self, earlier: Self) -> Self {
        Self {
            work: self.work.saturating_sub(earlier.work),
            reparents: self.reparents.saturating_sub(earlier.reparents),
        }
    }

    /// The allowance for a whole input of `input_bytes`.
    pub(crate) const fn for_input(input_bytes: usize) -> Self {
        Self {
            work: max_work(input_bytes),
            reparents: max_reparents(input_bytes),
        }
    }
}

/// An incremental probe over `input`, starting at a point where the parser
/// is at its initial state.
///
/// It plans the chunks the sanitizer parses independently: a chunk ends at
/// the first split candidate at least `chunk_target` bytes after its start at
/// which the parser is back at its initial state (see the module
/// documentation). The caller decides at the end of each `advance` whether to
/// close the current chunk there (`at_initial_state` + `close_chunk`).
pub(crate) struct Planner<'i> {
    input: &'i str,
    candidates: &'i [usize],
    parser: html5ever::driver::Parser<DepthSink>,
    verdict: Rc<Cell<Verdict>>,
    chunk_target: usize,
    chunk_start: usize,
    position: usize,
    next_candidate: usize,
    /// Closed chunks, each with the budget used when it was closed.
    chunks: Vec<(Range<usize>, Budget)>,
    used_at_cut: Budget,
}

impl<'i> Planner<'i> {
    /// `candidates` are absolute, sorted offsets into `input`; `start` must
    /// be 0 or a split candidate at which the parser is at its initial
    /// state.
    pub(crate) fn new(
        input: &'i str,
        candidates: &'i [usize],
        start: usize,
        chunk_target: usize,
        limit: usize,
        budget: Budget,
    ) -> Self {
        let verdict = Rc::new(Cell::new(Verdict::Within));
        let sink = DepthSink {
            nodes: RefCell::new(vec![Node::new(document_name())]),
            limit,
            budget: budget.work,
            work: Cell::new(0),
            reparents: Cell::new(0),
            max_reparents: budget.reparents,
            verdict: Rc::clone(&verdict),
        };
        let parser = parse_fragment(
            sink,
            probe_parse_opts(),
            QualName::new(None, ns!(html), local_name!("div")),
            Vec::new(),
            false,
        );
        // ammonia's parse discards a byte order mark at the start of its
        // input, and every chunk is its own input. Split candidates are never
        // followed by one, so only offset 0 can start with it.
        let position = if start == 0 && input.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            start
        };
        Self {
            input,
            candidates,
            parser,
            verdict,
            chunk_target,
            chunk_start: start,
            position,
            next_candidate: candidates.partition_point(|&candidate| candidate <= start),
            chunks: Vec::new(),
            used_at_cut: Budget::ZERO,
        }
    }

    /// Parse up to `end` (a split candidate or the input length), closing
    /// chunks at candidates before it. Stops at the first piece (at most
    /// `FEED_BYTES`) that breaks a limit or after which `cancelled` returns
    /// `true`.
    pub(crate) fn advance(&mut self, end: usize, cancelled: &dyn Fn() -> bool) -> Verdict {
        // One buffer per call; the pieces share it (no copy per piece).
        let base = self.position;
        let buffer = u32::try_from(end - base)
            .is_ok()
            .then(|| StrTendril::from_slice(&self.input[base..end]));
        while self.position < end {
            let earliest = self
                .chunk_start
                .saturating_add(self.chunk_target)
                .max(self.position + 1);
            while self
                .candidates
                .get(self.next_candidate)
                .is_some_and(|&candidate| candidate < earliest)
            {
                self.next_candidate += 1;
            }
            let mut piece_end = (self.position + FEED_BYTES).min(end);
            while !self.input.is_char_boundary(piece_end) {
                piece_end += 1;
            }
            let at_candidate = match self.candidates.get(self.next_candidate) {
                Some(&candidate) if candidate <= piece_end => {
                    piece_end = candidate;
                    true
                }
                _ => false,
            };
            let piece = match &buffer {
                // Both fit in u32: the whole range does.
                Some(buffer) => buffer.subtendril(
                    (self.position - base) as u32,
                    (piece_end - self.position) as u32,
                ),
                None => StrTendril::from_slice(&self.input[self.position..piece_end]),
            };
            self.parser.process(piece);
            self.position = piece_end;
            if self.verdict.get() != Verdict::Within {
                return self.verdict.get();
            }
            if cancelled() {
                return Verdict::Cancelled;
            }
            if at_candidate && self.position < end && self.at_initial_state() {
                self.close_chunk();
            }
        }
        Verdict::Within
    }

    /// The tree builder holds exactly the document, the `<html>` root (the
    /// only open element) and the context element: no other open element, no
    /// active formatting element, and no form or head element pointer.
    pub(crate) fn at_initial_state(&self) -> bool {
        let census = Census::default();
        self.parser.tokenizer.sink.trace_handles(&census);
        census.handles.get() == 3 && !census.beyond_root.get()
    }

    /// End the current chunk at the current position, which must be a split
    /// candidate at which `at_initial_state` holds.
    pub(crate) fn close_chunk(&mut self) {
        self.used_at_cut = self.used();
        if self.chunk_start < self.position {
            self.chunks
                .push((self.chunk_start..self.position, self.used_at_cut));
        }
        self.chunk_start = self.position;
        self.parser.tokenizer.sink.sink.forget_finished_nodes();
    }

    /// Process the end of the input (the position must be the input length)
    /// and close the last chunk.
    pub(crate) fn finish(&mut self) -> Verdict {
        debug_assert_eq!(self.position, self.input.len());
        // `Parser::finish` consumes the parser; end the input in place.
        self.parser.tokenizer.end();
        if self.verdict.get() == Verdict::Within {
            self.close_chunk();
        }
        self.verdict.get()
    }

    /// Where the current (unclosed) chunk starts.
    pub(crate) const fn chunk_start(&self) -> usize {
        self.chunk_start
    }

    /// Closed chunks so far, in order, each with the budget used when it
    /// was closed.
    pub(crate) fn take_chunks(&mut self) -> Vec<(Range<usize>, Budget)> {
        std::mem::take(&mut self.chunks)
    }

    /// Budget used so far, including the unclosed chunk.
    pub(crate) fn used(&self) -> Budget {
        let sink = &self.parser.tokenizer.sink.sink;
        Budget {
            work: sink.work.get(),
            reparents: sink.reparents.get(),
        }
    }

    /// Budget used by the closed chunks.
    pub(crate) const fn used_at_last_cut(&self) -> Budget {
        self.used_at_cut
    }
}

/// ammonia's parse options, except that the tokenizer never discards a byte
/// order mark itself: html5ever checks `discard_bom` on every `feed`, so it
/// would drop a U+FEFF at the start of every piece the probe feeds, where
/// ammonia (one feed per chunk) sees a character. `Planner::new` skips the
/// one at offset 0 by hand instead.
fn probe_parse_opts() -> ParseOpts {
    ParseOpts {
        tokenizer: TokenizerOpts {
            discard_bom: false,
            ..TokenizerOpts::default()
        },
        ..ParseOpts::default()
    }
}

/// Counts the handles the tree builder holds.
#[derive(Default)]
struct Census {
    handles: Cell<usize>,
    beyond_root: Cell<bool>,
}

impl Tracer for Census {
    type Handle = usize;

    fn trace_handle(&self, node: &usize) {
        self.handles.set(self.handles.get() + 1);
        if *node > ROOT {
            self.beyond_root.set(true);
        }
    }
}

fn document_name() -> QualName {
    QualName::new(None, ns!(), local_name!(""))
}

struct Node {
    name: QualName,
    depth: usize,
    parent: Option<usize>,
    children: Vec<usize>,
    annotation_xml_integration_point: bool,
    template_contents: Option<usize>,
    is_text: bool,
}

impl Node {
    fn new(name: QualName) -> Self {
        Self {
            name,
            depth: 0,
            parent: None,
            children: Vec::new(),
            annotation_xml_integration_point: false,
            template_contents: None,
            is_text: false,
        }
    }
}

struct DepthSink {
    nodes: RefCell<Vec<Node>>,
    limit: usize,
    budget: usize,
    work: Cell<usize>,
    reparents: Cell<usize>,
    max_reparents: usize,
    verdict: Rc<Cell<Verdict>>,
}

impl DepthSink {
    fn stopped(&self) -> bool {
        self.verdict.get() != Verdict::Within
    }

    fn fail(&self, verdict: Verdict) {
        if !self.stopped() {
            self.verdict.set(verdict);
        }
    }

    /// Charge `units` of work; returns `false` once the budget is exhausted.
    fn charge(&self, units: usize) -> bool {
        let work = self.work.get().saturating_add(units);
        self.work.set(work);
        if work > self.budget {
            self.fail(Verdict::TooMuchWork);
            return false;
        }
        true
    }

    fn push(&self, node: Node) -> usize {
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(node);
        nodes.len() - 1
    }

    fn detach(&self, nodes: &mut [Node], child: usize) {
        let Some(parent) = nodes[child].parent.take() else {
            return;
        };
        let siblings = &mut nodes[parent].children;
        if let Some(position) = siblings.iter().rposition(|&c| c == child) {
            // rcdom finds the child by scanning its parent's children
            // (text nodes included, as here) from the front.
            self.charge(position + 1);
            siblings.remove(position);
        }
    }

    /// Attach `child` under `parent`, before `before` when given, and
    /// re-measure the whole attached subtree.
    fn attach(&self, child: usize, parent: usize, before: Option<usize>) {
        if self.stopped() {
            return;
        }
        let mut nodes = self.nodes.borrow_mut();
        self.detach(&mut nodes, child);
        let siblings = &mut nodes[parent].children;
        let position = before
            .and_then(|sibling| siblings.iter().rposition(|&c| c == sibling))
            .unwrap_or(siblings.len());
        // Appending is O(1) in rcdom. Inserting before a sibling scans for
        // the sibling from the front and shifts the tail: the whole list.
        self.charge(if before.is_some() {
            siblings.len() + 1
        } else {
            1
        });
        siblings.insert(position, child);
        nodes[child].parent = Some(parent);
        let depth = nodes[parent].depth + 1;
        self.redepth(&mut nodes, child, depth);
    }

    /// Drop every node below the root after a chunk boundary, where the
    /// tree builder holds only the document, the context and the root.
    fn forget_finished_nodes(&self) {
        let mut nodes = self.nodes.borrow_mut();
        nodes.truncate(ROOT + 1);
        nodes[ROOT].children.clear();
        debug_assert!(nodes[CONTEXT].children.is_empty());
    }

    fn new_text_node(nodes: &mut Vec<Node>, parent: usize) -> usize {
        let mut node = Node::new(document_name());
        node.is_text = true;
        node.parent = Some(parent);
        nodes.push(node);
        nodes.len() - 1
    }

    /// rcdom `append(AppendText)`: merge into a trailing text child, or add
    /// a new text node.
    fn append_text(&self, parent: usize) {
        if self.stopped() {
            return;
        }
        let mut nodes = self.nodes.borrow_mut();
        if !matches!(nodes[parent].children.last(), Some(&last) if nodes[last].is_text) {
            let id = Self::new_text_node(&mut nodes, parent);
            nodes[parent].children.push(id);
            self.charge(1);
        }
    }

    /// rcdom `append_before_sibling(AppendText)`: merge into the text node
    /// right before `sibling`, or insert a new one there.
    fn insert_text_before(&self, parent: usize, sibling: usize) {
        if self.stopped() {
            return;
        }
        let mut nodes = self.nodes.borrow_mut();
        let Some(position) = nodes[parent].children.iter().rposition(|&c| c == sibling) else {
            return;
        };
        // Front-to-back scan for the sibling.
        self.charge(position + 1);
        let merges = position
            .checked_sub(1)
            .is_some_and(|index| nodes[nodes[parent].children[index]].is_text);
        if !merges {
            let id = Self::new_text_node(&mut nodes, parent);
            let len = nodes[parent].children.len();
            nodes[parent].children.insert(position, id);
            self.charge(len - position + 1);
        }
    }

    /// Recompute depths over the subtree rooted at `root` with an explicit
    /// stack, stopping at the first node beyond the limit.
    fn redepth(&self, nodes: &mut [Node], root: usize, depth: usize) {
        // Most placements attach a fresh leaf: no stack needed.
        if nodes[root].children.is_empty() && nodes[root].template_contents.is_none() {
            if depth > self.limit {
                self.fail(Verdict::TooDeep);
            } else if self.charge(depth.max(1)) {
                nodes[root].depth = depth;
            }
            return;
        }
        let mut stack = vec![(root, depth)];
        while let Some((node, depth)) = stack.pop() {
            if depth > self.limit {
                self.fail(Verdict::TooDeep);
                return;
            }
            if !self.charge(depth.max(1)) {
                return;
            }
            nodes[node].depth = depth;
            let children = &nodes[node].children;
            stack.extend(
                children
                    .iter()
                    .filter(|&&c| !nodes[c].is_text)
                    .map(|&c| (c, depth + 1)),
            );
            if let Some(contents) = nodes[node].template_contents {
                stack.push((contents, depth));
            }
        }
    }
}

impl TreeSink for DepthSink {
    type Handle = usize;
    type Output = ();
    type ElemName<'a> = Ref<'a, QualName>;

    fn finish(self) -> Self::Output {}

    fn parse_error(&self, _msg: Cow<'static, str>) {}

    fn get_document(&self) -> usize {
        0
    }

    fn elem_name<'a>(&'a self, target: &'a usize) -> Ref<'a, QualName> {
        Ref::map(self.nodes.borrow(), |nodes| &nodes[*target].name)
    }

    fn create_element(&self, name: QualName, _attrs: Vec<Attribute>, flags: ElementFlags) -> usize {
        let mut node = Node::new(name);
        node.annotation_xml_integration_point = flags.mathml_annotation_xml_integration_point;
        self.push(node)
    }

    fn create_comment(&self, _text: StrTendril) -> usize {
        self.push(Node::new(document_name()))
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> usize {
        self.push(Node::new(document_name()))
    }

    fn append(&self, parent: &usize, child: NodeOrText<usize>) {
        match child {
            NodeOrText::AppendNode(child) => self.attach(child, *parent, None),
            NodeOrText::AppendText(_) => self.append_text(*parent),
        }
    }

    fn append_based_on_parent_node(
        &self,
        element: &usize,
        prev_element: &usize,
        child: NodeOrText<usize>,
    ) {
        if self.nodes.borrow()[*element].parent.is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(&self, _: StrTendril, _: StrTendril, _: StrTendril) {}

    fn get_template_contents(&self, target: &usize) -> usize {
        if let Some(contents) = self.nodes.borrow()[*target].template_contents {
            return contents;
        }
        let contents = self.push(Node::new(document_name()));
        let mut nodes = self.nodes.borrow_mut();
        nodes[contents].depth = nodes[*target].depth;
        nodes[*target].template_contents = Some(contents);
        contents
    }

    fn same_node(&self, x: &usize, y: &usize) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &usize, new_node: NodeOrText<usize>) {
        let parent = self.nodes.borrow()[*sibling].parent;
        match new_node {
            NodeOrText::AppendNode(child) => match parent {
                Some(parent) => self.attach(child, parent, Some(*sibling)),
                None => {
                    let mut nodes = self.nodes.borrow_mut();
                    self.detach(&mut nodes, child);
                }
            },
            // Foster-parented text.
            NodeOrText::AppendText(_) => {
                if let Some(parent) = parent {
                    self.insert_text_before(parent, *sibling);
                }
            }
        }
    }

    fn add_attrs_if_missing(&self, _target: &usize, _attrs: Vec<Attribute>) {}

    fn remove_from_parent(&self, target: &usize) {
        let mut nodes = self.nodes.borrow_mut();
        self.detach(&mut nodes, *target);
    }

    fn reparent_children(&self, node: &usize, new_parent: &usize) {
        if self.stopped() {
            return;
        }
        let reparents = self.reparents.get() + 1;
        self.reparents.set(reparents);
        if reparents > self.max_reparents {
            self.fail(Verdict::TooMuchWork);
            return;
        }
        let mut nodes = self.nodes.borrow_mut();
        let moved = std::mem::take(&mut nodes[*node].children);
        let depth = nodes[*new_parent].depth + 1;
        for &child in &moved {
            nodes[child].parent = Some(*new_parent);
        }
        nodes[*new_parent].children.extend_from_slice(&moved);
        for child in moved {
            if self.stopped() {
                return;
            }
            self.redepth(&mut nodes, child, depth);
        }
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &usize) -> bool {
        self.nodes.borrow()[*handle].annotation_xml_integration_point
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn within(input: &str) -> bool {
        probe(input, MAX_NESTING_DEPTH) == Verdict::Within
    }

    #[test]
    fn depth_probe_accepts_ordinary_markup_and_rejects_deep_nesting() {
        assert_eq!(probe("<div><p>x</p></div>", 8), Verdict::Within);
        assert_eq!(probe(&"<p>x</p>".repeat(10_000), 8), Verdict::Within);
        assert!(!within(&"<div>".repeat(300)));
        assert!(within(&"<div>".repeat(200)));
        assert!(!within(&format!("<svg>{}</svg>", "<g>".repeat(300))));
        assert!(!within(&"<b><div><span>".repeat(100)));
        assert!(!within(&format!("<table>{}", "<div>".repeat(300))));
    }

    #[test]
    fn adoption_agency_moves_are_re_measured() {
        // The furthest block is appended into a detached formatting clone
        // before the clone is inserted; the subtree must be re-measured then.
        assert_eq!(
            probe(&"<b><i><div></b>".repeat(300), MAX_NESTING_DEPTH),
            Verdict::TooDeep
        );
        assert_eq!(
            probe(&"<a><b><div></a>".repeat(300), MAX_NESTING_DEPTH),
            Verdict::TooDeep
        );
        assert_eq!(
            probe(&"<b><i><div></b>".repeat(60), MAX_NESTING_DEPTH),
            Verdict::Within
        );
    }

    #[test]
    fn adoption_agency_reparenting_is_capped() {
        // Shallow but repeated misnesting: each `</b>` runs the adoption
        // agency without deepening the tree.
        let copies = BASE_REPARENTS * 2;
        let input = "<b><p>x</b></p>".repeat(copies);
        assert!(copies > max_reparents(input.len()));
        assert_eq!(probe(&input, MAX_NESTING_DEPTH), Verdict::TooMuchWork);
    }

    #[test]
    fn flat_content_at_maximum_depth_exhausts_the_work_budget() {
        let input = format!("{}{}", "<div>".repeat(250), "<div></div>".repeat(50_000));
        assert_eq!(probe(&input, MAX_NESTING_DEPTH), Verdict::TooMuchWork);
    }

    #[test]
    fn ordinary_nested_lists_stay_within_the_work_budget() {
        let mut list = String::new();
        for _ in 0..20 {
            list.push_str("<ul><li>item");
        }
        for _ in 0..20 {
            list.push_str("</li></ul>");
        }
        let document = format!("{list}<p>para</p>").repeat(2_000);
        assert_eq!(probe(&document, MAX_NESTING_DEPTH), Verdict::Within);
    }
}
