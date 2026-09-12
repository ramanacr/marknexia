# Marknexia Rust/Win32 Rewrite — Architecture Design

**Status:** Approved  
**Date:** 2026-09-12  
**Repository:** `ramanacr/marknexia`

## 1. Decision

Marknexia will be redesigned as a lightweight native Windows application implemented in Rust over raw Win32 APIs. The target architecture removes the runtime and installer dependencies on .NET, WinUI 3, XAML, and Windows App SDK.

The application may use components shipped with or shared by Windows:

- Win32, COM, Windows Shell, DirectWrite, Direct2D, UI Automation, and related OS APIs.
- Microsoft Edge WebView2 Evergreen Runtime solely for the rendered-document viewport.

Marknexia will not ship or depend on Electron, Node.js, bundled Chromium, a managed-language runtime, or a general-purpose Rust GUI framework. Web technology remains confined to rendering sanitized Markdown inside WebView2. The application shell will be compiled Rust and Win32.

The rewrite preserves functional, security, packaging, and accessibility behaviour. Pixel-identical WinUI styling and animations are not required.

## 2. Motivation

The product objective is a small and powerful native Windows Markdown viewer. The existing implementation is Windows-native in presentation but carries .NET, WinUI 3, and Windows App SDK deployment costs. Its installer is approximately 100 MB.

The rewrite should deliver:

- A compact ahead-of-time compiled application.
- No application-local .NET or Windows App SDK runtime.
- Fast startup and predictable memory use.
- Memory-safe processing of untrusted documents and filesystem input.
- Native x64 and ARM64 releases.
- Standards-based HTML, CSS, Mermaid, and DOM rendering through shared WebView2.
- Clear boundaries between portable business logic and Windows adapters.

Installer size alone does not establish success. The replacement must also demonstrate feature parity, security parity, accessibility, reliable packaging, and measured performance.

## 3. Goals

1. Produce a native Marknexia executable written in Rust.
2. Implement the shell through Microsoft's `windows-rs` bindings and raw Win32.
3. Remove .NET, WinUI 3, XAML, and Windows App SDK from the runtime and installer.
4. Retain WebView2 Evergreen only for the document viewport.
5. Preserve agreed rendering, navigation, keyboard, security, activation, packaging, and update behaviour.
6. Support Windows 10 version 19041 or later and Windows 11.
7. Produce native x64 and ARM64 artifacts.
8. Meet the measurable release gates in this specification.
9. Confine and document unsafe Windows/COM interop.
10. Migrate side-by-side without destabilizing the existing application.

## 4. Non-goals

The first Rust release will not:

- Reproduce WinUI controls or animations pixel-for-pixel.
- Add Linux or macOS support.
- Replace HTML/CSS with a custom document layout engine.
- Bundle Chromium or fixed-version WebView2.
- Introduce Electron, Node.js, Tauri, WinUI, Windows App SDK, Qt, GTK, Flutter, .NET, JVM, or a general-purpose Rust GUI framework.
- Add editing, synchronization, cloud, or collaboration features.
- Preserve internal .NET APIs when observable behaviour can be implemented more cleanly.

## 5. Technology

### 5.1 Platform

- Stable Rust, MSVC targets, Rust 2024 edition subject to dependency validation.
- `windows-rs` for Win32, COM, Shell, Direct2D, DirectWrite, UI Automation, and required Windows Runtime APIs.
- `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`.

### 5.2 Document viewport

- WebView2 Evergreen via its native COM interface.
- No fixed WebView2 runtime in ordinary packages.
- Installer detection and Microsoft's Evergreen bootstrapper only when missing.
- Offline-bundled application HTML, CSS, JavaScript, Mermaid, themes, and icons.
- Remote images blocked by default.

### 5.3 Candidate Rust libraries

Candidates must pass compatibility, licensing, security, maintenance, and binary-size evaluation:

- Markdown: `comrak` versus `pulldown-cmark`.
- Sanitization: `ammonia` plus Marknexia URL, SVG, CSS, and attribute policy.
- Highlighting: `syntect` versus a narrowly configured Tree-sitter implementation.
- Serialization: `serde` and `serde_json`.

