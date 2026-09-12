# Marknexia Rust/Win32 Feasibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Produce the evidence needed for a go/no-go decision on the Rust/raw-Win32 rewrite without replacing or destabilizing the releasable .NET/WinUI product.

**Architecture:** Keep the current .NET edition as the behavioral oracle and releasable product. Add language-neutral compatibility fixtures, the approved root-level Rust workspace, a secure native WebView2 vertical slice, a raw Win32 shell/UI Automation proof, and reproducible size/performance measurements. Work proceeds in three lanes after the shared baseline: portable parity, native shell/WebView2, and release measurement.

**Tech Stack:** .NET 10/C# baseline exporter, stable Rust 2024 with MSVC targets, `windows-rs`, native WebView2 COM, `serde`, `serde_json`, candidate `comrak`/`pulldown-cmark`, candidate `ammonia`, Cargo test/property/fuzz tooling, PowerShell, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-12-rust-win32-rewrite-design.md`

## Global Constraints

- The .NET/WinUI application, installer, updater, and existing release workflows remain intact and releasable throughout feasibility.
- The native shell uses raw Win32 through `windows-rs`; do not add a general-purpose Rust GUI framework, WinUI, Windows App SDK, Tauri, Electron, Node.js, bundled Chromium, .NET, or a JVM runtime.
- Web technology is confined to the rendered-document WebView2 viewport, using the shared Evergreen runtime.
- Support targets are Windows 10 version 19041 or later and Windows 11, on `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`.
- Local Markdown, filesystem input, browser messages, archives, and update metadata are untrusted and bounded.
- Every unsafe block is confined to a Windows/WebView adapter and has a nearby `SAFETY:` invariant covering ownership, lifetime, thread affinity, and callback assumptions.
- Normalized fixtures may remove only demonstrated nondeterminism. Library differences require a reviewed compatibility decision; tests must not silently bless them.
- Commit `Cargo.lock`; use exact dependency versions after the candidate bake-off; CI and verification commands use `--locked`.
- Feasibility passes only with measured evidence. A compile-only ARM64 build is not ARM64 runtime evidence.
- This plan does not authorize product cutover, retirement of the .NET edition, production signing-key creation, file-association takeover, or publishing a Rust release.

## Work Allocation and Dependencies

```text
Task 1: frozen .NET baseline
        |
Task 2: Rust workspace and policy
        |
        +----------------------+----------------------+
        |                      |                      |
Lane A: Tasks 3-4       Lane B: Tasks 5-6      Lane C: Task 7 setup
portable parity         WebView2 + Win32        measurement/release
        |                      |                      |
        +----------------------+----------------------+
                               |
                         Task 8: gate report
```

- Lane A owns `compat/`, `tools/Marknexia.ParityExporter/`, and portable Rust crates.
- Lane B owns `crates/marknexia-webview/` and `crates/marknexia-win32/`.
- Lane C owns `scripts/Measure-Rust*.ps1`, Rust feasibility workflows, and evidence schemas.
- Agents must not edit another lane's files. Changes to root `Cargo.toml`, `Cargo.lock`, `.github/workflows/ci.yml`, or shared schemas are integrated by the coordinating agent after lane review.

---

### Task 1: Freeze the .NET Behavioral and Security Baseline

**Files:**
- Create: `compat/schema/marknexia-parity-v1.schema.json`
- Create: `compat/README.md`
- Create: `compat/fixtures/v1/manifest.json`
- Create: `compat/fixtures/v1/{markdown,headings,navigation,sanitizer,rendering,settings,update-archives}/*.case.json`
- Create: `compat/decisions/README.md`
- Create: `tools/Marknexia.ParityExporter/Marknexia.ParityExporter.csproj`
- Create: `tools/Marknexia.ParityExporter/Program.cs`
- Create: `tools/Marknexia.ParityExporter/ParityCase.cs`
- Create: `tools/Marknexia.ParityExporter/ParityNormalizer.cs`
- Create: `tools/Marknexia.ParityExporter/ParityExporter.cs`
- Create: `tests/Marknexia.Parity.Tests/Marknexia.Parity.Tests.csproj`
- Create: `tests/Marknexia.Parity.Tests/ParityExporterTests.cs`
- Create: `tests/Marknexia.Parity.Tests/ParityTestSupport.cs`
- Modify: `Marknexia.slnx`

