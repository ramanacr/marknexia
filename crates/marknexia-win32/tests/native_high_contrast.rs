#![cfg(windows)]

//! Native gate `uia-native-high-contrast`: the shell follows a runtime
//! high-contrast change and returns to its brand theme afterwards.
//!
//! The test turns Windows high contrast on with `SPI_SETHIGHCONTRAST`,
//! broadcasts `WM_SETTINGCHANGE`, and checks, from DWM captures of the
//! shell window and through UI Automation:
//!
//! 1. the chrome repaints with system colors: the tab fills are
//!    `COLOR_BTNFACE`, the selected-tab line is `COLOR_HIGHLIGHT`, the chrome
//!    background is mostly `COLOR_WINDOW`, `COLOR_WINDOWTEXT` lines and text
//!    show, and no brand accent remains;
//! 2. the WebView2 document switches to forced colors: its background is
//!    `COLOR_WINDOW` and its text uses `COLOR_WINDOWTEXT`;
//! 3. the shell stays responsive and keyboard focus stays usable;
//! 4. turning high contrast off restores the brand tab fills and the
//!    document background seen before the change.
//!
//! Machine safety: the setting is changed ONLY on a GitHub Actions runner
//! (`CI=true`, `GITHUB_ACTIONS=true` and a `RUNNER_TEMP` folder), and a drop
//! guard restores the original high-contrast state even when an assertion
//! fails. Anywhere else the test prints why it skipped and passes. Keys are
//! sent only through `send_chord_to`, which re-checks that the launched shell
//! owns the foreground.

use std::{
    thread,
    time::{Duration, Instant},
};

use marknexia_win32::{
    accessibility::{tab_slot, tabs_extent},
    layout::Dip,
    theme::{ColorSpec, EffectiveTheme, Palette, palette},
};
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, COLOR_3DDKSHADOW, COLOR_3DLIGHT, COLOR_BTNFACE,
            COLOR_BTNHIGHLIGHT, COLOR_BTNSHADOW, COLOR_BTNTEXT, COLOR_GRAYTEXT, COLOR_HIGHLIGHT,
            COLOR_HIGHLIGHTTEXT, COLOR_HOTLIGHT, COLOR_WINDOW, COLOR_WINDOWFRAME, COLOR_WINDOWTEXT,
            CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject,
            GetDC, GetDIBits, GetSysColor, HGDIOBJ, MapWindowPoints, ReleaseDC, SYS_COLOR_INDEX,
            ScreenToClient, SelectObject,
        },
        Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Variant::VARIANT,
        },
        UI::{
            Accessibility::{
                CUIAutomation, HCF_HIGHCONTRASTON, HIGHCONTRASTW, HIGHCONTRASTW_FLAGS,
                IUIAutomation, IUIAutomationElement, IUIAutomationSelectionItemPattern,
                TreeScope_Children, TreeScope_Descendants, UIA_AutomationIdPropertyId,
                UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_SelectionItemPatternId,
                UIA_TabControlTypeId,
            },
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow,
                SetThreadDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::VK_TAB,
            WindowsAndMessaging::{
                FindWindowExW, GetClientRect, HWND_BROADCAST, SMTO_ABORTIFHUNG,
                SPI_GETHIGHCONTRAST, SPI_SETHIGHCONTRAST, SPIF_SENDCHANGE,
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SendMessageTimeoutW, SystemParametersInfoW,
                WM_NULL, WM_SETTINGCHANGE,
            },
        },
    },
    core::{BSTR, Interface, PWSTR, w},
};

#[path = "support/shell.rs"]
mod shell_support;

use shell_support::{
    ShellProcess, TwoDocuments, activate_attempt, find_shell_window, foreground_owner,
    on_github_actions, send_chord_to,
};

const TIMEOUT: Duration = Duration::from_secs(30);
/// `PW_CLIENTONLY | PW_RENDERFULLCONTENT`: the DWM-composed client area,
/// child windows and the WebView2 surface included.
const PRINT_CLIENT_FULL_CONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(1 | 2);

// ---------------------------------------------------------------------------
// The GitHub-only guard and the high-contrast setting.
// ---------------------------------------------------------------------------

