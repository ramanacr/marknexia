# 13 — Coding Agent Instructions

## Role

You are implementing Markdown Forge as a production-grade Windows application. Treat the documents in `/docs` as the product/engineering contract.

## Non-negotiable rules

1. Preserve the layered architecture.
2. Never bypass `NavigationResolver` for Markdown links.
3. Never allow untrusted Markdown to execute arbitrary JavaScript.
4. Never introduce a runtime CDN dependency for core rendering.
5. Keep diagram/math dependencies local for offline operation.
6. Keep WinUI/UI concerns out of Core.
7. Add tests for every navigation/security behavior change.
8. Do not silently change GitHub-compatibility semantics.
9. Prefer capability/abstraction interfaces where future providers are expected.
10. Avoid speculative frameworks/packages when a simple local abstraction is enough.

## Implementation sequence

1. Read all docs in `/docs`.
2. Establish solution and project boundaries.
3. Implement Core domain contracts.
4. Implement Files + path canonicalization.
5. Implement Navigation resolver with comprehensive tests.
6. Integrate Markdown parser adapter.
7. Implement render model and sanitization.
8. Integrate WebView2.
9. Implement same-document and cross-document navigation.
10. Add Mermaid/math providers.
11. Add WinUI shell, tabs, outline, commands.
12. Add repository mode.
13. Add packaging and Store validation.

## Definition of done for a code change

A change is done only when:

- Code compiles.
- Relevant tests pass.
- New behavior has automated coverage.
- Security impact was reviewed.
- Docs/acceptance criteria are updated where necessary.
- No unnecessary public API was introduced.

## When compatibility behavior is unclear

Do not guess from memory.

Prefer this order:

1. Existing product specification.
2. Current GitHub documentation.
3. Current implementation fixtures.
4. Automated comparison tests.
5. Document the decision in an ADR.

## Failure handling

Prefer explicit, typed diagnostics over exceptions crossing UI boundaries.

Expected failures include:

- File missing.
- Unsupported encoding.
- Broken link.
- Missing anchor.
- Invalid Mermaid.
- Sanitized HTML.
- Permission denied.

These should result in actionable UX, not process termination.

## Performance

Never move large parsing/rendering work onto the UI thread merely to simplify code. Use cancellation and async pipelines.

## Security

Treat every Markdown file as hostile input. Review any code that introduces:

- WebView script.
- HTML sanitization changes.
- File path handling.
- External URL handling.
- Reparse-point traversal.
- Local resource loading.

## Git discipline

Before opening a PR:

- Remove dead code.
- Avoid unrelated formatting churn.
- Keep tests deterministic.
- Record significant architectural changes in `/docs`.