No dependency is selected only because it is idiomatic Rust. Existing behaviour and measurable product constraints govern selection.

## 6. Workspace and boundaries

```text
marknexia/
├── crates/
│   ├── marknexia-core/
│   ├── marknexia-files/
│   ├── marknexia-navigation/
│   ├── marknexia-markdown/
│   ├── marknexia-security/
│   ├── marknexia-rendering/
│   ├── marknexia-webview/
│   ├── marknexia-update/
│   └── marknexia-win32/
├── assets/
├── tests/
└── packaging/
```

These paths may coexist with the current solution during migration.

### `marknexia-core`

Owns platform-independent domain types: documents, headings, tabs, navigation targets, diagnostics, render requests/results, heading slugs, limits, generation IDs, and cancellation contracts. It must not reference Win32, COM, WebView2, or UI types.

### `marknexia-files`

Owns bounded reads, encoding recognition, Windows path canonicalization, repository-root discovery, repository scanning, recent-file identity, and testable filesystem abstractions. Containment checks use canonical paths rather than string prefixes.

### `marknexia-navigation`

Classifies and resolves anchors, relative files, repository-root links, cross-document links, and external URLs. It owns traversal protection and navigation history. It returns decisions but does not launch processes or manipulate the browser.

### `marknexia-markdown`

Parses GitHub-Flavoured Markdown, extracts headings, applies stable identifiers and supported extensions, isolates Mermaid blocks, and generates source-aware diagnostics.

### `marknexia-security`

Owns HTML/SVG allowlists, URI and origin policy, removal of scripts and event handlers, remote-resource policy, bridge-message validation, and applicable archive/update validation. Rust memory safety does not replace content sanitization.

### `marknexia-rendering`

Builds HTML using offline templates and bundled assets, applies syntax themes, enforces size/diagram limits, and supplies accessible fallbacks. It cannot bypass sanitization.

### `marknexia-webview`

Owns native WebView2 COM lifecycle, restricted resource mapping, origin enforcement, typed messaging, find/copy/link/diagram commands, navigation cancellation, and renderer recovery. Unsafe COM details remain private behind safe Rust interfaces.

### `marknexia-update`

Owns explicit update checks, architecture-specific asset selection, hashes/signatures, bounded traversal-safe staging, restart, health confirmation, rollback, and separation of portable versus Store update paths.

### `marknexia-win32`

Owns the entry point, message loop, window and layout, command bar, tabs, repository sidebar, find/status surfaces, theme/DPI handling, keyboard routing, dialogs, drag-and-drop, file activation, Shell integration, and UI Automation. Business and security rules must not live in window procedures.

## 7. Native UI

The shell uses Win32 controls where appropriate and owner-drawn Direct2D/DirectWrite surfaces only where the lightweight branded design requires them.

The principal layout includes:

- Native window chrome and standard window behaviour.
- Lightweight command/menu surface.
- Custom tab strip.
- Collapsible repository sidebar.
- WebView2 document viewport.
- Native find bar.
- Status and diagnostics surface.

Required UI qualities:

- Per-monitor DPI awareness.
- Light, dark, and high-contrast modes.
- Keyboard-only operation with visible focus and logical tab order.
- Microsoft UI Automation support.
- Reduced-motion preference.
- Native file/folder dialogs.
- Existing shortcuts and workflows.

## 8. Runtime flow

When opening a Markdown file:

1. Win32 receives picker, drop, command-line, recent-file, or association activation.
2. The files layer canonicalizes the path, determines repository scope, checks limits, and reads it.
3. A worker receives a generation ID and cancellation token.
4. Markdown parsing extracts headings and isolates diagrams.
5. The security layer sanitizes generated HTML, SVG, attributes, and URLs.
6. Rendering applies offline templates and output limits.
7. WebView2 displays the immutable sanitized payload.
8. Versioned typed messages request navigation, search, copy, or diagram actions.
9. The native host validates every message and invokes the responsible service.
10. Results are applied only when their generation ID still owns the target tab.

Parsing, scanning, highlighting, diagram transformation, and sanitization must not block the UI thread.

