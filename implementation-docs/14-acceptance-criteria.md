# 14 — Acceptance Criteria

## Release-blocking behavior

### Markdown

- [ ] A `.md` file can be opened by double-click/file association.
- [ ] Markdown renders without network access.
- [ ] GFM-style tables render correctly.
- [ ] Task lists render correctly.
- [ ] Fenced code renders with syntax highlighting.
- [ ] Footnotes work.
- [ ] Alerts work.

### Navigation

- [ ] `#section` scrolls to a heading in the current document.
- [ ] Duplicate heading names receive deterministic distinct IDs.
- [ ] Custom anchors can be targeted.
- [ ] `docs/file.md` opens the correct local document.
- [ ] `docs/file.md#section` opens the file and scrolls to the target.
- [ ] `../README.md` resolves relative to current file.
- [ ] `/docs/file.md` resolves relative to repository root when repository context exists.
- [ ] Broken file targets do not crash the app.
- [ ] Broken anchors do not crash the app.
- [ ] Back/forward works for cross-file and fragment navigation.

### Assets

- [ ] Relative images resolve correctly.
- [ ] `../` asset paths work.
- [ ] Repository-root asset paths work in repository mode.
- [ ] Unsupported/missing images fail gracefully.

### Diagrams and math

- [ ] Mermaid renders offline.
- [ ] Mermaid syntax errors are isolated to the diagram block.
- [ ] Math renders offline.
- [ ] Diagram output does not permit arbitrary script execution.

### Security

- [ ] `javascript:` links are blocked.
- [ ] Inline script is blocked.
- [ ] Event-handler attributes are blocked.
- [ ] Local file traversal outside authorized roots is blocked in sandboxed repository mode.
- [ ] Remote resources do not load silently by default.

### Windows

- [ ] MSIX installs cleanly.
- [ ] MSIX uninstalls cleanly.
- [ ] Markdown file associations work.
- [ ] App launches from Start menu.
- [ ] Release package passes WACK validation.
- [ ] x64 package validated.
- [ ] ARM64 package validated where supported by the build environment.

## Visual acceptance

Create a fixture corpus and compare:

- Typography hierarchy.
- Table rendering.
- Code rendering.
- Blockquotes.
- Alerts.
- Task list controls.
- Link behavior.
- Image layout.
- Mermaid presentation.
- Dark/light themes.

Exact pixel equivalence with GitHub is not the contract; semantic and behavioral compatibility plus strong visual similarity are the goal.

## Regression requirement

Every bug involving navigation must become a permanent fixture/test case.
