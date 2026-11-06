# Rust/Win32 Feasibility Report

Status: **evidence complete for owner review — decision: CONDITIONAL-GO**
Branch: `ramana_rust_win32_feasibility` (pushed for CI only; not merged to `main`, no release).
Evidence commit `b7f0dce`, CI run [42146193618](https://github.com/ramanacr/marknexia/actions/runs/42146193618), all seven jobs green. Index: `artifacts/rust-feasibility/manifest.json`.

This report closes Task 8 of `docs/superpowers/plans/2026-09-12-rust-win32-feasibility.md`. It answers one question: can Marknexia be rewritten as a Rust raw-Win32 shell with WebView2 used only for the document viewport, and keep its behaviour, security and size goals? Evidence classes are kept separate:

- **implemented** — code exists;
- **source-reviewed** — an independent reviewer read it;
- **compiled** — built in CI for the stated target;
- **native-tested** — executed on a real Windows desktop of the stated architecture.

## 1. Decision

**CONDITIONAL-GO.** No stop condition from Task 8 Step 4 is triggered:

| Stop condition | Result | Evidence |
|---|---|---|
| WebView2 callbacks or linking unstable on either architecture | Not triggered | `webview-environment`, `webview-session`, `webview-browser-exit-recovery` pass natively on x64 and ARM64 (§3) |
| UIA provider lifetimes not deterministic | Not triggered | UIA smoke and native accessibility gates pass on x64 and ARM64; provider holds weak state (§4) |
| Browser recovery duplicates or loses tab state | Not triggered | `native_browser_exit_recreates_environment_and_immutable_tabs` passes natively on x64 and ARM64 |
| Sanitizer parity has an unresolved bypass | Not triggered | All review findings closed; fuzzing 300 s clean; differences recorded in `compat/decisions/sanitizer-ammonia-differences.md` |
| Mandatory content behaviour would need silent removal | Not triggered | All differences are written decisions awaiting approval (§6) |
| A size or performance target is structurally infeasible | Not triggered | All four targets met. Size and host memory have large margins; ARM64 cold start on a hosted runner is close to its limit, so condition C3 applies (§5) |

The decision is conditional because the items in §7 must be closed or accepted by the owner before the production plan starts. None of them is a technical blocker.

## 2. Gate run (Task 8 Step 1)

All commands ran in GitHub Actions on the evidence commit (`.github/workflows/rust-feasibility.yml`):

| Gate | Result |
|---|---|
| `.NET Release preservation` (`dotnet test` on the current solution) | pass — the .NET edition stays releasable |
| `cargo fmt --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -D warnings` | pass |
| `cargo test --workspace --locked` (default and all features, doctests included) | pass |
| `cargo deny check advisories bans licenses sources` | pass (MPL-2.0 allowed by owner decision 2026-09-26; no other licence change) |
| Release build x64 and ARM64, built twice with `/Brepro` | pass; byte-identical unsigned binaries on both architectures |
| Portable dependency boundary | pass (no Win32/COM crate in portable crates) |
| Fuzz `markdown_and_html`, 300 s, `--cfg ammonia_unstable` | pass, no crash |
| Native gates x64 and ARM64 (`scripts/Test-RustNativeGates.ps1`) | see §3 |

Local runs used `CARGO_TARGET_DIR=C:\marknexia_builds\<worktree>`. No synthetic or malformed PE bytes are written by any test or script (`tests/perf/no_synthetic_pe_disk.tests.ps1`).

## 3. Native evidence by architecture (Task 8 Step 2)

Hosts: x64 = GitHub `windows-latest` (AMD EPYC 7763, 4 vCPU, Windows Server 2025 build 26100). ARM64 = GitHub `windows-11-arm` (Microsoft Cobalt 100, 4 vCPU, Windows 11 Enterprise build 26200). Both are native; nothing is emulated.

| Native gate | x64 | ARM64 |
|---|---|---|
| WebView2 environment, two-tab session, brokered resources, typed messages | native-tested, pass | native-tested, pass |
| Browser-process exit recovery with immutable tabs restored | native-tested, pass | native-tested, pass |
| Shell visible contract | native-tested, pass | native-tested, pass |
| UIA smoke: named TabControl, two named TabItems, `Select` changes active view, Ctrl+Tab from inside the WebView document | native-tested, pass | native-tested, pass |
| Per-monitor-V2 DPI and 44-DIP targets; UIA structure event on tab close; palette vs. current high-contrast state | native-tested, pass (DPI 96) | native-tested, pass (DPI 96) |
| Desktop performance, 30 cold + 30 warm | native-tested | native-tested |

The keyboard-driven UIA gates need the shell to own the foreground before any synthetic input. On the hosted `windows-11-arm` image, a system "Microsoft account" prompt (`WWAHost.exe`) and then the Search flyout (`SearchHost.exe`) held the foreground in several runs (42104089745 to 42113919802). The tests now dismiss those system surfaces on GitHub Actions only, never on a developer desktop, and still refuse to send any key unless the shell owns the foreground. Each refused activation names the foreground window and executable. Gate retries are bounded to three, and every failed attempt stays in the gate summary.

## 4. Shell, accessibility and keyboard (Task 6)

- **Implemented, source-reviewed, native-tested (x64 and ARM64):** raw Win32 window with `PER_MONITOR_AWARE_V2`; owner-drawn tab strip; server-side UIA provider (`IRawElementProviderSimple`/`Fragment`/`FragmentRoot`, `ISelectionProvider`, `ISelectionItemProvider`) with weak state; `WM_GETOBJECT` root; structure-changed events on tab close; Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+W and F6 routing, including shell chords from inside WebView2 via `AcceleratorKeyPressed`; status-bar errors; shell usable without WebView2.
- **UIA `Select`:** WebView2 rejects COM inside the input-synchronous UIA call (`0x802A000C`), so the view switch is deferred through `WM_APPLY_WEBVIEW_SELECTION`. This is native-tested.
- **Deferred to the production plan:** toggling high contrast at runtime (doing so changes a system setting, which tests must not do); `WM_DPICHANGED` across two monitors with different DPI (runners have one monitor). Palette mapping and DPI layout at 96/144/192 are unit-tested.

## 5. Measurements (Task 7)

Empty-shell scenario, 30 cold + 30 warm runs per architecture, host and WebView2 memory measured separately (`scripts/Collect-RustDesktopPerf.ps1`):

Values are from the evidence run in the manifest (run 42146193618, commit b7f0dce):

| Metric (p95 unless noted) | x64 | ARM64 | Target |
|---|---|---|---|
| Cold shell visible | 151 ms (median 139) | 498 ms (median 421) | ≤ 500 ms |
| Warm shell visible | 152 ms | 434 ms (median 207) | — |
| WebView2 ready, cold | 1,008 ms | 938 ms | reported |
| First render, cold / warm | 3,253 / 677 ms | 4,085 / 1,125 ms | reported |
| Host private bytes, idle | 3.7 MB | 4.2 MB | ≤ 35 MB |
| WebView2 working set (separate processes) | 335 MB | 376 MB | reported separately, not part of the host gate |
| Unpacked payload (exe + architecture-matched `WebView2Loader.dll`) | 3.18 MB | 2.92 MB | ≤ 35 MB |
| Installer projection, excluding on-demand WebView2 | ≤ unpacked size (no installer built in feasibility) | same | ≤ 15 MB |

**Hosted ARM64 timing is noisy.** The same ARM64 binary (SHA-256 prefix `bf699577`) measured a cold shell-visible median of 71 ms and p95 of 81 ms in run 42099293588, and a median of 192 ms and p95 of 498 ms in run 42113919802. The binary did not change between those runs, so the difference comes from the runner, for example the image's sign-in prompt and Search host competing for CPU. Every run met the 500 ms target, but the hosted-runner margin on ARM64 is too thin to rely on. Condition C3 requires the measurement to be repeated on a declared, quiet ARM64 reference host. x64 measured between 151 and 199 ms p95 across runs.

Content scale (release, developer workstation; `compat/decisions/rendering-differences.md` REND-1 and `sanitizer-ammonia-differences.md` SAN-8): the Rust stack accepts the .NET limits (50 MiB source, 128 MiB page). At 16 MiB it renders 7–30× faster than .NET; .NET timed out at 50 MiB, the Rust stack renders typical 50 MiB documents in 3–8 s and fails closed (`TooComplex`, `InputTooLarge`) on pathological shapes. Peak heap figures are recorded per shape in REND-1.

WebView2 distribution (`packaging/webview/README.md`): the executable imports `WebView2Loader.dll` at load time. CI stages only the pinned, hash-verified, architecture-matched loader from Microsoft's NuGet package. No fixed runtime or Chromium is bundled.

Update contract (`docs/superpowers/specs/2026-09-12-rust-update-contract-decision.md`): defined, not deployed. Key creation and custody need a separate security approval.

## 6. Design gate audit (Task 8 Step 3)

| Requirement (design spec) | Evidence | Status |
|---|---|---|
| GFM Markdown parity, heading slugs | `compat/fixtures/v1/{markdown,headings}`, `crates/marknexia-markdown/tests/parity_v1.rs`; pulldown-cmark selected (owner decision 2026-09-28) | native-independent; passes |
| Relative, root, cross-document, external links; traversal prevention | `compat/fixtures/v1/navigation` (13 oracle-exported cases), `crates/marknexia-navigation/tests/parity_v1.rs`, path property tests | passes; NAV decisions proposed |
| Sanitizer protection equal or stricter | `compat/fixtures/v1/sanitizer`, `hostile_properties.rs`, `chunking_differential.rs`, fuzz | passes; SAN decisions proposed |
| Rendering, code-copy metadata, Mermaid shell, CSP template | `compat/fixtures/v1/rendering` 7/7 byte-exact | passes; REND decisions proposed |
| Strict brokered resources, typed messages, navigation cancel | `marknexia-webview` policy and session tests, native | passes |
| Renderer/browser crash recovery | native recovery gate | passes x64 and ARM64 |
| WebView2 missing keeps shell usable | `StartupFailed` signal, status-bar error, unit tests | implemented; Evergreen bootstrap UI deferred |
| DPI, theme, contrast, keyboard, UIA | §4 | passes x64 and ARM64 |
| Opening files from UNC shares | Owner decision 2026-09-26 overrides plan Task 3 "reject UNC"; identification-only QoS, disk-type and final-path checks; pipe, mailslot, `IPC$`, `DavWWWRoot`, device and non-ASCII share names refused | implemented, reviewed |
| x64 and ARM64 | §3 | compiled and native-tested |
| Size, startup, host memory | §5 | met |
| Reproducible unsigned builds | CI reproducibility report | passes both architectures |
| Settings migration, update/rollback, SBOM, installer, MSIX | Frozen fixtures for settings and update archives exist; no Rust port yet | **deferred to production plan** (not passed) |
| Search, repository browsing, drag-and-drop, associations, recent documents | Not in feasibility scope | **deferred to production plan** |

## 7. Conditions for the production plan

Each condition names an owner, an estimated cost and the release gate it blocks.

| # | Condition | Owner | Cost | Blocks |
|---|---|---|---|---|
| C1 | Approve or reject the proposed compatibility decisions: `markdown-resource-limits.md`, `markdown-source-diagnostics.md`, `navigation-probe-order.md`, `rendering-differences.md`, `sanitizer-ammonia-differences.md`, and the highlighting recommendation in the content dependency decision | Product owner | 0.5–1 day review | "Agreed compatibility suite 100%" |
| C2 | Highlighting: the Marknexia-owned tokenizer covers C# only (what the fixtures need). Decide the language set and port the rest of ColorCode's languages or accept a reduced set | Product owner + engineering | 1–3 weeks | Compatibility suite |
| C3 | Repeat the native gates and the 30 cold + 30 warm startup measurement on declared, quiet ARM64 and x64 reference hosts (not only hosted runners), including a high-DPI monitor. The hosted ARM64 cold p95 ranged from 81 to 498 ms with one identical binary. | Engineering / QA | 1–2 days | Cold start ≤ 500 ms on reference hardware; accessibility-critical failures = 0 |
| C4 | Runtime high-contrast toggle and multi-monitor `WM_DPICHANGED` verified on a manual evidence host | Engineering / QA | 1 day | Accessibility gate |
| C5 | Update signing key creation and custody approval before any updater is implemented | Security owner | separate approval | Tampered updates accepted = 0 |
| C6 | Port settings migration, repository browsing, search, activation, packaging (setup, portable ZIP, MSIX) and SBOM in the production plan | Engineering | production plan estimate | Release gates §13 of the design |

## 8. Review history

Every lane was reviewed by an independent reviewer before integration, and every finding was fixed or recorded. The final rounds covered the 128 MiB content scale (BOM at chunk boundaries, node budgets, streamed join, ammonia 4.2.4 upgrade), the UNC path gate (dot segments, trailing dots and spaces, pipe/mailslot/`IPC$`/`DavWWWRoot`, non-ASCII names, final-path validation), WebView2 callback panic containment, and the single startup decision (`FirstRender` xor `StartupFailed`).

CI fuzzing found one more sanitizer defect late in the round (run 42115856878). Leading characters that the parser drops, such as NUL, could leave U+FEFF as the first output character, and a second pass then discarded it as a byte order mark, so sanitizing was not a fixed point. The serializer now writes that one character as `&#xFEFF;` (SAN-11). A regression test covers the exact crash input and related control-character shapes, and the generated corpora were extended.

Commits in this repository are unsigned: `commit.gpgsign` is set, but no GPG installation or signing key exists on the evidence workstation. This matches the existing history of `main`.
