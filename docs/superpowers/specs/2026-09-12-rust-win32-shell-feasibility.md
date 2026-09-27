# Rust/raw-Win32 shell feasibility evidence (Task 6, in progress)

This records Task 6 evidence for
`docs/superpowers/plans/2026-09-12-rust-win32-feasibility.md`. It is not a
production-readiness claim and does not replace the .NET edition. Evidence
states are kept separate: *source-reviewed*, *compiled*, *headless-tested*,
*native-tested (x64)*, and *not yet verified*.

Host for native evidence, 2026-09-26: Windows 11 Pro 10.0.26200, x64, WebView2
Evergreen 153.0.4234.32, system dark app theme. No ARM64 hardware was
available.

## Native-tested on x64 (2026-09-26)

- **Window and lifecycle.** A release x64 `marknexia-win32.exe` shows a
  top-level window with the custom `MarknexiaRustTabStrip` child
  (`scripts/Test-RustNativeShell.ps1` passed) and exits cleanly on WM_CLOSE.
  One boxed `Rc<RefCell<AppState>>` lives in `GWLP_USERDATA` from
  `WM_NCCREATE` to `WM_NCDESTROY`. No `RefCell` borrow spans a reentrant
  Win32, WebView2 or UIA call.
- **UI Automation.** `tests/automation_smoke.rs` passes 4/4 runs against the
  shell process it launched:
  - It binds the HWND by PID through `EnumWindows`.
  - It finds the server-side `TabControl` provider and two named `TabItem`
    children. The default HWND proxy is not accepted.
  - It invokes `SelectionItem.Select` and waits until exactly one on-screen
  WebView document is visible and its name equals the selected tab, for two
  consecutive selections.
  - Providers use ABI-local `IRawElementProviderSimple`, `Fragment` and
  `FragmentRoot` interfaces. All three derive from IUnknown, as in
  UIAutomationCore.h. Every nullable out pointer is written before return.
- **Select acknowledgement contract.** WebView2 rejects outgoing COM with
  `0x802A000C` while UIA's input-synchronous `Select` call is on the stack,
  and this was observed natively. `Select` therefore returns once portable
  selection, the repainted tab strip and the provider state agree. The
  WebView switch is posted and applied on the next message-loop turn. If that
  switch fails, selection reverts to the tab the WebView still shows, and the
  matching UIA events are raised. Pointer and keyboard selection stay fully
  synchronous.
- **WebView2 lifecycle.** All six ignored native WebView2 tests pass:
  - loader detection
  - STA environment creation
  - independent bounds and visibility for two controllers
  - page messages delivered only after `poll`
  - resource, message, renderer and teardown contract
  - browser-process exit with the environment and immutable tabs recreated
  The browser-exit test was run with an automated kill of only the WebView2
  browser process that the test itself started.
- **Theme.** A native capture confirmed the system-dark rendering:
  - dark DWM title bar and Metallic Radium tab strip
  - dark `STATIC`, `LISTBOX` and `EDIT` controls, set through `WM_CTLCOLOR*`
    and the `DarkMode_*` visual styles
  - an owner-drawn command button
  - the document canvas following `color-scheme: light dark`
  Light and high-contrast themes restore the default styles and system
  colors. The high-contrast path has not been captured natively yet.
- **Keyboard handoff.** WebView2 `AcceleratorKeyPressed` routes only the
  shell-owned chords (Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+W and F6 or Shift+F6)
  back to the shell. They are drained after dispatch, outside COM, and F6
  into Document calls `MoveFocus` on the active controller.
  - The native smoke step focuses the visible WebView document through UIA,
    confirms that the launched shell owns the foreground, and injects
    Ctrl+Tab with `SendInput`. It then observes the other tab's document
    becoming the only visible one. It passed on 2026-09-26.
  - The step refuses to inject keys unless the shell owns the foreground.
