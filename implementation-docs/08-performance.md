# 08 — Performance & Scalability

## Performance targets

Baseline target for v1:

- Typical README (<1 MB): interactive within 500 ms after file read on a modern development machine.
- Typical documentation file (1–10 MB): remain usable with incremental/deferred work where needed.
- Large files (10–50 MB): avoid blocking the WinUI UI thread during read/parse/render.
- Very large files (>50 MB): show a deliberate large-document path; do not attempt unlimited DOM inflation.

These are engineering targets, not contractual Store claims.

## UI-thread rule

WinUI UI thread must not perform:

- Large synchronous file reads.
- Full Markdown parsing for large documents.
- Syntax highlighting of huge blocks.
- Mermaid rendering of many diagrams.
- Repository-wide indexing.

## Rendering strategy

Use asynchronous stages:

```text
Read -> Parse -> Transform -> Sanitize -> Render -> Attach
```

Cancellation must flow through each stage.

## Large documents

Implement guards:

- Maximum source size threshold.
- Maximum rendered HTML threshold.
- Maximum diagram count/size.
- Maximum image dimensions/decoded size where feasible.

When a threshold is crossed:

- Inform the user.
- Defer or disable expensive features rather than crash.

Marknexia currently enforces a 50 MiB source-document limit before allocating
the file contents. Documents at or below that limit are read asynchronously and
the full parse/transform/sanitize pipeline runs on a worker thread with
cancellation checks. The renderer also caps final UTF-8 HTML at 128 MiB, limits
Mermaid to 64 blocks and 1 MiB per diagram source, and degrades disabled or
over-limit diagrams to an accessible source disclosure without loading the
Mermaid runtime. The shell reports an actionable warning when the source or
rendered document limit is exceeded. This is a deliberate large-document path
until section-level progressive rendering is introduced.

Repository tree construction uses a cancellable worker task as well. Startup
restoration and a newly selected workspace cancel any superseded scan and only
attach the resulting `TreeViewNode` hierarchy after the scan completes on the
UI thread.

The bundled Mermaid asset is extracted and mapped lazily only for documents
whose generated HTML references a Mermaid runtime; ordinary Markdown does not
pay that startup cost.

## Virtualization

Web content inside WebView2 is not equivalent to a native `ItemsRepeater`. Do not promise DOM virtualization unless implemented explicitly.

For very large documents, consider future architecture options:

- Section-level progressive rendering.
- Collapsible sections.
- Chunked DOM insertion.
- Source-level navigation shortcuts.

## Caching

Cache by content hash and renderer configuration version.

Do not reuse rendered output if the theme, parser, renderer, or security policy version changes in a way that affects output.

The shell now uses bounded source and rendered-document caches. Rendered keys
also include the canonical source path, repository asset authority, theme,
diagram/math settings, remote-image policy, and an explicit renderer
configuration version so one document cannot reuse another document's asset
origin or security policy.

## Startup

Optimize startup by:

- Lazy-loading optional diagram/math components.
- Keeping the initial shell lightweight.
- Avoiding repository scan until requested.
- Loading cached settings asynchronously where possible.
