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
- **Startup milestones.** The shell signals the named events
  `Local\Marknexia.WebViewReady.<pid>` and `Local\Marknexia.FirstRender.<pid>`.
  `scripts/Collect-RustDesktopPerf.ps1` consumes them. The window is shown
  before WebView2 startup begins. The trial numbers so far came from a heavily
  loaded machine and are not evidence.

## Compiled and headless-tested

- `cargo clippy --workspace --all-targets -- -D warnings` passes, with default
  features and with all features.
- The headless test suites pass. They cover stable tab IDs and the selection
  and close rules; key routing (Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+W, F6);
  44-DIP targets at 96, 144 and 192 DPI; theme resolution; the `tab_slot`
  geometry shared by paint, hit testing and UIA bounds; and the shell
  accelerator filter.
- Release builds pass for `x86_64` (483–488 KB) and `aarch64`
  (440 KB, compile-only).

## Implemented, awaiting native verification

- **Keyboard handoff.** WebView2 `AcceleratorKeyPressed` routes only the
  shell-owned chords (Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+W, F6 and Shift+F6) back
  to the shell. They are drained after dispatch, outside COM. F6 into
  Document calls `MoveFocus` on the active controller.
  - The native smoke step refuses to inject keys unless the launched shell
    owns the foreground.
  - Its first attempt ran while the workstation was locked (`LockApp` owned
    the foreground), so it is still unverified.
- **Structure events.** `ChildrenInvalidated` on tab close and
  `AutomationFocusChanged` on keyboard selection are implemented but not yet
  asserted natively.

## Not yet verified or not implemented

- Native `WM_DPICHANGED` across monitors, and high-contrast capture.
- Keyboard handoff from inside a document (see above).
- WebView2 focus changes reflected back into portable focus state, for
  example when the user clicks into the document.
- Document open, render and bridge flows. Placeholder documents are
  plain-text only, going through `HtmlPolicy::encode_text` and
  `HostDocument::new_titled`.
- Drag and drop, dialogs, association activation, and the repository
  sidebar contents.
- ARM64 runtime on real hardware, and all release gates: packaging, size,
  performance on a quiet machine, and parity.
