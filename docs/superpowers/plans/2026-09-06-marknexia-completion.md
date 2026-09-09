# Marknexia Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the Marknexia Windows Markdown viewer as a functional, branded, offline-first product rather than a superficial shell.

**Architecture:** Keep the existing pure .NET document, parsing, navigation, security, and rendering libraries as the source of truth. Concentrate changes in the WinUI presentation layer and small, proven renderer/security seams, with explicit workspace and per-tab state so UI behavior is deterministic and testable.

**Tech Stack:** .NET 10, C# 13, WinUI 3 / Windows App SDK, WebView2, Markdig, Ganss.Xss, xUnit, Node/Playwright browser regressions, supplied Marknexia SVG/PNG assets.

**Spec:** `implementation-docs/01-product-requirements.md`, `implementation-docs/07-ux-spec.md`, `implementation-docs/14-acceptance-criteria.md`, `branding-docs/brand/BRAND-GUIDELINES.md`.

## Global Constraints

- Preserve offline rendering: no runtime CDN or network dependency for Markdown, Mermaid, math, or CSS.
- Preserve repository sandboxing and sanitizer protections for untrusted Markdown.
- Use supplied Marknexia logo assets without recoloring, stretching, or GitHub trade-dress mimicry.
- Use `Segoe UI Variable` for UI and `Cascadia Code`/`Consolas` for technical text.
- Preserve the existing public domain-library APIs unless a failing requirement proves an API change necessary.
- Every navigation regression becomes a permanent test fixture or unit test.
- Verify with `dotnet test Marknexia.slnx`, an x64 app build, and live UI inspection.

### Task 1: Establish state boundaries and navigation correctness

**Files:**
- Create: `src/Marknexia.App/WorkspaceState.cs`
- Create: `src/Marknexia.App/DocumentTabState.cs`
- Modify: `src/Marknexia.App/MainWindow.xaml.cs`
- Test: `tests/Marknexia.Navigation.Tests/NavigationHistoryManagerTests.cs`

**Interfaces:**
- `WorkspaceState` owns `RepositoryRoot`, repository entries, and active tab identity.
- `DocumentTabState` owns `FilePath`, `RenderedDocument`, `WebView`, current fragment, and tab-local history/search metadata.
- `MainWindow` consumes these state objects and no longer uses one global history manager for all tabs.

- [x] Add tests covering per-tab back/forward isolation, fragment entries, and forward-history pruning.
- [x] Run the focused navigation tests and confirm the new cases fail against the current global-history behavior.
- [x] Extract the state classes and update `MainWindow` to create, select, reload, and dispose tabs through them.
- [x] Route same-document, cross-document, root-relative, broken, and blocked links through the tab-local state.
- [x] Run the focused tests, then the full solution tests.

### Task 2: Build the Marknexia visual system and branded shell

**Files:**
- Create: `src/Marknexia.App/Styles/MarknexiaTheme.xaml`
- Create: `src/Marknexia.App/Styles/Controls.xaml`
- Modify: `src/Marknexia.App/App.xaml`
- Modify: `src/Marknexia.App/MainWindow.xaml`
- Modify: `src/Marknexia.App/Package.appxmanifest`
- Use: `branding-docs/logo/marknexia-lockup.svg`, `branding-docs/logo/marknexia-symbol.svg`, supplied icon assets

**Interfaces:**
- Resource keys define light/dark backgrounds, surfaces, ink, muted text, border, primary blue, accent cyan, spacing, and typography.
- The shell exposes named automation properties for sidebar, open file/folder, search, theme, tabs, and diagnostics.

- [x] Add resource dictionaries using the exact brand tokens and Segoe/Cascadia font families.
- [x] Replace generic toolbar composition with a clear branded header: symbol/wordmark, navigation, primary open actions, search, and theme control.
- [x] Add empty, loading, error, and no-results visual states using native XAML controls and the supplied symbol asset.
- [x] Add keyboard-visible focus, tooltip text, accessible names, and responsive pane behavior.
- [ ] Build the app and inspect both light and dark resource resolution.

### Task 3: Implement repository tree and outline UX

