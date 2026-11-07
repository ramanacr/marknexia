//! Marknexia-owned HTML serializer for ammonia's cleaned DOM.
//!
//! It writes exactly what html5ever 0.40.1's `HtmlSerializer` writes for the
//! same tree (the differential tests in this module check that), with two
//! changes:
//!
//! * **Linear escaping.** html5ever's `write_escaped` rescans the rest of a
//!   text node or attribute value up to the next `<`/`>` (and `"`) for every
//!   `&` or 0xC2 byte, which is quadratic. Here every byte is looked at once.
//!   The escaped output is the same per-character mapping: `&` → `&amp;`,
//!   U+00A0 → `&nbsp;`, `<` → `&lt;`, `>` → `&gt;`, and in attribute values
//!   also `"` → `&quot;`.
//! * **Leading line feed.** The HTML parser drops one line feed right after a
//!   `<pre>`, `<listing>` or `<textarea>` start tag, and html5ever does not
//!   write it back. When such an element's first child is a text node that
//!   starts with a line feed, one extra line feed follows the start tag, as
//!   the HTML fragment serialization algorithm of AngleSharp and browsers
//!   does, so re-parsing the output restores the same text and sanitization
//!   is a fixed point.
//!
//! The output budget is shared by every chunk of a document and charged while
//! writing, so an expanding document fails once the total passes the limit.

use std::sync::atomic::{AtomicUsize, Ordering};

use ammonia::rcdom::{Handle, NodeData};
use html5ever::{LocalName, QualName, local_name, ns};

use crate::SanitizeError;

/// Output bytes written by all chunks of one document, against its limit.
pub(crate) struct OutputBudget {
    written: AtomicUsize,
    limit: usize,
}

/// Local writes are charged to the shared budget in steps of this size.
const CHARGE_STEP: usize = 64 * 1024;

impl OutputBudget {
    pub(crate) const fn new(limit: usize) -> Self {
        Self {
            written: AtomicUsize::new(0),
            limit,
        }
    }

    fn charge(&self, bytes: usize) -> Result<(), SanitizeError> {
        let total = self.written.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if total > self.limit {
            Err(SanitizeError::OutputTooLarge { limit: self.limit })
        } else {
            Ok(())
        }
    }
}

struct ElemInfo {
    html_name: Option<LocalName>,
    ignore_children: bool,
}

enum Op {
    Open(Handle),
    Close(QualName),
}

struct Writer<'b> {
    out: String,
    charged: usize,
    budget: &'b OutputBudget,
    restore_leading_lf: bool,
    /// The output starts the whole fragment (see [`BOM_REFERENCE`]).
    document_start: bool,
}

/// The HTML parser discards a U+FEFF at the very start of its input. A text
/// node can still start with one there (when the input began with
/// characters the parser drops, such as NUL), so a U+FEFF that would be the
/// first character of the whole output is written as this character
/// reference: re-parsing decodes it to the same text, and sanitizing stays a
/// fixed point.
const BOM_REFERENCE: &str = "&#xFEFF;";