**Interfaces:**
- Consumes: `MarkdigParserAdapter.Parse`, `HeadingSlugGenerator.GenerateSlug`, `NavigationResolver.Resolve`, `HtmlSanitizerService.SanitizeHtml/SanitizeSvg`, `MarkdownRenderer.Render`, `SettingsService`, and safe update/archive validation seams.
- Produces: canonical JSON cases with `schemaVersion`, `area`, `name`, `input`, `virtualFileSystem`, `expected`, and `sourceRevision`; `ParityExporter.ExportAsync(string repositoryRoot, string outputRoot, CancellationToken)`; `ParityNormalizer.NormalizeHtml(string)`.

- [ ] **Step 1: Add a failing schema/round-trip test**

```csharp
[Fact]
public async Task ExportTwice_ProducesByteIdenticalFixtures()
{
    string first = ParityTestSupport.CreateTempDirectory();
    string second = ParityTestSupport.CreateTempDirectory();
    var exporter = new ParityExporter(ParityTestSupport.FindRepositoryRoot());

    await exporter.ExportAsync(first, CancellationToken.None);
    await exporter.ExportAsync(second, CancellationToken.None);

    ParityTestSupport.DirectoryDigest(first)
        .Should().Be(ParityTestSupport.DirectoryDigest(second));
    ParityTestSupport.ValidateEveryCaseAgainstSchema(first).Should().BeTrue();
}
```

`ParityTestSupport` owns these four helpers, deletes its temporary directories in test cleanup, and computes the digest from sorted repository-relative paths plus file bytes.

- [ ] **Step 2: Run the focused test and confirm the exporter is absent**

Run: `rtk dotnet test tests/Marknexia.Parity.Tests/Marknexia.Parity.Tests.csproj --no-restore`

Expected: FAIL because `ParityExporter` and the fixture schema do not exist.

- [ ] **Step 3: Implement the versioned fixture envelope and deterministic normalizer**

Use ordinal property ordering, UTF-8 without BOM, LF line endings, repository-relative paths with `/`, and a terminal newline. HTML normalization may normalize line endings and documented volatile virtual-host identifiers; it must preserve elements, attributes, text, whitespace with rendering meaning, URL values, and sanitizer output.

- [ ] **Step 4: Export the existing behavior corpus**

Include existing `test-fixtures/markdown/**` and permanent cases for duplicate/formatted headings, custom anchors, tables, grid tables, task lists, footnotes, mathematics, emphasis extras, alerts, language labels, Mermaid blocks, exact source lines, remote images on/off, code-copy metadata, diagram/input/output limits, malformed settings, recent-item bounds, percent-decoding exactly once, encoded traversal, UNC/file URI rejection before filesystem probing, unsafe HTML/SVG/CSS/URIs, checksum mismatch, archive traversal, expansion limits, missing payload files, and stale-stage cleanup.

- [ ] **Step 5: Record intentional target-only differences separately**

Add reviewed decisions for the Rust package manifest replacing `.dll`/`.pri` validation and for atomic settings backup/migration. Do not modify baseline expected output to represent target behavior that the .NET oracle does not currently implement.

- [ ] **Step 6: Verify repeatability and the existing product baseline**

Run:

```powershell
rtk dotnet test Marknexia.slnx --configuration Release --no-restore
rtk dotnet run --project tools/Marknexia.ParityExporter -- --output compat/fixtures/v1 --verify
rtk git diff --check
```

Expected: the full .NET suite passes; a second export changes no tracked fixture bytes; every case validates against `marknexia-parity-v1.schema.json`.

- [ ] **Step 7: Commit the frozen baseline**

```powershell
rtk git add compat tools/Marknexia.ParityExporter tests/Marknexia.Parity.Tests Marknexia.slnx
rtk git commit -m "test: freeze Rust rewrite compatibility baseline"
```

### Task 2: Establish the Reproducible Rust Workspace and Policy Gate

