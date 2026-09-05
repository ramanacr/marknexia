# ADR-0002 — WebView2 as Rich Rendering Surface

## Status
Accepted

## Decision
Use WebView2 as the rich HTML/SVG rendering surface, but keep parsing, URI resolution, sanitization, and content policy outside WebView2.

## Rationale

Markdown with tables, rich code blocks, diagrams, and math maps naturally to HTML/SVG/CSS.

## Security condition

WebView2 is a renderer, not the security boundary by itself. Content must be sanitized and navigation controlled before/at the boundary.
