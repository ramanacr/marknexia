//! Native adapters for the portable updater (`marknexia-update`): the
//! WinHTTP fetcher with the configured exact host allowlist, owner-only
//! staging folders, the PE machine and Authenticode checks of staged
//! executables, the MSIX identity check, the nonce source and the process
//! liveness probe. No test drives these against a server or a real
//! installer; the decisions they feed are tested headless in
//! `marknexia-update`.

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::AtomicBool,
};

use marknexia_update::{
    activation::Nonce,
    manifest::HttpsUrl,
    payload::{AllowedExecutable, Signer, pe_machine},
    stage::{FetchError, Fetcher, StagePlatform},
};
use marknexia_webview::{
    bootstrap::{
        BootstrapperDownloader, DownloadError, DownloadLimits, DownloadUrl, SignatureError,
        SignatureVerifier, StagedFile, open_locked,
    },
    bootstrap_native::{
        AuthenticodeVerifier, DownloadAbort, WinHttpDownloader, create_owner_only_dir,
        verify_publisher,
    },
};
use windows::Win32::{
    Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, STILL_ACTIVE},
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    Storage::Packaging::Appx::GetCurrentPackageFullName,
    System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
};

/// True when this process runs with an MSIX package identity (Store or
/// sideloaded package): the portable updater never runs then.
pub(super) fn is_packaged() -> bool {
    let mut length = 0_u32;
    // SAFETY: sizing call with a null buffer; `length` is a local.
    let status = unsafe { GetCurrentPackageFullName(&mut length, None) };
    // APPMODEL_ERROR_NO_PACKAGE (15700) without identity; a packaged
    // process reports the buffer size it needs.
    status == ERROR_INSUFFICIENT_BUFFER
}

/// 32 bytes from the system RNG.
pub(super) fn random_nonce() -> Option<Nonce> {
    let mut bytes = [0_u8; 32];
    // SAFETY: `bytes` is a live local buffer; no algorithm handle.
    let status = unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    status.is_ok().then(|| Nonce::from_bytes(bytes))
}

/// True while process `pid` exists and has not exited.
pub(super) fn is_process_running(pid: u32) -> bool {
    // SAFETY: query-only access; the handle is closed below.
    let Ok(handle) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };
    let mut code = 0_u32;
    // SAFETY: live handle opened above; `code` is a local.
    let running =
        unsafe { GetExitCodeProcess(handle, &mut code) }.is_ok() && code == STILL_ACTIVE.0 as u32;
    // SAFETY: closes the handle opened above exactly once.
    let _ = unsafe { CloseHandle(handle) };
    running
}

/// A normal (non-health) start: resolves an interrupted update in
/// `%LOCALAPPDATA%\Marknexia\Updates` (rollback before either version
/// runs), then, when the active pointer names a newer portable payload
/// than this executable, starts that payload's shell with the same
/// arguments and returns `true` (this process then exits). Without an
/// update folder, or for a packaged process, nothing happens.
pub(super) fn forward_to_active_version(arguments: &[std::ffi::OsString]) -> bool {
    use marknexia_update::{
        activation::{self, Route},
        layout::Layout,
        payload::SHELL_EXECUTABLE,
        version::Version,
    };
    if is_packaged() {
        return false;
    }
    let Some(root) = Layout::default_root() else {
        return false;
    };
    let layout = Layout::new(root);
    if !layout.active_file().exists() && !layout.transaction_file().exists() {
        return false;
    }
    let _ = activation::recover(&layout, &is_process_running);
    let (Ok(executable), Ok(version)) = (
        std::env::current_exe(),
        Version::parse(env!("CARGO_PKG_VERSION")),
    ) else {
        return false;
    };
    match activation::route(&layout, &executable, &version, SHELL_EXECUTABLE) {
        Route::RunHere => false,
        Route::Forward(shell) => {
            let Some(folder) = shell.parent() else {
                return false;
            };
            std::process::Command::new(&shell)
                .args(arguments)
                .current_dir(folder)
                .spawn()
                .is_ok()
        }
    }
}