**Files:**
- Create: `src/Marknexia.Files/RepositoryTreeNode.cs`
- Create: `src/Marknexia.Files/RepositoryTreeBuilder.cs`
- Modify: `src/Marknexia.App/MainWindow.xaml`
- Modify: `src/Marknexia.App/MainWindow.xaml.cs`
- Test: `tests/Marknexia.Files.Tests/RepositoryTreeBuilderTests.cs`

**Interfaces:**
- `RepositoryTreeBuilder.Build(string rootPath)` returns a hierarchical, sandboxed tree of Markdown and supported resource files.
- Tree nodes expose `DisplayName`, `FullPath`, `IsFolder`, `Children`, and `IsExpanded`.

- [x] Add tests for nested folders, stable ordering, ignored build/VCS folders, and root containment.
- [x] Implement the builder on top of the existing repository/file services without bypassing canonicalization.
- [ ] Verify hierarchical repository selection with native controls. The current implementation uses an indented list; earlier TreeView activation failures must be retested after restoring generated WinUI resources, not assumed to be a host limitation.
- [x] Keep outline indentation, heading levels, copy-anchor action, and empty-outline state visually coherent with the shell.
- [x] Run file/navigation tests and build the app.

### Task 4: Finish search, command handling, and diagnostics

**Files:**
- Create: `src/Marknexia.Core/DocumentSearchState.cs`
- Create: `src/Marknexia.App/MainWindow.Search.cs`
- Modify: `src/Marknexia.App/MainWindow.xaml`
- Modify: `src/Marknexia.App/MainWindow.xaml.cs`
- Modify: `src/Marknexia.Rendering/TemplateEngine.cs`
- Modify: `src/Marknexia.Rendering/MarkdownRenderer.cs`
- Test: `tests/Marknexia.Rendering.Tests/MarkdownRendererTests.cs`

**Interfaces:**
- `DocumentSearchState` exposes `Query`, `MatchCount`, `CurrentMatch`, and next/previous transitions.
- Bridge commands receive JSON-safe values instead of interpolated JavaScript strings.

- [x] Add focused tests for code-copy payloads, apostrophes/quotes in search queries, and Mermaid source containing HTML-sensitive characters.
- [x] Replace JavaScript interpolation in search/copy/anchor calls with serialized JSON arguments and explicit bridge methods.
- [x] Add match count/current-match feedback, Shift+Enter previous behavior, Escape close, and no-results diagnostics.
- [x] Render actionable non-blocking diagnostics for missing files, broken anchors, diagram failures, and blocked links.
- [x] Verify Mermaid and other assets are loaded from the local virtual host only, with no CDN URL in generated HTML.
- [x] Run renderer/security tests and inspect generated HTML for network references.

### Task 5: Complete persistence, startup, and release verification

**Files:**
- Modify: `src/Marknexia.App/MainWindow.xaml.cs`
- Modify: `src/Marknexia.Infrastructure/SettingsService.cs`
- Modify: `README.md`
- Modify: `implementation-docs/IMPLEMENTATION-CHECKLIST.md`
- Create: `tests/Marknexia.App.Tests/Marknexia.App.Tests.csproj` if app-state tests require a separate test assembly

**Interfaces:**
- Settings persist theme, sidebar state, recent files, and repository root using the existing JSON settings service.
- Startup accepts an associated Markdown file and displays a branded first-run empty state when no file is supplied.

- [x] Add persistence tests for theme and recent-workspace data, including malformed settings recovery.
- [x] Wire startup file association, drag/drop, recent files, and reload without losing tab-local state.
- [x] Update the implementation checklist only for behaviors proven by tests or live inspection.
- [x] Run `dotnet test Marknexia.slnx` and x64/ARM64 Release builds.
- [ ] Launch the packaged/portable app, verify open-folder → tree → README → outline → link → back/forward → search → theme → close-tab, and capture screenshots at desktop and narrow widths.
- [ ] Compare the live UI against the brand guidelines and record remaining intentional deviations before handoff.

## Verification Checklist

- [x] All solution tests pass with no skipped release-critical cases.
- [x] x64 Release build succeeds through `scripts/Build-App.ps1`, including generated PRI and bundled SBOM output checks (2026-09-06).
- [x] No generated runtime HTML references external scripts/styles/assets.
- [ ] Light, dark, and system themes render with the supplied Marknexia mark and exact palette.
- [ ] Repository tree, outline, tabs, search, links, fragments, history, diagnostics, and startup flow work live.
- [ ] Narrow-width layout has no clipped controls or horizontal overflow.
- [x] Working tree contains no temporary QA artifacts.