- **Startup milestones.** The shell signals the named events
  `Local\Marknexia.WebViewReady.<pid>` and `Local\Marknexia.FirstRender.<pid>`.
  `scripts/Collect-RustDesktopPerf.ps1` consumes them for the empty-shell,
  small-document and large-document scenarios. The window is shown
  before WebView2 startup begins. The trial numbers so far came from a heavily
  loaded machine and are not evidence.

## Documents: open, render and host (native-tested on x64, 2026-09-27)

- **Opening.** `marknexia-win32.exe <file.md> [...]` opens each path in its
  own tab titled with the file name; the placeholder tabs appear only when no
  path is given. The "Open Markdown" button runs `IFileOpenDialog`
  (multi-select; `*.md;*.markdown;*.txt`, Markdown and Text filters) from a
  posted message, with no `AppState` borrow across its modal loop. Reads
  follow .NET `FileService`: the file length goes through the renderer's
  `check_source_size` (50 MiB) before reading, `BoundedReader` bounds the read
  by the same limit, and bytes decode like `DecodeBytes` (UTF-8 BOM stripped,
  UTF-16 LE/BE BOMs, the .NET UTF-32 quirks, strict UTF-8, Latin-1 fallback).
- **Off-thread rendering.** A worker thread (8 MiB stack) draws a
  `PageIdentity` from `BCryptGenRandom` (system-preferred RNG, all-zero draws
  rejected), reads and renders, sends the `Send` result on a channel and
  posts `WM_APP+2`. The UI thread drains the channel, builds the
  `HostDocument` and adds it to the session; the worker never touches HWND
  state, `AppState` or WebView2. A per-tab ticket makes results stale when
  the tab closes (or re-renders) first, so an in-flight render is discarded.
  While pending the tab has no controller (`WebViewSession::clear_selection`
  hides all) and the status bar reads "Rendering <name>…". A failed read or
  render shows a plain-text error document and the error in the status bar.
- **Sealed hosting.** `HostDocument::from_rendered(tab, epoch, title,
  &RenderedDocument, &PageIdentity)` hosts the rendering crate's own page
  (`page_html()`): the .NET template with its inline bundled CSS and
  nonce'd inline bridge script around the `SanitizedFragment` body. Only
  `marknexia-rendering` can construct a `RenderedDocument`, so no raw HTML
  constructor is reopened; the host adds one entity-encoded `<title>` line
  at a fixed offset in the trusted template head. Fields stay private and
  the page bytes are shared immutably (`Arc<[u8]>`).
  - The document is served through the existing broker from the page
    identity's own origin (`https://document-<id>.marknexia.viewer/document`),
    so `<base href>`, `base-uri` and `img-src` name the serving origin.
  - The document response's `Content-Security-Policy` header is the page's
    meta policy, rebuilt from the identity (nonce and origin) and checked
    against the page head; a page rendered for another identity, or a
    template change, fails closed with `IdentityMismatch`. Other responses
    keep the default `'self'`-only header policy.
  - No separate CSS/JS assets are served: the rendering template inlines
    them under its nonce CSP. A header of `style-src 'self'; script-src
    'self'` would have blocked exactly those inline elements. The Mermaid
    runtime reference (`https://marknexia.assets/mermaid.min.js`) and every
    local image are denied by the broker, so diagrams show their source
    text and images show the bridge's "Image unavailable" fallback.
  - Bridge: `bridge.js` posts untyped `{type, href}` / `{type, text}` objects
    without protocol, tab or epoch, which the strict page-to-host protocol
    rejects (`InvalidPayload`, tested). The bridge is therefore limited to
    in-page behaviour: link clicks are cancelled and do nothing, copy does
    nothing, in-page anchor links do not scroll; diagram zoom and source
    toggles stay page-local (not exercised natively). Navigation stays
    pinned to the document URI.
- **Readiness.** With document paths, `FirstRender` fires when the first
  rendered Markdown document's navigation completes (placeholders and error
  documents never count); `WebViewReady` still means the active tab's
  controller exists.
