# Math parity (MATH-1 to MATH-3)

Status: **approved — owner decision 2026-11-06 (G0)**

Scope: math in `crates/marknexia-markdown` (which `$` text becomes a math span) and `crates/marknexia-rendering` (`math.rs`, the `SimpleMathRenderer` port). REND-5 in `rendering-differences.md` keeps the math bounds; this file adds the oracle evidence and three more differences.

## Oracle evidence

`crates/marknexia-rendering/tests/oracle/math.v1.json` holds 43 cases rendered by the unmodified .NET `MarkdownRenderer` (Markdig 0.40.0 `UseMathematics`, `SimpleMathRenderer`, the HtmlSanitizer pass and the template). The generator is `crates/marknexia-rendering/tests/oracle/MathOracle`. It references `src/` and changes no .NET source. It ran against the `src/` tree `c91f107e…`, which is the tree of the frozen fixture revision `38b39f6`. `tests/math_oracle.rs` renders every case with Rust:

* 34 cases match byte for byte. One of them is also compared as a complete page. They cover inline math, `$$…$$` within a line, every symbol in the .NET table (including the `\ell` → `…` quirk), `\frac`, `\dfrac`, `\tfrac`, `\sqrt` with and without an index, `\text`, `\operatorname`, `\left`/`\right`, single-character commands, unknown commands, the `x^{2}` → `x<sup></sup>` quirk, white space before arguments, a script at the end, HTML-special and Latin-1 characters, empty groups, code spans and code blocks, escaped dollars, math in raw HTML blocks (including `<img onerror>` inside a math block and upper-case tags), unterminated raw spans, nesting within the budget, lists, headings and tables.
* 9 cases differ. Each one is listed in `KNOWN_DIFFERENCES` with its record. The exact Rust body of each is pinned in `tests/oracle/math.known-differences.v1.json`. The test fails if a listed case starts to match .NET or if its Rust body changes:

| Case | Record |
| --- | --- |
| `deep-nesting` (20 nested `\sqrt`) | REND-5: content past the element budget stays text |
| `caret-backslash-at-end`, `underscore-backslash-at-end` | REND-5: .NET throws `ArgumentOutOfRangeException` and the whole render fails; Rust clamps the argument to `\` |
| `display-block`, `multi-line-display`, `math-disabled` | MATH-1 |
| `unbalanced`, `leading-space` | MATH-2 |
| `astral` | MATH-3 |

### Regenerating the oracle

Run this from the repository root. It needs the .NET 10 SDK, and the `src/` tree must still be the one in the file (`git rev-parse HEAD:src` prints `c91f107edd5baa3bcba27723f19330cb35837273`):

```powershell
dotnet run --project crates/marknexia-rendering/tests/oracle/MathOracle -- `
  "$PWD\crates\marknexia-rendering\tests\oracle\math.v1.json" `
  38b39f668ce52026261ac4802602c0cf7e0711a3 (git rev-parse HEAD:src)
```

The three arguments are the output file, the fixture revision recorded as `sourceRevision`, and the `src/` tree recorded as `srcTree`. Then delete `MathOracle/bin` and `MathOracle/obj`. Review the diff of `math.v1.json` as a baseline change. If `src/` has changed, regenerate only with owner approval, because the oracle would then describe different .NET behavior. When a known difference changes on purpose, update its pinned body in `math.known-differences.v1.json` and its record here.

## MATH-1 — `$$` math blocks

Markdig's math block parser treats a line that is exactly `$$` (up to three spaces of indent, in any container) as a fence. The block runs to a closing line of two or more `$`, or to the end of its container. It renders `<div class="math">\n\[\n{lines}\n\]</div>`. The .NET math pass then sees the expression `\n\[…\]` with leading white space, so the delimiters are not stripped (REND-5 quirk) and the page shows a display `div` with literal `[` and `]` around the rendered content.

pulldown-cmark has no math block. It reads `$$\nE\n$$` as display math inside a paragraph, and Rust renders it as inline `\(…\)` math with the content trimmed.

| Input | .NET (Markdig HTML) | Rust (Markdown HTML) |
| --- | --- | --- |
| `$$\nE\n$$` | `<div class="math">\n\[\nE\n\]</div>` | `<p><span class="math">\(\nE\n\)</span></p>` |
| `text\n$$\nE\n$$` | the block interrupts the paragraph | one paragraph |
| `$$\n\nE\n\n$$` | one block with blank lines | three paragraphs `$$`, `E`, `$$` |
| `$$\nE` (no closing fence) | a block to the end of the document | the text `$$\nE` |
| `$$ E\n$$` | paragraph `$$ E`, then an empty, unterminated block | inline math ` E\n` |
| `> $$\n> E\n> $$`, `- $$\n  E\n  $$` | a block inside the quote or item | inline math in a paragraph |

With math turned off, .NET shows the block as a `div` fallback and Rust as a `span` fallback in a paragraph.

**Proposal.** Keep the pulldown-cmark reading until the owner decides. Reproducing MATH-1 exactly needs a Markdig-compatible block pass in `marknexia-markdown` that respects containers, and it would also reproduce the visible `[` `]` of the .NET page.

## MATH-2 — inline `$` span rules

Markdig and pulldown-cmark use different delimiter rules for inline math. A probe of 53 inputs through both parsers gives these differences (Markdig 0.40.0 with the .NET pipeline; pulldown-cmark through `PulldownAdapter`):

| Input | Markdig | pulldown-cmark |
| --- | --- | --- |
| `a$x$b`, `x$a$`, `$a$x`, `é$a$`, `$a$é`, `$x$5`, `$$a$$x` | text: an opening `$` needs white space or punctuation before it, a closing `$` needs white space or punctuation after it | math |
| `$ x $`, `$$ x $$` | math, content trimmed | `$ x $` is text; `$$ x $$` keeps the spaces |
| `$\frac{a$`, `${$`, `$}$` | math | text: pulldown-cmark requires balanced braces |
| `$x$$` | math `x$` | math `x`, then `$` |
| `$$x$`, `$$$a$$$` | text, math `$a$` | `$` then math `x`; `$` math `a` `$` |
| `$a\nb$`, `$$a\nb$$` | text: no line break inside | math |
| `$a$$b$`, `$x$y$` | text | two spans; one span then `y$` |

The renderer then sees different input, so the page differs. In the oracle, `unbalanced` and `leading-space` show this.

**Proposal.** Keep the pulldown-cmark rules for now. GitHub renders `a$x$b` as math too, so the Rust reading is not worse for README files. Matching Markdig needs a Marknexia-owned inline math scanner.

## MATH-3 — characters outside the Basic Multilingual Plane

`SimpleMathRenderer` walks UTF-16 code units. An astral character such as U+1D465 (𝑥) is two surrogates: `char.IsLetter` is false for each, `\𝑥` reads a one-surrogate command, and each lone surrogate is encoded separately. The page then shows two U+FFFD characters for each such letter. Rust walks Unicode scalar values and keeps the character (`astral` case: `𝑥<sup>2</sup> + \𝑥`).

**Proposal.** Keep the Rust behavior: it shows the author's character instead of replacement characters.

## Approval needed

Task 8 must approve or change MATH-1 to MATH-3. If MATH-1 or MATH-2 is rejected, the follow-up is a Markdig-compatible math scanner in `marknexia-markdown`, verified against `math.v1.json` and a larger probe set.
