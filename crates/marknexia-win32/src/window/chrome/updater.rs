//! Help > Check for updates (.NET `CheckForUpdates_Click`) for the
//! portable channel.
//!
//! Reentrancy (`crate::reentry`): the menu command, the worker results and
//! the dialogs run as chrome actions from the message-loop tail with no
//! `AppState` borrow held; the dialogs' nested modal loops therefore never
//! meet an outstanding borrow. `BUSY` lives outside the state, so a second
//! request while a check, a dialog or an install is running is ignored
//! without borrowing anything. State changes go through `with_chrome`,
//! which uses `try_borrow_mut` and never panics.

use std::{cell::Cell, sync::atomic::AtomicBool};

use marknexia_update::{
    activation::{self, ProcessLauncher, SystemClock},
    archive::ArchiveLimits,
    check::{
        CheckOutcome, DOWNLOADING_NOTICE, PACKAGED_MESSAGE, available_text, check_for_update,
        install_failed_notice, unavailable_notice, up_to_date_text,
    },
    config::UpdateConfig,
    layout::Layout,
    manifest::{Architecture, Manifest},
    payload::PayloadPolicy,
    stage::{StageRequest, download_and_stage, ensure_private_dir},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE},
};

use super::{ChromeAction, DiagnosticSeverity, WorkerResult, dialogs, queue_action, with_chrome};
use crate::window::update_native::{
    NativeStagePlatform, WinHttpFetcher, current_process, is_packaged, is_process_running,
    os_build, random_nonce,
};

thread_local! {
    /// A check, its dialog or an install is in progress.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

/// What a worker reports.
pub(in crate::window) enum UpdateResult {
    Checked {
        config: Box<UpdateConfig>,
        outcome: Result<CheckOutcome, String>,
    },
    Installed(Result<(), String>),
}

/// The menu item is enabled only for a portable (unpackaged) process.
pub(in crate::window) fn menu_enabled() -> bool {
    !is_packaged()
}

pub(in crate::window) fn request_check(hwnd: HWND) {
    if BUSY.with(Cell::get) {
        return;
    }
    // The item is disabled for MSIX; a command that arrives anyway gets
    // the .NET packaged-installation text.
    if is_packaged() {
        warn(hwnd, unavailable_notice(PACKAGED_MESSAGE));
        return;
    }
    let config = match UpdateConfig::embedded() {
        Ok(config) => config,
        Err(reason) => {
            warn(hwnd, unavailable_notice(reason.message()));
            return;
        }
    };
    let Some(architecture) = Architecture::current() else {
        return;
    };
    BUSY.with(|busy| busy.set(true));
    let mut started = false;
    with_chrome(hwnd, |chrome| {
        started = chrome.link.spawn("marknexia-update-check", move || {
            let fetcher = WinHttpFetcher {
                hosts: config.allowed_hosts.clone(),
            };
            let layout = Layout::default_root().map(Layout::new);
            let highest = layout.as_ref().and_then(activation::read_highest_seen);
            let outcome = check_for_update(
                &config,
                &fetcher,
                env!("CARGO_PKG_VERSION"),
                architecture,
                os_build(),
                highest.as_ref(),
                &AtomicBool::new(false),
            )
            .map_err(|error| error.message());
            // A verified, newer manifest raises the floor: an older one is
            // never offered again (a frozen or replayed channel).
            if let (Ok(CheckOutcome::Available(manifest)), Some(layout)) = (&outcome, &layout)
                && ensure_private_dir(&NativeStagePlatform, layout.root()).is_ok()
            {
                let _ = activation::raise_highest_seen(layout, &manifest.version);
            }
            Some(WorkerResult::Update(UpdateResult::Checked {
                config: Box::new(config),
                outcome,
            }))
        });
    });
    if !started {
        BUSY.with(|busy| busy.set(false));
        warn(hwnd, unavailable_notice("the check could not start."));
    }
}

/// A worker finished: queue it as a chrome action (dialogs run there).
pub(in crate::window) fn deliver(result: UpdateResult) {
    queue_action(ChromeAction::Update(result));
}

/// Runs from the chrome action queue with no borrow held.
pub(in crate::window) fn apply(hwnd: HWND, result: UpdateResult) {
    match result {
        UpdateResult::Checked {
            outcome: Err(error),
            ..
        } => {
            BUSY.with(|busy| busy.set(false));
            warn(hwnd, unavailable_notice(&error));
        }
        UpdateResult::Checked {
            outcome: Ok(CheckOutcome::UpToDate),
            ..
        } => {
            dialogs::show_up_to_date(hwnd, &up_to_date_text(env!("CARGO_PKG_VERSION")));
            BUSY.with(|busy| busy.set(false));
        }
        UpdateResult::Checked {
            outcome: Ok(CheckOutcome::Available(manifest)),
            config,
        } => {
            let install = dialogs::show_update_available(hwnd, &available_text(&manifest.version))
                == Some(true);
            if install {
                start_install(hwnd, *config, manifest);
            } else {
                BUSY.with(|busy| busy.set(false));
            }
        }
        UpdateResult::Installed(Ok(())) => {
            // The new version acknowledged its health and is running: this
            // (old) version closes.
            // SAFETY: posts to this thread's own live window; no pointers.
            let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        UpdateResult::Installed(Err(error)) => {
            BUSY.with(|busy| busy.set(false));
            warn(hwnd, install_failed_notice(&error));
        }
    }
}

fn start_install(hwnd: HWND, config: UpdateConfig, manifest: Manifest) {
    let mut started = false;
    with_chrome(hwnd, |chrome| {
        chrome.diagnostic(DOWNLOADING_NOTICE, DiagnosticSeverity::Info);
        started = chrome.link.spawn("marknexia-update-install", move || {
            Some(WorkerResult::Update(UpdateResult::Installed(install(
                &config, &manifest,
            ))))
        });
    });
    if !started {
        BUSY.with(|busy| busy.set(false));
        warn(hwnd, install_failed_notice("the download could not start."));
    }
}

/// Download, verify and stage side by side, then activate with the
/// nonce-bound health check (up to 60 seconds) and roll back on failure.
fn install(config: &UpdateConfig, manifest: &Manifest) -> Result<(), String> {
    let root = Layout::default_root().ok_or("the local data folder is not available.")?;
    let layout = Layout::new(root);
    // An interrupted earlier attempt is resolved first.
    let _ = activation::recover(&layout, &is_process_running);
    let policy = PayloadPolicy::rust_portable();
    let request = StageRequest {
        manifest,
        policy: &policy,
        limits: ArchiveLimits::default(),
        publisher: &config.publisher,
    };
    let fetcher = WinHttpFetcher {
        hosts: config.allowed_hosts.clone(),
    };
    let staged = download_and_stage(
        &request,
        &layout,
        &fetcher,
        &NativeStagePlatform,
        &AtomicBool::new(false),
    )
    .map_err(|error| error.message())?;
    let nonce = random_nonce().ok_or("no random nonce is available.")?;
    activation::activate(
        &layout,
        &staged,
        &nonce,
        &ProcessLauncher,
        current_process(),
        &|| Box::new(SystemClock::start()),
    )
    .map_err(|error| error.message())
}

fn warn(hwnd: HWND, text: String) {
    with_chrome(hwnd, |chrome| {
        chrome.diagnostic(text, DiagnosticSeverity::Warning);
    });
}