- **Native UIA.** The ignored test
  `command_line_markdown_file_opens_as_a_named_rendered_document` launches
  the release shell with `test-fixtures/markdown/gfm/features.md` and asserts
  one tab named `features.md`, a single visible document named `features.md`
  whose Text-pattern content contains "GFM Full Features Test" and no raw
  alert or table syntax. It passed in 2 of 2 runs.
- **Capture.** A PrintWindow capture (system dark theme) shows the heading,
  the styled table, task-list checkboxes, the five alerts with icons,
  footnotes, the highlighted C# block, math superscripts and the Mermaid
  frame with its source text. Two rendering-asset observations, identical in
  the .NET CSS: task-list items keep their list bullets, and the Mermaid zoom
  badge (`.marknexia-diagram-zoom-status`, `position: absolute`) is placed
  against the page because `.marknexia-mermaid` is not positioned.
- **Startup samples (loaded machine, not evidence).** `small-document`, 5 cold
  and 5 warm runs: first render median 3235 ms cold and 3452 ms warm
  (first render lands about 0.6 s after `WebViewReady`); host private bytes
  about 7.2 MB; WebView2 working set about 360 MB. `empty-shell` on the same
  machine: first render 3.5–4.3 s. Shell-visible times of 1.2–4.5 s show the
  machine load.
- **Blocked: large-document.** The generated fixture (features.md plus LF,
  2000 times; 1,864,000 bytes) renders to a body over the sanitizer's
  4 MiB HTML input budget (`PolicyLimits::default().max_html_input_bytes`,
  REND-1), so the shell shows "sanitizer rejected the document: HTML input
  exceeds the 4194304-byte policy limit" and `FirstRender` never fires. The
  collector generates, hashes and passes the file correctly; collection
  needs a security/rendering budget change.
- **Not run natively this session:** the Open dialog (the workstation locked
  during the session), and the keyboard-handoff step of the two-tab smoke:
  the foreground belonged to `LockApp`, so its foreground guard refused
  `SendInput`.

### Review fixes (2026-09-27)

These supersede the matching statements above.

- **Panic containment.** Release now uses `panic = "unwind"`. Each render
  job runs under `catch_unwind`, and a drop guard answers the tab with
  `Panicked` if a panic ever escapes, so one bad document fails only its
  own tab and the worker keeps serving. Option (b), keeping abort, would
  need proof that the markdown, rendering and sanitizer stacks (including
  third-party parsers) never panic, which this lane cannot provide. Cost:
  the release x64 executable grows from 2,298,368 to 2,908,672 bytes
  (+610,304, +26.6%). Panics in `extern "system"` callbacks still abort.
- **Worker pool.** A FIFO queue feeds 2–4 workers
  (`available_parallelism - 1`, clamped). A closed tab's cancel flag lets a
  queued job skip its work.
- **Page building off the UI thread.** Workers read, render and build the
  `HostDocument` (it is `Send`). `from_rendered` consumes the
  `RenderedDocument`, drops the body once the page exists, inserts the
  title in place and moves the buffer into an `Arc<Vec<u8>>` with no
  further copy.
- **128 MiB cap and streaming.** `MAX_DOCUMENT_BYTES` is the 128 MiB page
  limit plus 64 KiB for title and template text. The document response
  streams from the shared buffer through a read-only, free-threaded
  `IStream` (no per-request copy). Small fixed bodies still use an HGLOBAL.
  - Measured on the loaded x64 host, from controller creation to
    `NavigationCompleted`, with a paragraph-per-line page: 64 KiB in
    0.3 s, 8 MiB in 2.6–4.4 s, 32 MiB in 18.0 s (24.9 s with the old
    per-request copy).
  - A 128 MiB page is served but Chromium did not finish parsing and
    layout within 90 s, in either mode.
