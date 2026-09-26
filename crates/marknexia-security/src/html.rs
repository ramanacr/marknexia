//! Parser-backed HTML/SVG sanitizer built on `ammonia` (html5ever).
//!
//! The effective policy reproduces the .NET `HtmlSanitizerService`
//! configuration of Ganss HtmlSanitizer 9.2.1039: the Ganss default tag and
//! attribute sets plus the Marknexia additions, minus the explicit removals.
//! Every decision is taken on the parsed DOM; markup is never rewritten with
//! string replacement.

use std::{
    borrow::Cow,
    collections::HashMap,
    io,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use ammonia::{Builder, UrlRelative};

use crate::{ContentPolicy, SanitizeError, UrlContext, UrlPolicy, attrs, css, depth, escape_cost};

/// Ganss default `AllowedTags` + .NET additions − .NET removals (`script`,
/// `iframe`, `object`, `embed`, `applet`, `form`, `base`, `meta`, `link`).
/// `html`, `head` and `body` are Ganss defaults too, but fragment parsing
/// never produces them, so listing them would have no effect.
const ALLOWED_TAGS: &[&str] = &[
    // Ganss defaults.
    "a",
    "abbr",
    "acronym",
    "address",
    "area",
    "b",
    "big",
    "blockquote",
    "br",
    "button",
    "caption",
    "center",
    "cite",
    "code",
    "col",
    "colgroup",
    "dd",
    "del",
    "dfn",
    "dir",
    "div",
    "dl",
    "dt",
    "em",
    "fieldset",
    "font",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "input",
    "ins",
    "kbd",
    "label",
    "legend",
    "li",
    "map",
    "menu",
    "ol",
    "optgroup",
    "option",
    "p",
    "pre",
    "q",
    "s",
    "samp",
    "select",
    "small",
    "span",
    "strike",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "tr",
    "tt",
    "u",
    "ul",
    "var",
    "section",
    "nav",
    "article",
    "aside",
    "header",
    "footer",
    "main",
    "figure",
    "figcaption",
    "data",
    "time",
    "mark",
    "ruby",
    "rt",
    "rp",
    "bdi",
    "wbr",
    "datalist",
    "keygen",
    "output",
    "progress",
    "meter",
    "details",
    "summary",
    "menuitem",
    // .NET additions (div, span, details, summary and input are already defaults).
    "svg",
    "path",
    "g",
    "circle",
    "rect",
    "line",
    "text",
    "defs",
    "marker",
    "polyline",
    "polygon",
    "use",
];

/// Known element names outside [`ALLOWED_TAGS`]. Ganss removes a disallowed
/// element together with its content (`KeepChildNodes = false`); ammonia keeps
/// the children of a removed element unless it is listed here, so every known
/// HTML, SVG and MathML element that is not allowed is enumerated.
const REMOVED_WITH_CONTENT: &[&str] = &[
    // .NET explicit removals.
    "script",
    "iframe",
    "object",
    "embed",
    "applet",
    "form",
    "base",
    "meta",
    "link",
    // Other HTML elements and raw-text / template containers.
    "style",
    "noscript",
    "noembed",
    "noframes",
    "frameset",
    "frame",
    "template",
    "title",
    "xmp",
    "plaintext",
    "listing",
    "audio",
    "video",
    "source",
    "track",
    "canvas",
    "picture",
    "param",
    "dialog",
    "slot",
    "portal",
    "search",
    "hgroup",
    "bdo",
    "rb",
    "rtc",
    "basefont",
    "bgsound",
    "blink",
    "marquee",
    "multicol",
    "nextid",
    "nobr",
    "spacer",
    "isindex",
    "image",
    "selectedcontent",
    "fencedframe",
    // SVG elements outside the .NET set (html5ever case-adjusted names).
    "animate",
    "animateMotion",
    "animateTransform",
    "clipPath",
    "desc",
    "discard",
    "ellipse",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
    "filter",
    "foreignObject",
    "linearGradient",
    "mask",
    "metadata",
    "mpath",
    "pattern",
    "radialGradient",
    "set",
    "stop",
    "switch",
    "symbol",
    "textPath",
    "tspan",
    "view",
    "font-face",
    "glyph",
    "missing-glyph",
    "hatch",
    "solidcolor",
    // MathML.
    "math",
    "mi",
    "mo",
    "mn",
    "ms",
    "mtext",
    "mglyph",
    "malignmark",
    "annotation",
    "annotation-xml",
    "semantics",
    "mrow",
    "mfrac",
    "msqrt",
    "mroot",
    "mstyle",
    "merror",
    "mpadded",
    "mphantom",
    "mfenced",
    "menclose",
    "msub",
    "msup",
    "msubsup",
    "munder",
    "mover",
    "munderover",
    "mmultiscripts",
    "mprescripts",
    "none",
    "mtable",
    "mtr",
    "mtd",
    "mlabeledtr",
    "maction",
    "mspace",
];

/// Ganss default `AllowedAttributes` + .NET additions. Matched on the parsed
/// local name, globally (not per tag), exactly like Ganss.
const ALLOWED_ATTRIBUTES: &[&str] = &[
    // Ganss defaults.
    "abbr",
    "accept",
    "accept-charset",
    "accesskey",
    "action",
    "align",
    "alt",
    "axis",
    "bgcolor",
    "border",
    "cellpadding",
    "cellspacing",
    "char",
    "charoff",
    "charset",
    "checked",
    "cite",
    "clear",
    "cols",
    "colspan",
    "color",
    "compact",
    "coords",
    "datetime",
    "dir",
    "disabled",
    "enctype",
    "for",
    "frame",
    "headers",
    "height",
    "href",
    "hreflang",
    "hspace",
    "ismap",
    "label",
    "lang",
    "longdesc",
    "maxlength",
    "media",
    "method",
    "multiple",
    "name",
    "nohref",
    "noshade",
    "nowrap",
    "prompt",
    "readonly",
    "rel",
    "rev",
    "rows",
    "rowspan",
    "rules",
    "scope",
    "selected",
    "shape",
    "size",
    "span",
    "src",
    "start",
    "style",
    "summary",
    "tabindex",
    "target",
    "title",
    "type",
    "usemap",
    "valign",
    "value",
    "vspace",
    "width",
    "high",
    "keytype",
    "list",
    "low",
    "max",
    "min",
    "novalidate",
    "open",
    "optimum",
    "pattern",
    "placeholder",
    "pubdate",
    "radiogroup",
    "required",
    "reversed",
    "spellcheck",
    "step",
    "wrap",
    "challenge",
    "contenteditable",
    "draggable",
    "dropzone",
    "autocomplete",
    "autosave",
    // .NET additions.
    "class",
    "id",
    "aria-hidden",
    "aria-label",
    "role",
    "viewBox",
    "fill",
    "stroke",
    "stroke-width",
    "d",
    "data-marknexia-action",
    "data-copy-text",
    "data-marknexia-zoom-status",
    "aria-live",
];

/// Schemes ammonia lets through its own absolute-URL screen. `javascript` and
/// `vbscript` are listed only so that the attribute filter below receives them
/// and rewrites them to `"#"` (the .NET pre-filter's observable behavior);
/// they can never be emitted.
const PASS_THROUGH_SCHEMES: &[&str] = &["http", "https", "mailto", "tel", "javascript", "vbscript"];

pub(crate) fn sanitize(input: &str, policy: ContentPolicy) -> Result<String, SanitizeError> {
    if input.trim().is_empty() {
        return Ok(String::new());
    }
    // Bound the tokenizer's per-tag O(k²) duplicate-attribute check before
    // any html5ever parse (probe or ammonia) runs.
    if !attrs::within_attribute_limit(input, attrs::MAX_ATTRIBUTES_PER_TAG) {
        return Err(SanitizeError::TooComplex);
    }
    let outcome = depth::measure(input, depth::MAX_NESTING_DEPTH);
    match outcome.verdict {
        depth::Verdict::Within => {}
        depth::Verdict::TooDeep => {
            return Err(SanitizeError::NestingTooDeep {
                limit: depth::MAX_NESTING_DEPTH,
            });
        }
        depth::Verdict::TooMuchWork | depth::Verdict::TooExpensiveToSerialize => {
            return Err(SanitizeError::TooComplex);
        }
    }
    // Attribute values are measured as ammonia finalizes them.
    let attribute_escape_cost = Arc::new(AtomicU64::new(0));
    let attribute_cost_sink = Arc::clone(&attribute_escape_cost);
    let mut builder = Builder::empty();
    builder
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .clean_content_tags(REMOVED_WITH_CONTENT.iter().copied().collect())
        .tag_attributes(HashMap::new())
        .tag_attribute_values(HashMap::new())
        .set_tag_attribute_values(HashMap::new())
        .generic_attributes(ALLOWED_ATTRIBUTES.iter().copied().collect())
        .url_schemes(PASS_THROUGH_SCHEMES.iter().copied().collect())
        .url_relative(UrlRelative::PassThrough)
        .link_rel(None)
        .strip_comments(true)
        .id_prefix(None)
        .attribute_filter(move |element, attribute, value| {
            let kept = filter_attribute(policy, element, attribute, value);
            if let Some(kept) = &kept {
                attribute_cost_sink.fetch_add(escape_cost::attribute_cost(kept), Ordering::Relaxed);
            }
            kept
        });
    let document = builder.clean(input);
    // html5ever's escaping is quadratic in `&`/0xC2 density; refuse before
    // serializing when its scans would exceed the budget.
    let serialize_cost = outcome
        .text_escape_cost
        .saturating_add(attribute_escape_cost.load(Ordering::Relaxed));
    if serialize_cost > escape_cost::MAX_ESCAPE_SCAN_BYTES {
        return Err(SanitizeError::TooComplex);
    }

    let limit = policy.limits().max_html_output_bytes;
    let mut writer = BoundedWriter {
        buffer: Vec::new(),
        limit,
        exceeded: false,
    };
    let written = document.write_to(&mut writer);
    if writer.exceeded {
        return Err(SanitizeError::OutputTooLarge { limit });
    }
    written.map_err(|_| SanitizeError::Serialization)?;
    String::from_utf8(writer.buffer).map_err(|_| SanitizeError::Serialization)
}

/// Final authority for every surviving attribute value (already
/// entity-decoded by the parser).
fn filter_attribute<'u>(
    policy: ContentPolicy,
    element: &str,
    attribute: &str,
    value: &'u str,
) -> Option<Cow<'u, str>> {
    // Ganss: the "& JavaScript include" construct removes the attribute.
    if value.contains("&{") {
        return None;
    }
    match attribute {
        "href" | "src" | "action" | "cite" | "longdesc" => {
            filter_url(policy, element, attribute, value)
        }
        "style" => {
            if value.len() > policy.limits().max_css_input_bytes {
                return None;
            }
            let sanitized = css::sanitize_declarations(value);
            if sanitized.is_empty() {
                None
            } else if sanitized == value {
                Some(Cow::Borrowed(value))
            } else {
                Some(Cow::Owned(sanitized))
            }
        }
        // SVG paint may reference a same-document paint server only:
        // `url(#id)` is kept, any other `url()` removes the attribute.
        "fill" | "stroke" => css::is_safe_paint(value).then_some(Cow::Borrowed(value)),
        _ => Some(Cow::Borrowed(value)),
    }
}