## Task 6: Product support, update, and supply-chain surfaces

**Files:**
- Create: `scripts/Generate-Sbom.ps1`
- Create: `src/Marknexia.Infrastructure/UpdateService.cs`
- Modify: `src/Marknexia.App/MainWindow.xaml`
- Modify: `src/Marknexia.App/MainWindow.xaml.cs`
- Modify: `src/Marknexia.Infrastructure/SettingsService.cs`
- Modify: `README.md`

- [x] Add branded About and self-help dialogs with keyboard-accessible menu actions.
- [x] Implement self-update through verified portable-release download, SHA-256 verification, safe extraction, rollback-capable installation, and restart-worker orchestration. Signed MSIX/Store update integration remains release-specific.
- [x] Add deterministic SPDX 2.3 SBOM generation from the application restore graph, excluding local projects and unrelated test dependencies, with package hashes, bundled Mermaid hash, regression tests, and repository documentation. This is not a complete native-runtime file inventory; unresolved licenses remain NOASSERTION.
- [ ] Verify menu interactions and self-update end to end in the native packaged application; source-level SBOM/update staging coverage is now present.

## Verification checkpoint: 2026-09-06

- `dotnet test Marknexia.slnx --configuration Release --no-restore --verbosity minimal`: **54 passed**, zero failed/skipped across seven projects.
- New renderer tests first reproduced missing safe copy bindings and lost Mermaid source text; all six renderer tests pass after replacing inline handlers with delegated controls and rendering each Mermaid block independently.
- `scripts/Build-App.ps1 -Configuration Release -Platform x64 -NoRestore`: **passed**, using Visual Studio 18 BuildTools MSBuild. A clean `-Rebuild` also passed before the renderer changes.
- `scripts/Test-AppOutput.ps1` rejects the earlier output lacking `Marknexia.App.pri`; the current build passes executable/assembly/PRI/SBOM checks.
- `scripts/Test-Sbom.ps1 -SchemaPath artifacts/spdx-2.3-schema.json`: **passed**. The actual Release SBOM also passed `Test-Json` against the official SPDX 2.3 schema.
- Native Windows automation through `@oai/sky` now works with an isolated QA copy under `artifacts/desktop-qa-7ec20fbee81345b59063c20ee5213b58`. The QA host is named `Marknexia.Verify.exe` and requires a matching `Marknexia.Verify.pri` copy; this avoids collision with the older registered installation. Activate the returned QA window before capture. Earlier occluded captures are **not** UI evidence. The installed application was not overwritten.
- **Live verified:** repository README rendering with the supplied logo and tagline; Help menu; Software components dialog; Copy SBOM yielding the matching bundled SPDX document namespace and ten inventory packages (nine NuGet plus the product); branded About dialog opening; Mermaid rendering; Source toggle; Mermaid Copy matching the README source after line-ending normalization.
- **Live defects found:** About displayed the stable assembly version 1.0.0 instead of product version 1.0.11; remote badges appear as broken images under the default blocked-assets policy; generated QA artifacts clutter the repository tree. The About/update version source is now corrected to use informational product version without build metadata, and the corrected Release app has rebuilt and relaunched. Its About display recheck remains pending after intervening desktop input.
- The user resumed desktop testing after the Escape interruption. Subsequent Mermaid checks passed. A new self-update architecture was proposed for approval: Windows-managed MSIX updates plus a verified, rollback-capable portable updater, both with explicit Install and restart confirmation. No updater installation engine has been implemented or tested yet.
- Self-update download/install/restart, packaged activation, full live interaction coverage, narrow layouts, and remaining acceptance criteria are still incomplete. The goal remains active.

## Verification checkpoint: 2026-09-07 — rendered search and bridge controls