impl Writer<'_> {
    fn push(&mut self, text: &str) -> Result<(), SanitizeError> {
        self.out.push_str(text);
        self.settle(false)
    }

    /// Charge what was written since the last charge, in steps (or all of it
    /// when `all`).
    fn settle(&mut self, all: bool) -> Result<(), SanitizeError> {
        let pending = self.out.len() - self.charged;
        if pending >= CHARGE_STEP || (all && pending > 0) || self.out.len() > self.budget.limit {
            self.charged = self.out.len();
            self.budget.charge(pending)?;
        }
        Ok(())
    }

    fn escaped(&mut self, text: &str, attribute: bool) -> Result<(), SanitizeError> {
        let bytes = text.as_bytes();
        let mut copied = 0;
        let mut index = 0;
        while index < bytes.len() {
            let replacement = match bytes[index] {
                b'&' => "&amp;",
                b'<' => "&lt;",
                b'>' => "&gt;",
                b'"' if attribute => "&quot;",
                0xC2 if bytes.get(index + 1) == Some(&0xA0) => "&nbsp;",
                _ => {
                    index += 1;
                    continue;
                }
            };
            self.out.push_str(&text[copied..index]);
            self.out.push_str(replacement);
            index += if bytes[index] == 0xC2 { 2 } else { 1 };
            copied = index;
            if self.out.len() - self.charged >= CHARGE_STEP {
                self.settle(false)?;
            }
        }
        self.out.push_str(&text[copied..]);
        self.settle(false)
    }

    fn start_elem(
        &mut self,
        node: &Handle,
        name: &QualName,
        attrs: &[html5ever::Attribute],
        stack: &mut Vec<ElemInfo>,
    ) -> Result<(), SanitizeError> {
        let html_name = (name.ns == ns!(html)).then(|| name.local.clone());
        if stack.last().is_some_and(|parent| parent.ignore_children) {
            stack.push(ElemInfo {
                html_name,
                ignore_children: true,
            });
            return Ok(());
        }
        self.out.push('<');
        self.out.push_str(&name.local);
        for attribute in attrs {
            self.out.push(' ');
            match attribute.name.ns {
                ns!() => {}
                ns!(xml) => self.out.push_str("xml:"),
                ns!(xmlns) => {
                    if attribute.name.local != local_name!("xmlns") {
                        self.out.push_str("xmlns:");
                    }
                }
                ns!(xlink) => self.out.push_str("xlink:"),
                _ => self.out.push_str("unknown_namespace:"),
            }
            self.out.push_str(&attribute.name.local);
            self.out.push_str("=\"");
            self.escaped(&attribute.value, true)?;
            self.out.push('"');
        }
        self.out.push('>');
        if self.restore_leading_lf
            && matches!(
                html_name,
                Some(local_name!("pre") | local_name!("listing") | local_name!("textarea"))
            )
            && starts_with_line_feed(node)
        {
            self.out.push('\n');
        }
        let ignore_children = html_name.as_ref().is_some_and(is_void);
        stack.push(ElemInfo {
            html_name,
            ignore_children,
        });
        self.settle(false)
    }

    fn end_elem(
        &mut self,
        name: &QualName,
        stack: &mut Vec<ElemInfo>,
    ) -> Result<(), SanitizeError> {
        let ignore = stack.pop().is_some_and(|info| info.ignore_children);
        if ignore {
            return Ok(());
        }
        self.out.push_str("</");
        self.out.push_str(&name.local);
        self.push(">")
    }

    fn text(&mut self, text: &str, stack: &[ElemInfo]) -> Result<(), SanitizeError> {
        let raw = matches!(
            stack.last().and_then(|parent| parent.html_name.as_ref()),
            Some(
                &local_name!("style")
                    | &local_name!("script")
                    | &local_name!("xmp")
                    | &local_name!("iframe")
                    | &local_name!("noembed")
                    | &local_name!("noframes")
                    | &local_name!("plaintext")
                    // Scripting is enabled in the serializer options.
                    | &local_name!("noscript")
            )
        );
        if raw {
            self.push(text)
        } else if self.document_start && self.out.is_empty() && text.starts_with('\u{feff}') {
            self.push(BOM_REFERENCE)?;
            self.escaped(&text['\u{feff}'.len_utf8()..], false)
        } else {
            self.escaped(text, false)
        }
    }
}

fn is_void(name: &LocalName) -> bool {
    matches!(
        *name,
        local_name!("area")
            | local_name!("base")
            | local_name!("basefont")
            | local_name!("bgsound")
            | local_name!("br")
            | local_name!("col")
            | local_name!("embed")
            | local_name!("frame")
            | local_name!("hr")
            | local_name!("img")
            | local_name!("input")
            | local_name!("keygen")
            | local_name!("link")
            | local_name!("meta")
            | local_name!("param")
            | local_name!("source")
            | local_name!("track")
            | local_name!("wbr")
    )
}

fn starts_with_line_feed(node: &Handle) -> bool {
    node.children.borrow().first().is_some_and(|first| {
        matches!(&first.data, NodeData::Text { contents } if contents.borrow().starts_with('\n'))
    })
}

/// Serialize the children of `root` (ammonia's `<html>` fragment root), as
/// ammonia's `Document::write_to` does, into a new string. `document_start`
/// is set for the chunk whose output starts the fragment.
pub(crate) fn serialize_children(
    root: &Handle,
    budget: &OutputBudget,
    capacity: usize,
    document_start: bool,
) -> Result<String, SanitizeError> {
    serialize_with(root, budget, capacity, true, document_start)
}

