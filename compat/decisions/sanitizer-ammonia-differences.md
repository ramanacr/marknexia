# Sanitizer differences: ammonia-based Rust policy vs .NET Ganss oracle

Status: **proposed — requires Task 8 approval.** Nothing here changes a v1 expected result. The frozen fixtures under `compat/fixtures/v1/sanitizer/` stay as they are. Each difference below is asserted by name in `crates/marknexia-security/tests/parity_v1.rs` or `crates/marknexia-security/tests/hostile_properties.rs`, so none of them passes silently.

Oracle: `src/Marknexia.Security/HtmlSanitizerService.cs` (Ganss HtmlSanitizer 9.2.1039 on AngleSharp, with a regex pre-filter). Target: `crates/marknexia-security` (ammonia 4.2.4 / html5ever 0.40, with CSS decided on `cssparser` 0.38 tokens). ammonia was raised from 4.2.0 for RUSTSEC-2021-0157, RUSTSEC-2021-0158 and RUSTSEC-2026-0373. The policy never used ammonia's `filter_style_properties`, and it removes every SVG animation element, now including `animateColor`, with its content.

## SAN-1 — CSS shorthand expansion (`unsafe-css` fixture)

* **Input:** `<div style="background:url(javascript:alert(1))">text</div>`
* **.NET:** `<div style="background-position: initial; background-size: initial; background-repeat: initial; background-attachment: initial; background-origin: initial; background-clip: initial; background-color: initial">text</div>`
* **Rust:** `<div>text</div>`
* **Why it differs:** AngleSharp's CSSOM expands `background` into longhands before Ganss removes the `background-image` longhand that holds the `url()`. The leftover longhands, all set to `initial`, are an artifact of that CSSOM. To match them, Rust would need AngleSharp's per-property shorthand grammar. The Rust policy works on tokens. It drops any declaration that contains `url()` or another active construct, and it drops the `style` attribute when no declaration is left (as Ganss does for a style that ends up empty).
* **Observable effect:** The .NET output resets seven background longhands to `initial`, which does nothing on an element with no other background. Rendering is the same. The security outcome is the same: no URL is fetched and no script runs.

## SAN-2 — CSS value normalization

AngleSharp re-serializes values it understands, for example `color: red` becomes `color: rgba(255, 0, 0, 1)`, and it drops values that fail property grammar. Rust keeps any allowlisted property whose value contains only inert tokens (identifiers, numbers, dimensions, percentages, hashes, strings, commas, `! + - * / . %` delimiters, and allowlisted functions such as `rgb()`, `calc()` and gradients). It re-serializes the value token by token as `name: value` joined by `"; "`. The value must re-tokenize to itself; otherwise the whole style attribute is dropped. Rust therefore keeps grammatically invalid but inert values such as `color: foo`. The simple forms the rendering fixtures use (`display: none`) serialize the same way in both.

## SAN-3 — URL policy is stricter than Ganss

Rust sends every `href`, `src`, `action`, `cite` and `longdesc` value through the existing `UrlPolicy`. Links and images are decided independently.

| Value | .NET | Rust |
|---|---|---|
| `/absolute/path`, `//host/x` | kept | attribute removed |
| `https://host:8443/`, IP literals, trailing-dot hosts, userinfo | kept | attribute removed |
| `mailto:` with headers, `tel:` extensions | kept | attribute removed |
| value with leading or trailing whitespace (non-script scheme) | kept (trimmed scheme check) | attribute removed |
| `<img src="https://…">` under `RemoteImagePolicy::Deny` | kept (the renderer blanks it afterwards) | attribute removed |
| `<img src="http://…">`, `<img src="//host/…">` under `Deny` or `AllowHttps` | kept | attribute removed |
| `<img src>` starting with `http://`, `https://` or `//`, any case, under `RemoteImagePolicy::Blank` | kept, then blanked to `src=""` by the renderer's `BlockRemoteImages` (unless the value holds a quote character) | `src=""`, decided in the attribute filter on the entity-decoded value (REND-4) |
| empty value (`src=""`) | kept | kept |