**Files:**
- Create: `Cargo.toml`
- Create: `Cargo.lock`
- Create: `rust-toolchain.toml`
- Create: `.cargo/config.toml`
- Create: `deny.toml`
- Create: `tools/rust-tools.lock`
- Create: `crates/marknexia-core/Cargo.toml`
- Create: `crates/marknexia-core/src/lib.rs`
- Create: `crates/marknexia-core/src/contracts.rs`
- Create: `crates/marknexia-{files,navigation,markdown,security,rendering,webview,update,win32}/Cargo.toml`
- Create: minimal `src/lib.rs` or `src/main.rs` in each crate
- Create: `scripts/Build-Rust.ps1`
- Create: `.github/workflows/rust-feasibility.yml`

**Interfaces:**
- Produces: the nine-crate workspace from the approved design; `DocumentId`, `GenerationId`, `Heading`, `Diagnostic`, `RenderRequest`, `RenderResult`, `AppTheme`, `NavigationIntent`, and bounded-size newtypes in `marknexia-core`.
- No portable crate may import `windows` or WebView2 types.

- [ ] **Step 1: Install and record an exact stable Rust toolchain**

This machine currently has no `rustc`, Cargo, or rustup. With explicit approval for system installation, run:

```powershell
rtk winget install --id Rustlang.Rustup --exact --accept-package-agreements --accept-source-agreements
rtk proxy rustup toolchain install stable --profile minimal --component rustfmt,clippy
rtk proxy rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc --toolchain stable
rtk proxy rustc +stable --version --verbose
rtk proxy cargo install cargo-deny --locked
rtk proxy cargo install cargo-fuzz --locked
```

Write the returned exact compiler release into `rust-toolchain.toml`; do not leave `channel = "stable"` in the committed file. Record the installed `cargo-deny --version` and `cargo-fuzz --version` output in `tools/rust-tools.lock` so CI installs the same versions.

- [ ] **Step 2: Add a failing architecture-boundary test**

Create a test that scans portable crate metadata and rejects any dependency whose package name is `windows`, `windows-core`, `webview2-com`, or `webview2-com-sys`.

- [ ] **Step 3: Define the workspace and core contracts**

```rust
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct GenerationId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Heading {
    pub text: String,
    pub level: u8,
    pub slug: String,
    pub source_line: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum DiagnosticSeverity { Info, Warning, Error }
```

Set release profiles to `incremental = false`, `lto = "thin"`, `codegen-units = 1`, `panic = "abort"`, and `strip = "symbols"`. Tests keep unwind semantics.

- [ ] **Step 4: Add formatting, lint, dependency, dual-target, and .NET-preservation CI jobs**

The new workflow may build feasibility artifacts but must not upload them to GitHub Releases. Keep the existing `ci.yml` and `release.yml` .NET jobs unchanged.

- [ ] **Step 5: Verify the workspace**

Run:

```powershell
rtk proxy cargo fmt --all -- --check
rtk proxy cargo clippy --workspace --all-targets -- -D warnings
rtk proxy cargo test --workspace --locked
rtk proxy cargo build --workspace --locked --release --target x86_64-pc-windows-msvc
rtk proxy cargo build --workspace --locked --release --target aarch64-pc-windows-msvc
rtk proxy cargo deny check advisories bans licenses sources
rtk dotnet test Marknexia.slnx --configuration Release --no-restore
```

- [ ] **Step 6: Commit the workspace gate**

```powershell
rtk git add Cargo.toml Cargo.lock rust-toolchain.toml .cargo deny.toml tools/rust-tools.lock crates scripts/Build-Rust.ps1 .github/workflows/rust-feasibility.yml
rtk git commit -m "build: establish Rust feasibility workspace"
```

### Task 3: Prove Portable Core, Path, and Navigation Parity

**Files:**
- Create: `crates/marknexia-files/src/{lib.rs,path.rs,bounded_read.rs,virtual_fs.rs}`
- Create: `crates/marknexia-navigation/src/{lib.rs,classify.rs,resolve.rs,history.rs}`
- Create: `crates/marknexia-navigation/tests/parity_v1.rs`
- Create: `crates/marknexia-files/tests/{path_properties.rs,bounded_read.rs}`
- Create: `crates/marknexia-core/src/slug.rs`
- Create: `crates/marknexia-core/tests/slug_properties.rs`
- Create: `fuzz/Cargo.toml`
- Create: `fuzz/fuzz_targets/path_and_uri.rs`

