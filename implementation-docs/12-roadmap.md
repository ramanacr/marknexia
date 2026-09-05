# 12 — Product Roadmap

## Phase 0 — Foundation

- Solution/project structure.
- WinUI 3 shell.
- MSIX packaging.
- Core models and interfaces.
- Logging/diagnostics.
- Dependency policy.

## Phase 1 — GitHub-style reader MVP

- Open `.md` files.
- GFM rendering.
- GitHub-inspired theme.
- Code highlighting.
- Tables/task lists/footnotes/alerts.
- Same-document anchors.
- Heading outline.
- Search.

## Phase 2 — Navigation engine

- Cross-file Markdown links.
- Cross-file + fragment links.
- Repository-root links.
- Asset resolution.
- Navigation history.
- Tabs.
- Repository/folder mode.

## Phase 3 — Rich rendering

- Mermaid.
- Math.
- Diagram error UX.
- Copy source/copy anchor.
- Large document improvements.

## Phase 4 — Production hardening

- Security fuzzing.
- Performance optimization.
- WACK/Store readiness.
- Accessibility audit.
- Crash diagnostics.
- Documentation.

## Phase 5 — Commercialization foundations

Potential features, not required for v1:

- PDF export.
- Print.
- HTML export.
- Custom themes.
- Advanced workspace/bookmarks.
- Saved reading positions.
- Pro features.
- Enterprise deployment controls.
- Optional cloud sync.

## Phase 6 — Product expansion

Potential competitive directions:

- Markdown editor with live preview.
- Git-aware documentation browser.
- API documentation mode.
- Workspace search.
- Documentation graph/dependency map.
- Broken-link scanner.
- Markdown linting.
- Diagram source inspector.
- Accessibility report.

## Strategic constraint

Do not add features that force the core document/navigation model to become UI-specific. Keep the domain and resolution abstractions stable.