- Full Release .NET suite: **69 passed**, zero failed/skipped across seven projects (Core 13, Files 10, Navigation 25, Markdown 5, Infrastructure 5, Security 5, Rendering 6).
- Headless Chrome suite: **10 passed**, zero failed/skipped. It loads production `bridge.js` before a real document load. Coverage includes rendered occurrence counts, selection and wraparound, literal metacharacters/Unicode, inline/block boundaries, hidden and collapsed content, mutation invalidation, exact code-copy host messages, isolated diagram source toggles, and relative-link host dispatch. Only the native host message receiver is stubbed; uncaught page errors fail the suite.
- Browser tests first reproduced the missing search method, then independently exposed closed-details text being counted and a false match across the end of a paragraph. The visibility and boundary fixes passed the expanded regression suite.
- Native search now consumes the browser's count and current position rather than recounting Markdown source. Tab-local drafts/results are separate, navigation invalidates old counts, navigation IDs reject obsolete completion events, and query/tab changes reject stale script responses. The domain-state tests pass; native tab/reload/search interaction still needs live re-verification.
- `scripts/Build-App.ps1 -Configuration Release -Platform x64 -NoRestore` completed with exit 0 and passed executable/assembly/generated-PRI/SBOM output checks after navigation/search integration. The final incremental build including the initialization null guard also passed. `git diff --check` passed.
- CI and release workflows now run the pinned Node/Playwright browser suite; release also runs the .NET suite before packaging. These workflow changes have been checked locally but have not run on GitHub in this task.
- The generated-directory repository-tree filter has seven passing regression cases (Files suite totals 10). This fixes QA/build folders appearing in the tree; reparse-point containment and native hierarchical control verification remain unfinished.
- Native desktop input remains paused after the second Escape interruption. The installed application was not overwritten, and the older isolated QA process has not been refreshed with these changes. Build/headless evidence is not a substitute for native live verification.
- Full completion remains unproven: navigation/scroll history, repository asset mapping and containment, offline math, remote-image fallback, settings startup handling, packaging/activation, responsive branding/accessibility, and the approved acceptance checklist still require work. The updater architecture decision remains pending; no download/install/restart engine has been implemented.

## Verification checkpoint: 2026-09-08 — review fixes and encoded navigation

- Navigation link resolution now trims only outer destination whitespace, decodes paths/fragments exactly once, preserves literal percent-escape filenames, and checks repository traversal after decoding. Network-file destinations are rejected before filesystem probes. Eleven new cases first failed, then passed; the navigation suite now has 36 passing tests. Permanent fixtures include `docs/encoded name.md` and `docs/literal%20.md`, linked from the navigation fixture README.
- Independent search review reproduced five important gaps: CSS-collapsed whitespace, missing block boundaries, trimmed literal query spaces, inactive-tab response loss, and hidden matches inside scroll containers. Fixes now normalize the rendered text stream with DOM offset maps, detect CSS block boundaries, preserve query whitespace, scope request revisions per tab/query/document, and reveal selected ranges through scrollable ancestors on both axes.
- Full Release .NET suite: **86 passed**, zero failed/skipped (Core 19, Files 10, Navigation 36, Markdown 5, Infrastructure 5, Security 5, Rendering 6).
- Full headless Chrome suite: **16 passed**, zero failed/skipped, including the new review regressions. These tests use the actual bridge; the scroll and mixed-whitespace tests also use the production stylesheet.
- Current x64 Release build: **passed** with executable/assembly/generated-PRI/SBOM output checks. `git diff --check` passed. No installed application replacement or new native live test was performed.
- The original reviewer completed the focused follow-up: four fixes were accepted at source/headless level, and one remaining mixed-whitespace boundary was reproduced (`<code>read </code> me`). A regression first failed, then passed after tracking whether the preceding space is preserved. The final 16-test browser suite, 86-test .NET suite, and Release build all passed again. Native tab/search/reload verification remains outstanding because desktop input is paused.
- The prior approval-service usage-limit rejection prevented a test launch, not a test failure. Account availability was checked before retrying, and the formerly blocked browser suite has now completed successfully.
- These checkpoints do not complete the overall goal. The remaining acceptance and packaging/security/branding gaps listed above remain open.

## Verification checkpoint: 2026-09-09 — assets, math, updater, and release surfaces

