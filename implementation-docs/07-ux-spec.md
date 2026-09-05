# 07 — UX Specification

## Main window

Recommended layout:

```text
┌────────────────────────────────────────────────────────────┐
│ App title   Back Forward  Open  Folder  Search   Theme     │
├───────────────┬────────────────────────────────────────────┤
│ Outline /     │ Tabs                                       │
│ Repository    ├────────────────────────────────────────────┤
│ Explorer      │                                            │
│               │              Rendered Markdown              │
│               │                                            │
│               │                                            │
└───────────────┴────────────────────────────────────────────┘
```

The sidebar can be collapsed.

## Tabs

Each tab owns a document/navigation state. Closing a tab should not destroy its recent history until disposal.

## Document actions

- Open file.
- Open folder.
- Reload.
- Copy anchor.
- Copy code.
- Search.
- Back/forward.
- Open link.
- Reveal file in Explorer.

## Heading outline

Generate from heading metadata rather than scraping rendered DOM text.

Clicking an outline item should invoke the same navigation mechanism as a fragment link, so there is a single source of truth.

## Themes

Three modes:

- Light.
- Dark.
- System.

Use GitHub-inspired semantics, not an attempt to copy proprietary branding assets.

## Keyboard shortcuts

Minimum:

| Shortcut | Action |
|---|---|
| Ctrl+O | Open file |
| Ctrl+Shift+O | Open folder |
| Ctrl+F | Find |
| F5 / Ctrl+R | Reload |
| Alt+Left | Back |
| Alt+Right | Forward |
| Ctrl+W | Close tab |
| Ctrl+Tab | Next tab |
| Ctrl+Shift+Tab | Previous tab |
| Esc | Close transient UI |

Exact conflicts with Windows/WinUI should be resolved during implementation.

## Accessibility

- Keyboard navigation for all actions.
- Visible focus states.
- Semantic headings in rendered content.
- Images should preserve alt text.
- Diagrams should provide a text fallback/source disclosure.
- Color should not be the only meaning of an alert.
- UI automation identifiers for key controls.

## Error UX

Prefer inline, actionable diagnostics.

Example:

```text
Diagram could not be rendered.
Show Mermaid source  |  Copy source
```

Do not display raw exception stacks in normal UX.