**Interfaces:**
- Produces: `CanonicalPath`, `RepositoryScope`, `BoundedReader`, `UriClassification`, `ResolutionContext`, `NavigationIntent`, `NavigationHistory`, and `generate_heading_slug(&str, &mut SlugSet) -> String`.
- Consumes: Task 1 fixtures and Task 2 core contracts.

- [ ] **Step 1: Add fixture-driven tests that fail against empty crates**

```rust
#[test]
fn navigation_v1_matches_frozen_contract() {
    for case in parity_cases("navigation") {
        let actual = resolve(&case.input.href, &case.input.context, &case.vfs);
        assert_eq!(actual, case.expected, "{}", case.name);
    }
}
```

- [ ] **Step 2: Implement slug generation, single decoding, canonical containment, and navigation decisions**

Containment compares canonical path components with Windows case-insensitive semantics. Reject UNC, network `file://`, encoded traversal, device namespaces, alternate data streams, and out-of-root targets before filesystem probes.

- [ ] **Step 3: Add property tests**

Generate path segments, percent encodings, mixed separators, Unicode headings, duplicate headings, and history operations. Assert containment cannot be escaped, decoding occurs once, generated slugs are stable, and history cursors remain valid.

- [ ] **Step 4: Verify Lane A foundation**

Run:

```powershell
rtk proxy cargo test -p marknexia-core -p marknexia-files -p marknexia-navigation --locked
rtk proxy cargo clippy -p marknexia-core -p marknexia-files -p marknexia-navigation --all-targets -- -D warnings
rtk proxy cargo fuzz run path_and_uri -- -max_total_time=60
```

Expected: all frozen cases and properties pass; the fuzz target exits without panic or sanitizer finding.

- [ ] **Step 5: Commit portable path/navigation parity**

```powershell
rtk git add crates/marknexia-core crates/marknexia-files crates/marknexia-navigation
rtk git commit -m "feat: prove Rust path and navigation parity"
```

### Task 4: Select Markdown, Sanitizer, and Highlighting Dependencies by Evidence

**Files:**
- Create: `crates/marknexia-markdown/src/{lib.rs,engine.rs,comrak_adapter.rs,pulldown_adapter.rs,extensions.rs}`
- Create: `crates/marknexia-markdown/tests/parity_v1.rs`
- Create: `crates/marknexia-security/src/{lib.rs,html_policy.rs,url_policy.rs,svg_policy.rs}`
- Create: `crates/marknexia-security/tests/{parity_v1.rs,hostile_properties.rs}`
- Create: `crates/marknexia-rendering/src/{lib.rs,template.rs,limits.rs}`
- Create: `crates/marknexia-rendering/tests/parity_v1.rs`
- Create: `fuzz/fuzz_targets/markdown_and_html.rs`
- Create: `docs/superpowers/specs/2026-09-12-rust-content-dependency-decision.md`

**Interfaces:**
- Produces: `MarkdownEngine::parse`, `HtmlPolicy::sanitize_fragment`, `SvgPolicy::sanitize`, and `Renderer::render` behind crate-owned traits.
- Consumes: frozen Markdown, heading, sanitizer, and rendering fixtures.

```rust
pub trait MarkdownEngine {
    fn parse(&self, source: &str, options: &MarkdownOptions)
        -> Result<ParsedDocument, Vec<Diagnostic>>;
}

pub trait HtmlPolicy {
    fn sanitize_fragment(&self, html: &str) -> SanitizedFragment;
}
```

- [ ] **Step 1: Make every candidate run the same mandatory fixture suite**

Create Cargo features `candidate-comrak` and `candidate-pulldown`. The test matrix must expose differences rather than maintaining candidate-specific expected files.

- [ ] **Step 2: Implement only the Marknexia-owned compatibility extensions required by evidence**

Cover Markdig grid tables, mathematics, emphasis extras, GitHub alerts, heading IDs, source diagnostics, Mermaid isolation, and agreed syntax labels. Keep extensions outside third-party adapter modules.

- [ ] **Step 3: Evaluate `ammonia` as a base sanitizer**

Prove explicit HTML/SVG element and attribute allowlists, `data-marknexia-*` command metadata, URL/CSS policy, relative links, remote image policy, event removal, protocol handling, and bounded output. Rust memory safety is not accepted as content-safety evidence.

