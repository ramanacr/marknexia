# Rust WebView2 feasibility evidence (in progress)

This is an interim evidence record for Task 5 of
`docs/superpowers/plans/2026-09-12-rust-win32-feasibility.md`. It is not a
go/no-go decision or a releasable native application.

## Source recovery, 2026-09-24

The recovered Task 5 source had duplicate COM imports and repeated broker,
environment, and recovery test functions. These duplicate declarations were
removed, and Rust source formatting passed. This recovery did not run Cargo,
WebView2, a native probe, or any browser/runtime test. The historical x64
observations below remain historical evidence, not verification of this
recovered tree. The current lockfile still has local path-patched Windows and
WebView2 entries and must be regenerated from verified registry packages by
the integration owner before a portable locked build.

## Verified on Windows x64, 2026-09-16

- The native adapter initializes and balances an STA, detects installed
  Evergreen, creates an environment asynchronously, and creates two controllers
  sharing that environment. The normal Cargo test run has 42 passing tests and
  three deliberately ignored native tests. The ignored tests require explicit
  execution on a host with Evergreen installed.
- Browser and renderer process-failure kinds are mapped to typed events.
  Controller `ProcessFailed` tokens and the environment's
  `BrowserProcessExited` token are owned and removed on explicit close; callbacks
  retain only weak application observers.
- The pure recovery coordinator now requires both controller teardown and the
  environment's browser-exit notification before recreating the environment.
  Tests cover either notification order, duplicate notifications, a second
  failure cycle, and close during a callback.
- The pure resource broker rejects decoded Win32-invalid filename characters,
  trailing-dot/space aliases, and reserved device names in every segment
  (including extension forms and superscript COM/LPT digits). The regression
  tests failed against the earlier broker and pass with these checks.
- `cargo fmt --all -- --check`, `git diff --check`, and Clippy for all WebView
  targets with `-D warnings` exited successfully. The local source override
  still emits warnings from upstream Windows crates; this is not a clean
  dependency/release gate.

## Native host observation

The explicitly run two-controller x64 test sometimes succeeds, but repeatedly
fails when the WebView2 browser process exits unexpectedly. The controller then
returns `0x8007139F` (`ERROR_INVALID_STATE`). Both WebView2 event paths
reported the failure: `ProcessFailed = BrowserExited` and
`BrowserProcessExited = Failed`. A 20-second single-controller hold reproduced
the browser exit *before* the second controller was created, so the second
controller and visibility toggle are not necessary triggers. Changing the
invisible test parent to an officially supported message-only HWND did not
prevent the failure; that diagnostic experiment was reverted.

Crashpad produced a browser-process minidump for a failing sandboxed run. Its
exception stream contains `0x80000003` in `msedge.dll` from installed WebView2
Runtime `153.0.4234.32`. Browser logging on repeated sandboxed runs showed GPU
child exit code `-1073741790` (`0xC0000022`, access denied) followed by
`GPU process isn't usable. Goodbye.` This is stronger evidence of an execution
environment restriction than the minidump breakpoint alone, but it does not
prove which specific sandbox policy caused the denial. The original profile
dumps remain under the test's temporary `marknexia-webview-feasibility` folder
for debugger analysis; diagnostic browser logs are under the ignored `target/`
directory.

Moving the test profile into this worktree did not prevent sandboxed failures.
Process-scoped `--disable-gpu` also left the GPU access-denied/fatal sequence in
the logs, so it is not a product workaround. The test now pumps messages for a
20-second stable dwell after both controllers are created, to catch exits that
would otherwise occur after a superficially passing test. Running that exact
test outside the Codex sandbox, with no diagnostic browser flags, passed on
this x64 machine (`1 passed`, 0 failed, 22.86 seconds including the dwell).
That is an x64 controller-lifecycle observation, not proof of full adapter
security, recovery, ARM64 operation, or release readiness.

## Not yet proved

- Repeatable two-controller lifecycle across supported machines and recovery
  after a real browser exit; the one unsandboxed x64 pass is not enough.