`Blank` is the renderer's policy when remote assets are off. It blanks every remote image prefix the .NET pattern blanks, whatever the rest of the URL is. Every other remote image source, such as `<input type="image" src="https://…">` or a value with leading white space, is removed as under `Deny`.

## SAN-4 — Script schemes: rewrite instead of removal

The .NET regex pre-filter rewrites a *literal, quoted* `href|src="javascript:…"` or `vbscript:` value to `"#"`. Any other script-scheme spelling reaches Ganss, which removes the attribute. That covers entity-encoded (`&#106;avascript:`), unquoted, and tab/newline-obfuscated values, plus `xlink:href`. Rust makes the decision on the parsed, entity-decoded value using the WHATWG URL parser. Every value whose scheme is `javascript` or `vbscript` becomes `"#"`, including `xlink:href` (which Rust treats as `href`, because html5ever gives it the local name `href`). Either way the result is inert.

## SAN-5 — `<use>` references are fragment-only

Ganss keeps `<use href="https://…">`. Rust accepts only same-document `#fragment` references on `<use>`, validated with `UrlContext::SvgReference`, and removes every other value. `xlink:href` is kept only when it passes the same check. Ganss would remove `xlink:href` on `<use>` in all cases, because it is not in its allowed attributes.

## SAN-6 — Unknown elements keep their text

Ganss removes a disallowed element together with its children. Rust lists every known HTML, SVG and MathML element outside the allowlist in ammonia's `clean_content_tags`, so those elements are removed with their content, as in .NET. An **unknown or custom** element (for example `<x-widget>text<b>b</b></x-widget>`) is unwrapped: the element goes, and its allowlisted children and text stay. What survives is still sanitized by the same policy.

## SAN-7 — SVG output budget and inline-only SVG

.NET has no separate SVG output bound. Rust bounds `SvgPolicy::sanitize` input with `max_svg_input_bytes` and its output with `max_html_output_bytes`, while serializing.

Like .NET's `SanitizeSvg`, the result is HTML-serialized fragment markup, not a standalone `image/svg+xml` document. It has no `xmlns`, it may contain several roots or HTML siblings, and it follows HTML serialization rules. `SanitizedSvg` is therefore **inline-only**: it enters a document through `SanitizedSvg::into_fragment`. The WebView path that served it as a generated `image/svg+xml` asset (`GeneratedAsset::sanitized_svg`) has been removed. Only trusted bundled SVG assets remain.

SVG `fill` and `stroke` are tokenized as CSS. `url()` there may only name a same-document fragment (`url(#id)`); any other `url()` or non-allowlisted function removes the attribute. Ganss keeps these values verbatim.

## SAN-8 — Size limits, budgets, chunking and serializer

AngleSharp has no nesting or work limit and no chunking. The Rust sanitizer accepts fragments up to the .NET page limit and adds budgets that fail closed, in time linear in the input, with no partial output.

**Size.** `MAX_HTML_INPUT_BYTES` and `MAX_HTML_OUTPUT_BYTES` are both 128 MiB (`MarkdownRenderer.MaxRenderedHtmlBytes`), and they are the `PolicyLimits` defaults. A rendered Markdown body up to the .NET page limit is therefore sanitized. The output limit is charged while serializing, across all chunks of the document.

**Budgets.** html5ever's tree builder scans the open-element stack on many tags, so 20,000 nested `<div>`s (100 KiB) cost tens of seconds, and flat content under deep nesting is linear but slow. Before ammonia parses anything, a probe runs the same html5ever fragment parse into a sink that keeps only the tree shape. It keeps the exact tree shape and re-measures every subtree that the adoption agency or foster parenting moves. These are the constants, re-derived for the 128 MiB input:

