# Markdown Forge — Product & Engineering Documentation

## Purpose

Markdown Forge is a production-grade Windows Markdown viewer whose primary compatibility target is GitHub-style rendering and navigation for local Markdown documents and repositories.

The product is **not** a browser wrapper around a Markdown-to-HTML library. It should have an explicit Markdown document model, URI/link resolver, secure rendering boundary, repository-aware resource resolution, and testable navigation behavior.

## Primary goals

1. Render GitHub Flavored Markdown (GFM)-style documents with high visual and behavioral fidelity.
2. Render diagrams such as Mermaid offline.
3. Resolve intra-document anchors, custom anchors, and cross-file Markdown links correctly.
4. Resolve local images and other resources relative to the current Markdown document.
5. Support repository-aware `/...` paths and `./` / `../` traversal.
6. Provide a native Windows desktop experience using WinUI 3 / Windows App SDK and packaged MSIX.
7. Remain secure when opening untrusted Markdown.
8. Keep the codebase extensible for future editing, repository, export, and commercialization capabilities.

## Non-goals for v1

- Full Git client.
- Full Markdown editor/IDE.
- Cloud synchronization.
- GitHub API integration as a dependency for local rendering.
- Server-side rendering requirements.

## Recommended stack

- .NET 10 / C# with nullable reference types enabled.
- WinUI 3 + Windows App SDK.
- Packaged application with MSIX for Store distribution.
- WebView2 as the controlled rendering surface for rich HTML/diagram output.
- A GFM-capable Markdown parser chosen through an abstraction so it can be replaced without changing the UI or navigation layers.
- A syntax-highlighting engine that runs locally.
- Mermaid bundled locally for offline operation.
- KaTeX or an equivalent local math renderer, behind an abstraction.
- xUnit/NUnit plus Playwright/WebView2-compatible UI verification strategy for rendering/navigation tests.

## Repository layout

```text
/docs
  README.md
  01-product-requirements.md
  02-architecture.md
  03-rendering-engine.md
  04-navigation-resolution.md
  05-file-system-repo-model.md
  06-security.md
  07-ux-spec.md
  08-performance.md
  09-testing.md
  10-msix-store.md
  11-dev-workflow.md
  12-roadmap.md
  13-agent-instructions.md
  14-acceptance-criteria.md
  15-dependencies.md
```

## Source references

GitHub documentation currently describes headings/section links, relative links, custom anchors, task lists, tables, footnotes, alerts, images, and related Markdown features. GitHub also documents Mermaid, GeoJSON, TopoJSON, and ASCII STL as diagram syntaxes in supported Markdown contexts.

- GitHub basic writing and formatting: https://docs.github.com/en/get-started/writing-on-github/getting-started-with-and-formatting-on-github/basic-writing-and-formatting-syntax
- GitHub diagram support: https://docs.github.com/en/get-started/writing-on-github/working-with-advanced-formatting/creating-diagrams
- Windows Store / MSIX packaging: https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/packaging/
- Microsoft Store MSIX submission: https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/msix/create-app-submission