fn serialize_with(
    root: &Handle,
    budget: &OutputBudget,
    capacity: usize,
    restore_leading_lf: bool,
    document_start: bool,
) -> Result<String, SanitizeError> {
    let mut writer = Writer {
        out: String::with_capacity(capacity.min(budget.limit)),
        charged: 0,
        budget,
        restore_leading_lf,
        document_start,
    };
    // `TraversalScope::ChildrenOnly(None)`: the root's own name is not used.
    let mut stack = vec![ElemInfo {
        html_name: None,
        ignore_children: false,
    }];
    let mut ops: Vec<Op> = root
        .children
        .borrow()
        .iter()
        .rev()
        .map(|child| Op::Open(child.clone()))
        .collect();
    while let Some(op) = ops.pop() {
        match op {
            Op::Close(name) => writer.end_elem(&name, &mut stack)?,
            Op::Open(node) => match &node.data {
                NodeData::Element { name, attrs, .. } => {
                    writer.start_elem(&node, name, &attrs.borrow(), &mut stack)?;
                    ops.push(Op::Close(name.clone()));
                    ops.extend(
                        node.children
                            .borrow()
                            .iter()
                            .rev()
                            .map(|child| Op::Open(child.clone())),
                    );
                }
                NodeData::Text { contents } => writer.text(&contents.borrow(), &stack)?,
                NodeData::Comment { contents } => {
                    writer.out.push_str("<!--");
                    writer.out.push_str(contents);
                    writer.push("-->")?;
                }
                NodeData::Doctype { name } => {
                    writer.out.push_str("<!DOCTYPE ");
                    writer.out.push_str(name);
                    writer.push(">")?;
                }
                NodeData::ProcessingInstruction { target, contents } => {
                    writer.out.push_str("<?");
                    writer.out.push_str(target);
                    writer.out.push(' ');
                    writer.out.push_str(contents);
                    writer.push(">")?;
                }
                NodeData::Document => {}
            },
        }
    }
    writer.settle(true)?;
    Ok(writer.out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ammonia::rcdom::SerializableHandle;
    use html5ever::serialize::{SerializeOpts, serialize};

    /// html5ever's serializer over the same DOM.
    fn reference(root: &Handle) -> String {
        let mut buffer = Vec::new();
        let handle: SerializableHandle = root.clone().into();
        serialize(&mut buffer, &handle, SerializeOpts::default()).unwrap();
        String::from_utf8(buffer).unwrap()
    }

    fn both(input: &str) -> (String, String) {
        // A permissive builder so comments, raw-text and foreign elements
        // reach the serializer too.
        let document = ammonia::Builder::default()
            .rm_clean_content_tags(["style", "script"])
            .add_tags([
                "style", "xmp", "svg", "path", "pre", "textarea", "listing", "noscript",
            ])
            .strip_comments(false)
            .add_generic_attributes(["xlink:href", "xml:lang", "title", "href"])
            .clean(input);
        let root = document.to_dom_node();
        let budget = OutputBudget::new(usize::MAX);
        let ours = serialize_with(&root, &budget, 0, false, false).unwrap();
        (ours, reference(&root))
    }

    #[test]
    fn matches_html5ever_byte_for_byte() {
        for input in [
            "a &amp; b &lt; c &gt; d \u{a0} \u{a9} \u{c2}",
            "<p title=\"&quot;&amp;&lt;&gt;\u{a0}\">x</p>",
            "<br><img src=\"a.png\" alt=\"x\"><hr><input value=\"&amp;\">",
            "<style>a > b & c</style><xmp>&</xmp><noscript><b>&</b></noscript>",
            "<!-- c & d --><p>x</p>",
            "<svg><path d=\"M0 0\"/></svg><svg xlink:href=\"#a\"></svg>",
            "<pre>\n\nx</pre><textarea>\nx</textarea>",
            "<table><tr><td>a</td></tr></table>",
            "<b><i>x</b>y</i>",
        ] {
            let (ours, theirs) = both(input);
            assert_eq!(ours, theirs, "{input}");
        }
    }

    #[test]
    fn matches_html5ever_on_generated_markup() {
        const PARTS: &[&str] = &[
            "<p>",
            "</p>",
            "<b>",
            "</b>",
            "<pre>",
            "</pre>",
            "<textarea>",
            "<br>",
            "<img src=x>",
            "&amp;",
            "&",
            "\u{a0}",
            "\u{c2}\u{a9}",
            "\"",
            "'",
            "<",
            ">",
            "\n",
            "x",
            "é",
            "<a href=\"a&amp;b\">",
            "</a>",
            "<!--",
            "-->",
            "<table>",
            "<td>",
            "<svg>",
            "<style>",
            "</style>",
            "<span title=\"a<b>&c\u{a0}\">",
        ];
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        for _ in 0..2_000 {
            let mut input = String::new();
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            for step in 0..(seed % 40) {
                let index = (seed.rotate_left(step as u32 * 5) % PARTS.len() as u64) as usize;
                input.push_str(PARTS[index]);
            }
            let (ours, theirs) = both(&input);
            assert_eq!(ours, theirs, "{input:?}");
        }
    }

    #[test]
    fn escaping_is_linear() {
        let text = "x&y ".repeat(2_000_000);
        let document = ammonia::Builder::default().clean(&text);
        let started = std::time::Instant::now();
        let budget = OutputBudget::new(usize::MAX);
        let out = serialize_children(&document.to_dom_node(), &budget, 0, true).unwrap();
        assert_eq!(out.len(), text.len() + 4 * 2_000_000);
        let bound = if cfg!(debug_assertions) { 20 } else { 1 };
        assert!(started.elapsed().as_secs() < bound);
    }

    #[test]
    fn output_budget_is_shared_and_exact() {
        let document = ammonia::Builder::default().clean("<p>abc</p>");
        let root = document.to_dom_node();
        let budget = OutputBudget::new(20);
        assert!(serialize_children(&root, &budget, 0, true).is_ok());
        assert!(serialize_children(&root, &budget, 0, true).is_ok());
        assert_eq!(
            serialize_children(&root, &budget, 0, true),
            Err(SanitizeError::OutputTooLarge { limit: 20 })
        );
    }
}
