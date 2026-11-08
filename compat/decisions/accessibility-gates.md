# Accessibility gates: runtime high contrast and keyboard reachability

Status: **proposed**

Scope: `crates/marknexia-win32` (`window.rs` tab strip and status bar, `window/chrome.rs` Tab traversal and theme refresh, `accessibility.rs` tab-strip focus event) and the native gates `tests/native_high_contrast.rs` (`uia-native-high-contrast`) and `tests/native_keyboard.rs` (`uia-native-keyboard`). The .NET sources are `src/Marknexia.App/MainWindow.xaml` and `MainWindow.xaml.cs` (`ActualThemeChanged` → `RefreshWebViewBackgroundColors`, WinUI theme resources).

Evidence class: implemented, compiled, headless-tested and native-tested in CI. The keyboard gate ran locally on 2026-11-08 before the fixes and found A11Y-1 and A11Y-3. Both gates passed on x64 and ARM64 in CI run 42447568420 (all 13 native gates green). The high-contrast gate changes a system setting and runs only on GitHub Actions; there it measured, with high contrast on, COLOR_WINDOW on 96% of the chrome, system colors on 98%, brand colors below 0.01%, the selected-tab line fully COLOR_HIGHLIGHT, and the page background COLOR_WINDOW with COLOR_WINDOWTEXT text. Items marked *unverified* describe .NET behavior that was read from the source or the WinUI defaults, not observed.

## Parity that holds

* **Runtime high contrast:** the .NET edition relies on WinUI theme resources and on Chromium: WebView2 applies forced colors when Windows high contrast is on. The Rust shell re-reads `SPI_GETHIGHCONTRAST` on `WM_SETTINGCHANGE`, `WM_SYSCOLORCHANGE` and `WM_THEMECHANGED`, repaints the chrome with system colors (`COLOR_WINDOW`, `COLOR_WINDOWTEXT`, `COLOR_HIGHLIGHT`, `COLOR_BTNFACE`), sets the WebView2 default background, and the page follows Chromium's forced colors, as in .NET. Turning high contrast off restores the brand palette.

## Observable differences

| ID | Condition | .NET oracle | Rust target |
| --- | --- | --- | --- |
| A11Y-1 | Tab reaches the status text | WinUI `TextBlock` is not a tab stop (*unverified*). | The status text ("Ready", control 1005) is not a tab stop. Before this change it was one: Tab stopped on a non-interactive Text element that showed no focus. |
| A11Y-2 | Sidebar resize handle in UIA | WinUI resize handle (*unverified*: control type). | A plain window with the default proxy: a Pane named `Resize sidebar`, keyboard focusable, Left/Right/Home/End resize it, and it is filled with the accent (Highlight in high contrast) while focused. A dedicated UIA provider (for example Thumb or a RangeValue pattern) is a later improvement, not an accessibility-critical finding. |
| A11Y-3 | Focus on the document tab strip | WinUI `TabView` focuses its selected `TabViewItem` (*unverified*). | On `WM_SETFOCUS` the strip raises a UIA focus-changed event for the selected tab and repaints. The focus rectangle shows only while the strip owns keyboard focus and keyboard input is in use (Tab and Shift+Tab now count as keyboard input). Before this change the rectangle depended only on keyboard mode, so it could show while focus was elsewhere, and was missing after Tab. |
| A11Y-4 | Tab after the last chrome control | WinUI moves focus into the WebView2 control in tab order (*unverified*). | Tab moves from the last chrome stop into the WebView2 document and, from the page's last focusable element, back to the first chrome control. F6 still cycles the surfaces. The gate records the full order. |

## Still manual

Mixed-DPI multi-monitor (`WM_DPICHANGED` with differing DPI) and a Narrator listening pass still need a manual evidence host. The gates assert UIA names, control types and focus, not what Narrator speaks.
