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
//!   input length. Adoption-agency reparenting is capped separately.
//!
//! The input is fed in chunks and the probe stops at the first chunk that
//! breaks a limit, so rejected work stays linear in the input length.

use std::{
    borrow::Cow,
    cell::{Cell, Ref, RefCell},
    rc::Rc,
};

use html5ever::{
    Attribute, ParseOpts, QualName, local_name, ns, parse_fragment,
    tendril::{StrTendril, TendrilSink},
    tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink},
};

/// Maximum element nesting accepted by the sanitizer.
pub const MAX_NESTING_DEPTH: usize = 256;

/// Work units allowed per input byte, on top of [`BASE_WORK`]. Ordinary
/// Markdown output uses well under 3 units per byte (a 20-level nested list is
/// about 2). Flat content at maximum depth uses about 23.
pub(crate) const WORK_PER_INPUT_BYTE: usize = 8;
pub(crate) const BASE_WORK: usize = 256 * 1024;

/// Maximum adoption-agency `reparent_children` calls per input.
pub(crate) const MAX_REPARENTS: usize = 4_096;

const CHUNK_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Verdict {
    Within,
    TooDeep,
    TooMuchWork,
}

/// Measure `input` against the depth `limit` and the linear work budget.
pub(crate) fn probe(input: &str, limit: usize) -> Verdict {
    let verdict = Rc::new(Cell::new(Verdict::Within));
    let sink = DepthSink {
        nodes: RefCell::new(vec![Node::new(document_name())]),
        limit,
        budget: input
            .len()
            .saturating_mul(WORK_PER_INPUT_BYTE)
            .saturating_add(BASE_WORK),
        work: Cell::new(0),
        reparents: Cell::new(0),
        verdict: Rc::clone(&verdict),
    };
    let mut parser = parse_fragment(
        sink,
        ParseOpts::default(),
        QualName::new(None, ns!(html), local_name!("div")),
        Vec::new(),
        false,
    );
    let mut rest = input;
    while !rest.is_empty() {
        let mut split = rest.len().min(CHUNK_BYTES);
        while !rest.is_char_boundary(split) {
            split += 1;
        }
        let (chunk, tail) = rest.split_at(split);
        parser.process(StrTendril::from_slice(chunk));
        if verdict.get() != Verdict::Within {
            return verdict.get();
        }
        rest = tail;
    }
    parser.finish();
    verdict.get()
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
        }
    }
}

struct DepthSink {
    nodes: RefCell<Vec<Node>>,
    limit: usize,
    budget: usize,
    work: Cell<usize>,
    reparents: Cell<usize>,
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
            self.charge(siblings.len() - position);
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
        self.charge(siblings.len() - position + 1);
        siblings.insert(position, child);
        nodes[child].parent = Some(parent);
        let depth = nodes[parent].depth + 1;
        self.redepth(&mut nodes, child, depth);
    }

    /// Recompute depths over the subtree rooted at `root` with an explicit
    /// stack, stopping at the first node beyond the limit.
    fn redepth(&self, nodes: &mut [Node], root: usize, depth: usize) {
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
            stack.extend(nodes[node].children.iter().map(|&c| (c, depth + 1)));
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
        if let NodeOrText::AppendNode(child) = child {
            self.attach(child, *parent, None);
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
        if let NodeOrText::AppendNode(child) = new_node {
            let parent = self.nodes.borrow()[*sibling].parent;
            match parent {
                Some(parent) => self.attach(child, parent, Some(*sibling)),
                None => {
                    let mut nodes = self.nodes.borrow_mut();
                    self.detach(&mut nodes, child);
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
        if reparents > MAX_REPARENTS {
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
        let input = "<b><p>x</b></p>".repeat(MAX_REPARENTS + 1);
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
