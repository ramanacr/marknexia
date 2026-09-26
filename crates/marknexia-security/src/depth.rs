//! Nesting-depth probe run before sanitization.
//!
//! html5ever's tree builder scans the stack of open elements for many start
//! and end tags (for example "has a `p` element in button scope" on every
//! `<div>`), so deeply nested input costs time quadratic in its depth. That
//! makes it a CPU denial-of-service vector well inside the byte limits:
//! 20,000 nested `<div>`s is 100 KiB. This probe runs the same html5ever
//! fragment parse (same options and `<div>` context as ammonia) into a sink
//! that stores no content, only each node's tree depth. The input is fed in
//! chunks and the probe stops as soon as the depth budget is exceeded, so the
//! rejected work stays bounded by `input length × budget`.
//!
//! Tree depth is an upper bound on the size of the open-element stack, up to
//! the constant offsets introduced by foster parenting and the adoption agency.
//! Those are covered by a margin in the budget, which is well above any
//! rendered Markdown and below the 512-level cap browsers apply to parser
//! nesting.

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

const CHUNK_BYTES: usize = 16 * 1024;

/// Returns `true` when `input` parses with at most `limit` nested nodes.
pub(crate) fn within_depth(input: &str, limit: usize) -> bool {
    let exceeded = Rc::new(Cell::new(false));
    let sink = DepthSink {
        nodes: RefCell::new(vec![Node::new(document_name(), 0, None)]),
        limit,
        exceeded: Rc::clone(&exceeded),
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
        if exceeded.get() {
            return false;
        }
        rest = tail;
    }
    parser.finish();
    !exceeded.get()
}

fn document_name() -> QualName {
    QualName::new(None, ns!(), local_name!(""))
}

struct Node {
    name: QualName,
    depth: usize,
    parent: Option<usize>,
    annotation_xml_integration_point: bool,
    template_contents: Option<usize>,
}

impl Node {
    fn new(name: QualName, depth: usize, parent: Option<usize>) -> Self {
        Self {
            name,
            depth,
            parent,
            annotation_xml_integration_point: false,
            template_contents: None,
        }
    }
}

struct DepthSink {
    nodes: RefCell<Vec<Node>>,
    limit: usize,
    exceeded: Rc<Cell<bool>>,
}

impl DepthSink {
    fn push(&self, node: Node) -> usize {
        let mut nodes = self.nodes.borrow_mut();
        nodes.push(node);
        nodes.len() - 1
    }

    fn place(&self, child: usize, parent: Option<usize>, depth: usize) {
        let mut nodes = self.nodes.borrow_mut();
        nodes[child].parent = parent;
        nodes[child].depth = depth;
        if depth > self.limit {
            self.exceeded.set(true);
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
        let mut node = Node::new(name, 0, None);
        node.annotation_xml_integration_point = flags.mathml_annotation_xml_integration_point;
        self.push(node)
    }

    fn create_comment(&self, _text: StrTendril) -> usize {
        self.push(Node::new(document_name(), 0, None))
    }

    fn create_pi(&self, _target: StrTendril, _data: StrTendril) -> usize {
        self.push(Node::new(document_name(), 0, None))
    }

    fn append(&self, parent: &usize, child: NodeOrText<usize>) {
        if let NodeOrText::AppendNode(child) = child {
            let depth = self.nodes.borrow()[*parent].depth + 1;
            self.place(child, Some(*parent), depth);
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
        let depth = self.nodes.borrow()[*target].depth;
        let contents = self.push(Node::new(document_name(), depth, Some(*target)));
        self.nodes.borrow_mut()[*target].template_contents = Some(contents);
        contents
    }

    fn same_node(&self, x: &usize, y: &usize) -> bool {
        x == y
    }

    fn set_quirks_mode(&self, _mode: QuirksMode) {}

    fn append_before_sibling(&self, sibling: &usize, new_node: NodeOrText<usize>) {
        if let NodeOrText::AppendNode(child) = new_node {
            let (parent, depth) = {
                let nodes = self.nodes.borrow();
                (nodes[*sibling].parent, nodes[*sibling].depth)
            };
            self.place(child, parent, depth);
        }
    }

    fn add_attrs_if_missing(&self, _target: &usize, _attrs: Vec<Attribute>) {}

    fn remove_from_parent(&self, target: &usize) {
        self.nodes.borrow_mut()[*target].parent = None;
    }

    fn reparent_children(&self, _node: &usize, _new_parent: &usize) {
        // Children are not tracked; moved subtrees keep their recorded depth.
        // The adoption agency that calls this adds one level per run, and
        // every new node is still measured against its actual parent.
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &usize) -> bool {
        self.nodes.borrow()[*handle].annotation_xml_integration_point
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_probe_accepts_ordinary_markup_and_rejects_deep_nesting() {
        assert!(within_depth("<div><p>x</p></div>", 8));
        assert!(within_depth(&"<p>x</p>".repeat(10_000), 8));
        assert!(!within_depth(&"<div>".repeat(300), MAX_NESTING_DEPTH));
        assert!(within_depth(&"<div>".repeat(200), MAX_NESTING_DEPTH));
        assert!(!within_depth(
            &format!("<svg>{}</svg>", "<g>".repeat(300)),
            MAX_NESTING_DEPTH
        ));
        assert!(!within_depth(
            &"<b><div><span>".repeat(100),
            MAX_NESTING_DEPTH
        ));
        assert!(!within_depth(
            &format!("<table>{}", "<div>".repeat(300)),
            MAX_NESTING_DEPTH
        ));
    }
}
