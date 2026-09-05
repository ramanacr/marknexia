# 01 — Product Requirements

## Product statement

Build a Windows-native Markdown reader that behaves like a local, offline GitHub-style Rich Text Viewer for `.md` files and repositories.

## Target users

- Developers reading README files and technical documentation.
- Engineers reviewing local repositories.
- Documentation authors previewing rendered Markdown.
- Users who want GitHub-like Markdown rendering without opening a browser.

## Platforms

- Windows 10 and Windows 11 where supported by the selected Windows App SDK baseline.
- Architectures: x64 and ARM64.
- Packaging: MSIX.
- Primary distribution: Microsoft Store.
- Secondary distribution can be added later without changing the document model.

## Core feature set

### Markdown rendering

- GFM-compatible headings, paragraphs, emphasis, strong, strikethrough, blockquotes, lists, tables, task lists, code blocks, inline code, links, images, footnotes, alerts, and compatible inline HTML.
- GitHub-style heading anchors and section links.
- Safe handling of unsupported or malformed Markdown.

### Diagrams

- Mermaid fenced code blocks rendered locally/offline.
- Mermaid support must not require a network connection.
- Diagram rendering errors must degrade to an understandable code/error presentation rather than crash the document.
- Architecture must allow future diagram providers.
- Optional future providers: Graphviz/DOT, PlantUML, GeoJSON, TopoJSON, ASCII STL.

### Math

- Inline and display math rendered locally.
- Use a local renderer such as KaTeX behind an abstraction.
- Math failures must not prevent the rest of the document from rendering.

### Code

- Local syntax highlighting.
- Copy button.
- Optional line numbers behind a feature flag if visual fidelity allows.
- Support a broad language set through an extensible grammar/highlighter abstraction.

### Navigation

The navigation subsystem is a release-critical feature.

Supported examples:

```md
[Same section](#caching)
[Another file](docs/api.md)
[File and section](docs/api.md#authentication)
[Parent document](../README.md#installation)
[Repository root path](/docs/architecture.md)
[Custom anchor](#custom-anchor)
```

Behavior:

- Same-document fragment → smooth/instant scroll to anchor according to user preference.
- Cross-document path → resolve and open the Markdown document in-app.
- Cross-document path + fragment → load target document, wait for rendering, then navigate to the fragment.
- External `http`/`https` link → open externally or in a controlled external-link flow.
- Relative image/resource path → resolve from the source Markdown file.
- Missing target → show a non-blocking broken-link state and preserve document context.
- Navigation history → back/forward across document and fragment transitions.

### Repository mode

- User can open a folder/repository.
- File tree shows Markdown and resource files.
- Root path is established once for repository mode.
- `/...` paths resolve relative to that root.
- `./` and `../` paths resolve relative to the current Markdown file.
- Symlink/junction handling must be security-reviewed and configurable.

### UX

- GitHub-inspired light and dark themes.
- System theme option.
- Headings outline/table of contents.
- Document search (`Ctrl+F`).
- Tabs.
- Drag and drop.
- Recent files.
- Windows file association for Markdown extensions.
- Copy anchor on heading.
- Status indicators for large/slow documents and failed diagrams.

## Functional requirements

| ID | Requirement | Priority |
|---|---|---|
| FR-001 | Open Markdown file directly | Must |
| FR-002 | Render GFM-style Markdown | Must |
| FR-003 | Resolve same-document anchors | Must |
| FR-004 | Resolve cross-file Markdown links | Must |
| FR-005 | Resolve cross-file fragment links | Must |
| FR-006 | Resolve relative local assets | Must |
| FR-007 | Support repository-root `/...` resolution | Must |
| FR-008 | Mermaid offline rendering | Must |
| FR-009 | Secure untrusted content handling | Must |
| FR-010 | Heading outline | Should |
| FR-011 | Search | Should |
| FR-012 | Tabs | Should |
| FR-013 | Print/PDF | Later |
| FR-014 | Editing | Later |

## Quality attributes

- Offline-first.
- Deterministic rendering for tests.
- No external network dependency for rendering.
- Fail-soft behavior.
- Accessibility-conscious semantic HTML where practical.
- Responsive UI under normal documents.
- Strong separation between file access, parsing, rendering, and presentation.