- Added immutable per-document asset origins and a WebView2 resource broker backed by a bounded local-image reader. It decodes URL paths exactly once, rejects network/reparse escapes, enforces an image allowlist and 32 MiB limit, and checks Windows final paths against the repository root.
- Added offline semantic math rendering for common LaTeX expressions, including inline/display modes, superscripts, subscripts, fractions, roots, Greek/operator symbols, safe encoding, accessible labels, and branded CSS. Rendering coverage now includes CSP/nonces, remote-image blocking, math, and bundle repair.
- Replaced the release-page-only updater with trusted-host asset selection, redirect validation, streaming size limits, SHA-256 sidecar verification, required portable payload validation (executable, assembly, PRI, and SBOM), safe ZIP extraction, staging cleanup, and rollback-capable restart-worker orchestration. Integration tests cover verified extraction, incomplete-payload rejection, and untrusted redirects; native restart and signed-release flows remain pending.
- Corrected the live updater metadata path to trust `api.github.com`; the regression test failed before the allowlist fix and now passes with the full infrastructure suite.
- Added an explicit Settings-menu toggle for remote images, persisted through `SettingsService`, and a compact toolbar visual state for narrow windows; both are covered by the Release build, while native visual inspection remains pending.
- Persisted the selected repository root and restore it safely on startup; missing or inaccessible roots are cleared without preventing launch. Replaced the former flattened repository `ListView` with a native hierarchical `TreeView` backed by the existing sandboxed builder; x64 XAML/resource compilation passes, while native expansion/invocation remains pending.
- Navigation now restores fragments after actual WebView navigation completion, preserves fragment state in back/forward, rolls back failed tab replacement, removes stale large-document cache files, and restricts cache-host requests to the active exact file. Repository tree/workspace enumeration skips reparse points; settings initialization no longer saves transient XAML defaults.
- Full Release solution suite: **113 passed**, zero failures/skips. Headless Chrome suite: **18 passed**, zero failures/skips. x64 Release build passed executable/assembly/generated-PRI/SBOM output checks. Native WinUI/WebView2 interaction remains paused and is not claimed by these checks.
- Regenerated the unsigned x64 Store-layout MSIX with MakeAppx (295 files) and added `scripts/Test-MsixPackage.ps1`. The verifier passed identity `RRCLabs.Marknexia`, x64 version `1.0.11.0`, required executable/PRI/SBOM payloads, and MakeAppx unpackability. Signing, WACK, Store installation, and native activation remain release/native verification work.
- Added the same MSIX verifier to `.github/workflows/release.yml` after MakeAppx packaging, so release artifacts now fail the pipeline if identity, payload, architecture/version, or unpackability checks regress.
- Updated `scripts/prepare-store-package.ps1` to suppress MakeAppx noise and invoke the verifier automatically; the local package command now exits nonzero on structural package regressions.

## Verification checkpoint: 2026-09-09 — native TabView source completion

- Replaced the superficial horizontal document `ListView` with a native WinUI `TabView`. The shell now exposes standard close requests, `CanReorderTabs`, `TabItemsChanged` synchronization, keyboard selection, branded surface resources, and per-document state mapping.
- Active-tab close selects the nearest remaining document; closing a background tab preserves the active document. Tab reordering updates the authoritative tab-state order used by navigation, search, and rendering.
- `dotnet test Marknexia.slnx --configuration Release --no-restore --verbosity minimal`: **113 passed**, zero warnings across seven projects.
- `scripts/Build-App.ps1 -Configuration Release -Platform x64 -NoRestore`: **passed**, including generated PRI and bundled SBOM checks.
- `scripts/prepare-store-package.ps1`: **passed**, including MSIX verification for identity `RRCLabs.Marknexia`, x64 version `1.0.11.0`, required executable/assembly/PRI/SBOM payloads, and MakeAppx unpackability.
- Native input remains paused, so actual drag-reorder, close-button, repository-tree expansion/invocation, WebView2 interaction, signed installation, WACK, and restart-worker behavior remain explicitly unclaimed.

## Verification checkpoint: 2026-09-09 — performance and packaged activation completion

