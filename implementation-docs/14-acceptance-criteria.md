# 14 — Acceptance Criteria

## Release-blocking behavior

### Markdown

- [ ] A `.md` file can be opened by double-click/file association.
- [x] Markdown renders without network access.
- [x] GFM-style tables render correctly.
- [x] Task lists render correctly.
- [x] Fenced code renders with syntax highlighting.
- [x] Footnotes work.
- [x] Alerts work.

### Navigation

- [x] `#section` scrolls to a heading in the current document.
- [x] Duplicate heading names receive deterministic distinct IDs.
- [x] Custom anchors can be targeted.
- [x] `docs/file.md` opens the correct local document.
- [x] `docs/file.md#section` opens the file and scrolls to the target.
- [x] `../README.md` resolves relative to current file.
- [x] `/docs/file.md` resolves relative to repository root when repository context exists.
- [x] Broken file targets do not crash the app.
- [x] Broken anchors do not crash the app.
- [x] Back/forward works for cross-file and fragment navigation.

### Assets

- [x] Relative images resolve correctly.
- [x] `../` asset paths work.
- [x] Repository-root asset paths work in repository mode.
- [x] Unsupported/missing images fail gracefully.

### Diagrams and math

- [x] Mermaid renders offline.
- [x] Mermaid syntax errors are isolated to the diagram block.
- [x] Math renders offline.
- [x] Diagram output does not permit arbitrary script execution.

### Security

- [x] `javascript:` links are blocked.
- [x] Inline script is blocked.
- [x] Event-handler attributes are blocked.
- [x] Local file traversal outside authorized roots is blocked in sandboxed repository mode.
- [x] Remote resources do not load silently by default.

### Windows

- [ ] MSIX installs cleanly.
- [ ] MSIX uninstalls cleanly.
- [ ] Markdown file associations work.
- [ ] App launches from Start menu.
- [ ] Release package passes WACK validation.
- [x] x64 package validated.
- [x] ARM64 package validated where supported by the build environment.

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