/// The setting may change only on a GitHub Actions runner: `CI` and
/// `GITHUB_ACTIONS` are `true` (the guard of `activate_attempt`) and the
/// runner's `RUNNER_TEMP` folder exists. A developer machine never matches.
fn may_change_system_settings() -> bool {
    on_github_actions()
        && std::env::var_os("RUNNER_TEMP")
            .filter(|folder| !folder.is_empty())
            .is_some_and(|folder| std::path::Path::new(&folder).is_dir())
}

/// A copy of the `HIGHCONTRASTW` state: the flags and the scheme name.
#[derive(Clone, Debug)]
struct HighContrast {
    flags: HIGHCONTRASTW_FLAGS,
    /// NUL-terminated scheme name, when the system reported one.
    scheme: Option<Vec<u16>>,
}

impl HighContrast {
    fn read() -> Self {
        let mut value = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            ..Default::default()
        };
        unsafe {
            SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                value.cbSize,
                Some(std::ptr::from_mut(&mut value).cast()),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
        }
        .expect("SPI_GETHIGHCONTRAST");
        // The pointer refers to system-owned text; copy it at once.
        let scheme = (!value.lpszDefaultScheme.is_null()).then(|| {
            let mut text: Vec<u16> = (0..260)
                .map(|index| unsafe { *value.lpszDefaultScheme.0.add(index) })
                .take_while(|unit| *unit != 0)
                .collect();
            text.push(0);
            text
        });
        Self {
            flags: value.dwFlags,
            scheme,
        }
    }

    fn is_on(&self) -> bool {
        self.flags.0 & HCF_HIGHCONTRASTON.0 != 0
    }

    /// Applies this state and broadcasts the change. Returns whether the
    /// system accepted it.
    fn apply(&self) -> bool {
        let mut scheme = self.scheme.clone();
        let mut value = HIGHCONTRASTW {
            cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
            dwFlags: self.flags,
            lpszDefaultScheme: scheme
                .as_mut()
                .map_or(PWSTR::null(), |text| PWSTR(text.as_mut_ptr())),
        };
        // SPIF_SENDCHANGE broadcasts WM_SETTINGCHANGE (SPI_SETHIGHCONTRAST).
        let applied = unsafe {
            SystemParametersInfoW(
                SPI_SETHIGHCONTRAST,
                value.cbSize,
                Some(std::ptr::from_mut(&mut value).cast()),
                SPIF_SENDCHANGE,
            )
        }
        .is_ok();
        // A second, explicit broadcast that cannot block on a hung window.
        let mut reply = 0_usize;
        let _ = unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                WPARAM(SPI_SETHIGHCONTRAST.0 as usize),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                2000,
                Some(&mut reply),
            )
        };
        applied
    }

    fn with_contrast(&self, on: bool) -> Self {
        let flags = if on {
            self.flags.0 | HCF_HIGHCONTRASTON.0
        } else {
            self.flags.0 & !HCF_HIGHCONTRASTON.0
        };
        Self {
            flags: HIGHCONTRASTW_FLAGS(flags),
            scheme: self.scheme.clone(),
        }
    }
}

/// Restores the high-contrast state read at creation, on drop: after a
/// pass, a failed assertion or a panic in a helper.
struct RestoreHighContrast {
    original: HighContrast,
}

impl RestoreHighContrast {
    fn capture() -> Self {
        Self {
            original: HighContrast::read(),
        }
    }

    /// Sets high contrast on or off and waits until the system reports it.
    fn set(&self, on: bool) {
        let wanted = self.original.with_contrast(on);
        assert!(wanted.apply(), "SPI_SETHIGHCONTRAST (on: {on}) failed");
        wait_for(&format!("SPI_GETHIGHCONTRAST to report {on}"), || {
            (HighContrast::read().is_on() == on).then_some(())
        });
    }
}

impl Drop for RestoreHighContrast {
    fn drop(&mut self) {
        let now = HighContrast::read();
        if now.is_on() == self.original.is_on() && now.flags == self.original.flags {
            return;
        }
        let restored = self.original.apply();
        eprintln!(
            "restored the original high-contrast state (on: {}; accepted: {restored})",
            self.original.is_on()
        );
    }
}

// ---------------------------------------------------------------------------
// UIA helpers.
// ---------------------------------------------------------------------------

/// Single-threaded COM apartment for the UIA client, released on drop.
struct Apartment;

impl Apartment {
    fn enter() -> Self {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
            .ok()
            .expect("UIA STA");
        Self
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(200));
    }
}

fn name(element: &IUIAutomationElement) -> String {
    unsafe { element.CurrentName() }
        .map(|name| name.to_string())
        .unwrap_or_default()
}

