# 09 — Testing Strategy

## Test pyramid

```text
              UI / End-to-end
             /---------------\
            /                 \
      Rendering / WebView integration
          /---------------------\
         /                       \
  Navigation / security / parser unit tests
  /------------------------------------------\
```

## Parser/render tests

Golden/snapshot tests should cover representative GitHub-style Markdown.

Required corpus categories:

- Headings.
- Duplicate headings.
- Tables.
- Task lists.
- Footnotes.
- Alerts.
- Nested lists.
- Blockquotes.
- Fenced code.
- Inline HTML.
- Images.
- Links.
- Mermaid.
- Math.

Do not blindly snapshot volatile browser-generated attributes.

## Navigation tests

Parameterize these cases:

```text
Current: C:epo\README.md

#one
./docs/api.md
../README.md
./docs/api.md#auth
/docs/api.md
/docs/api.md#auth
https://example.com
javascript:alert(1)
```

Assert:

- Classification.
- Canonical path.
- Fragment.
- Security decision.
- Final navigation intent.

## Anchor tests

Test:

- Heading IDs.
- Duplicate heading IDs.
- Custom `<a name>` anchors.
- `id` anchors if supported.
- Fragment-only links.
- Cross-document fragment links.
- Missing anchors.
- Special Unicode heading text.

## Security tests

Security tests are mandatory in CI.

The sanitizer test suite should attempt bypasses rather than only test happy paths.

## UI tests

Use UI automation for:

1. Open Markdown file.
2. Render document.
3. Click same-document anchor.
4. Click cross-file Markdown link.
5. Click cross-file + fragment link.
6. Navigate Back.
7. Navigate Forward.
8. Click external URL.
9. Open Mermaid document offline.
10. Switch theme and verify rendering survives.

## Regression fixtures

Create a `test-fixtures/markdown/` tree mirroring realistic documentation:

```text
test-fixtures/
  markdown/
    navigation/
      README.md
      docs/
        api.md
        nested/
          details.md
    diagrams/
      mermaid.md
    security/
      hostile.html.md
```

## Golden compatibility suite

Maintain a known corpus of GitHub examples or independently authored equivalents. Do not redistribute copyrighted GitHub page snapshots unless licensing permits.

## Store validation

Before release:

- Build Release packages.
- Run Windows App Certification Kit.
- Install/uninstall cleanly.
- Upgrade from previous version.
- Test x64 and ARM64 where supported.
- Test Store install path separately from local developer install.