- [ ] **Step 4: Measure compatibility, performance, binary contribution, licenses, and advisories**

Run both candidates over the entire fixture corpus and repeated small/large documents. Record mismatches, median/tail parse-render time, release binary delta, transitive packages, maintenance status, licenses, and advisories.

- [ ] **Step 5: Write and review the dependency decision**

Select one parser and one highlighting approach only when mandatory fixtures pass or a specific observable difference is approved in `compat/decisions/`. Reject a candidate with an unresolved sanitizer bypass, unbounded allocation path, incompatible license, or high/critical advisory.

- [ ] **Step 6: Verify and commit the selected content stack**

```powershell
rtk proxy cargo test -p marknexia-markdown -p marknexia-security -p marknexia-rendering --all-features --locked
rtk proxy cargo fuzz run markdown_and_html -- -max_total_time=300
rtk proxy cargo deny check advisories bans licenses sources
rtk git add crates/marknexia-markdown crates/marknexia-security crates/marknexia-rendering compat/decisions docs/superpowers/specs/2026-09-12-rust-content-dependency-decision.md Cargo.lock
rtk git commit -m "docs: select Rust content stack from parity evidence"
```

### Task 5: Prove Native WebView2 COM Lifecycle and Security Boundary

**Files:**
- Create: `crates/marknexia-webview/src/{lib.rs,environment.rs,host.rs,broker.rs,protocol.rs,recovery.rs}`
- Create: `crates/marknexia-webview/tests/{broker.rs,protocol.rs,recovery.rs}`
- Create: `crates/marknexia-webview/assets/{probe.html,probe.css,probe-image.png}`
- Create: `crates/marknexia-webview/examples/webview_probe.rs`
- Create: `docs/superpowers/specs/2026-09-12-rust-webview2-feasibility.md`

**Interfaces:**
- Produces: safe `WebViewEnvironment`, `WebViewHost`, `ResourceBroker`, typed `PageToHost`/`HostToPage`, `RecoveryCoordinator`, and event-token ownership.
- Consumes: an HWND supplied by the Win32 layer; it does not own application business rules.

```rust
#[derive(serde::Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase", deny_unknown_fields)]
pub enum PageToHost {
    Ready { protocol: u16, document_epoch: u64 },
    OpenLink { href: String },
    CopyText { text: String },
    FocusChanged { focused: bool },
}

pub trait ResourceBroker {
    fn resolve(&self, request: &BrokerRequest) -> BrokerDecision;
}
```

- [ ] **Step 1: Add failing pure tests for broker, protocol, and recovery state**

Cover GET-only handling, same-tab origin authority, cross-tab denial, traversal, external schemes, remote network denial, 64 KiB message cap, wrong protocol, wrong source/origin, unknown fields/types, stale document epochs, duplicate renderer failure, browser exit, and close-during-callback.

- [ ] **Step 2: Implement the private COM projection and lifecycle**

Initialize COM as STA before HWND/WebView creation. Use one shared environment, explicit controller bounds/visibility/focus, checked HRESULTs, retained event tokens, handler removal before controller close, and callbacks retaining only `Weak` application state. Never hold a Rust mutable borrow across a COM call or Windows message dispatch.

- [ ] **Step 3: Implement the broker and typed bridge**

Use controlled HTTPS virtual origins, deferrals for resource requests, bounded in-memory responses, fixed headers without request-controlled CR/LF, and deny-by-default routing modeled on `MainWindow.Resources.cs`. Parse JSON into strict tagged enums and serialize all outbound messages.

- [ ] **Step 4: Implement renderer/browser recovery as a pure state machine plus adapter**

Renderer failure recreates one host. Browser-process failure deduplicates notifications, closes all controllers, waits for exit, recreates the environment/controllers, and restores active tab identity and immutable probe content exactly once.

- [ ] **Step 5: Exercise the native probe**

Run the probe with two controllers, resize/focus cycles, link/copy round trips, blocked cross-origin requests, explicit controller teardown, renderer crash, and browser-process exit. The shell must remain usable when Evergreen is absent and expose the bootstrap action without downloading automatically.

- [ ] **Step 6: Verify and commit WebView2 feasibility**

