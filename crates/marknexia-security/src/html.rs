//! Parser-backed HTML/SVG sanitizer built on `ammonia` (html5ever).
//!
//! The effective policy reproduces the .NET `HtmlSanitizerService`
//! configuration of Ganss HtmlSanitizer 9.2.1039: the Ganss default tag and
//! attribute sets plus the Marknexia additions, minus the explicit removals.
//! Every decision is taken on the parsed DOM; markup is never rewritten with
//! string replacement.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    collections::HashMap,
    ops::Range,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use ammonia::{Builder, UrlRelative};

use crate::{
    ContentPolicy, RemoteImagePolicy, SanitizeError, UrlContext, UrlPolicy, attrs, css, depth,
    serialize::{self, OutputBudget},
};

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

/// Chunks are aimed at this size (they end at the first possible split point
/// after it). The chunk is the unit of ammonia's parse and of the DOM kept in
/// memory at once per worker.
pub(crate) const CHUNK_TARGET_BYTES: usize = 256 * 1024;

/// Segments are aimed at this size. The probe runs over the segments in
/// parallel, each from a fresh parser; see [`sanitize`].
pub(crate) const SEGMENT_TARGET_BYTES: usize = 1024 * 1024;

/// Upper bound on sanitizer worker threads for one document.
const MAX_WORKERS: usize = 8;

/// Per-call settings. None of them changes the output; the chunk and
/// segment targets can change which budget error a hostile input gets.
#[derive(Clone, Copy)]
pub(crate) struct Run<'c> {
    /// Polled between parser pieces and chunks; `true` stops with
    /// [`SanitizeError::Cancelled`].
    pub(crate) cancel: Option<&'c (dyn Fn() -> bool + Sync)>,
    /// Chunk size target; `usize::MAX` keeps the input in one chunk.
    pub(crate) chunk_target: usize,
    /// Segment size target; `usize::MAX` probes the input in one run.
    pub(crate) segment_target: usize,
    /// Upper bound on worker threads (1 runs everything on the caller).
    pub(crate) max_workers: usize,
}

impl Run<'_> {
    pub(crate) const DEFAULT: Run<'static> = Run {
        cancel: None,
        chunk_target: CHUNK_TARGET_BYTES,
        segment_target: SEGMENT_TARGET_BYTES,
        max_workers: MAX_WORKERS,
    };

    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|cancel| cancel())
    }
}

/// Sanitize `input` under `policy`.
///
/// 1. The attribute pre-scan bounds attributes per tag and finds the split
///    candidates, where the tokenizer is certainly in the data state
///    (`attrs::scan`).
/// 2. The input is cut into segments of about [`SEGMENT_TARGET_BYTES`] at
///    split candidates, preferably at likely block ends. Each segment is
///    probed from a fresh parser, in parallel (`depth::Planner`); the probe
///    checks depth and work and plans the chunks inside the segment.
/// 3. The segment results are confirmed in order (`plan_chunks`): a probe is
///    the real parse from the first point where both are at the initial
///    state, and from there its chunks are accepted and its work is charged
///    to the document's budget. Between such points the real parse runs
///    sequentially, with what is left of the budget.
/// 4. ammonia cleans every accepted chunk as its own fragment, in parallel,
///    and the Marknexia serializer writes it (`serialize`). Outputs are
///    joined in input order.
///
/// The result is byte-identical to cleaning the whole input as one fragment,
/// and independent of the number of threads. It is returned only when the
/// whole input passed every budget; otherwise no partial output exists.
pub(crate) fn sanitize(
    input: &str,
    policy: ContentPolicy,
    run: Run<'_>,
) -> Result<String, SanitizeError> {
    // Only empty input short-circuits (M1): white space is parsed like any
    // other text, so sanitizing is a fixed point.
    if input.is_empty() {
        return Ok(String::new());
    }
    // Bound the tokenizer's per-tag O(k²) duplicate-attribute check before
    // any html5ever parse (probe or ammonia) runs.
    let candidates =
        attrs::scan(input, attrs::MAX_ATTRIBUTES_PER_TAG).ok_or(SanitizeError::TooComplex)?;
    let workers = run
        .max_workers
        .min(std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
        .max(1);
    let chunks = plan_chunks(input, &candidates, run, workers)?;
    drop(candidates);
    clean_chunks(input, &chunks, policy, run, workers)
}

/// Steps 1 to 3 of [`sanitize`]: the chunks of `input`.
pub(crate) fn chunk_ranges(
    input: &str,
    run: Run<'_>,
) -> Result<Vec<(Range<usize>, usize)>, SanitizeError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let candidates =
        attrs::scan(input, attrs::MAX_ATTRIBUTES_PER_TAG).ok_or(SanitizeError::TooComplex)?;
    plan_chunks(input, &candidates, run, run.max_workers).map(|chunks| {
        chunks
            .into_iter()
            .map(|chunk| (chunk.range, chunk.nodes))
            .collect()
    })
}