struct Launched {
    // Field order is drop order: UIA objects, the apartment, then the shell.
    tab_control: IUIAutomationElement,
    root: IUIAutomationElement,
    automation: IUIAutomation,
    _apartment: Apartment,
    hwnd: HWND,
    shell: ShellProcess,
    _documents: TwoDocuments,
}

fn launch() -> Launched {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let executable = std::env::var_os("MARKNEXIA_SHELL_EXE").expect("MARKNEXIA_SHELL_EXE");
    let documents = TwoDocuments::new();
    let shell = ShellProcess::launch(&executable, &documents.args());
    let hwnd = wait_for("the launched shell HWND", || find_shell_window(shell.id()));
    let apartment = Apartment::enter();
    let automation: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .expect("CUIAutomation");
    let root = unsafe { automation.ElementFromHandle(hwnd) }.expect("ElementFromHandle");
    let tab_control = wait_for("the document TabControl", || {
        let condition = unsafe {
            automation.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_TabControlTypeId.0),
            )
        }
        .ok()?;
        let found = unsafe { root.FindAll(TreeScope_Children, &condition) }.ok()?;
        (0..unsafe { found.Length() }.ok()?)
            .filter_map(|index| unsafe { found.GetElement(index) }.ok())
            .find(|element| name(element) == "Document tabs")
    });
    Launched {
        tab_control,
        root,
        automation,
        _apartment: apartment,
        hwnd,
        shell,
        _documents: documents,
    }
}

impl Launched {
    fn tabs(&self) -> Vec<IUIAutomationElement> {
        let Ok(condition) = (unsafe { self.automation.CreateTrueCondition() }) else {
            return Vec::new();
        };
        let Ok(found) = (unsafe { self.tab_control.FindAll(TreeScope_Children, &condition) })
        else {
            return Vec::new();
        };
        (0..unsafe { found.Length() }.unwrap_or(0))
            .filter_map(|index| unsafe { found.GetElement(index) }.ok())
            .collect()
    }

    fn select(&self, index: usize) -> String {
        let tabs = wait_for("two document tabs", || {
            let tabs = self.tabs();
            (tabs.len() == 2).then_some(tabs)
        });
        let pattern: IUIAutomationSelectionItemPattern =
            unsafe { tabs[index].GetCurrentPattern(UIA_SelectionItemPatternId) }
                .expect("SelectionItem pattern")
                .cast()
                .expect("SelectionItem interface");
        unsafe { pattern.Select() }.expect("Select");
        let title = name(&tabs[index]);
        wait_for(&format!("the document of tab {title:?}"), || {
            self.visible_document()
                .filter(|(document, _)| *document == title)
        });
        title
    }

    /// The single on-screen WebView document: its name and its bounds in
    /// the shell's client coordinates.
    fn visible_document(&self) -> Option<(String, RECT)> {
        let condition = unsafe {
            self.automation.CreatePropertyCondition(
                UIA_ControlTypePropertyId,
                &VARIANT::from(UIA_DocumentControlTypeId.0),
            )
        }
        .ok()?;
        let found = unsafe { self.root.FindAll(TreeScope_Descendants, &condition) }.ok()?;
        let mut visible = (0..unsafe { found.Length() }.ok()?)
            .filter_map(|index| unsafe { found.GetElement(index) }.ok())
            .filter(|document| {
                unsafe { document.CurrentIsOffscreen() }.is_ok_and(|offscreen| !offscreen.as_bool())
            })
            .filter(|document| !name(document).is_empty());
        let first = visible.next()?;
        if visible.next().is_some() {
            return None;
        }
        let screen = unsafe { first.CurrentBoundingRectangle() }.ok()?;
        let mut corners = [
            POINT {
                x: screen.left,
                y: screen.top,
            },
            POINT {
                x: screen.right,
                y: screen.bottom,
            },
        ];
        for corner in &mut corners {
            let _ = unsafe { ScreenToClient(self.hwnd, corner) };
        }
        Some((
            name(&first),
            RECT {
                left: corners[0].x,
                top: corners[0].y,
                right: corners[1].x,
                bottom: corners[1].y,
            },
        ))
    }

    fn element_by_id(&self, id: &str) -> Option<IUIAutomationElement> {
        let condition = unsafe {
            self.automation
                .CreatePropertyCondition(UIA_AutomationIdPropertyId, &VARIANT::from(BSTR::from(id)))
        }
        .ok()?;
        unsafe { self.root.FindFirst(TreeScope_Descendants, &condition) }.ok()
    }