```powershell
rtk proxy cargo test -p marknexia-webview --locked
rtk proxy cargo clippy -p marknexia-webview --all-targets -- -D warnings
rtk proxy cargo build -p marknexia-webview --example webview_probe --release --target x86_64-pc-windows-msvc --locked
rtk proxy cargo build -p marknexia-webview --example webview_probe --release --target aarch64-pc-windows-msvc --locked
rtk git add crates/marknexia-webview docs/superpowers/specs/2026-09-12-rust-webview2-feasibility.md Cargo.lock
rtk git commit -m "feat: prove native WebView2 security lifecycle"
```

### Task 6: Prove Raw Win32 Tabs, DPI, Theme, Keyboard, and UI Automation

**Files:**
- Create: `crates/marknexia-win32/src/{main.rs,app.rs,window.rs,layout.rs,theme.rs,tabs.rs,keyboard.rs,accessibility.rs}`
- Create: `crates/marknexia-win32/tests/{layout.rs,tabs.rs,keyboard.rs,automation_smoke.rs}`
- Create: `scripts/Test-RustNativeShell.ps1`
- Create: `docs/superpowers/specs/2026-09-12-rust-win32-shell-feasibility.md`

**Interfaces:**
- Produces: `AppState`, `TabStore`, `TabId`, `ShellLayout`, `Theme`, and a server-side UIA provider for the custom tab strip.
- Consumes: safe `marknexia-webview` interfaces; no COM interface escapes into portable state.

- [ ] **Step 1: Add failing layout, tabs, keyboard, and accessibility tests**

Assert 44-DIP minimum targets at 96/144/192 DPI, selected/closed/reordered tab invariants, Ctrl+Tab/Ctrl+Shift+Tab/Ctrl+W/F6 behavior, visible focus, logical tab order, and UIA selection semantics.

- [ ] **Step 2: Implement the raw Win32 window and layout**

Use `PER_MONITOR_AWARE_V2`, a checked message loop, native window chrome, Win32 controls where suitable, and owner drawing only for the tab strip. Store one `Box<AppState>` pointer in `GWLP_USERDATA` during `WM_NCCREATE`, clear it during `WM_NCDESTROY`, and never dereference it afterward. Apply the suggested physical rectangle on `WM_DPICHANGED`.

- [ ] **Step 3: Implement system, light, dark, and high-contrast behavior**

High contrast uses system colors rather than branded low-contrast overrides. Recompute brushes, metrics, hit regions, and WebView bounds after DPI/theme changes. Respect reduced-motion preference by avoiding nonessential shell animation.

- [ ] **Step 4: Implement keyboard/focus routing and custom tabs**

Support selection, close, reorder, pointer hit testing, Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+W, arrow navigation, and F6 focus cycling. Route WebView accelerators without trapping system shortcuts.

- [ ] **Step 5: Implement server-side UI Automation for the custom tab strip**

Implement `IRawElementProviderSimple`, `IRawElementProviderFragment`, `IRawElementProviderFragmentRoot`, `ISelectionProvider`, and `ISelectionItemProvider`. Handle `WM_GETOBJECT`/`UiaRootObjectId` with `UiaReturnRawElementProvider`; expose a named `TabControl` and named `TabItem` children. Providers hold weak state and return no stale children after destruction.

- [ ] **Step 6: Run native smoke and UIA tests**

`automation_smoke` launches the shell, calls `IUIAutomation::ElementFromHandle`, asserts two named tab items and selection pattern support, invokes `Select`, and observes the active WebView change. A default HWND proxy without custom tab children is a failure.

- [ ] **Step 7: Verify and commit the shell proof**

```powershell
rtk proxy cargo test -p marknexia-win32 --locked
rtk proxy cargo clippy -p marknexia-win32 --all-targets -- -D warnings
rtk proxy cargo build -p marknexia-win32 --release --target x86_64-pc-windows-msvc --locked
rtk proxy cargo build -p marknexia-win32 --release --target aarch64-pc-windows-msvc --locked
rtk proxy pwsh -NoProfile -File scripts/Test-RustNativeShell.ps1 -Architecture x64
rtk git add crates/marknexia-win32 scripts/Test-RustNativeShell.ps1 docs/superpowers/specs/2026-09-12-rust-win32-shell-feasibility.md Cargo.lock
rtk git commit -m "feat: prove accessible raw Win32 shell"
```

