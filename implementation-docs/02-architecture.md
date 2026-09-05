# 02 — Architecture

## Architectural principle

Do not implement the application as `Markdown -> HTML -> arbitrary WebView navigation`.

Use this pipeline:

```text
                    ┌─────────────────────┐
                    │   WinUI 3 Shell     │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │ Document Workspace  │
                    │ Tabs / History / UI │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │ Document Services   │
                    │ Open/Read/Cache     │
                    └──────────┬──────────┘
                               │
              ┌────────────────┴────────────────┐
              │                                 │
      ┌───────▼────────┐              ┌────────▼────────┐
      │ Markdown Parser │              │ URI Resolver    │
      │ GFM AST         │              │ File + Fragment │
      └───────┬────────┘              └────────┬────────┘
              │                                 │
      ┌───────▼─────────────────────────────────▼──────┐
      │              Rendering Pipeline                 │
      │ HTML + syntax highlighting + math + diagrams   │
      └─────────────────────┬───────────────────────────┘
                            │
                   ┌────────▼────────┐
                   │ Security Layer  │
                   │ URL/HTML/Assets │
                   └────────┬────────┘
                            │
                   ┌────────▼────────┐
                   │ WebView2 Host   │
                   │ local document  │
                   └─────────────────┘
```

## Proposed projects

```text
src/
  MarkdownForge.App/             WinUI shell and composition root
  MarkdownForge.Core/            Domain models and interfaces
  MarkdownForge.Markdown/        Parser + AST adapter + renderer
  MarkdownForge.Navigation/      URI resolution + anchors + history
  MarkdownForge.Rendering/       HTML document generation + WebView bridge
  MarkdownForge.Diagrams/        Mermaid and future diagram providers
  MarkdownForge.Syntax/          Code highlighting
  MarkdownForge.Files/           Filesystem/repository access
  MarkdownForge.Security/        Sanitization, capability policy
  MarkdownForge.Infrastructure/  Caching, logging, settings

tests/
  MarkdownForge.Core.Tests/
  MarkdownForge.Markdown.Tests/
  MarkdownForge.Navigation.Tests/
  MarkdownForge.Security.Tests/
  MarkdownForge.Rendering.Tests/
  MarkdownForge.App.UITests/
```

## Layering rules

### Core

Pure .NET. No WinUI, WebView2, filesystem, registry, or Windows-specific APIs.

### Markdown

Produces an intermediate representation or trusted render model. It must not decide whether a link is opened by Windows, the app, or a browser.

### Navigation

Owns URI classification and resolution. It returns a structured navigation intent.

Example:

```csharp
public sealed record NavigationIntent(
    NavigationKind Kind,
    DocumentUri? TargetDocument,
    string? Fragment,
    ExternalUri? ExternalUri);
```

### Files

Owns path canonicalization, repository-root context, file reads, encodings, and file existence checks.

### Rendering

Converts a render model into safe HTML and coordinates WebView2. It must not bypass the Security layer.

### App

Owns commands, tabs, window state, keyboard shortcuts, menus, and user-facing notifications.

## Dependency rules

- Core depends on nothing application-specific.
- Navigation depends on Core.
- Files depends on Core.
- Markdown depends on Core.
- Rendering depends on Markdown + Security + Diagram/Syntax abstractions.
- App depends on all orchestration layers.
- No lower-level project may reference `MarkdownForge.App`.

## State model

A document tab should track:

```text
DocumentId
SourcePath
RepositoryContext
ContentHash
RenderState
DirtyState (future editor use)
CurrentFragment
ScrollOffset
NavigationHistoryEntry
```

## Caching

Use separate caches:

1. Source cache — file bytes/text keyed by canonical file identity + last-write stamp.
2. Parse cache — AST/render model keyed by content hash + parser version.
3. Asset metadata cache — image dimensions/format where helpful.
4. Web render cache — avoid if it compromises determinism or security.

Avoid global unbounded caches.