/// Segment boundaries: 0, then the first likely candidate at least `target`
/// bytes after the previous boundary, and the input length. Without likely
/// candidates the input stays one segment: a speculative probe that starts
/// inside open markup would only be parsed again.
fn segment_bounds(input: &str, candidates: &attrs::Candidates, target: usize) -> Vec<usize> {
    let likely = &candidates.likely;
    let mut bounds = vec![0_usize];
    loop {
        let from = bounds[bounds.len() - 1].saturating_add(target);
        if from >= input.len() {
            break;
        }
        let index = likely.partition_point(|&candidate| candidate < from);
        match likely.get(index) {
            Some(&candidate) if candidate < input.len() => bounds.push(candidate),
            _ => break,
        }
    }
    bounds.push(input.len());
    bounds
}

/// One segment's probe from a fresh parser.
struct SegmentProbe {
    verdict: depth::Verdict,
    /// The parser was at its initial state at the end of the segment.
    ends_initial: bool,
    /// Closed chunks.
    chunks: Vec<depth::Chunk>,
    /// Budget used by `chunks`.
    used: depth::Budget,
    /// Budget used when the probe stopped.
    total: depth::Budget,
    /// Start of the unclosed tail after `chunks`.
    tail: usize,
}

fn probe_segment(
    input: &str,
    candidates: &[usize],
    start: usize,
    end: usize,
    run: Run<'_>,
) -> SegmentProbe {
    let mut planner = depth::Planner::new(
        input,
        candidates,
        start,
        run.chunk_target,
        depth::MAX_NESTING_DEPTH,
        depth::Budget::for_input(end - start),
    );
    let mut verdict = planner.advance(end, &|| run.cancelled());
    let mut ends_initial = false;
    if verdict == depth::Verdict::Within {
        if end == input.len() {
            verdict = planner.finish();
            ends_initial = verdict == depth::Verdict::Within;
        } else if planner.at_initial_state() {
            planner.close_chunk();
            ends_initial = true;
        }
    }
    SegmentProbe {
        verdict,
        ends_initial,
        used: planner.used_at_last_cut(),
        total: planner.used(),
        tail: planner.chunk_start(),
        chunks: planner.take_chunks(),
    }
}

fn verdict_result(verdict: depth::Verdict) -> Result<(), SanitizeError> {
    match verdict {
        depth::Verdict::Within => Ok(()),
        depth::Verdict::TooDeep => Err(SanitizeError::NestingTooDeep {
            limit: depth::MAX_NESTING_DEPTH,
        }),
        depth::Verdict::TooMuchWork => Err(SanitizeError::TooComplex),
        depth::Verdict::Cancelled => Err(SanitizeError::Cancelled),
    }
}

