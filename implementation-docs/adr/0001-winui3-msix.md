# ADR-0001 — WinUI 3 + Windows App SDK + MSIX

## Status
Accepted

## Decision
Use a packaged WinUI 3 / Windows App SDK desktop application distributed primarily via Microsoft Store using MSIX.

## Rationale

- Native Windows UX.
- Strong Windows integration.
- First-class MSIX/package identity.
- Store-managed distribution/update path.
- File associations through package manifest.

## Consequences

- Packaging is part of the product architecture.
- Store validation must be part of release CI.
- Windows-specific APIs stay in infrastructure/application layers.