| Budget | Constant | Error |
| --- | --- | --- |
| Tree depth | 256 (`MAX_NESTING_DEPTH`), unchanged | `NestingTooDeep` |
| Tree-construction work: each placement is charged its depth, plus re-measurement and child-scan steps | `8 × input bytes + 262,144` for the whole document (the base is added once, not per chunk or segment), unchanged | `TooComplex` |
| Adoption-agency reparent operations | `4,096 + input bytes / 256` for the whole document, was a flat 4,096 | `TooComplex` |
| Attributes on any start or end tag | 256 (`MAX_ATTRIBUTES_PER_TAG`), unchanged | `TooComplex` |
| Nodes in one chunk's tree (the unit of ammonia's DOM) | 4,194,304 (`MAX_CHUNK_NODES`), new | `TooComplex` |
| Nodes created in the whole document | `1,048,576 + input bytes / 3`, new | `TooComplex` |
| Serializer escape scans | **retired**: the quadratic serializer is gone (see below) | — |

* **Work.** Ordinary Markdown output uses well under 3 units per byte, and the shapes that exceed 8 are pathological: flat content under 250 open `<div>`s uses 28, misnesting under 250 open elements 67 to 197, and fostering into rcdom's front-to-back child scans grows without bound (5,350 units per byte at 1 MiB, 21,400 at 4 MiB). In the probe a unit costs a few nanoseconds, so at 50 MiB even the slowest rejection takes about 9 s.
* **Reparents.** Each reparent costs 4 to 12 µs of tree-builder work (8 outer adoption-agency iterations over a stack of up to 256 elements), which the work units do not charge. A flat cap made any document with more than 4,096 misnesting repairs fail, whatever its size. One reparent per 256 bytes bounds that uncharged cost to under 50 ns per input byte. Misnesting with no nesting (`<b><p>x</b></p>` repeated, one repair per 15 bytes) was measured at 0.27 s/MiB in the probe without a cap, so it is linear either way; it is still rejected, after the first 6% of a 50 MiB input.
* **Nodes (review I3).** ammonia's DOM plus the probe's tree cost about 200 bytes per node: 864 MiB were measured for one chunk of 4.19 M nodes (16 MiB of `<b>x</b>` in one paragraph). Without a limit, a 46 MiB `*a* ` document, one paragraph with a 126 MiB body, was projected at about 6.3 GiB for its single chunk. The chunk limit bounds one chunk's DOM to about 850 MiB. Chunks over 256 Ki nodes are cleaned one at a time, so at most one large DOM is in memory. The ordinary chunks, about 30,000 nodes each, are cleaned on the worker threads.
  * Calibration against the Markdown shapes of `crates/marknexia-rendering/tests/scale.rs` at 50 MiB: bodies create 0.037 (code) to 0.13 (raw HTML blocks, lists) nodes per byte, and their largest chunk holds 9,700 to 34,000 nodes. The largest legitimate single chunk measured is a 50 MiB document inside one raw `<div>`, with 3.18 M nodes (718 MiB), which passes.
  * Rejected: one paragraph or one list with more than 4 Mi nodes. That is 16 MiB or more of `*a* `, `- x` or `<b>x</b>`, since a single list or paragraph is a single top-level block.
  * The document limit allows three times the densest Markdown measured.
* **Attributes.** html5ever's duplicate-attribute check is quadratic per tag and invisible to the tree sink, so a linear pre-scan runs before any html5ever parse. The pre-scan follows the tokenizer's tag and quoting states. Where the real state depends on the tree builder (raw-text elements, comments, CDATA), it tracks every possible state at once, so it can over-count but never under-count. A tag of 256 attributes costs html5ever about 33,000 comparisons in about 2.3 KB, which stays linear (50 MiB of such tags: 2.9 s).

**Chunking.** Large fragments are cleaned in chunks, in parallel, with output byte-identical to cleaning the whole fragment:

1. The attribute pre-scan also reports split candidates: offsets right after `>` + LF where its set of possible tokenizer states is exactly the data state, and not followed by U+FEFF. The real tokenizer is then in the data state with no token in progress. As a hint for step 2, it marks the candidates that follow the end tag of a block element (`</p>`, `</table>`, `</ul>`, `</pre>`, `</div>`, …) or `<hr>` and are not followed by another end tag.
2. The input is cut into segments of about 1 MiB, preferably at those likely candidates. Each segment is probed from a fresh parser, in parallel. Inside a segment, the probe closes a chunk (about 256 KiB) at a candidate where `TreeBuilder::trace_handles` shows only the document, the `<html>` root and the context element. The stack of open elements is then just the root, no element is in the list of active formatting elements, and neither a form nor a head pointer is set, so the insertion mode is "in body" again and no later token can reach an earlier node. At a chunk boundary the probe forgets the finished nodes, as a fresh parse would, so its charges model the chunked parses and its memory stays proportional to one chunk.
3. Segment probes are confirmed in order. A probe is the real parse from any point where both it and the real parse are at the initial state: its start, when the previous segment ended there, or one of its own chunk boundaries. From such a point, both parse the same bytes from the same state. The work and reparents after that point are charged to the document budget. Where a segment ends inside open markup, the real parse continues sequentially from its last chunk boundary, with the budget left, until it meets a later probe at such a point (usually within one chunk). A probe that exceeded its own allowance continues the same way, unless the document budget would be exceeded by the same point of the same parse.
4. ammonia cleans every chunk as its own `<div>` fragment, on up to 8 threads (`std::thread::scope`, no new crate), and the outputs are joined in input order.

Parsing from the initial state rebuilds the same top-level nodes. The only difference is that text on both sides of a split is not merged into one text node, and that serializes to the same bytes. ammonia's only document-wide state, the `id` map, is used only for SVG `<animate>`/`<set>`, which this policy removes. Segmentation depends only on the input, so the output and the error are the same on any number of threads. The differential tests compare one-chunk and chunked results (chunk and segment targets of 1, 7, 16, 64, 512 and 4,096 bytes; 1 to 8 threads) and require them to be identical, errors included:

* `crates/marknexia-security/tests/chunking_differential.rs`: every fixture input and Markdown body, each also repeated 20 times; 4,000 generated documents whose raw HTML spans blocks (open `<div>`, `<b>`, `<table>`, `<form>`, `<textarea>`, `<select>`, `<svg>`, `<math>`, `<template>`, comments, CDATA, `<script>`, `<plaintext>`, `<frameset>`, `<body>`/`<html>` attributes and more), under the `Deny`, `AllowHttps` and `Blank` policies;
* `crates/marknexia-rendering/tests/chunking_differential.rs`: the renderer's real pre-sanitizer bodies (after the code, diagram and math passes) for every Markdown fixture, and 1,500 generated Markdown documents.

**Byte order marks (review C1, I2).** html5ever discards a U+FEFF at the start of its input (`discard_bom`, checked on every `feed`). Every chunk is its own ammonia input, so a chunk that started with U+FEFF lost it. The adversarial review found 488 of 30,000 random documents with a BOM token that differed, and `"<div>a</div>\n\u{FEFF}"` × 60,000 kept 59,997 of its BOMs. Two fixes:

* A split candidate is never followed by U+FEFF.
* The probe fed 16 KiB pieces and so dropped a BOM at every piece start. There ammonia sees a character that reopens the active formatting elements, so real depth 450 passed the 256 limit. The probe now parses with `discard_bom` off and skips only a leading U+FEFF itself, exactly as ammonia's single feed does.

Tests: `byte_order_marks_after_a_split_point_are_kept`, `byte_order_mark_at_a_probe_piece_boundary_still_counts_depth` (`NestingTooDeep`), and U+FEFF tokens in the generated differential corpus.

No class of input produced a difference after these fixes, so no whole-document fallback class is needed. A document whose raw HTML leaves an element open (an unclosed `<div>`, or a `<b>` whose paragraph closed without it) has no split point after it. Everything after it is then one chunk, probed and cleaned on one thread. Such documents are correct but not parallel: about 0.9 s per MiB of body, and the whole chunk's DOM is in memory at once, up to the chunk node limit.

**Entity-dense text (review minor 4).** Character references are decoded by html5ever's tokenizer twice (probe and ammonia), in linear time but slowly: 4 M `&amp;` take about 8.2 s. Inside one element (a paragraph or code block) there is no split point, so 128 MiB of such text would take about a minute on one thread. It is text, not nodes, so the node limits do not apply. It is recorded, not improved: a split inside an open element is never at the parser's initial state. Markdown bodies of entity-heavy prose split at every paragraph and are cleaned in parallel as usual.

