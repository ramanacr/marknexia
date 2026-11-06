# Marknexia Rust/Win32 Production Rewrite Plan

Status: **draft.** Phase 1 is complete (integrated at 816d931; CI run 42154523108 green after one rerun). Phase 1 ran in unattended mode, because it changes no behaviour covered by pending decisions. Later phases wait for their owner gates.
Inputs: `docs/superpowers/specs/2026-09-12-rust-win32-rewrite-design.md` (architecture), `docs/superpowers/reports/2026-09-12-rust-win32-feasibility-report.md` (decision CONDITIONAL-GO, conditions C1–C6), `artifacts/rust-feasibility/manifest.json` (evidence index).
Branch base: `ramana_rust_win32_feasibility` at the manifest commit or later.

## Global constraints

- The .NET edition stays the releasable product until gate G6. Every phase ends with the `.NET Release preservation` CI job green.
- One writer per worktree; parallel lanes use separate worktrees; every lane gets an independent review before integration.
- Never write synthetic or malformed PE bytes to disk and never create archives of such bytes. Never add antivirus exclusions. Local builds use `CARGO_TARGET_DIR=C:\marknexia_builds\<worktree>`; `C:\marknexia_builds\Clear-StaleBuilds.ps1` removes stale output.
- Observable differences from .NET need a `compat/decisions/*.md` record and owner approval; nothing is removed silently.
- Native UI tests send synthetic input only when the shell under test owns the foreground; system-surface dismissal is GitHub Actions only.
- Report evidence in four classes: implemented, source-reviewed, compiled, native-tested.

## Owner gates

| Gate | Needed before | Decision |
|---|---|---|
| G0 | Any proposed decision record becomes approved; preview (Phase 8) | Accept CONDITIONAL-GO; approve or reject the five proposed `compat/decisions/*.md` files and the highlighting recommendation (C1) |
| G1 | Phase 3 | Highlighting language set (C2): port ColorCode's languages, or approve a reduced set with a decision record |
| G2 | Phase 6 | Update signing key creation and custody approved by the security owner (C5) |
| G3 | Phase 7 | Declared reference hardware for x64 and ARM64 (C3) |
| G4 | Phase 8 | Preview channel and side-by-side install policy |
| G5 | Phase 9 | Promotion: every design §13 release gate passes |
| G6 | Phase 10 | Retire the .NET edition after the overlap period |

## .NET feature inventory to port

From `src/` (line counts are approximate):

| .NET source | Feature | Rust target |
|---|---|---|
| `Marknexia.App/MainWindow*.cs` (~1,800) | Tabs, history, outline, search, repository tree, recent files, drag-and-drop, sidebar modes and resize, theme selector, remote-assets toggle, dialogs (About, Help, updates), diagnostics bar | `marknexia-win32` |
| `Marknexia.App/App.xaml.cs` | Single instance, command-line and association activation | `marknexia-win32` |
| `Marknexia.Core/*` | Models, slugs, search state, startup file resolution, theme resolution | `marknexia-core` (slugs done) |
| `Marknexia.Files/*` | Canonical paths, asset reader, remote images, repository scan and tree | `marknexia-files` (paths done) |
| `Marknexia.Navigation/*` | Resolver, history | `marknexia-navigation` (done) |
| `Marknexia.Markdown`, `Rendering`, `Security`, `Syntax`, `Diagrams` | Parse, render, sanitize, highlight, Mermaid | content crates (feasibility-complete except highlighting set and Mermaid runtime) |
| `Marknexia.Infrastructure/SettingsService.cs` | Settings and recent files | `marknexia-core` settings module |
| `Marknexia.Infrastructure/DocumentCache.cs` | Render cache | `marknexia-rendering` |
| `Marknexia.Infrastructure/UpdateService.cs` (~410) | Update check, download, verify, apply | `marknexia-update` (contract decided, not built) |
| `Marknexia.Setup/*` (~690) | Installer UI and install logic | new `marknexia-setup` crate |

## Phases

Each phase lists its tasks, the files it owns, and its exit gate. Estimates assume one engineer plus review.

### Phase 1 — Core services (1–2 weeks)

1. Settings: read the current .NET settings format, validate paths and values, back up, write atomically, version the schema (design §16). Fixtures: `compat/fixtures/v1/settings`.
2. Recent documents and startup file resolution (`StartupFileResolver` parity).
3. Document search state (find in page) contract shared with the bridge.
4. Repository scan with cancellation and bounded results (`RepositoryService`, `RepositoryTreeBuilder` parity), using the existing path gate including UNC.

Exit: unit and property tests; settings fixture parity; fuzz target for settings parsing; review.