- **Safe open.**
  - `read_source` accepts only drive-letter paths that `CanonicalPath`
    validates, plus the verbatim `\\?\C:\` form of one. It refuses UNC,
    `\\.\` and `\\?\` device namespaces (GLOBALROOT, volumes, pipes),
    reserved device names and alternate data streams, all before opening.
  - It opens with `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION` and a
    sequential-scan hint, and requires `GetFileType == FILE_TYPE_DISK` and
    a regular file.
  - This is stricter than .NET: `FileStream` refuses non-disk handles for
    ordinary paths but allows UNC shares.
  - An over-limit read reports the file's current size.
  - `file:///` and `file://` arguments convert like the .NET
    `PathCanonicalizer`.
- **Status and readiness.**
  - The status bar shows, in order of priority: the active document's
    error, "Rendering…", a transient notice (cleared when the active tab
    changes), the environment error, or "Ready".
  - `Local\Marknexia.StartupFailed.<pid>` is signalled when every startup
    document failed. The collector then fails fast: the large-document
    run stopped in 6 s instead of timing out.
  - Results that arrive while the session is checked out are applied by
    the message loop after the outer call, not re-posted.
  - The COM STA stays entered without WebView2, so the Open dialog still
    works.
- **Header policy.** The document response header is the page's meta
  policy plus the header-only `frame-ancestors 'none'`.
- **Identity binding.** The check still rebuilds the policy and base from
  the identity and matches the page head. A direct comparison needs
  `marknexia-rendering` to expose `RenderedDocument::identity() ->
  &PageIdentity` (the identity the page was rendered with; `PageIdentity`
  is already `Eq`) and `RenderedDocument::content_security_policy() ->
  &str` (the unencoded policy). Optionally it could also take a plain-text
  document title in `RenderContext`, so the host would not need to insert
  `<title>`.
- **Native re-verification (workstation locked, foreground `LockApp`).**
  - UIA and SendInput steps and screenshots were skipped.
  - Non-interactive runs passed: the two-tab resource/teardown and
    page-message WebView2 tests, the large-document streaming measurement,
    and `small-document` collection with the streamed document (first
    render 2.95–6.45 s over 2 cold and 2 warm runs).

## Compiled and headless-tested

- `cargo clippy --workspace --all-targets -- -D warnings` passes, with default
  features and with all features.
- The headless test suites pass. They cover stable tab IDs and the selection
  and close rules; key routing (Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+W, F6);
  44-DIP targets at 96, 144 and 192 DPI; theme resolution; the `tab_slot`
  geometry shared by paint, hit testing and UIA bounds; and the shell
  accelerator filter.
- Document tests: path-argument parsing, .NET-compatible decoding, the
  size pre-check and bounded read, stale-render discard
  (`tests/documents.rs`); rendered hosting, identity and CSP binding, and
  bridge-message rejection (`marknexia-webview/tests/rendered_document.rs`);
  collector fixture generation and digests without launching the shell
  (`tests/perf/collect_rust_desktop_perf.tests.ps1`).
- `cargo deny check` fails only on licenses reached through the optional
  `candidate-comrak` feature (comrak `BSD-2-Clause`, finl_unicode
  `Unicode-DFS-2016`), which predates this work; `deny.toml` is unchanged.
- Release builds pass for `x86_64` (483–488 KB) and `aarch64`
  (440 KB, compile-only).

## Implemented, awaiting native verification

- **Structure events.** `ChildrenInvalidated` on tab close and
  `AutomationFocusChanged` on keyboard selection are implemented but not yet
  asserted natively.

## Not yet verified or not implemented

- Native `WM_DPICHANGED` across monitors, and high-contrast capture.
- WebView2 focus changes reflected back into portable focus state, for
  example when the user clicks into the document.
- Page-to-host bridge actions (links, copy), the Mermaid runtime and local
  images; the large-document scenario (sanitizer budget).
- Drag and drop, association activation, and the repository sidebar
  contents (so `repository-scan` stays uncollectable).
- ARM64 runtime on real hardware, and all release gates: packaging, size,
  performance on a quiet machine, and parity.