/// Steps 2 and 3 of [`sanitize`]: the accepted chunks, covering the input.
///
/// A segment probe is the real parse from any point at which both it and the
/// real parse are at the initial state: its start, when the previous segment
/// ended there, or else one of its own chunk boundaries. From such a point
/// both parse the same bytes from the same state. So where a segment ends
/// inside open markup, the real parse continues sequentially only until it
/// meets the next segment's probe at such a point, usually within one chunk.
fn plan_chunks(
    input: &str,
    candidates: &attrs::Candidates,
    run: Run<'_>,
    workers: usize,
) -> Result<Vec<depth::Chunk>, SanitizeError> {
    // Segments start where the parser is likely back at its initial state,
    // so that most probes are the real parse from their start.
    let bounds = segment_bounds(input, candidates, run.segment_target);
    let candidates = &candidates.all;
    let segments = bounds.len() - 1;
    let probes = in_parallel(segments, workers, |_, segment| {
        probe_segment(input, candidates, bounds[segment], bounds[segment + 1], run)
    });
    let cancelled = || run.cancelled();
    let cap = depth::Budget::for_input(input.len());
    let mut used = depth::Budget::ZERO;
    let mut chunks = Vec::new();
    let mut segment = 0;
    // `None`: `bounds[segment]` is at the initial state in the real parse.
    // `Some(i)`: the end of the probe's chunk `i` is (and so is the probe).
    let mut synced_at: Option<usize> = None;
    while segment < segments {
        let probe = &probes[segment];
        let (base, first) = match synced_at {
            None => (depth::Budget::ZERO, 0),
            Some(index) => (probe.chunks[index].used, index + 1),
        };
        // Everything the probe found after the synchronization point is
        // what the real parse finds there.
        if matches!(
            probe.verdict,
            depth::Verdict::TooDeep | depth::Verdict::Cancelled
        ) {
            verdict_result(probe.verdict)?;
        }
        let at_stop = probe.total.since(base);
        if probe.verdict == depth::Verdict::TooMuchWork && used.plus(at_stop).exceeds(cap) {
            // The segment's own allowance ran out, and the document's would
            // have run out by the same point of the same parse.
            return Err(SanitizeError::TooComplex);
        }
        let left = charge(&mut used, cap, probe.used.since(base))?;
        chunks.extend(probe.chunks[first..].iter().cloned());
        synced_at = None;
        if probe.ends_initial {
            segment += 1;
            continue;
        }
        // The segment ended inside open markup (or its own allowance ran
        // out): continue the real parse from its last chunk boundary, within
        // what is left of the document budget.
        let mut planner = depth::Planner::new(
            input,
            candidates,
            probe.tail,
            run.chunk_target,
            depth::MAX_NESTING_DEPTH,
            left,
        );
        verdict_result(planner.advance(bounds[segment + 1], &cancelled))?;
        loop {
            segment += 1;
            if segment == segments {
                verdict_result(planner.finish())?;
                charge(&mut used, cap, planner.used_at_last_cut())?;
                chunks.extend(planner.take_chunks());
                break;
            }
            // Look for a point where the real parse meets this probe.
            let next = &probes[segment];
            let mut met = planner.at_initial_state().then_some(None);
            if met.is_none() {
                for (index, chunk) in next.chunks.iter().enumerate() {
                    verdict_result(planner.advance(chunk.range.end, &cancelled))?;
                    if planner.at_initial_state() {
                        met = Some(Some(index));
                        break;
                    }
                }
            }
            if let Some(point) = met {
                #[cfg(test)]
                if point.is_some() {
                    tests::SYNCED_INSIDE_A_SEGMENT.fetch_add(1, Ordering::Relaxed);
                }
                planner.close_chunk();
                charge(&mut used, cap, planner.used_at_last_cut())?;
                chunks.extend(planner.take_chunks());
                synced_at = point;
                break;
            }
            verdict_result(planner.advance(bounds[segment + 1], &cancelled))?;
        }
    }
    debug_assert!(
        chunks.first().is_some_and(|first| first.range.start == 0)
            && chunks
                .windows(2)
                .all(|pair| pair[0].range.end == pair[1].range.start)
            && chunks
                .last()
                .is_some_and(|last| last.range.end == input.len())
    );
    Ok(chunks)
}

/// Add `more` to `used`; the budget left under `cap`, or `TooComplex`.
fn charge(
    used: &mut depth::Budget,
    cap: depth::Budget,
    more: depth::Budget,
) -> Result<depth::Budget, SanitizeError> {
    *used = used.plus(more);
    if used.exceeds(cap) {
        Err(SanitizeError::TooComplex)
    } else {
        Ok(cap.since(*used))
    }
}