    /// The shell's UI thread answers within five seconds.
    fn responsive(&self) -> bool {
        let mut reply = 0_usize;
        unsafe {
            SendMessageTimeoutW(
                self.hwnd,
                WM_NULL,
                WPARAM(0),
                LPARAM(0),
                SMTO_ABORTIFHUNG,
                5000,
                Some(&mut reply),
            )
        }
        .0 != 0
    }

    fn activate(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut attempt = 0;
        while foreground_owner() != self.shell.id() && Instant::now() < deadline {
            activate_attempt(self.hwnd, attempt);
            attempt += 1;
            thread::sleep(Duration::from_millis(200));
        }
        foreground_owner() == self.shell.id()
    }

    fn focused_id(&self) -> Option<(String, bool)> {
        let focused = unsafe { self.automation.GetFocusedElement() }.ok()?;
        let id = unsafe { focused.CurrentAutomationId() }.ok()?.to_string();
        let has_focus = unsafe { focused.CurrentHasKeyboardFocus() }.ok()?.as_bool();
        Some((id, has_focus))
    }
}

// ---------------------------------------------------------------------------
// Captures and color checks.
// ---------------------------------------------------------------------------

/// A DWM capture of the shell's client area, in `COLORREF` order
/// (0x00BBGGRR), top-down rows.
struct Frame {
    width: i32,
    height: i32,
    pixels: Vec<u32>,
}

