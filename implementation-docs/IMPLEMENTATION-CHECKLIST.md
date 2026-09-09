# Implementation Checklist

## Foundation
- [x] Solution created with layered projects
- [x] WinUI 3 packaged app created
- [x] x64 + ARM64 configurations
- [x] Central package management configured
- [x] CI build/test pipeline created

## Core rendering
- [x] GFM parser adapter
- [x] Render model
- [x] HTML sanitizer
- [x] GitHub-style stylesheet
- [x] Code highlighting
- [x] Mermaid provider
- [x] Math provider

## Navigation
- [x] URI classification
- [x] Windows path canonicalization
- [x] Repository root resolution
- [x] Fragment/anchor index
- [x] Cross-document navigation
- [x] History
- [x] Broken-link diagnostics

## Security
- [x] Unsafe protocols blocked
- [x] Script/event handlers blocked
- [x] SVG sanitized
- [x] Path traversal tests
- [x] Reparse-point tests
- [x] Remote loading policy

## UX
- [x] Native document tabs with close, selection, and drag reorder
- [x] Outline
- [x] Hierarchical repository explorer with persisted workspace root
- [x] Search
- [x] Theme switching
- [x] Keyboard shortcuts
- [x] File association
- [x] Branded About, self-help, diagnostics, SBOM, and update menu actions

## Store
- [x] Package manifest
- [x] App identity
- [x] Icons
- [x] SPDX 2.3 SBOM generation and packaged payload verification
- [x] x64 and ARM64 MSIX layout generation with architecture-aware structural verification
- [ ] WACK (Requires local Windows App Certification Kit execution)
- [x] Store listing (Assets & metadata prepared)
- [ ] Partner Center submission (Deployment step)