/// Step 4 of [`sanitize`].
fn clean_chunks(
    input: &str,
    chunks: &[depth::Chunk],
    policy: ContentPolicy,
    run: Run<'_>,
    workers: usize,
) -> Result<String, SanitizeError> {
    let budget = OutputBudget::new(policy.limits().max_html_output_bytes);
    let failed = AtomicBool::new(false);
    let workers = workers.min(chunks.len()).max(1);
    // One builder per worker; building one costs microseconds.
    let builders: Vec<Builder<'static>> = (0..workers).map(|_| builder(policy)).collect();
    // Outputs are appended to one buffer in input order as soon as they are
    // next, so at most the out-of-order ones are held besides it.
    let capacity = (input.len() + input.len() / 8).min(policy.limits().max_html_output_bytes);
    let joined = Mutex::new(Joined {
        output: String::with_capacity(capacity),
        next: 0,
        pending: BTreeMap::new(),
    });
    let clean = |worker: usize, index: usize| -> Result<(), SanitizeError> {
        if failed.load(Ordering::Relaxed) {
            return Err(SanitizeError::Serialization);
        }
        if run.cancelled() {
            failed.store(true, Ordering::Relaxed);
            return Err(SanitizeError::Cancelled);
        }
        let result = clean_chunk(
            &builders[worker],
            &input[chunks[index].range.clone()],
            &budget,
        );
        match result {
            Ok(cleaned) => {
                let mut joined = joined.lock().map_err(|_| SanitizeError::Serialization)?;
                joined.add(index, cleaned);
                Ok(())
            }
            Err(error) => {
                failed.store(true, Ordering::Relaxed);
                Err(error)
            }
        }
    };
    // A chunk with a large tree is cleaned alone, so at most one large DOM
    // (up to `MAX_CHUNK_NODES`) is in memory at a time; the ordinary
    // chunks, a few hundred KiB each, are cleaned in parallel around them.
    let large = |index: usize| chunks[index].nodes > LARGE_CHUNK_NODES;
    let mut results = in_parallel(chunks.len(), workers, |worker, index| {
        if large(index) {
            Ok(())
        } else {
            clean(worker, index)
        }
    });
    for index in (0..chunks.len()).filter(|&index| large(index)) {
        results[index] = clean(0, index);
    }
    // The first real error; `Serialization` placeholders only stand for
    // chunks skipped after an earlier failure.
    let rank = |error: &SanitizeError| match error {
        SanitizeError::Cancelled => 0,
        SanitizeError::Serialization => 2,
        _ => 1,
    };
    if let Some(error) = results
        .into_iter()
        .filter_map(Result::err)
        .min_by_key(|error| rank(error))
    {
        return Err(error);
    }
    let joined = joined
        .into_inner()
        .map_err(|_| SanitizeError::Serialization)?;
    debug_assert!(joined.next == chunks.len() && joined.pending.is_empty());
    Ok(joined.output)
}

/// A chunk whose tree holds more nodes than this is cleaned on its own.
const LARGE_CHUNK_NODES: usize = 256 * 1024;

/// Chunk outputs joined in input order.
struct Joined {
    output: String,
    /// Index of the next chunk to append.
    next: usize,
    /// Finished chunks that are not next yet.
    pending: BTreeMap<usize, String>,
}

impl Joined {
    fn add(&mut self, index: usize, cleaned: String) {
        if index != self.next {
            self.pending.insert(index, cleaned);
            return;
        }
        self.output.push_str(&cleaned);
        drop(cleaned);
        self.next += 1;
        while let Some(cleaned) = self.pending.remove(&self.next) {
            self.output.push_str(&cleaned);
            self.next += 1;
        }
    }
}

/// Clean one chunk as a fragment and serialize it.
///
/// Chunking relies on ammonia's cleaning being local to each node. Its only
/// document-wide state, the `id` → tag map in `clean_dom`, is read only for
/// SVG `<animate>` and `<set>`, which this policy removes with their content
/// (`ids_only_matter_for_removed_elements` checks that), so which chunk an
/// `id` is in cannot change the output.
fn clean_chunk(
    builder: &Builder<'static>,
    chunk: &str,
    budget: &OutputBudget,
) -> Result<String, SanitizeError> {
    let document = builder.clean(chunk);
    let capacity = chunk.len() + chunk.len() / 8;
    serialize::serialize_children(&document.to_dom_node(), budget, capacity)
}

