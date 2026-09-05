# 03 — Rendering Engine

## Rendering contract

The Markdown layer should expose a renderer contract:

```csharp
public interface IMarkdownRenderer
{
    RenderedDocument Render(MarkdownSource source, RenderContext context);
}
```

Where `RenderedDocument` contains:

- Safe HTML fragment/document.
- Headings and generated anchor metadata.
- Link metadata.
- Asset references.
- Diagram blocks.
- Diagnostics.

The exact concrete parser package is an implementation choice. Keep it behind an adapter.

## GFM feature matrix

### Block constructs

- ATX headings.
- Setext headings where parser supports GFM behavior.
- Paragraphs.
- Blockquotes.
- Ordered/unordered lists.
- Task list items.
- Fenced code.
- Tables.
- Horizontal rules.
- Footnotes.
- Alerts.
- HTML blocks subject to sanitization policy.

### Inline constructs

- Emphasis.
- Strong emphasis.
- Strikethrough.
- Inline code.
- Links.
- Images.
- Autolinks.
- Escapes.
- HTML subject to sanitization.

## Heading IDs

Heading anchors must be generated deterministically.

Required behavior:

1. Normalize heading text.
2. Remove formatting artifacts.
3. Apply lowercase behavior.
4. Convert appropriate whitespace to hyphens.
5. Remove disallowed punctuation according to the compatibility algorithm.
6. De-duplicate IDs deterministically.
7. Preserve a lookup table from source heading to final ID.

Do not rely exclusively on browser-generated IDs.

## Links

Do not emit raw `href` values without classifying them first.

Create an internal link metadata model:

```text
OriginalDestination
NormalizedDestination
Classification
ResolvedTarget
Fragment
IsSafe
Diagnostic
```

## Images

Image sources must use the same URI resolver as links, but with an asset capability policy.

Images may resolve to:

- Relative local files.
- Repository-root local files.
- Data URIs only if explicitly allowed and size-limited.
- Remote URLs only if a user-enabled policy permits them.

Default v1 behavior should avoid silent remote fetches.

## Mermaid

Recognize fenced blocks using the `mermaid` info string.

Pipeline:

```text
Markdown fence
   ↓
Diagram block model
   ↓
Mermaid validator
   ↓
Local Mermaid runtime
   ↓
SVG/HTML result
   ↓
Sanitize SVG/HTML
   ↓
Render
```

Requirements:

- Local bundled Mermaid runtime.
- No CDN.
- Theme integration.
- Error boundary around each diagram.
- Diagram source remains copyable.
- Do not execute arbitrary JavaScript supplied by diagram content.

## Math

Support inline/display math through a local renderer.

Use an abstraction:

```csharp
public interface IMathRenderer
{
    MathRenderResult Render(string expression, MathMode mode);
}
```

## Code highlighting

Use a local highlighter. Highlighting should be advisory: if a grammar is unavailable, render plain code without failing the document.

## HTML handling

Treat Markdown-provided HTML as untrusted.

At minimum, block:

- `script`.
- Event-handler attributes such as `onclick`.
- Dangerous URI schemes.
- Active plugin/object/embed mechanisms where applicable.
- Unexpected navigation primitives.

Prefer allowlisting supported structural/formatting tags rather than trying to remove only known bad tags.

## Styling

Create a GitHub-inspired design-token layer:

```text
body typography
heading scale
link styles
code typography
table styles
blockquote styles
alert styles
border radius
spacing scale
background / foreground / muted tokens
```

Do not hardcode theme colors throughout generated HTML. Inject a versioned stylesheet.