### Phase 2 — Shell features (3–4 weeks)

1. Sidebar with outline, repository tree and recent files as native controls (TreeView/ListView with UIA from the platform), resizable splitter with keyboard resize.
2. Toolbar and menus: open file and folder (common item dialog), reload, back and forward, theme selector, remote-assets toggle, About, Help.
3. Find in page through the bridge; Ctrl+F, F3, Shift+F3, Escape.
4. Drag-and-drop of files and folders (`IDropTarget`), single-instance hand-off, command-line and file-association activation.
5. Tab close buttons and reordering with UIA `Invoke` on close, so closing no longer needs Ctrl+W in tests.
6. Diagnostics bar parity, including localized messages.

Exit: native UIA tests for each workflow on x64 and ARM64 CI; keyboard-only walkthrough of every command; review.

### Phase 3 — Content completion (2–3 weeks, after G1)

1. Highlighting for the approved language set, byte-exact with ColorCode where fixtures exist.
2. Mermaid runtime in the viewport: pinned asset, CSP nonce, count and source limits, accessible fallback, diagnostic.
3. Math rendering parity (`SimpleMathRenderer`).
4. Remote image policy UI wired to `RemoteImagePolicy`.

Exit: rendering fixtures extended for each language and Mermaid case; fuzz 300 s per target; review.

### Phase 4 — Theming and accessibility hardening (1–2 weeks)

1. Runtime high-contrast toggle and theme change verified on a manual evidence host (C4).
2. Multi-monitor `WM_DPICHANGED` with differing DPI, verified manually and with layout tests (C4).
3. Narrator and keyboard audit of every surface; fix all accessibility-critical findings.

Exit: zero accessibility-critical failures; evidence recorded per architecture.

### Phase 5 — WebView2 distribution (1 week)

1. Missing-runtime state with user-initiated Evergreen bootstrapper download, signature check, retry and offline fallback (`packaging/webview/README.md`).
2. Tests for offline, failed download, failed signature and cancellation; shell stays usable in every case.

Exit: native tests on a host without WebView2 (clean VM image); review.

### Phase 6 — Updater (2–3 weeks, after G2)

1. Implement `update-manifest-v1` verification (Ed25519 with embedded public key), Authenticode check, bounded staging, side-by-side payload directories.
2. `--post-update-health` with a nonce-bound 60-second acknowledgement and automatic rollback.
3. Tamper, interruption, health-failure and rollback tests using test keys only.

Exit: tampered updates accepted = 0; rollback proven; separate from Store/MSIX updates; security review.

### Phase 7 — Packaging and supply chain (2 weeks, after G3)

1. Per-user setup executable (`marknexia-setup`), portable ZIP of real release builds, and MSIX where appropriate.
2. SPDX or CycloneDX SBOM covering crates, assets, the WebView2 loader and artifacts.
3. Checksums, signing under the release policy, build provenance.
4. Measure installer and unpacked sizes and startup on the declared reference hardware (C3).

Exit: installer ≤ 15 MB, unpacked ≤ 35 MB, cold start p95 ≤ 500 ms and idle host ≤ 35 MB on reference hardware for x64 and ARM64.

### Phase 8 — Side-by-side preview (4–6 weeks, after G4)

1. Publish the Rust edition as a preview alongside .NET with distinct identity, associations and update channel.
2. Collect crash, compatibility, size, startup and memory evidence.
3. Fix regressions; every fix gets a fixture or test.

Exit: no release-blocking regressions for the agreed period.

### Phase 9 — Promotion (after G5)

Promote the Rust edition only after every design §13 gate passes on both architectures, then migrate users' settings and associations.

### Phase 10 — Retirement (after G6)

Retire the .NET edition after the defined overlap and successful migration.

## Risks carried from feasibility

| Risk | Mitigation in this plan |
|---|---|
| Hosted ARM64 timing noise (cold p95 81–498 ms on one binary) | Reference-hardware measurement in Phase 7 decides the gate |
| Hosted runner launch failures (`0xC0000142` three times in a row in run 42154523108; the rerun passed) | Phase 7 measures on reference hosts; the collector records every retry |
| Hosted runner foreground interference | Tests dismiss system surfaces on GitHub Actions only; reference hosts in Phase 4/7 |
| Highlighting scope larger than fixtures | Gate G1 before Phase 3 |
| Updater security | Gate G2 and a separate security review in Phase 6 |
| Raw Win32 UI cost | Native controls for sidebar and dialogs; owner drawing only for tabs |

## Estimated total

About 17–25 engineer-weeks to the start of the preview (Phases 1–7), plus the preview period. The estimate assumes the owner gates do not add work beyond their records.