- Native resource interception, typed message delivery, navigation blocking,
  focus/link/copy round trips, and the runnable `webview_probe` example.
- ARM64 compile and runtime evidence, portable registry-backed Cargo.lock,
  production performance measurements, and all release gates.

The current Cargo.lock uses local path overrides of pinned upstream sources
only to permit evaluation on this host while Cargo's Schannel TLS fails. It
must be regenerated against verified registry packages before a portable
commit or release.

## Task 5 source-only implementation, 2026-09-24

The current source now connects the broker to a WebView2 resource-request
handler. The handler obtains a deferral, maps the request's
method/URI/resource context through an immutable per-controller document,
and supplies bounded in-memory 200/403/405 responses with fixed content-type,
nosniff, no-store, and CSP headers. Navigation permits only the exact virtual
document URI; new windows are suppressed. Web messages are parsed from
WebView2's `Source` and JSON event fields against the exact origin, source,
tab, protocol, and document epoch. Typed outbound messages also check their
identity before posting. Event handlers hold weak application observers, and
event tokens are removed before controller closure; failed removals remain
retryable while the host is retained.

`WebViewSession` is the STA-owned adapter for the pure recovery coordinator.
It queues callback notifications, deduplicates old-generation signals,
closes controllers before environment recreation, and restores the same
immutable two-tab documents and selected tab. Retry state is retained for
failed controller teardown, event-token cleanup, environment replacement,
and active-tab visibility. Text-only HTML/CSS/JavaScript/SVG probe assets,
an interactive example, pure policy tests, and ignored native test source
were added. The approved plan names a PNG probe image; SVG was used in this
source-only pass because binary artifact creation was explicitly prohibited.

Only Rust source formatting/parsing, manifest/static inspection, and Git
whitespace checks are permitted for this pass. No Cargo command, native
test, WebView2 process, executable, UI, or PE/archive generator was run.
Consequently this implementation is **uncompiled and unproved**. In
particular, a COM failure while constructing or setting a resource response
needs native validation for fail-closed behavior. The example does not yet
exercise every planned focus/resize/link/copy/bootstrap path automatically;
native x64 and ARM64 lifecycle/security/recovery runs and portable lockfile
regeneration remain integration gates. The historical observations above do
not verify this new source.

## Source-only review fix round 1, 2026-09-24

The session now reapplies active-tab visibility immediately after each new
controller is installed and retries a failed visibility call on the next
poll. A parsed page message is queued with both environment and controller
generation; only `poll` invokes the application's weak observer, outside
the COM callback. Each replacement controller receives a distinct generation,
so queued process-failure and page-message events from its predecessor are
discarded. Ignored native test source covers active visibility, restoration,
deferred observer delivery, and stale-event rejection; those tests were not
executed.

The resource callback preconstructs a reusable empty-body 403 response before
navigation. A normal response-construction or assignment failure retries
`SetResponse` with this explicit deny. `Complete` is invoked only after one
`SetResponse` succeeds. If both assignments fail, the deferral remains
uncompleted and owned by the host until `controller.Close` succeeds; the STA
session closes the controller fleet and enters a terminal security-error
state rather than retrying navigation. A failed `Complete` follows the same
abort path. Security abort attempts `controller.Close` even when callback-token
removal fails; successful controller closure invalidates those registrations.
This is based on WebView2's documented behavior that a request
with no response continues to the network, while a deferred request remains
blocked until completion:
<https://learn.microsoft.com/en-us/microsoft-edge/webview2/how-to/webresourcerequested>,
<https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/icorewebview2webresourcerequestedeventargs>.
The rare joint failure of `GetDeferral` and immediate `SetResponse` cannot
be proven fail-closed from these COM semantics alone; the callback now attempts
synchronous controller closure before returning the COM error, but whether
WebView2 has already continued the request
requires native fault-injection evidence. The source-only gate does not claim
that evidence or a complete security proof.

## Reference behavior

Microsoft documents that `ProcessFailed` and `BrowserProcessExited` can arrive
in either order and that a browser-process failure closes associated controls;
the host must recreate them. See
<https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/process-related-events>.