/// The Windows build number (registry `CurrentBuildNumber`).
pub(super) fn os_build() -> u32 {
    super::shell_ops::os_version()
        .split('.')
        .nth(2)
        .and_then(|build| build.parse().ok())
        .unwrap_or(0)
}

/// WinHTTP (the WebView2 bootstrapper adapter) with the update channel's
/// exact host allowlist for the URL and every redirect, TLS 1.2 or later,
/// the bootstrapper's timeouts and a size bound per request.
pub(super) struct WinHttpFetcher {
    pub(super) hosts: Vec<String>,
}

impl Fetcher for WinHttpFetcher {
    fn fetch(
        &self,
        url: &HttpsUrl,
        max_bytes: u64,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, FetchError> {
        let url = DownloadUrl::parse_with_hosts(&url.to_string(), &self.hosts)
            .map_err(|_| FetchError::UntrustedLocation)?;
        let limits = DownloadLimits {
            max_bytes,
            min_bytes: 0,
            ..DownloadLimits::default()
        };
        WinHttpDownloader::with_abort(DownloadAbort::new())
            .with_agent("Marknexia updater")
            .download(&url, &limits, cancel)
            .map_err(|error| match error {
                DownloadError::Offline(_) => FetchError::Offline,
                DownloadError::TimedOut => FetchError::TimedOut,
                DownloadError::Cancelled => FetchError::Cancelled,
                DownloadError::Url(_) => FetchError::UntrustedLocation,
                DownloadError::TooManyRedirects => FetchError::TooManyRedirects,
                DownloadError::MissingLocation => FetchError::Network(0),
                DownloadError::Http(status) => FetchError::Http(status),
                DownloadError::TooLarge => FetchError::TooLarge,
                DownloadError::Truncated => FetchError::Truncated,
                DownloadError::Network(code) => FetchError::Network(code),
            })
    }
}

/// Owner-only folders (`D:P(A;OICI;FA;;;OW)`) and the executable checks
/// on a write-locked handle.
pub(super) struct NativeStagePlatform;

impl StagePlatform for NativeStagePlatform {
    fn create_private_dir(&self, path: &Path) -> std::io::Result<()> {
        create_owner_only_dir(path)
    }

    fn check_executable(
        &self,
        path: &Path,
        executable: &AllowedExecutable,
        machine: u16,
        publisher: &str,
    ) -> Result<(), String> {
        let mut file = open_locked(path)
            .map_err(|error| format!("it could not be opened ({}).", error.kind()))?;
        let mut header = vec![0_u8; 4096];
        let read = file
            .read(&mut header)
            .map_err(|error| format!("it could not be read ({}).", error.kind()))?;
        if pe_machine(&header[..read]) != Some(machine) {
            return Err("it is not a native executable for this architecture.".to_owned());
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|error| format!("it could not be read ({}).", error.kind()))?;
        let folder = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let staged = StagedFile::locked(path.to_path_buf(), folder, file);
        let result = match executable.signer {
            Signer::Release => verify_publisher(&staged, publisher),
            Signer::Microsoft => AuthenticodeVerifier.verify(&staged),
        };
        result.map_err(|error| signature_reason(&error))
    }
}

fn signature_reason(error: &SignatureError) -> String {
    match error {
        SignatureError::Untrusted(code) => {
            format!("its signature is not trusted (0x{:08X}).", *code as u32)
        }
        SignatureError::NoSigner => "it is not signed.".to_owned(),
        SignatureError::NotMicrosoft { signer }
        | SignatureError::UnexpectedPublisher { signer } => {
            format!("it is signed by '{signer}', not by the expected publisher.")
        }
        SignatureError::NotMicrosoftRoot(_) => {
            "its signature does not chain to a Microsoft root.".to_owned()
        }
        SignatureError::NoCodeSigningUsage => {
            "its certificate is not valid for code signing.".to_owned()
        }
    }
}
