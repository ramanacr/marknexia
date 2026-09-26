# Markdown parser nesting and output limits

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-markdown`, both parser candidates. These limits come from the review of `d8f0767`. Unbounded nesting overflowed a 1 MiB stack (`STATUS_STACK_OVERFLOW`, which cannot be caught). Unbounded output let alerts amplify input about 50×.

## Observable differences

No frozen fixture reaches these limits. The mandatory parity suite still reports 0 mismatches for both candidates.

### 1. Nesting depth

Markdig 0.40 has no comparable small limit. The Rust adapters keep at most 32 nested block containers (block quotes, lists, list items, footnote definitions), `MAX_BLOCK_DEPTH`, and at most 32 nested inline containers (emphasis, strong, strikethrough, sub/superscript, inserted, links, images), `MAX_INLINE_DEPTH`.

What happens beyond the limit:

| Input beyond the limit | .NET (Markdig) | Rust |
| --- | --- | --- |
| Deeper block container | Nested markup to any depth | The container's paragraphs, headings, code blocks, HTML blocks, rules, and tables move into the deepest kept container, in source order. Its own `<blockquote>`, `<ul>`/`<ol>`, and `<li>` wrappers are omitted. |
| Deeper inline span | Nested markup | The span's text is kept, but not its tag. For a link that means no `<a>` and no entry in `links`; for an image, its alt text is kept as plain text. |
| `==mark==` nesting (pulldown's Marknexia mark pass) | Nested `<mark>` | Delimiters past the remaining inline depth render as literal `==`. |

The first flattened block and the first flattened inline each add one `Warning` diagnostic, `block|inline nesting deeper than 32 levels was flattened`, with the zero-based source line. The warnings are not serialized, just like `markdown-source-diagnostics.md`.

The stack budget is measured by `tests/hostile_inputs.rs` on a 1 MiB thread:

* The unoptimized test build overflowed with caps of 512.
* It passed with caps of 128.
* 32 leaves at least a 4× margin for debug builds and more for release builds.

### 2. Rendered body size

`MarkdownOptions::max_rendered_body_bytes` defaults to 128 MiB, the .NET `MarkdownRenderer.MaxRenderedHtmlBytes`.

* Rendering, footnote back-links, and the alert post-transform stop as soon as the body exceeds the limit.
* `parse` then returns `Err` with one `Error` diagnostic: `rendered HTML exceeds its <limit> byte limit`.

.NET applies the limit to the complete templated document and throws `DocumentTooLargeException` (see `compat/fixtures/v1/rendering/rendered-output-limit.case.json`). The rendering layer must still apply the .NET full-document check. The parser limit only makes sure the body is never built past the budget.

## Approval needed

Task 8 must approve three things, or change them:

1. The two depth values.
2. The flattening behavior.
3. Keeping a parser-level output limit alongside the renderer's full-document limit.

The .NET exporter should add frozen cases for nesting deeper than 32 levels, so the difference is recorded against the oracle.