fn builder(policy: ContentPolicy) -> Builder<'static> {
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
            filter_attribute(policy, element, attribute, value)
        });
    builder
}

/// Run `job(worker, index)` for every index in `0..count` on up to `workers`
/// threads (the caller's thread is worker 0) and return the results in index
/// order.
fn in_parallel<T: Send>(
    count: usize,
    workers: usize,
    job: impl Fn(usize, usize) -> T + Sync,
) -> Vec<T> {
    let workers = workers.min(count);
    if workers <= 1 {
        return (0..count).map(|index| job(0, index)).collect();
    }
    let next = AtomicUsize::new(0);
    let work = |worker: usize| {
        let mut done = Vec::new();
        loop {
            let index = next.fetch_add(1, Ordering::Relaxed);
            if index >= count {
                return done;
            }
            done.push((index, job(worker, index)));
        }
    };
    let mut results: Vec<(usize, T)> = std::thread::scope(|scope| {
        let work = &work;
        let handles: Vec<_> = (1..workers)
            .map(|worker| scope.spawn(move || work(worker)))
            .collect();
        let mut all = work(0);
        for handle in handles {
            match handle.join() {
                Ok(done) => all.extend(done),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        all
    });
    results.sort_unstable_by_key(|&(index, _)| index);
    results.into_iter().map(|(_, result)| result).collect()
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
    // The .NET renderer's `BlockRemoteImages` turns a remote `<img src>`
    // into `src=""` after sanitizing; here the same decision is taken on the
    // parsed, entity-decoded value in one pass.
    if policy.remote_images() == RemoteImagePolicy::Blank
        && element == "img"
        && attribute == "src"
        && is_remote_reference(value)
    {
        return Some(Cow::Borrowed(""));
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

/// `(?:https?://|//)` at the start, ASCII case-insensitive, as the .NET
/// blanking pattern matches it.
fn is_remote_reference(value: &str) -> bool {
    ["http://", "https://", "//"].iter().any(|prefix| {
        value
            .get(..prefix.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    })
}

fn is_script_scheme(value: &str) -> bool {
    url::Url::parse(value)
        .map(|url| matches!(url.scheme(), "javascript" | "vbscript"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How often the real parse met a later probe at one of its chunk
    /// boundaries rather than at a segment start.
    pub(super) static SYNCED_INSIDE_A_SEGMENT: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn a_segment_inside_open_markup_is_rejoined_at_a_probe_chunk_boundary() {
        // Segment boundaries fall inside the `<div>`: those probes start
        // speculatively, and the real parse meets them after `</div>`.
        let input = format!(
            "{}<div>
{}</div>
{}",
            "<p>a</p>
"
            .repeat(40),
            "<p>b</p>
"
            .repeat(40),
            "<p>c</p>
"
            .repeat(40)
        );
        let policy = ContentPolicy::default();
        let whole = sanitize(
            &input,
            policy,
            Run {
                chunk_target: usize::MAX,
                segment_target: usize::MAX,
                ..Run::DEFAULT
            },
        );
        let before = SYNCED_INSIDE_A_SEGMENT.load(Ordering::Relaxed);
        for workers in [1, 4] {
            let run = Run {
                chunk_target: 20,
                segment_target: 200,
                max_workers: workers,
                cancel: None,
            };
            assert_eq!(sanitize(&input, policy, run), whole);
            let chunks = chunk_ranges(&input, run).unwrap();
            assert!(chunks.len() > 20, "{}", chunks.len());
        }
        assert!(SYNCED_INSIDE_A_SEGMENT.load(Ordering::Relaxed) > before);
    }

    #[test]
    fn ids_only_matter_for_removed_elements() {
        // ammonia consults its document-wide id map only for these; they
        // must stay removed for chunked output to equal whole output.
        for tag in ["animate", "set"] {
            assert!(REMOVED_WITH_CONTENT.contains(&tag) && !ALLOWED_TAGS.contains(&tag));
        }
    }

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
