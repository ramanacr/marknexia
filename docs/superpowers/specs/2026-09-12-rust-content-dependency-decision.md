# Rust content dependency decision: Markdown parser

Status: **RECOMMENDATION pending review. Not approved.** Task 8 is the approval authority. This document recommends a parser; it does not select one. Both candidates stay in `crates/marknexia-markdown` behind `candidate-comrak` and `candidate-pulldown` until the recommendation is approved. The losing candidate must not be removed before then.

Scope: feasibility plan Task 4, Steps 1, 2, and 4, for the Markdown parser only. The sanitizer (`ammonia`, Step 3) and the highlighting approach are evaluated separately and are not decided here. Candidate HTML is raw, unsanitized, and content-unsafe.

## Recommendation

**pulldown-cmark 0.13.4**, if two conditions are met before Task 8 approval:

1. Add a Marknexia-owned bare-URL autolink extension. The .NET pipeline enables Markdig `UseAutoLinks`; pulldown-cmark has no GFM autolink extension.
2. Extend the frozen corpus through the Task 1 .NET exporter. The current corpus exercises none of the following, so today they are unverified against the oracle:
   * bare URLs
   * `++inserted++`
   * intraword `~sub~`/`^sup^`
   * escaped `\==a\==` and `\=\=b\=\=`
   * `+==+==+` (see [Hostile-input hardening](#hostile-input-hardening-review-of-d8f0767))
   * non-ASCII grid tables `+--+--+\n|é |😀|\n+==+==+\n|a |b |\n+--+--+` and `+----+\n|😀|\n+----+`
   * nesting deeper than 32 levels
   * the items listed under [Known gaps](#known-gaps-and-unverified-markdig-assumptions)

Reasons:

* Both candidates pass the mandatory suite with 0 mismatches, so parity does not decide it.
* pulldown-cmark is 2–3.5× faster through the Marknexia path on this machine.
* It adds 5 transitive crates instead of 12.
* Its licenses pass `deny.toml` unchanged. comrak's do not.
* It has had no breaking release in the observed window. comrak shipped six breaking 0.x releases in 2026.

comrak's advantage is breadth: native bare-URL autolinks, `++ins++`, and Markdig-compatible sub/superscript. Choosing comrak would also need a reviewed license-policy change, because `deny.toml` currently rejects BSD-2-Clause and Unicode-DFS-2016.

## Method

* **One mandatory suite, no candidate goldens.** `tests/parity_v1.rs` loads all 19 frozen `compat/fixtures/v1/{markdown,headings}` cases.
  * Before running anything, it checks that the manifest and the directory list the same cases.
  * It compares every `marknexia-parity-v1` field of each candidate's `ParsedDocument` with the .NET expected value.
* **How a difference can pass.** A difference passes only if a `compat/decisions/*.md` file with status "proposed — requires Task 8 approval" has a `parity-allowance` entry that records the exact candidate, case, field path, expected value, and actual value.
  * A changed difference fails.
  * An allowance with no matching difference (stale) fails.
  * An allowance in a file without proposed status fails.
* **Architecture.** Adapters (`comrak_adapter.rs`, `pulldown_adapter.rs`) only translate the library's syntax tree into a Marknexia-owned model (`src/extensions/model.rs`). Everything Markdig-specific lives in `src/extensions/`, outside the third-party adapters:
  * a Markdig 0.40-shaped HTML writer
  * heading IDs through `marknexia_core::slug`
  * GitHub alerts, reproducing the .NET regex post-transform byte for byte
  * `==mark==`, used only when the parser lacks it
  * the grid-table subset the fixtures use
  * footnote, task-list, and table markup
  * `language-*` code labels, with Markdig URL and HTML escaping
  * Mermaid extraction, with the .NET empty-line and `\r\n` handling
  * link, image, and anchor extraction, emulating the .NET anchor regex
  * source-position diagnostics

  Both candidates share this layer, so remaining differences are parse differences.
* **Line endings.** Input `\r\n`/`\r` become `\n` before parsing. This matches Markdig, which renders `\n`. Line numbers do not change.

## Evidence

### Fixture parity (mandatory, 19 cases)

| Candidate | Mismatched fields | Covered by proposed decisions | Result |
| --- | --- | --- | --- |
| comrak 0.55.0 | 0 | 0 | pass |
| pulldown-cmark 0.13.4 | 0 | 0 | pass |

For comparison, before this work the permissive bake-off reported 10 differing fields for comrak and 9 for pulldown. No parity allowance exists, so no library difference is being blessed.

Two decision files cover target-only behavior. Neither carries a parity allowance, because neither changes any frozen field:

* `compat/decisions/markdown-source-diagnostics.md`: an additive diagnostics field. It is not part of the parity schema, and the suite asserts it is empty for all 19 cases.
* `compat/decisions/markdown-resource-limits.md`: nesting-depth flattening at 32 levels and the rendered-body size limit.

The suite parses decision files strictly. It requires the exact line `Status: **proposed — requires Task 8 approval**`, and exactly one `Status:` line per file. Without a candidate feature, `cargo test -p marknexia-markdown` fails on purpose (`parity_gate_requires_a_candidate_feature`) rather than passing with no gate. CI must run `--all-features` so both candidates and their allowances are checked.

### Extension contract and divergence probe (no oracle)

`tests/compat_extensions.rs` runs the same checks on both candidates, and both pass. It covers:

* Mermaid info `Mermaid extra`, empty-line skipping, and the limit diagnostics
* angle autolinks: excluded from `links` and from heading text, as in .NET
* image alt text and email autolinks
* ordered start, loose and task lists, `==mark==` inside a quote, and grid-table widths
* Unicode and duplicate heading IDs
* setext heading lines

The ignored test `candidate_divergence_probe` renders 26 syntax probes outside the corpus. **4 of 26 diverge.** Three are pulldown feature gaps:

* bare-URL autolinks (comrak links them; pulldown leaves text)
* `++inserted++`
* intraword `H~2~O` / `x^2^`

The fourth is `+==+==+`. The shared mark pass applies CommonMark flanking rules and gives `+<mark>+</mark>+`; comrak's native highlight renders it literally. Markdig's result is unknown, so it is a fixture request and is not being fixed toward either candidate.

The other 22 render identically, including:

* block math and entities in headings
* HTML blocks and reference links
* pipe-table edges and multi-paragraph footnotes
* unclosed fences, hard breaks, emphasis edge cases, tabs, and setext headings
* escaped `\=` next to `==` (fixed in pulldown after review)
* both non-ASCII grid tables (fixed after review)

### Performance (release, `candidate_parse_render_timing`)

Setup:

* Small document: the `gfm-features` fixture, 931 bytes, 2000 iterations.
* Large document: all 19 frozen inputs repeated in memory to 1,048,950 bytes, 60 iterations.
* Times cover parse plus render through `MarkdownEngine::parse`.
* The machine was noisy, with Trend Micro active, so absolute numbers varied by up to 2× between runs. The ranking did not change.

| Path | Small median / p95 | Large median / p95 |
| --- | --- | --- |
| comrak (Marknexia path), run 1 | 156.9 / 251.6 µs | 140.9 / 163.0 ms |
| comrak (Marknexia path), run 2 | 165.9 / 316.1 µs | 205.6 / 283.1 ms |
| pulldown (Marknexia path), run 1 | 76.2 / 100.9 µs | 88.5 / 119.1 ms |
| pulldown (Marknexia path), run 2 | 42.7 / 78.5 µs | 60.3 / 72.5 ms |
| comrak (Marknexia path), run 3, after review fixes | 99.0 / 183.0 µs | 97.1 / 110.2 ms |
| pulldown (Marknexia path), run 3, after review fixes | 39.7 / 73.4 µs | 56.3 / 67.4 ms |
| comrak native `markdown_to_html` (baseline), run 1 | 70.2 / 112.1 µs | 86.3 / 106.4 ms |
| comrak native `markdown_to_html` (baseline), run 3 | 49.3 / 85.5 µs | 57.5 / 68.7 ms |
| pulldown native `push_html` (baseline), run 1 | 23.0 / 29.5 µs | 22.7 / 37.0 ms |
| pulldown native `push_html` (baseline), run 3 | 11.7 / 23.0 µs | 14.3 / 17.5 ms |

The shared Marknexia layer costs roughly 40–60 ms per MiB on this machine. Phase profiling showed where it goes:

* translating the event stream into the model
* the writer
* about 7 ms for the alert post-transform

The layer is the same for both candidates. Optimizing it, for example by borrowing text from the source instead of copying it, is possible later work and does not change the ranking.

### Hostile-input hardening (review of d8f0767)

`tests/hostile_inputs.rs` runs every case on both candidates.

| Finding | Before | Fix | After (release, 1 MiB input unless noted) |
| --- | --- | --- | --- |
| C1: recursive walks and `Drop` overflowed a 1 MiB stack (`STATUS_STACK_OVERFLOW`) | Crashed on `">"×2000`, nested `*a `, `==`, `![`, `~~` (10k–50k), and nested `- ` lists | Adapters cap nesting at 32 block and 32 inline levels and flatten deeper content with one warning per kind. The comrak translator flattens iteratively via `descendants()`. The pulldown builder suppresses frames through a start/end marker stack. The mark pass bounds its own nesting. See `markdown-resource-limits.md`. | 12 hostile inputs (20k-deep quotes, emphasis, strong, strikethrough, mark, images, links, `- ` and `> - ` lists, 400-level indented lists, a footnote holding deep quotes, ordered lists in quotes) pass on a 1 MiB thread in both debug and release. Caps of 128 still pass in debug; caps of 512 overflow. |
| C2: anchor regex emulation was cubic | 32 KiB 6.6 s, 64 KiB 55 s, 128 KiB 437 s (reviewer) | Stop once no `>` remains. Skip every start that shares a failed start's first `>`. Compare keys as bytes with no per-start allocation. Search past a `>` at most once per start. Regex semantics are unchanged, and unit tests cover them. | no `>`: 5.1–5.7 ms; one shared `>`: 11–14 ms; valid anchors: 25–34 ms |
| I1: alert transform rescanned to the end per candidate | 79 s (pulldown) / 66 s (comrak) | Stop when a required `</p>` or `</blockquote>` is absent. The output is identical: a unit test compares against the exhaustive scan. | unclosed: 6–10 ms; valid alerts (about 45× output amplification): 240–457 ms |
| I2: pulldown turned escaped `\=` into `<mark>` | `\==a\==` → `<mark>a</mark>` | An `=` that is backslash-escaped (odd run of preceding `\`) or comes from an entity becomes `Inline::Escaped`, which never forms a delimiter run | Matches comrak on both repros |
| I3: grid columns measured in UTF-8 bytes | `é`/`😀` grid rejected; `|😀|` in a 4-wide grid accepted | Columns are UTF-16 code units, as explained below | Both repros behave as .NET does; unit tests cover them |
| M1: unbounded output | none | `max_rendered_body_bytes`, default 128 MiB, the .NET `MaxRenderedHtmlBytes`. Rendering, back-links, and alerts stop at the limit. | The fixture shape `# bounded output` with a 1-byte limit is rejected. Alert and footnote amplification is rejected at 64 KiB. |

**Grid tables and UTF-16.** Markdig's grid-table parser slices .NET `string`/`StringSlice` values, whose positions are UTF-16 code-unit indices. Column boundaries and widths therefore count `é` as 1 unit and `😀` as 2. This is recalled from Markdig's source, not verified against its code in this environment. Separator lines are all ASCII, so their byte and unit offsets agree. Content lines are measured in units, and a column that lands inside a surrogate pair rejects the table. A non-ASCII grid fixture is requested.

**Alert splicing.** Parity requires copying .NET's lazy-regex alert transform, and that transform can produce broken nesting. With a nested quote, the alert `<div>` closes at the inner `</blockquote>` and leaves a stray outer `</blockquote>`; the test `nested_quote_splice_is_parity_required_broken_nesting` pins this. Downstream code must therefore sanitize with an HTML5-parser-based sanitizer, which repairs the tree. A regex- or string-based sanitizer must never be used on this output.

### Dependencies, licenses, advisories

| Candidate | Crates in `cargo tree -e normal` (baseline without a candidate: 9) | Added crates | Licenses added | `cargo deny check licenses advisories` |
| --- | --- | --- | --- | --- |
| comrak 0.55.0 | 21 | caseless, unicode-normalization, tinyvec, finl_unicode, jetscii, phf, phf_shared, siphasher, rustc-hash, smallvec, typed-arena, comrak | BSD-2-Clause (comrak), `(MIT OR Apache-2.0) AND Unicode-DFS-2016` (finl_unicode) | **licenses FAILED**: both licenses are outside the `deny.toml` allowlist `[Apache-2.0, MIT, Unicode-3.0]` |
| pulldown-cmark 0.13.4 | 14 | bitflags, memchr, pulldown-cmark-escape, unicase, pulldown-cmark | MIT, MIT OR Apache-2.0, Unlicense OR MIT | passes |

Notes:

* `deny.toml` checks the whole workspace with `all-features = true`, so the license failure is entirely comrak's.
* `advisories ok` for the whole graph.
* `deny.toml` was not changed.
* BSD-2-Clause is permissive and compatible with MIT distribution. Accepting it is a policy decision, not a legal blocker.

### Maintenance signals

These come from the local crates.io index cache and packaged metadata. They were not verified online.

| | comrak | pulldown-cmark |
| --- | --- | --- |
| Repository | github.com/kivikakk/comrak | github.com/raphlinus/pulldown-cmark (used by rustdoc and mdBook) |
| Versions published | 101 | 55 |
| Recent cadence | 0.50–0.55 between 2026-01-22 and 2026-09-06: six breaking 0.x releases in 2026 | 0.13.0–0.13.4 patch series |
| MSRV / edition | 1.85 / 2024 | 1.71.1 / 2021 |
| Model | Arena AST with source positions on every node | Pull event stream with byte offsets |

## Known gaps and unverified Markdig assumptions

These do not affect the frozen corpus but must be closed or covered by fixtures before cutover.

* **Bare-URL autolinks.** pulldown lacks them and they need an extension. comrak's GFM rules may also differ from Markdig `AutoLinks` details.
* **`++ins++` and intraword sub/superscript.** pulldown lacks them.
* **Markdig renderings reproduced from source knowledge, not fixtures.** Each needs a frozen .NET case before cutover:
  * URL escaping. Only `'` → `%27` is fixture-verified.
  * Footnotes:
    * reference numbering when there are several references
    * unreferenced footnotes, which are omitted
    * footnote bodies that do not end in a paragraph, where back-links go in a new `<p>`
    * case-insensitive, whitespace-collapsed footnote labels
  * `id=""` for blank headings.
  * Block math `$$…$$` as `<div class="math">`. Both candidates currently render it inline.
  * Heading text excluding HTML entities. Markdig `HtmlEntityInline` is not a literal. Both parsers merge entities into text.
  * Lists:
    * loose, tight, and nested lists; implicit-paragraph placement and `EnsureLine` newlines
    * the ordered-list `start` attribute
    * task-marker placement in loose items and in items whose first block is not a paragraph
  * Block quotes with several paragraphs, and alerts that contain nested quotes. The latter produce parity-required broken nesting, described above.
  * Hard breaks (`<br />\n`) and a lone `\r` as a line ending.
  * Links and images:
    * image alt text flattening (plain text, markup dropped)
    * link and image `title` attributes
    * angle and email autolinks excluded from `links` and from heading text
  * Indented code blocks (no language class).
  * Mermaid detection by the first word of the info string, case-insensitive.
  * Tables:
    * header-only tables and body rows with extra or missing cells
    * grid width formatting (`Math.Round` to 2 places, `0.##`)
  * Line numbers of setext headings and of blocks nested in containers.
  * `==mark==` details: flanking rules, runs longer than two `=`, and precedence against other delimiters.
  * Whitespace in the alert and anchor emulation. .NET `\s` and `Trim()` are approximated with Rust `char::is_whitespace`/`trim`. Both follow the Unicode White_Space set but may differ on rare code points.
* **Grid tables.** Only the subset the fixtures exercise is supported: top-level, uniform columns, optional `=` header. Alignment markers, row and column spans, and nested containers stay paragraphs.
* **Not measured:** release binary size delta and fuzzing. The allowed command set for this step had no binary build or `cargo fuzz`, so they remain for Step 6.

## Reproduce

```powershell
cargo test -p marknexia-markdown --all-features
cargo test -p marknexia-markdown --features candidate-comrak
cargo test -p marknexia-markdown --features candidate-pulldown
cargo test --release -p marknexia-markdown --all-features --test hostile_inputs -- --nocapture
cargo test --release -p marknexia-markdown --all-features -- --ignored candidate_parse_render_timing --nocapture
cargo test -p marknexia-markdown --all-features -- --ignored candidate_divergence_probe --nocapture
cargo tree -p marknexia-markdown --features candidate-pulldown -e normal
cargo deny check licenses advisories
```

## Rendering/Highlighting (Task 4, Steps 3–6)

Status: **RECOMMENDATION pending review. Not approved.** Scope: `crates/marknexia-rendering` and `fuzz/fuzz_targets/markdown_and_html.rs`. Behavior differences are listed in `compat/decisions/rendering-differences.md` (REND-1 to REND-7, proposed).

### What .NET does

`MarkdownRenderer` runs these steps in order, each as a regex pass over the unsanitized Markdig body:

1. `HighlightCodeBlocks`: every `<pre><code class="language-X">` becomes a `code-container` holding a `copy-btn` whose `data-copy-text` is `Uri.EscapeDataString` of the decoded code, followed by `ColorCodeSyntaxHighlighter.HighlightCode` (ColorCode.Core/HTML 2.0.15, `HtmlClassFormatter`).
2. The Mermaid shell, or a fallback when diagrams are disabled or a count or source-size limit is hit.
3. `SimpleMathRenderer`.
4. Ganss sanitization.
5. `BlockRemoteImages`.
6. `TemplateEngine`: CSP with a nonce, the embedded `github-markdown.css` and `bridge.js`, and a Mermaid `<script>` only when a shell was emitted.
7. The full-page `DocumentTooLargeException` check (128 MiB).

The 50 MiB source limit sits in `FileService`, before the read.

ColorCode emits `<div class="{language}"><pre>`, then the source with `<span class="{style}">` around scoped regex captures, then `</pre></div>`. Its unknown-language fallback is `<pre><code>…</code></pre>`. The only highlighted fixture is `code-copy-metadata` (`c#`), which needs exactly `<span class="keyword">var</span>` and `<span class="number">1</span>` inside `<div class="csharp"><pre>`.

### Decision: a small Marknexia-owned tokenizer (C# only)

| Option | Reproduces the fixture? | Cost |
| --- | --- | --- |
| No highlighter (plain `<pre><code>`) | **No**: `code-copy-metadata` needs the ColorCode `csharp` div and `keyword`/`number` spans | none |
| syntect 5.x (MIT) | **No**: its scopes and classes are Sublime/TextMate (`source cs`, `keyword.other`), not ColorCode's `keyword`/`number`. Matching would need a Marknexia-written class-mapping layer on top | regex engine (onig, a C library, or fancy-regex), plus MB-scale bundled syntax/theme dumps, plus the largest dependency set of the options |
| tree-sitter (MIT) + grammars | **No**: node kinds differ from ColorCode scopes, so a mapping layer would again be needed | C runtime and generated C parsers compiled with `cc`, one grammar crate per language |
| **Marknexia-owned C# tokenizer** | **Yes, byte-for-byte** | about 450 lines in `src/highlight.rs`; no new crate |

Evidence and design:

* The C# rules are the ten ColorCode.Core 2.0.15 `CSharp` rule regexes, in string-heap order: block comment, XML doc comment, line comment, char literal, verbatim string, string, attribute target, preprocessor directive, keyword list, number. They were read from the installed package's user-string heap (`~/.nuget/packages/colorcode.core/2.0.15`). The style class names (`keyword`, `number`, `string`, `stringCSharpVerbatim`, `comment`, `xmlDocTag`, `xmlDocComment`, `preprocessorKeyword`) come from the same heap.
* Each rule is a hand-written matcher with the same leftmost-first and backtracking outcome, including the backtracking quirk of unterminated verbatim strings. Memoized searches make it linear, whereas the .NET regex is quadratic on unterminated quotes, `/*`, `[type:"` and long blank runs (unit test `hostile_shapes_stay_linear`).
* **Review C1 fix:** the attribute rule's `]`/`"` stop search was not memoized. `[type:` repeated 64,000 times took 27.2 s (release) and would take hours at 4 MB. After the fix, n = 16,000 / 64,000 / 200,000 render in 19 / 60 / 159 ms (`csharp_attribute_prefix_floods_stay_linear`).
* Other ColorCode languages fall back to the .NET plain-text output (REND-2). Adding a grammar later means porting its rules the same way and adding a frozen oracle case.
* **Dependencies.** The only dependency added to `marknexia-rendering` is `markup5ever =0.40.0`, already in the graph through html5ever. It provides the HTML5 named-entity table for the `WebUtility.HtmlDecode` emulation. `Cargo.lock` gains no package. Licenses are unchanged.
* **Binary size.** Not measured, because a binary build is outside the allowed command set. No crate was added, so the delta is only the tokenizer's code plus the 34.8 KB of bundled CSS and JS.

### Renderer result

Frozen rendering fixtures: **7/7 exact, byte-for-byte including the complete templated page**, for both the pulldown and comrak candidates. There are 0 allowances. The gate is `crates/marknexia-rendering/tests/parity_v1.rs`, which uses the same strict and stale-allowance pattern as the Markdown and security gates.

The public result `RenderedDocument` holds:

* the body only as a `SanitizedFragment` (the shell builds `HostDocument::new` from `body()`);
* headings, the .NET anchor index, image references and diagrams as `PlainText`;
* diagnostics;
* `page_html()`, which rebuilds the .NET page from trusted template text and the sanitized body. `page_bytes()` is the exact value that the output limit checks.

With remote assets off, .NET sanitizes and then blanks remote images. Rust sanitizes under `AllowHttps`, blanks, and sanitizes again under `Deny`, so the output is always produced under the real policy. A body without `src` takes one pass (REND-4).

### Hostile inputs and timings (release, this machine)

These come from `tests/hostile_inputs.rs`. Every case must render or fail with a limit error within the time budget. A successful render must produce a sanitizer-fixed-point, script-free body.

| Case | Result | Time |
| --- | --- | --- |
| 20,000-deep quotes, emphasis, links, lists (1 MiB stack) | rendered | 5–32 ms |
| 20,000 raw `<div>` / 50,000 raw `<span>` | `Sanitizer(NestingTooDeep)` (SAN-8) | 84 / 114 ms |
| 50,000-deep `\sqrt{` math | rendered, element depth capped at 32 | 19 ms |
| `[type:` × 200,000 in a C# block (review C1) | rendered | 159 ms (was quadratic: 27.2 s at × 64,000) |
| `1 ` × 1.5 M in a C# block / `\frac{a}{b}` × 300,000 | rejected against the budget before building | 14 / 76 ms |
| 5,000 Mermaid blocks | 64 shells, 4,936 fallbacks, 4,936 diagnostics | 449 ms |
| 4 MiB paragraph / lines | rendered (4.23 MB page) | 87 / 462 ms |
| 4 MiB C# code, images, math, `[type:"` | `Sanitizer(InputTooLarge)` after amplification (REND-1) | 50–1,120 ms |
| 4 MiB `&amp;&#x41;&copy;` | `Sanitizer(TooComplex)` | 726 ms |
| 4 MiB `<b>x</b>` raw HTML | rendered | 4.9 s, of which 4.56 s is `sanitize_fragment` alone (security crate) |
| 2,000 mixed C# blocks | rendered, all tokens highlighted | 491 ms |

The output-limit edges are exact: at `page_bytes` the page renders, and at `page_bytes - 1` it is rejected with the measured size. The source limit is exact at 50 MiB and 50 MiB + 1.

### Fuzzing

`fuzz/fuzz_targets/markdown_and_html.rs` feeds arbitrary UTF-8 through `HtmlPolicy::sanitize_fragment` and through `Renderer::render`, with flag bits that vary remote images, diagrams, math, the directory, and small output and diagram limits. It asserts:

* no panic;
* body within 8 MiB and page within `max_rendered_html_bytes`;
* `page_html().len() == page_bytes()`;
* re-sanitizing is a fixed point.

The seed corpus (`fuzz/corpus/markdown_and_html`, 18 files) comes from the rendering and Markdown fixture inputs plus four hostile shapes.

**The libFuzzer run did not execute on this machine.** `cargo +nightly fuzz build markdown_and_html` succeeds. The ASan-instrumented binary needs `clang_rt.asan_dynamic-x86_64.dll`: without it, the process exits with `STATUS_DLL_NOT_FOUND`. With that DLL on `PATH`, the process blocks after `INFO: Loaded 1 PC tables (94834 PCs)`, before the first execution. That happens even with an empty corpus and with `-runs=0`, using 0.16 s of CPU in 10 minutes. `-s none` does not link on MSVC (unresolved `__start___sancov_cntrs`). The block is environmental and appears before any target code runs. AV behavior monitoring is suspected but not confirmed, and no exclusion was added. The 300 s run therefore has to happen off-machine (CI). The reported executions, crashes and coverage are **none measured**.

A deterministic stand-in, `crates/marknexia-rendering/tests/fuzz_invariants.rs`, checks the same invariants on 3,000 pseudo-random token soups on every `cargo test`. It adds one more check: with remote images off, no rendered `<img>` keeps an `http(s)://` or `//` source. It found two **sanitizer (`marknexia-security`) fixed-point exceptions**. Both targets now tolerate them explicitly, and a test fails once they are fixed:

1. A `<pre>` whose text begins with a line feed serializes as `<pre>` + LF. Parsing drops a line feed after `<pre>`, so each re-sanitization removes one: `<pre>\n\nx</pre>` → `<pre>\nx</pre>` → `<pre>x</pre>`. A C# code block whose first line is blank produces this through the renderer.
2. White-space-only input becomes `""` (`html::sanitize` early return). A body of `"\n"`, as from `</span>`, re-sanitizes to `""`.

Neither affects safety. Both are for the security crate's owner to decide.

### Reproduce

```powershell
cargo test -p marknexia-rendering --all-features
cargo test --release -p marknexia-rendering --all-features -- --nocapture --test-threads=1
cargo test --release -p marknexia-rendering --test hostile_inputs -- --ignored stage_timing --nocapture
cd fuzz; cargo +nightly fuzz run markdown_and_html -- -max_total_time=300
```

On Windows the ASan runtime `clang_rt.asan_dynamic-x86_64.dll` must be on `PATH`. It ships with MSVC under `VC/Tools/MSVC/<ver>/bin/Hostx64/x64`.
