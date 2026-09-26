# Markdown parser source-position diagnostics

Status: **proposed — requires Task 8 approval**

Scope: `crates/marknexia-markdown`, both parser candidates (`candidate-comrak`, `candidate-pulldown`).

## Observable difference

The .NET `MarkdigParserAdapter.Parse` always returns an empty `Diagnostics` list. The Rust `ParsedDocument::diagnostics` list gets one `Warning` for each Mermaid block that breaks a renderer limit. The warning's `source_line` is the block's zero-based fence line:

| Condition (checked in the .NET renderer's order) | Rust message | .NET parser |
| --- | --- | --- |
| Diagram number `n` > `MarkdownOptions::max_diagram_count` (default 64, the .NET `MaxDiagramCount`) | `mermaid-<n>: diagram limit exceeded` | no diagnostic |
| Rendered code text (code lines with `\n`) > `max_diagram_source_bytes` UTF-8 bytes (default 1 MiB, the .NET `MaxDiagramSourceBytes`) | `mermaid-<n>: diagram source limit exceeded` | no diagnostic |

Nothing else changes:

* `renderedBodyHtml`, `diagrams`, `headings`, `customAnchors`, `links`, and `images` stay the same. A diagram over a limit is still extracted and still rendered as `<pre><code class="language-mermaid">`.
* `diagnostics` is `#[serde(skip)]`, so it never appears in `marknexia-parity-v1` output.
* The mandatory suite (`tests/parity_v1.rs`) asserts the list is empty for all 19 frozen Markdown and heading cases, which matches .NET.

## Rationale

The plan asks for source-position diagnostics. The warnings let the shell point at the offending line. They do not replace the renderer's limit enforcement: `marknexia-rendering` must still apply the .NET fallback (`RenderDiagramFallback`) on its own. It must also count Mermaid blocks the way the .NET renderer does, by matching rendered `<pre><code class="language-mermaid">` HTML. That count can include raw-HTML blocks the parser does not report.

## Approval needed

Task 8 must accept that parser diagnostics are an additive target-only field. If it is rejected, remove the two diagnostic cases. No fixture or HTML output changes either way.