## 9. Security boundary

Local Markdown remains untrusted:

- Prohibit arbitrary document scripts and inline event handlers.
- Remove dangerous elements and unsafe URI/CSS constructs.
- Permit only approved schemes and controlled origins.
- Block network subresources by default.
- Broker local files rather than grant broad `file://` access.
- Version and strictly validate host messages.
- Validate origin, document identity, types, size, and path scope.
- Cancel navigation outside the controlled origin.
- Hand external URLs to Windows only after policy approval.
- Bound input, output, Mermaid count, diagram source, and archive extraction.
- Recover from a WebView renderer crash without terminating the host.

Unsafe Rust must be restricted to adapter crates. Every unsafe block requires a nearby invariant documenting ownership, lifetime, thread affinity, and callback assumptions.

## 10. Failure handling

Expected failures use typed results, not panics.

| Failure | Required result |
|---|---|
| Unsupported Markdown | Safe content or localized fallback |
| Mermaid failure | Accessible source fallback and diagnostic |
| Unsafe content/message | Remove, reject, or disable it |
| Oversized input/output | Reject before unbounded allocation and state the limit |
| Invalid encoding | Preserve the file and explain the problem |
| WebView2 missing | Keep shell usable and offer Evergreen installation |
| Renderer crash | Recreate controller and restore tab state |
| Cancelled scan | Discard partial results |
| Corrupt settings | Preserve corrupt file, load defaults, notify user |
| Update validation failure | Leave installed version unchanged |
| Failed update health check | Roll back |

## 11. Compatibility contract

The rewrite must preserve:

- Supported GitHub-Flavoured Markdown.
- Heading slug and anchor rules.
- Relative, repository-root, cross-document, and external-link behaviour.
- Repository traversal prevention.
- Existing sanitizer protection or a stricter compatible policy.
- Remote-image controls.
- Mermaid output and accessible fallbacks.
- Agreed syntax languages.
- Search, copy, links, and diagram bridge behaviour.
- Tabs and navigation history.
- Repository browsing.
- Settings and recent documents.
- Drag-and-drop, command-line, and association activation.
- Keyboard shortcuts and offline rendering.
- Update verification/rollback.
- SBOM production.
- x64 and ARM64 releases.

Observable differences require an explicit compatibility decision and fixture change; replacement-library differences are not silently accepted.

## 12. Verification

### Behavioural baseline

Capture language-neutral fixtures before replacing each component:

- Source Markdown and normalized HTML.
- Headings and identifiers.
- Navigation classifications/resolved paths.
- Sanitizer inputs/outputs.
- Bridge requests/responses.
- Diagnostics and settings migration.
- Archive/update validation.

Normalization removes only proven nondeterminism.

### Test suites

- Unit tests for all non-UI crates.
- Property tests for path containment, slugs, URIs, and messages.
- Fuzzing for Markdown, sanitizer, bridge payloads, paths, and archives.
- Golden comparisons against the current application.
- Adapted existing headless-browser tests.
- Native UI automation for shortcuts, focus, tabs, theme, contrast, and accessibility.
- Clean Windows 10/11 installation tests.
- Native x64 and ARM64 execution.
- Update tampering, interruption, health failure, and rollback.
- License, vulnerability, dependency-lock, and SBOM checks.

### Performance method

On declared reference hardware, record repeated median and tail measurements for installer/unpacked size, cold/warm start, shell readiness, first render, host versus WebView memory, large-document rendering, repository scanning, cancellation, and update staging. Every published result identifies hardware, OS, build, fixture, run count, and WebView residency.

## 13. Production release gates

| Metric | Required gate |
|---|---:|
| Installer, excluding an on-demand Microsoft WebView2 download | ≤15 MB |
| Unpacked Marknexia payload | ≤35 MB |
| Cold start to interactive shell on reference hardware | ≤500 ms |
| Idle host private working set before document open, excluding WebView2 | ≤35 MB |
| Agreed compatibility suite | 100% passing |
| Security regression suite | 100% passing |
| Accessibility-critical failures | Zero |
| Unresolved high/critical dependency vulnerabilities | Zero |
| Tampered updates accepted | Zero |
| Architectures | x64 and ARM64 |

