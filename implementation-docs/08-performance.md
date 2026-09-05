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

## Startup

Optimize startup by:

- Lazy-loading optional diagram/math components.
- Keeping the initial shell lightweight.
- Avoiding repository scan until requested.
- Loading cached settings asynchronously where possible.