- Added a tested 50 MiB source-document guard that rejects oversized files before allocation and reports an actionable diagnostic. Rendering now runs parsing, highlighting, diagram transformation, math rendering, and sanitization on a worker task with cancellation checks and synchronized non-thread-safe adapters. The new async-render and oversized-file tests are included in the full suite.
- Added `StartupFileResolver` with unit coverage and wired `AppInstance.GetCurrent().GetActivatedEventArgs()` into `App.OnLaunched`. Packaged Windows file activation now supplies `.md`, `.markdown`, `.mdown`, and `.mkdn` paths before the existing command-line fallback; direct executable launches remain supported.
- Full elevated Release solution gate: **117 tests**, zero failures/skips/warnings across seven projects.
- x64 and ARM64 Release Windows App SDK builds passed executable/assembly/generated-PRI/SBOM output checks.
- The package scripts now accept `-Platform ARM64`; both `Marknexia-Store-Ready.msix` (x64) and `Marknexia-Store-Ready-ARM64.msix` passed architecture-aware identity, required payload, SBOM, and MakeAppx unpackability checks. Both packages are unsigned.
- Headless Chromium bridge suite: **18 tests**, zero failures/skips. SBOM regression checks, x64 app-output checks, and the unsigned x64 MSIX verifier all passed. The package verifier confirmed identity `RRCLabs.Marknexia`, version `1.0.11.0`, required executable/PRI/SBOM payloads, and MakeAppx unpackability.
- Native packaged double-click activation, signed installation, WACK, Store submission, live updater restart, and native narrow-width/theme/accessibility inspection remain explicitly unverified because desktop input is paused. Source/build/package evidence does not claim those runtime gates.

## Verification checkpoint: 2026-09-09 — performance guardrails, async workspace, and cache integration

- Added renderer guardrails required by the performance specification: 128 MiB final UTF-8 HTML limit, 64 Mermaid blocks per document, and 1 MiB per Mermaid source. Over-limit or explicitly disabled diagrams become branded, accessible source disclosures and do not load the Mermaid runtime. The rendering suite now has **19 passing tests**.
- Hardened `FileService` with a bounded asynchronous stream read so a file that grows after the initial metadata check cannot bypass the 50 MiB source limit.
- Moved repository tree construction behind `RepositoryTreeBuilder.BuildAsync`, with cancellation checks throughout recursive enumeration. Startup restoration, automatic repository discovery, and folder selection now cancel superseded scans and attach `TreeViewNode` instances only after worker completion.
- Connected the bounded document cache to open, replace, and reload flows. Render keys include content hash, canonical source path, repository authority, theme, diagram/math policy, remote-image policy, and an explicit renderer configuration version. The cache regression suite covers cross-document and policy isolation.
- Mermaid extraction and virtual-host mapping are now lazy; ordinary documents do not initialize the bundled diagram runtime. Branded fallback styling, menu automation names, and custom surface resources align the remaining shell states with the supplied palette and accessibility labels.
- Tightened updater trust to HTTPS default ports and enforced the extraction ceiling against actual copied ZIP bytes in addition to entry metadata.
- Full elevated Release solution gate: **124 tests**, zero failures/skips/warnings across seven projects. x64 and ARM64 Release builds, SBOM checks, app-output checks, unsigned x64 MSIX verification, and the **18-test** Chromium bridge suite all pass.
- Native packaged double-click activation, signed installation, WACK, Store submission, live updater restart, and native visual/accessibility inspection remain explicitly unverified because desktop input is paused.

## Verification checkpoint: 2026-09-09 — release architecture and updater parity

- Extended the tagged release workflow to build x64 and ARM64, publish architecture-specific portable ZIPs and SHA-256 sidecars, generate both MSIX variants, and upload both architecture-specific SBOMs.
- Made self-update select the package/checksum pair for the running x64 or ARM64 process; unsupported process architectures fail with an explicit platform message. Added regression coverage for both release asset families.
- Hardened direct unpackaged startup so Windows activation interfaces that are unavailable outside packaged activation (including `InvalidCastException`) fall back to command-line arguments instead of terminating before the shell is shown.
- Native smoke launch of the rebuilt x64 executable with `README.md` produced a responsive process and no new unhandled-exception entry; the available desktop connector could not enumerate the WinUI window, so pixel-level shell/WebView interaction remains unclaimed.
- Final elevated Release solution gate: **131 tests**, zero failures/skips/warnings across seven projects. The focused updater project passed **16 tests**. Existing x64/ARM64 build, SBOM, app-output, unsigned MSIX, and **18-test** Chromium bridge evidence remains valid.
- Native packaged double-click activation, signed installation, WACK, Store submission, live updater restart, and native visual/accessibility inspection remain explicitly unverified because desktop input is paused.