**Output.** Chunk outputs are appended in input order to one buffer, as soon as each is next. The buffer reserves the expected output size once, never more than the 128 MiB limit. A single chunk's output becomes the buffer without a copy. Besides the buffer, the join holds only the outputs that finish out of order and the chunks being cleaned.

**ammonia's document-wide state.** ammonia's only document-wide state is `clean_dom`'s id-to-tag map. It is read only for the SVG animation elements (`animate`, `animateMotion`, `animateTransform`, `animateColor`, `set`), which this policy removes with their content (`ids_only_matter_for_removed_elements`), so the chunk an `id` is in cannot change the output.

**Serializer.** ammonia exposes its cleaned DOM only under `cfg(ammonia_unstable)`. The workspace `.cargo/config.toml` now sets that flag, and `marknexia-security` fails to compile without it. The crate serializes the DOM itself (`src/serialize.rs`):

* It writes exactly what html5ever 0.40.1's `HtmlSerializer` writes, checked byte for byte against html5ever on a fixed set and on 2,000 generated fragments.
* **Escaping is linear.** html5ever's `write_escaped` rescans the rest of a text node or attribute value up to the next `<`/`>` (plus `"` in attributes) for every `&` or 0xC2 byte, so 2 MiB of `x&y ` took about 26 s. The escape-cost budget that used to reject such input before serializing is removed. These inputs are now sanitized: 50 MiB of `x&y ` in 20 s, almost all of it in html5ever's character-reference tokenizer, and 10× the input takes 7.6× the time.
* It restores the leading line feed of `<pre>`, `<listing>` and `<textarea>` (SAN-11).

**Measured at 50 MiB** (release, this machine, `tests/scale_budgets.rs`; each input is also measured at 5 MiB, and 10× the input must take at most 16× the time plus 0.5 s):

| Input | Result | 5 MiB | 50 MiB |
| --- | --- | --- | --- |
| nested `<div>` | `NestingTooDeep` | 0.12 s | 0.59 s |
| flat content under 250 `<div>` | `TooComplex` | 0.57 s | 4.9 s |
| same, one block per line (every segment speculative, then re-probed) | `TooComplex` | 0.89 s | 9.0 s |
| `<b><i><div></b>` | `NestingTooDeep` | 0.09 s | 0.74 s |
| `<b><p>x</b></p>` | `TooComplex` (reparents) | 0.27 s | 1.8 s |
| `<a><table></a>` | `TooComplex` | 0.08 s | 0.80 s |
| fostered text / fostered rows | `TooComplex` | 0.10 / 0.13 s | 0.83 / 0.94 s |
| one tag with millions of attributes | `TooComplex` (pre-scan) | < 1 ms | < 1 ms |
| 256 attributes on every tag | accepted | 0.37 s | 2.9 s |
| `x&y ` (formerly quadratic escaping) | accepted | 2.6 s | 20 s |
| unclosed `<b>`, then blocks (no split point) | accepted at 5 MiB; `TooComplex` (chunk node limit) at 50 MiB | 2.6–4.5 s | 4.5–9.4 s |
| one paragraph of `<b>x</b>` (13 M nodes, one chunk) | `TooComplex` (chunk node limit) | accepted | 5.6 s |

Browsers cap parser nesting at 512.

## SAN-9 — Attribute value escaping

AngleSharp's formatter escapes only `&`, U+00A0 and `"` in attribute values. html5ever also escapes `<` and `>`, so `alt="<b>"` is emitted as `alt="&lt;b&gt;"`. Both parse back to the same value. The Marknexia serializer (SAN-8) keeps html5ever's escaping exactly.

## SAN-10 — White-space-only input (review M1)

`HtmlSanitizerService.SanitizeHtml` and `SanitizeSvg` return `""` for any input for which `string.IsNullOrWhiteSpace` holds. Ganss itself has no such rule: `HtmlSanitizer.Sanitize` parses white space like any other text and returns it. Rust follows Ganss. Only empty input returns `""` without parsing. Any other input, white space included, goes through the parser, so sanitizing stays a fixed point.

