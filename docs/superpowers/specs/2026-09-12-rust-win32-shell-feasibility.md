# Rust/raw-Win32 shell feasibility evidence (in progress)

This is an interim Task 6 record for
`docs/superpowers/plans/2026-09-12-rust-win32-feasibility.md`. It is not a
production-readiness claim or a replacement for the existing .NET edition.

## Source recovery, 2026-09-24

The recovered Task 6 library now exports the existing layout and raw-window
modules used by the binary and integration tests. Theme tests were aligned to
the current palette shape and the approved dark Metallic Radium colors; the
light accent follows the current WinUI XAML resource. Rust source formatting
and the native smoke script's PowerShell AST parse passed. Cargo, the native
shell, UI Automation, and WebView2 were not run during this recovery. The
historical x64 observations below are not a current runtime gate. The custom
tab strip, UIA providers, connected WebView viewport, and full native flows
remain incomplete.

## Evidence on Windows x64, 2026-09-17

- `marknexia-win32.exe` now creates a real top-level Win32 window with native
  chrome and a visible native `STATIC` development-status surface. Its window
  procedure transfers exactly one boxed state pointer into `GWLP_USERDATA`
  during `WM_NCCREATE`, clears it during `WM_NCDESTROY`, and runs a checked
  message loop. Raw Win32 calls are isolated to the adapter module.
- `scripts/Test-RustNativeShell.ps1` first failed against the old console stub,
  then verified a visible window, expected class/title, owning process ID, and
  native status child on both debug and release x64 builds. The script closes
  only the process it starts. Its window lookup is scoped to that process, so
  an already-open preview cannot be mistaken for the test instance. The
  release-mode smoke returned a visible HWND after this check was added.
- Error paths after top-level creation now destroy that HWND, allowing
  `WM_NCDESTROY` to release its boxed state if child creation or the message
  loop fails. The normal path and error-path build passed the focused test,
  lint, and native smoke gates; error injection remains future test work.
- Pure shell tests cover stable tab IDs, active selection, wraparound,
  selected/inactive close behavior, reordering, and mapping of Ctrl+Tab,
  Ctrl+Shift+Tab, Ctrl+W, and F6 without consuming Alt+F4 or plain Tab.
- Pure layout tests cover 44-DIP interactive targets at 96, 144, and 192 DPI,
  optional sidebar/find surfaces, all surface bounds, non-overlap, and
  deterministic degradation for zero/tiny clients. They do not prove native
  `WM_DPICHANGED` behavior or accessibility by themselves.
- The Win32 adapter now requests Per-Monitor DPI v2 before window creation,
  applies the suggested rectangle on `WM_DPICHANGED`, and recomputes the
  current layout on resize. The existing development-status child follows
  the computed viewport bounds. These paths compile and pass headless unit
  tests, but moving the window between monitors has not been observed in a
  native automation run.
- Pure theme resolution and palette tests cover system/light/dark/high-contrast
  priority. The dark tokens match the approved Metallic Radium reference and
  high contrast uses symbolic system-color roles, not fixed branded RGB.
  These colors are not yet applied to native controls or the WebView viewport.
- `cargo test -p marknexia-win32 -p marknexia-webview --offline --locked`,
  formatting, and Clippy with `-D warnings` passed using a local dependency
  evaluation override. Release builds of this partial shell compiled for both
  x64 and ARM64. ARM64 was **not** run on a native ARM64 host.

## Still required

- Connect native tab controls, repository/sidebar, find/status surfaces, and a
  secured WebView2 document viewport. The visible status surface explicitly
  says the viewport is not connected; the shell is not yet a Markdown viewer.
- Per-monitor DPI v2, responsive layout, theme/high contrast, keyboard focus
  routing, drag/drop, dialogs, association activation, UI Automation providers,
  and native automation assertions.
- Real document open/render/bridge flows; native renderer/browser recovery;
  ARM64 runtime; clean package, dependency, size, startup, memory, parity,
  accessibility, and security release gates.

The current `Cargo.lock` still depends on local path-patched evaluation sources.
It is not a portable release lockfile and cannot be used as release evidence.
