# 15 — Dependency & Technology Selection Record

## Selection principles

Choose dependencies by these criteria:

1. GitHub/GFM compatibility.
2. Offline operation.
3. Active maintenance.
4. License compatibility.
5. Small attack surface.
6. Deterministic output.
7. Ability to run in Windows packaged environments.
8. Replaceability through a local abstraction.

## Markdown parser

Required capabilities:

- GFM.
- AST or equivalent structured representation.
- Extensibility for custom rendering.
- Link/image node interception.
- Source metadata where feasible.

The project must not couple the rest of the solution directly to the parser package's concrete AST types.

## Mermaid

Bundle a pinned local Mermaid runtime compatible with the selected release.

Requirements:

- Offline.
- No CDN.
- Security reviewed.
- Deterministic rendering for supported diagram types.

## Syntax highlighting

Select a local highlighting engine with broad language support.

## Math

Select a local math renderer such as KaTeX or equivalent, subject to license/security/runtime-size review.

## WebView

Use Microsoft WebView2 through the supported Windows/Windows App SDK integration strategy selected during implementation.

## Dependency inventory

The implementation repository must contain a generated dependency inventory such as:

```text
Component | Version | License | Direct/Transitive | Purpose | Offline | Security review date
```

## Upgrade policy

Do not bulk-upgrade parser, Mermaid, sanitizer, or WebView dependencies together with unrelated changes.

For security-sensitive dependencies:

1. Review release notes.
2. Run full security suite.
3. Run rendering snapshots.
4. Run navigation suite.
5. Run Store/package smoke tests.

## Licensing

Before public release, perform a license audit and make sure all redistributed web assets, fonts, icons, Mermaid assets, and parser/highlighter packages satisfy their licenses.