| Input | .NET service (probed, `SanitizeHtml`) | Rust `HtmlPolicy` / `SvgPolicy` |
| --- | --- | --- |
| `""` | `""` | `""` |
| `" "`, `" \n\t "` | `""` | unchanged |
| U+00A0 | `""` | `&nbsp;` |
| U+3000 | `""` | U+3000 |
| `<x-widget> </x-widget>` | `""` (SAN-6 removes the element with its content) | `" "` |

Trimming only ASCII white space would still map `" "` to `""`, so sanitizing `<x-widget> </x-widget>` twice would not be a fixed point. The renderer keeps the .NET service rule for the Markdown body: a body that is only white space (`char.IsWhiteSpace`, the same set as Unicode `White_Space`) becomes `""` before sanitizing. The rendered page therefore matches .NET (`crates/marknexia-rendering/src/lib.rs`, `sanitize_body`). Test: `only_empty_input_short_circuits`.

## SAN-11 — Leading line feed in `<pre>` and `<textarea>` (review I2)

The HTML parser drops one line feed right after a `<pre>`, `<listing>` or `<textarea>` start tag. html5ever's serializer does not write it back, so `<pre>\n\nx</pre>` used to sanitize to `<pre>\nx</pre>` and then to `<pre>x</pre>`. The Marknexia serializer (SAN-8) writes one extra line feed after the start tag when the element's first child is a text node that starts with a line feed. This is the rule of AngleSharp's `HtmlMarkupFormatter` and of browsers' fragment serialization. The decision is taken on the cleaned DOM, never by a string pass over the output. Sanitizing is now a fixed point, and `crates/marknexia-rendering/tests/fuzz_invariants.rs` asserts that without tolerance.

AngleSharp was probed through the .NET service. It gives the same output as Rust for every case except the one SAN-6 already covers:

| Input | .NET (AngleSharp) | Rust |
| --- | --- | --- |
| `<pre>\n\nx</pre>` | `<pre>\n\nx</pre>` | same |
| `<pre>\nx</pre>` | `<pre>x</pre>` | same |
| `<pre>\n\n\nx</pre>` | `<pre>\n\n\nx</pre>` | same |
| `<textarea>\n\nx</textarea>` | `<textarea>\n\nx</textarea>` | same |
| `<pre><!--c-->\nabc</pre>` | `<pre>\n\nabc</pre>` | same |
| `<pre><x-foo>\nabc</x-foo></pre>` | `<pre></pre>` (SAN-6: unknown element removed with its content) | `<pre>\n\nabc</pre>` (element unwrapped, and its text is now the first child) |

All of these are fixed points in both editions.

**A U+FEFF that starts the output.** The parser discards U+FEFF only as the first character of its input. Characters that it drops, such as NUL, can come before a U+FEFF. In that case the text node keeps the U+FEFF, and the output starts with it. The next parse then discarded it as a byte order mark, so sanitizing was not a fixed point; CI fuzzing found this (run 42115856878). The Marknexia serializer now writes a U+FEFF that would be the first character of the whole output as `&#xFEFF;`, which parses back to the same text. Everywhere else it writes the character itself, as html5ever does. Test: `a_byte_order_mark_that_starts_the_output_stays_a_fixed_point`.

## Note — attacker-controlled attributes kept for parity

These are not differences. Markdown source controls these values, and they survive sanitization in both editions:

* `id` and `name` are kept, so DOM clobbering of named globals and `document` properties is possible. Page scripts must not look up globals by id or name.
* `data-marknexia-action`, `data-copy-text` and `data-marknexia-zoom-status` are kept verbatim. They are inert metadata, but their values are **untrusted**. The delegated bridge must validate `data-marknexia-action` against its fixed command set, and treat `data-copy-text` only as clipboard text (bounded, never evaluated or navigated to).
* `target` and `rel` are kept, so `target="_blank" rel="opener"` survives, as it does with Ganss. The host's navigation interception has to enforce link handling; the markup does not.
