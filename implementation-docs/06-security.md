# 06 — Security Model

## Threat model

Assume users may open arbitrary Markdown from:

- Downloaded files.
- Email attachments.
- Unknown Git repositories.
- Generated documentation.
- ZIP-extracted repositories.

Therefore Markdown must be treated as untrusted input.

## Core security principles

1. Rendering is not execution.
2. No arbitrary script execution from Markdown.
3. No arbitrary protocol execution.
4. No silent network fetching by default.
5. Repository path access stays within user-authorized roots.
6. Diagram content is treated as untrusted.
7. External links are separated from local document rendering.
8. Diagnostics never leak secrets into telemetry by default.

## WebView boundary

Host rendered content in a controlled local origin or equivalent isolated mechanism.

Do not expose sensitive .NET objects through a broad WebView2 JavaScript bridge.

If a JS bridge is required, expose a narrow command surface, e.g.:

```text
requestOpenLink(destination)
requestCopy(text)
requestContextMenu(action)
requestScroll(position)
```

Never expose filesystem APIs directly to page script.

## URL allowlist

Allowed local navigation types:

- Internal app-generated document references.
- Safe local file references under approved roots.

Allowed external types by default:

- `https`.
- `http` optionally.

Explicitly reject:

- `javascript:`.
- `vbscript:`.
- Unknown custom schemes unless explicitly supported.
- Dangerous `data:` payloads except narrowly controlled image/math cases if implemented.

## HTML sanitizer

Sanitize both:

1. Markdown-produced HTML.
2. Diagram-generated SVG/HTML.

Do not assume a third-party diagram library's output is automatically safe.

## Local file policy

Opening a Markdown file grants access only to resources necessary for rendering that document/workspace.

Do not recursively index or read all drives.

## Network policy

v1 renderer should be offline-first.

Remote image loading should be disabled by default or explicitly user-enabled later. This prevents a document from becoming a tracking surface through remote pixels.

## Privacy

No content telemetry by default.

Crash reports should avoid uploading raw Markdown contents or local paths unless explicitly redacted/hashed.

## Security test cases

Test:

- XSS via raw HTML.
- XSS via SVG.
- `javascript:` links.
- `data:` links.
- Path traversal.
- UNC/network share behavior.
- Reparse point escapes.
- Extremely deep nesting.
- Huge code blocks.
- Huge images.
- Malformed Mermaid.
- Malformed HTML.
- Zip bomb-like extracted workspaces are outside renderer scope but should not trigger recursive extraction.