### Task 7: Measure Release Feasibility and Decide Loader/Update Contracts

**Files:**
- Create: `scripts/Measure-RustArtifacts.ps1`
- Create: `scripts/Measure-RustDesktopPerf.ps1`
- Create: `scripts/Test-RustArchitecture.ps1`
- Create: `tests/perf/rust-feasibility.schema.json`
- Create: `tests/perf/scenarios/{empty-shell,small-document,large-document,repository-scan}.json`
- Create: `packaging/webview/README.md`
- Create: `docs/superpowers/specs/2026-09-12-rust-update-contract-decision.md`
- Modify: `.github/workflows/rust-feasibility.yml`

**Interfaces:**
- Produces: machine-readable measurements separating host and WebView2 processes; a pinned WebView2 loader/bootstrap policy; `update-manifest-v1` and post-update health/rollback contracts.
- Consumes: Tasks 4–6 release binaries and the current .NET build as a comparative baseline.

- [ ] **Step 1: Add failing validation tests for evidence completeness**

Reject measurements missing hardware, OS build, architecture, commit, fixture digest, run count, WebView residency, median, p95, host private bytes, or separately reported WebView2 memory.

- [ ] **Step 2: Implement repeatable artifact and performance measurement**

Record 30 cold and 30 warm runs for shell-visible time, WebView-ready time, first render, host private working set, WebView2 working set, and binary/assets/unpacked sizes. Do not combine host and WebView2 memory when evaluating the host gate.

- [ ] **Step 3: Verify reproducible unsigned builds**

Set `SOURCE_DATE_EPOCH`, disable incremental compilation, enable `/Brepro`, build twice from clean copies, and compare hashes before signing. Authenticode timestamps are intentionally outside the byte-for-byte comparison.

- [ ] **Step 4: Prove both architecture boundaries**

Compile x64 and ARM64 in CI. Execute shell, WebView2, UIA, installer-free portable launch, and measurements on native x64 and native ARM64 Windows. Static PE validation is recorded separately and cannot satisfy runtime gates.

- [ ] **Step 5: Decide the WebView2 distribution contract**

Package only the architecture-matched `WebView2Loader.dll` from a pinned, hash-verified Microsoft source. Detect Evergreen through the native loader API; keep the shell usable when absent; offer Microsoft's Evergreen bootstrapper only after user action; test offline/bootstrap failure. Do not bundle fixed WebView2 or Chromium.

- [ ] **Step 6: Decide, but do not deploy, the signed update contract**

Define immutable `update-manifest-v1.json` fields for version, architecture, URL, SHA-256, byte length, minimum OS, and artifact identity. Specify Ed25519 manifest verification with an embedded public key, Authenticode verification, versioned side-by-side payload directories, a nonce-bound 60-second `--post-update-health` acknowledgement after shell/WebView/offline-render success, rollback on failure, and complete separation from Store/MSIX updates. Key creation and custody require a separate security approval.

- [ ] **Step 7: Evaluate the quantitative gates**

Feasibility targets are: installer projection excluding on-demand WebView2 download ≤15 MB; unpacked Marknexia payload ≤35 MB; cold interactive shell p95 ≤500 ms on declared hardware; idle host private working set before document open ≤35 MB. If the vertical slice already makes a target structurally infeasible, stop and quantify the responsible binaries/assets.

- [ ] **Step 8: Verify and commit measurement contracts**

```powershell
rtk proxy pwsh -NoProfile -File scripts/Measure-RustArtifacts.ps1 -Configuration Release
rtk proxy pwsh -NoProfile -File scripts/Measure-RustDesktopPerf.ps1 -Scenario tests/perf/scenarios/empty-shell.json -Runs 30
rtk proxy pwsh -NoProfile -File scripts/Test-RustArchitecture.ps1 -Expected x64
rtk git add scripts/Measure-RustArtifacts.ps1 scripts/Measure-RustDesktopPerf.ps1 scripts/Test-RustArchitecture.ps1 tests/perf packaging/webview docs/superpowers/specs/2026-09-12-rust-update-contract-decision.md .github/workflows/rust-feasibility.yml
rtk git commit -m "test: measure Rust release feasibility"
```