If required syntax/Markdown assets make a size gate infeasible, measured evidence and explicit approval are required. Functionality may not be silently removed to satisfy size.

## 14. Packaging

Produce architecture-specific:

- Compact per-user setup executable.
- Portable ZIP.
- MSIX where Store or managed deployment is appropriate.
- Checksums and signatures under the release policy.
- SPDX or CycloneDX SBOM covering Rust crates, JavaScript/assets, native loaders, and artifacts.

The normal installer detects rather than bundles WebView2 Evergreen. Store-managed and portable self-update channels remain distinct. Dependency locking, build provenance, and source-to-artifact traceability are required.

## 15. Migration sequence

1. Freeze behavioural and security baselines.
2. Establish Rust workspace, CI matrix, policy checks, and parity harness.
3. Implement core, paths/files, navigation, and settings.
4. Select Markdown, sanitizer, and highlighting dependencies through measured spikes.
5. Implement rendering and secured WebView2 adapter.
6. Implement minimal Win32 shell, DPI, and theme support.
7. Add tabs, repository browsing, search, activation, drag-and-drop, and accessibility.
8. Port updater, packaging, SBOM, and release controls.
9. Exercise both editions against shared fixtures and clean machines.
10. Publish the Rust edition as a side-by-side preview.
11. Gather crash, compatibility, size, startup, and memory evidence.
12. Promote Rust only after every release gate passes.
13. Retire the .NET edition after a defined overlap and successful user migration.

Every phase leaves the current product releasable.

## 16. User settings migration

The Rust edition will read the current settings format where safe. Migration must:

- Preserve supported preferences, recent files, sidebar state, and remote-image choice.
- Validate paths and values.
- Back up existing settings.
- Write atomically.
- Permit rollback without corrupting old settings.
- Version the new schema.
- Define side-by-side preview, upgrade, uninstall, association ownership, and rollback before preview release.

## 17. Rationale

Rust/raw Win32 was selected because:

- C++/Win32 could be similarly small but increases memory-safety and maintenance risk for untrusted documents, paths, archives, and updates.
- Rust with WinUI would retain the unwanted framework and complicate UI interoperability.
- Tauri would make the shell web-driven, contrary to the native Win32 decision.
- Other GUI frameworks add dependencies and abstractions contrary to the goal.
- Replacing WebView2 with a custom layout engine would create disproportionate HTML/CSS, Mermaid, accessibility, and compatibility work.

The design concentrates platform complexity in narrow adapters while retaining independently testable safe Rust services.

## 18. Principal risks

| Risk | Mitigation |
|---|---|
| Raw Win32 UI engineering cost | Keep visuals lightweight; prefer native controls; isolate owner drawing |
| Parser differences | Golden parity suite and explicit decisions |
| Sanitizer regression | Hostile fixtures, fuzzing, strict allowlists, mandatory gate |
| COM lifecycle errors | Narrow adapter, safety invariants, integration/recovery tests |
| Binary growth | Measure every dependency/asset; bundle only supported data |
| Accessibility regression | Design UI Automation with each control |
| Rewrite stalls | Side-by-side phases and preview gates |
| WebView2 absent | Detection, usable shell, Evergreen bootstrap |
| Update damage | Signatures, hashes, bounded staging, health check, rollback |

## 19. Acceptance criteria

Implementation is complete when:

1. Marknexia runs without .NET, WinUI, Windows App SDK, Electron, Node.js, or bundled Chromium.
2. Its shell is Rust/Win32 and only the document viewport uses shared WebView2 Evergreen.
3. x64 and ARM64 packages pass every release gate.
4. Shared fixtures demonstrate approved behavioural parity.
5. Security and update suites show no regression.
6. Keyboard and UI Automation accessibility-critical workflows pass.
7. Size, startup, and memory targets are measured and satisfied.
8. Existing users can migrate settings and associations safely.
9. The preview overlap completes without release-blocking regressions.
10. Documentation, SBOM, licensing, provenance, and recovery procedures are complete.
