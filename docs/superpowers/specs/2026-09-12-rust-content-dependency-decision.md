# Rust content dependency decision: Markdown parser

Status: **RECOMMENDATION pending review. Not approved.** Task 8 is the approval authority. This document recommends a parser; it does not select one. Both candidates stay in `crates/marknexia-markdown` behind `candidate-comrak` and `candidate-pulldown` until the recommendation is approved. The losing candidate must not be removed before then.

Scope: feasibility plan Task 4, Steps 1, 2, and 4, for the Markdown parser only. The sanitizer (`ammonia`, Step 3) and the highlighting approach are evaluated separately and are not decided here. Candidate HTML is raw, unsanitized, and content-unsafe.

## Recommendation

**pulldown-cmark 0.13.4**, if two conditions are met before Task 8 approval:

1. Add a Marknexia-owned bare-URL autolink extension. The .NET pipeline enables Markdig `UseAutoLinks`; pulldown-cmark has no GFM autolink extension.
2. Extend the frozen corpus through the Task 1 .NET exporter with cases for bare URLs, `++inserted++`, and intraword `~sub~`/`^sup^`. The current corpus exercises none of these, so today they are unverified against the oracle.

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

The one decision file, `compat/decisions/markdown-source-diagnostics.md`, covers an additive target-only field. That field is not part of the parity schema, and the suite asserts it is empty for all 19 cases.

### Extension contract and divergence probe (no oracle)

`tests/compat_extensions.rs` runs the same checks on both candidates, and both pass. It covers:

* Mermaid info `Mermaid extra`, empty-line skipping, and the limit diagnostics
* angle autolinks: excluded from `links` and from heading text, as in .NET
* image alt text and email autolinks
* ordered start, loose and task lists, `==mark==` inside a quote, and grid-table widths
* Unicode and duplicate heading IDs
* setext heading lines

The ignored test `candidate_divergence_probe` renders 21 syntax probes outside the corpus. **3 of 21 diverge, and all are pulldown feature gaps:**

* bare-URL autolinks (comrak links them; pulldown leaves text)
* `++inserted++`
* intraword `H~2~O` / `x^2^`

The other 18 render identically, including block math, entities in headings, HTML blocks, reference links, pipe-table edges, multi-paragraph footnotes, unclosed fences, hard breaks, emphasis edge cases, tabs, and setext headings.

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
| comrak native `markdown_to_html` (baseline), run 1 | 70.2 / 112.1 µs | 86.3 / 106.4 ms |
| pulldown native `push_html` (baseline), run 1 | 23.0 / 29.5 µs | 22.7 / 37.0 ms |

The shared Marknexia layer costs roughly 40–60 ms per MiB on this machine. Phase profiling showed where it goes:

* translating the event stream into the model
* the writer
* about 7 ms for the alert post-transform

The layer is the same for both candidates. Optimizing it, for example by borrowing text from the source instead of copying it, is possible later work and does not change the ranking.

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
* **Markdig renderings reproduced from source knowledge, not fixtures:**
  * URL escaping. Only `'` → `%27` is fixture-verified.
  * Footnote reference numbering when there are several references.
  * Unreferenced footnotes, which are omitted.
  * `id=""` for blank headings.
  * Block math `$$…$$` as `<div class="math">`. Both candidates currently render it inline.
  * Heading text excluding HTML entities. Markdig `HtmlEntityInline` is not a literal. Both parsers merge entities into text.
* **Grid tables.** Only the subset the fixtures exercise is supported: top-level, uniform columns, optional `=` header. Alignment markers, row and column spans, and nested containers stay paragraphs.
* **Not measured:** release binary size delta and fuzzing. The allowed command set for this step had no binary build or `cargo fuzz`, so they remain for Step 6.

## Reproduce

```powershell
cargo test -p marknexia-markdown --all-features
cargo test -p marknexia-markdown --features candidate-comrak
cargo test -p marknexia-markdown --features candidate-pulldown
cargo test --release -p marknexia-markdown --all-features -- --ignored candidate_parse_render_timing --nocapture
cargo test -p marknexia-markdown --all-features -- --ignored candidate_divergence_probe --nocapture
cargo tree -p marknexia-markdown --features candidate-pulldown -e normal
cargo deny check licenses advisories
```