### Task 8: Produce the Reviewed Go/No-Go Evidence Package

**Files:**
- Create: `docs/superpowers/reports/2026-09-12-rust-win32-feasibility-report.md`
- Create: `artifacts/rust-feasibility/manifest.json`
- Modify: `docs/superpowers/plans/2026-09-12-rust-win32-feasibility.md`

**Interfaces:**
- Consumes: all lane test outputs, fixture digests, dependency decision, native demonstrations, architecture evidence, measurements, and risk records.
- Produces: one of `GO`, `CONDITIONAL-GO`, or `NO-GO`, with every condition tied to an owner, cost range, and release gate.

- [ ] **Step 1: Run the complete feasibility gate from a clean checkout**

```powershell
rtk dotnet test Marknexia.slnx --configuration Release --no-restore
rtk proxy cargo fmt --all -- --check
rtk proxy cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk proxy cargo test --workspace --all-features --locked
rtk proxy cargo deny check advisories bans licenses sources
rtk proxy cargo build --workspace --release --target x86_64-pc-windows-msvc --locked
rtk proxy cargo build --workspace --release --target aarch64-pc-windows-msvc --locked
rtk proxy pwsh -NoProfile -File scripts/Test-RustNativeShell.ps1 -Architecture x64
rtk git diff --check
```

- [ ] **Step 2: Validate native ARM64 evidence**

Attach the native ARM64 run identifier and results for shell, WebView2, UIA, teardown/recovery, and performance. If native ARM64 infrastructure is unavailable, the report cannot say `GO`.

- [ ] **Step 3: Audit every approved design gate**

For each feasibility-relevant requirement in the approved design, link the exact fixture, test, native run, measurement, or decision record. Mark unavailable production-only gates as deferred to the full rewrite plan, not passed.

- [ ] **Step 4: Apply stop conditions**

Return `NO-GO` or `CONDITIONAL-GO` if WebView2 callbacks/linking are unstable on either architecture, UIA provider lifetimes cannot be made deterministic, browser recovery duplicates or loses tab state, sanitizer parity has an unresolved bypass, mandatory content behavior would need silent removal, or a size/performance target is structurally infeasible.

- [ ] **Step 5: Review and commit the report**

```powershell
rtk git add docs/superpowers/reports/2026-09-12-rust-win32-feasibility-report.md artifacts/rust-feasibility/manifest.json docs/superpowers/plans/2026-09-12-rust-win32-feasibility.md
rtk git commit -m "docs: record Rust Win32 feasibility decision"
```

## Feasibility Exit Criteria

- Frozen versioned fixtures reproduce current Markdown, headings, navigation, sanitization, rendering, settings, and update/archive behavior.
- Candidate parser, sanitizer, and highlighting choices have reviewed parity, security, license, maintenance, binary-size, and performance evidence.
- A Rust/raw-Win32 executable hosts native WebView2 COM without WinUI/XAML/.NET and demonstrates two tabs, strict brokered resources, typed messages, explicit teardown, and renderer/browser recovery.
- DPI changes, light/dark/high-contrast behavior, keyboard focus, custom tabs, and UI Automation work in native tests.
- x64 and ARM64 compile; both execute on native Windows evidence hosts.
- Size/startup/memory results use declared hardware and distinguish host from WebView2 processes.
- WebView2 loader/bootstrap and signed update/health/rollback contracts are explicit and reviewed.
- The .NET release pipeline still passes and no Rust artifact has replaced or been published as the production edition.

## Estimated Schedule

- Week 1: Task 1 and Task 2.
- Weeks 2–3: Lane A Tasks 3–4; Lane B Task 5; Lane C Task 7 measurement scaffolding in parallel.
- Weeks 3–4: Lane B Task 6; Lane A mismatch remediation; Lane C x64/ARM64 evidence.
- Week 5: integrated gates and Task 8 report.
- Week 6 reserve: only measured parity, COM/UIA, ARM64, or quantitative-gate remediation.

Recommended staffing is two senior engineers with complementary Rust/Windows skills plus scheduled native ARM64 QA access. A single engineer can execute the same dependency order, but the likely calendar duration is six to nine weeks rather than five to six.