fn filter_url<'u>(
    policy: ContentPolicy,
    element: &str,
    attribute: &str,
    value: &'u str,
) -> Option<Cow<'u, str>> {
    // An empty reference is inert and the .NET sanitizer keeps it (the
    // remote-images-off rendering fixture emits `<img src="">`).
    if value.is_empty() {
        return Some(Cow::Borrowed(value));
    }
    // Parity with the .NET pre-filter, which rewrites script-scheme href/src
    // to "#". The scheme comes from the WHATWG URL parser, so whitespace,
    // tab/newline and case obfuscation are normalized exactly as a browser
    // would normalize them.
    if is_script_scheme(value) {
        return Some(Cow::Borrowed("#"));
    }
    let context = if attribute == "src" {
        UrlContext::Image
    } else if element == "use" {
        UrlContext::SvgReference
    } else {
        UrlContext::Link
    };
    UrlPolicy::new(policy)
        .validate(value, context)
        .ok()
        .map(|_| Cow::Borrowed(value))
}

fn is_script_scheme(value: &str) -> bool {
    url::Url::parse(value)
        .map(|url| matches!(url.scheme(), "javascript" | "vbscript"))
        .unwrap_or(false)
}

/// Writer that refuses to grow past the output budget, so an expanding
/// document is rejected while it is being serialized instead of afterwards.
struct BoundedWriter {
    buffer: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl io::Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.buffer.len().saturating_add(bytes.len()) > self.limit {
            self.exceeded = true;
            return Err(io::Error::other("sanitized output budget exceeded"));
        }
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_and_removed_sets_are_disjoint() {
        for tag in REMOVED_WITH_CONTENT {
            assert!(
                !ALLOWED_TAGS.contains(tag),
                "{tag} is both allowed and removed"
            );
        }
    }
}