impl Frame {
    fn capture(shell: &Launched) -> Option<Self> {
        // PrintWindow sends WM_PRINT synchronously: prove first that the UI
        // thread answers, so a hung shell cannot block the test.
        if !shell.responsive() {
            return None;
        }
        let mut client = RECT::default();
        unsafe { GetClientRect(shell.hwnd, &mut client) }.ok()?;
        let (width, height) = (client.right, client.bottom);
        if width <= 0 || height <= 0 {
            return None;
        }
        let screen = unsafe { GetDC(None) };
        let memory = unsafe { CreateCompatibleDC(Some(screen)) };
        let bitmap = unsafe { CreateCompatibleBitmap(screen, width, height) };
        let previous = unsafe { SelectObject(memory, HGDIOBJ(bitmap.0)) };
        let printed =
            unsafe { PrintWindow(shell.hwnd, memory, PRINT_CLIENT_FULL_CONTENT) }.as_bool();
        unsafe { SelectObject(memory, previous) };
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                // Negative: top-down rows.
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0_u32; (width as usize) * (height as usize)];
        let rows = unsafe {
            GetDIBits(
                memory,
                bitmap,
                0,
                height as u32,
                Some(bgra.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        unsafe {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
        }
        if !printed || rows != height {
            return None;
        }
        // 0xAARRGGBB in memory order B, G, R, A -> COLORREF 0x00BBGGRR.
        let pixels = bgra
            .into_iter()
            .map(|pixel| {
                let (red, green, blue) = ((pixel >> 16) & 0xff, (pixel >> 8) & 0xff, pixel & 0xff);
                red | (green << 8) | (blue << 16)
            })
            .collect();
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    fn at(&self, x: i32, y: i32) -> Option<u32> {
        (0..self.width).contains(&x).then_some(())?;
        (0..self.height).contains(&y).then_some(())?;
        self.pixels
            .get((y as usize) * (self.width as usize) + x as usize)
            .copied()
    }

    /// Pixels inside `region` (or outside it, when `inside` is false).
    fn pixels_where(&self, region: &RECT, inside: bool) -> Vec<u32> {
        let mut out = Vec::new();
        for y in 0..self.height {
            for x in 0..self.width {
                let within =
                    x >= region.left && x < region.right && y >= region.top && y < region.bottom;
                if within == inside {
                    out.push(self.pixels[(y as usize) * (self.width as usize) + x as usize]);
                }
            }
        }
        out
    }
}

/// The most frequent color and its share of `pixels`.
fn dominant(pixels: &[u32]) -> Option<(u32, f64)> {
    let mut counts = std::collections::HashMap::<u32, usize>::new();
    for pixel in pixels {
        *counts.entry(*pixel).or_default() += 1;
    }
    let (color, count) = counts.into_iter().max_by_key(|(_, count)| *count)?;
    Some((color, count as f64 / pixels.len().max(1) as f64))
}

fn share(pixels: &[u32], wanted: impl Fn(u32) -> bool) -> f64 {
    pixels.iter().filter(|pixel| wanted(**pixel)).count() as f64 / pixels.len().max(1) as f64
}

fn count(pixels: &[u32], color: u32) -> usize {
    pixels.iter().filter(|pixel| **pixel == color).count()
}

fn sys(index: SYS_COLOR_INDEX) -> u32 {
    unsafe { GetSysColor(index) }
}

/// The system colors a high-contrast chrome may paint with.
fn system_colors() -> Vec<u32> {
    [
        COLOR_WINDOW,
        COLOR_WINDOWTEXT,
        COLOR_HIGHLIGHT,
        COLOR_HIGHLIGHTTEXT,
        COLOR_BTNFACE,
        COLOR_BTNTEXT,
        COLOR_GRAYTEXT,
        COLOR_HOTLIGHT,
        COLOR_BTNSHADOW,
        COLOR_BTNHIGHLIGHT,
        COLOR_3DDKSHADOW,
        COLOR_3DLIGHT,
        COLOR_WINDOWFRAME,
    ]
    .into_iter()
    .map(sys)
    .collect()
}

fn colorref(spec: ColorSpec) -> Option<u32> {
    match spec {
        ColorSpec::Rgb(rgb) => Some(((rgb & 0xff) << 16) | (rgb & 0x00ff00) | ((rgb >> 16) & 0xff)),
        ColorSpec::System(_) => None,
    }
}

/// (inactive tab fill, active tab fill) as the shell paints them.
fn tab_fills(colors: Palette) -> (u32, u32) {
    (
        colorref(colors.surface).expect("brand color"),
        colorref(colors.elevated).expect("brand color"),
    )
}

/// The brand-only colors: accents and fills of both brand palettes.
fn brand_colors() -> Vec<u32> {
    [EffectiveTheme::Light, EffectiveTheme::Dark]
        .into_iter()
        .flat_map(|theme| {
            let colors = palette(theme);
            [
                colors.accent,
                colors.accent_hover,
                colors.info,
                colors.elevated,
                colors.hover,
            ]
        })
        .filter_map(colorref)
        .collect()
}

/// The tab strip's position in the shell's client area.
struct Strip {
    origin: POINT,
    rect: RECT,
    dpi: u32,
}

impl Strip {
    fn locate(shell: &Launched) -> Self {
        let strip =
            unsafe { FindWindowExW(Some(shell.hwnd), None, w!("MarknexiaRustTabStrip"), None) }
                .expect("MarknexiaRustTabStrip child");
        let mut rect = RECT::default();
        unsafe { GetClientRect(strip, &mut rect) }.expect("tab strip rectangle");
        let mut origin = [POINT::default()];
        unsafe { MapWindowPoints(Some(strip), Some(shell.hwnd), &mut origin) };
        Self {
            origin: origin[0],
            rect,
            dpi: unsafe { GetDpiForWindow(shell.hwnd) }.max(96),
        }
    }

    fn px(&self, dip: u32) -> i32 {
        (dip * self.dpi / 96) as i32
    }

    fn slot(&self, index: usize) -> Option<(i32, i32)> {
        let extent = tabs_extent(
            self.rect.right,
            2,
            Dip::TAB_MAX_WIDTH.to_pixels(self.dpi) as i32,
        );
        tab_slot(extent, 2, index)
    }

    /// (inactive, active) tab fills; tab 0 must be the active tab. Sampled
    /// away from the slot edges (focus rectangle) and the title text.
    fn fills(&self, frame: &Frame) -> Option<(u32, u32)> {
        let inset = self.px(6).max(4);
        let y = self.origin.y + self.rect.bottom - inset;
        let at = |index: usize| {
            let (_, right) = self.slot(index)?;
            frame.at(self.origin.x + right - inset, y)
        };
        Some((at(1)?, at(0)?))
    }

    /// The share of the active tab's indicator row painted in `color`.
    fn indicator_share(&self, frame: &Frame, color: u32) -> f64 {
        let Some((left, right)) = self.slot(0) else {
            return 0.0;
        };
        // The 3-DIP line runs 10 DIPs inside each slot edge.
        let y = self.origin.y + self.rect.bottom - 2;
        let (from, to) = (left + self.px(12), right - self.px(12));
        let row: Vec<u32> = (from..to)
            .filter_map(|x| frame.at(self.origin.x + x, y))
            .collect();
        share(&row, |pixel| pixel == color)
    }
}

/// What one capture shows.
#[derive(Debug)]
struct Observation {
    fills: (u32, u32),
    indicator_highlight: f64,
    chrome_window: f64,
    chrome_window_text: usize,
    chrome_system: f64,
    chrome_brand: f64,
    page: (u32, f64),
    page_window_text: usize,
}

fn observe(shell: &Launched, strip: &Strip) -> Option<Observation> {
    let (_, document) = shell.visible_document()?;
    let frame = Frame::capture(shell)?;
    let chrome = frame.pixels_where(&document, false);
    // The page without its outer 16 pixels (scroll bar, borders).
    let inner = RECT {
        left: document.left + 16,
        top: document.top + 16,
        right: document.right - 32,
        bottom: document.bottom - 16,
    };
    let page = frame.pixels_where(&inner, true);
    let system = system_colors();
    let brand: Vec<u32> = brand_colors()
        .into_iter()
        .filter(|color| !system.contains(color))
        .collect();
    let (window, window_text, highlight) = (
        sys(COLOR_WINDOW),
        sys(COLOR_WINDOWTEXT),
        sys(COLOR_HIGHLIGHT),
    );
    Some(Observation {
        fills: strip.fills(&frame)?,
        indicator_highlight: strip.indicator_share(&frame, highlight),
        chrome_window: share(&chrome, |pixel| pixel == window),
        chrome_window_text: count(&chrome, window_text),
        chrome_system: share(&chrome, |pixel| system.contains(&pixel)),
        chrome_brand: share(&chrome, |pixel| brand.contains(&pixel)),
        page: dominant(&page)?,
        page_window_text: count(&page, window_text),
    })
}

/// The high-contrast expectations; `Err` names the first one not met.
fn high_contrast_met(seen: &Observation) -> Result<(), String> {
    let face = sys(COLOR_BTNFACE);
    if seen.fills != (face, face) {
        return Err(format!(
            "tab fills {:06x?} are not COLOR_BTNFACE {face:06x}",
            seen.fills
        ));
    }
    if seen.indicator_highlight < 0.5 {
        return Err(format!(
            "selected-tab line is COLOR_HIGHLIGHT in only {:.0}% of its row",
            seen.indicator_highlight * 100.0
        ));
    }
    if seen.chrome_window < 0.2 {
        return Err(format!(
            "COLOR_WINDOW covers only {:.1}% of the chrome",
            seen.chrome_window * 100.0
        ));
    }
    if seen.chrome_window_text < 50 {
        return Err(format!(
            "only {} chrome pixels are COLOR_WINDOWTEXT",
            seen.chrome_window_text
        ));
    }
    if seen.chrome_system < 0.8 {
        return Err(format!(
            "system colors cover only {:.1}% of the chrome",
            seen.chrome_system * 100.0
        ));
    }
    if seen.chrome_brand > 0.01 {
        return Err(format!(
            "brand colors still cover {:.2}% of the chrome",
            seen.chrome_brand * 100.0
        ));
    }
    let window = sys(COLOR_WINDOW);
    if seen.page.0 != window {
        return Err(format!(
            "page background {:06x} is not COLOR_WINDOW {window:06x} (forced colors)",
            seen.page.0
        ));
    }
    if seen.page_window_text == 0 {
        return Err("no page text in COLOR_WINDOWTEXT (forced colors)".to_owned());
    }
    Ok(())
}

/// Polls captures until `met` accepts one; panics with the last
/// observation and reason otherwise.
fn wait_until(
    shell: &Launched,
    strip: &Strip,
    what: &str,
    met: impl Fn(&Observation) -> Result<(), String>,
) -> Observation {
    let deadline = Instant::now() + TIMEOUT;
    let mut last = (None, String::from("no capture"));
    loop {
        if let Some(seen) = observe(shell, strip) {
            match met(&seen) {
                Ok(()) => {
                    eprintln!("{what}: {seen:x?}");
                    return seen;
                }
                Err(reason) => last = (Some(seen), reason),
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {}; last observation {:x?}",
            last.1,
            last.0
        );
        thread::sleep(Duration::from_millis(250));
    }
}

/// Keyboard focus is usable: UIA focus lands on the Theme combo box and
/// reports keyboard focus, and Tab (sent only while the shell owns the
/// foreground) moves it to another focused control.
fn assert_focus_usable(shell: &Launched, phase: &str) {
    assert!(shell.responsive(), "{phase}: the shell stopped answering");
    assert!(
        find_shell_window(shell.shell.id()).is_some(),
        "{phase}: the shell window is gone"
    );
    assert!(
        shell.activate(),
        "{phase}: the shell must own the foreground to test focus"
    );
    let theme = wait_for("the Theme combo box", || shell.element_by_id("2007"));
    unsafe { theme.SetFocus() }.expect("UIA SetFocus on the Theme combo box");
    wait_for(&format!("{phase}: focus on the Theme combo box"), || {
        shell
            .focused_id()
            .filter(|(id, has_focus)| id == "2007" && *has_focus)
    });
    send_chord_to(shell.shell.id(), &[VK_TAB]);
    let (next, _) = wait_for(&format!("{phase}: Tab to move focus"), || {
        shell
            .focused_id()
            .filter(|(id, has_focus)| id != "2007" && *has_focus)
    });
    eprintln!("{phase}: Tab moved focus from the Theme combo box to automation id {next:?}");
}

#[test]
#[ignore = "requires MARKNEXIA_SHELL_EXE, an interactive desktop and a GitHub Actions runner"]
fn runtime_high_contrast_repaints_chrome_and_page_and_restores_the_theme() {
    if !may_change_system_settings() {
        eprintln!(
            "SKIPPED uia-native-high-contrast: it switches Windows high contrast \
             (SPI_SETHIGHCONTRAST), which is allowed only on a GitHub Actions runner \
             (CI=true, GITHUB_ACTIONS=true and RUNNER_TEMP). No setting was changed."
        );
        return;
    }
    // Created before the shell, dropped after it: the original state comes
    // back even when an assertion below fails.
    let restore = RestoreHighContrast::capture();
    if restore.original.is_on() {
        eprintln!("high contrast was already on; turning it off for the baseline");
        restore.set(false);
    }
    let shell = launch();
    let first = shell.select(0);
    let strip = Strip::locate(&shell);
    let brand =
        [EffectiveTheme::Light, EffectiveTheme::Dark].map(|theme| tab_fills(palette(theme)));

    // 1. Baseline: brand tab fills; remember the page background.
    let baseline = wait_until(&shell, &strip, "the brand baseline", |seen| {
        if brand.contains(&seen.fills) {
            Ok(())
        } else {
            Err(format!("tab fills {:06x?} are no brand pair", seen.fills))
        }
    });

    // 2. High contrast on, at runtime.
    restore.set(true);
    let contrast = wait_until(
        &shell,
        &strip,
        "system colors in chrome and page",
        high_contrast_met,
    );
    // Read after the theme switch settled (right after the SPI call the
    // system colors can still be the old ones).
    eprintln!(
        "high contrast on: COLOR_WINDOW {:06x}, COLOR_WINDOWTEXT {:06x}, COLOR_HIGHLIGHT {:06x}, \
         COLOR_BTNFACE {:06x}",
        sys(COLOR_WINDOW),
        sys(COLOR_WINDOWTEXT),
        sys(COLOR_HIGHLIGHT),
        sys(COLOR_BTNFACE)
    );
    if contrast.page.0 == baseline.page.0 {
        eprintln!(
            "note: the high-contrast window color equals the brand page background; \
             the page check rests on its COLOR_WINDOWTEXT text"
        );
    }
    // A tab switch through UIA still renders the other document.
    shell.select(1);
    shell.select(0);
    assert_focus_usable(&shell, "high contrast on");

    // 3. High contrast off: the brand theme and the page background return.
    restore.set(false);
    let restored = wait_until(&shell, &strip, "the brand theme again", |seen| {
        if seen.fills != baseline.fills {
            return Err(format!(
                "tab fills {:06x?}, baseline {:06x?}",
                seen.fills, baseline.fills
            ));
        }
        if seen.page.0 != baseline.page.0 {
            return Err(format!(
                "page background {:06x}, baseline {:06x}",
                seen.page.0, baseline.page.0
            ));
        }
        Ok(())
    });
    assert!(brand.contains(&restored.fills));
    assert_focus_usable(&shell, "high contrast off");
    eprintln!("uia-native-high-contrast passed for {first:?}");
    drop(shell);
    drop(restore);
}
