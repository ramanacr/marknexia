//! Native adapters for the bootstrapper install (`bootstrap`): a WinHTTP
//! downloader, an Authenticode verifier (`WinVerifyTrust`) with a
//! Microsoft-signer check, and a non-elevated `CreateProcessW` runner.
//!
//! No test calls these: they would reach the network or run the real
//! bootstrapper. Their pure decisions (URL and redirect validation, size
//! bounds, error and exit-code mapping, signer names) live in `bootstrap`
//! and are tested there.

use std::{
    ffi::c_void,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ELEVATION_REQUIRED, GetLastError, HANDLE, HWND,
            INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
        },
        Networking::WinHttp::{
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE,
            WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3,
            WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_SECURE_PROTOCOLS,
            WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_FLAG_NUMBER64,
            WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect,
            WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders,
            WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
            WinHttpSetTimeouts,
        },
        Security::{
            Cryptography::{CERT_CONTEXT, CERT_NAME_SIMPLE_DISPLAY_TYPE, CertGetNameStringW},
            WinTrust::{
                WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
                WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_DISABLE_MD2_MD4,
                WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT, WTD_REVOKE_WHOLECHAIN,
                WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTD_UICONTEXT_EXECUTE,
                WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
            },
        },
        System::Threading::{
            CreateProcessW, GetExitCodeProcess, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
            STARTUPINFOW, WaitForSingleObject,
        },
    },
    core::{PCWSTR, PWSTR, w},
};

use crate::bootstrap::{
    BootstrapperDownloader, BoundedBody, DownloadError, DownloadLimits, DownloadUrl,
    INSTALLER_ARGUMENTS, InstallError, InstallerRunner, MAX_REDIRECTS, SignatureError,
    SignatureVerifier, StagedFile, classify_exit_code, classify_network_error, is_microsoft_signer,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

fn win32_code(error: &windows::core::Error) -> u32 {
    let code = error.code().0 as u32;
    // HRESULT_FROM_WIN32: facility 7.
    if code & 0xFFFF_0000 == 0x8007_0000 {
        code & 0xFFFF
    } else {
        code
    }
}

fn last_error() -> u32 {
    // SAFETY: reads this thread's last-error value.
    unsafe { GetLastError() }.0
}

fn millis(duration: Duration) -> i32 {
    i32::try_from(duration.as_millis()).unwrap_or(i32::MAX)
}

// ---------------------------------------------------------------------------
// WinHTTP downloader.
// ---------------------------------------------------------------------------

/// One WinHTTP handle, closed on drop.
struct Internet(*mut c_void);

impl Internet {
    fn new(raw: *mut c_void) -> Result<Self, DownloadError> {
        if raw.is_null() {
            Err(classify_network_error(last_error()))
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Internet {
    fn drop(&mut self) {
        // SAFETY: the handle came from WinHTTP and is closed exactly once;
        // child handles are dropped before their parents (declaration order
        // in `fetch_once`).
        let _ = unsafe { WinHttpCloseHandle(self.0) };
    }
}

enum Fetched {
    Body(Vec<u8>),
    Redirect(String),
}

/// Synchronous WinHTTP over HTTPS (TLS 1.2 or later), with the system
/// proxy, manual redirects, per-phase timeouts, an overall deadline and a
/// size bound. Cancellation is checked between reads.
#[derive(Debug, Default, Clone, Copy)]
pub struct WinHttpDownloader;

impl BootstrapperDownloader for WinHttpDownloader {
    fn download(
        &self,
        url: &DownloadUrl,
        limits: &DownloadLimits,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, DownloadError> {
        let deadline = Instant::now() + limits.total_timeout;
        // SAFETY: static agent string; no proxy override strings.
        let session = Internet::new(unsafe {
            WinHttpOpen(
                w!("Marknexia WebView2 setup"),
                WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            )
        })?;
        // SAFETY: live session handle.
        unsafe {
            WinHttpSetTimeouts(
                session.0,
                millis(limits.resolve_timeout),
                millis(limits.connect_timeout),
                millis(limits.send_timeout),
                millis(limits.receive_timeout),
            )
        }
        .map_err(|error| classify_network_error(win32_code(&error)))?;
        set_secure_protocols(&session)?;
        let mut current = url.clone();
        for _ in 0..=MAX_REDIRECTS {
            match fetch_once(&session, &current, limits, cancel, deadline)? {
                Fetched::Body(bytes) => return Ok(bytes),
                Fetched::Redirect(location) => {
                    current = current.resolve(&location).map_err(DownloadError::Url)?;
                }
            }
        }
        Err(DownloadError::TooManyRedirects)
    }
}

fn set_secure_protocols(session: &Internet) -> Result<(), DownloadError> {
    let modern = WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;
    // Older builds reject the TLS 1.3 flag: fall back to TLS 1.2 only.
    for protocols in [modern, WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2] {
        // SAFETY: live session handle; the option buffer is a local u32.
        let result = unsafe {
            WinHttpSetOption(
                Some(session.0),
                WINHTTP_OPTION_SECURE_PROTOCOLS,
                Some(&protocols.to_ne_bytes()),
            )
        };
        if result.is_ok() {
            return Ok(());
        }
    }
    Err(DownloadError::Network(last_error()))
}

fn check_running(cancel: &AtomicBool, deadline: Instant) -> Result<(), DownloadError> {
    if cancel.load(Ordering::Acquire) {
        return Err(DownloadError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(DownloadError::TimedOut);
    }
    Ok(())
}

fn fetch_once(
    session: &Internet,
    url: &DownloadUrl,
    limits: &DownloadLimits,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<Fetched, DownloadError> {
    check_running(cancel, deadline)?;
    let host = wide(url.host());
    let target = wide(url.path_and_query());
    let network = |error: windows::core::Error| classify_network_error(win32_code(&error));
    // SAFETY: `host` is NUL-terminated and outlives the call.
    let connection = Internet::new(unsafe {
        WinHttpConnect(session.0, PCWSTR::from_raw(host.as_ptr()), 443, 0)
    })?;
    // SAFETY: `target` is NUL-terminated and outlives the call; no referrer
    // and no accept-type list (null).
    let request = Internet::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR::from_raw(target.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    // Redirects are followed here, one validated hop at a time.
    // SAFETY: live request handle; the option buffer is a local u32.
    unsafe {
        WinHttpSetOption(
            Some(request.0),
            WINHTTP_OPTION_DISABLE_FEATURE,
            Some(&WINHTTP_DISABLE_REDIRECTS.to_ne_bytes()),
        )
    }
    .map_err(network)?;
    // SAFETY: live request handle; no headers and no body.
    unsafe { WinHttpSendRequest(request.0, None, None, 0, 0, 0) }.map_err(network)?;
    check_running(cancel, deadline)?;
    // SAFETY: live request handle; reserved pointer is null.
    unsafe { WinHttpReceiveResponse(request.0, std::ptr::null_mut()) }.map_err(network)?;
    let status = query_u32(&request, WINHTTP_QUERY_STATUS_CODE).ok_or(DownloadError::Http(0))?;
    match status {
        301 | 302 | 303 | 307 | 308 => {
            return query_text(&request, WINHTTP_QUERY_LOCATION)
                .map(Fetched::Redirect)
                .ok_or(DownloadError::MissingLocation);
        }
        200 => {}
        other => {
            return Err(DownloadError::Http(
                u16::try_from(other).unwrap_or(u16::MAX),
            ));
        }
    }
    let declared = query_u64(&request, WINHTTP_QUERY_CONTENT_LENGTH);
    let mut body = BoundedBody::new(limits.max_bytes, declared)?;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        check_running(cancel, deadline)?;
        let mut available = 0_u32;
        // SAFETY: live request handle; `available` is a local.
        unsafe { WinHttpQueryDataAvailable(request.0, &mut available) }.map_err(network)?;
        if available == 0 {
            break;
        }
        let wanted = available.min(buffer.len() as u32);
        let mut read = 0_u32;
        // SAFETY: `buffer` holds at least `wanted` bytes; `read` is a local.
        unsafe { WinHttpReadData(request.0, buffer.as_mut_ptr().cast(), wanted, &mut read) }
            .map_err(network)?;
        if read == 0 {
            break;
        }
        body.push(&buffer[..read as usize])?;
    }
    drop(request);
    drop(connection);
    body.finish().map(Fetched::Body)
}

fn query_u32(request: &Internet, info: u32) -> Option<u32> {
    let mut value = 0_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: live request handle; the buffer is a local u32 of `size`.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            info | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(std::ptr::from_mut(&mut value).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
    }
    .ok()
    .map(|()| value)
}

fn query_u64(request: &Internet, info: u32) -> Option<u64> {
    let mut value = 0_u64;
    let mut size = std::mem::size_of::<u64>() as u32;
    // SAFETY: live request handle; the buffer is a local u64 of `size`.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            info | WINHTTP_QUERY_FLAG_NUMBER64,
            PCWSTR::null(),
            Some(std::ptr::from_mut(&mut value).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
    }
    .ok()
    .map(|()| value)
}

fn query_text(request: &Internet, info: u32) -> Option<String> {
    // Bytes, including the terminator; longer values are refused.
    const MAX_BYTES: usize = (crate::bootstrap::MAX_URL_LENGTH + 1) * 2;
    let mut buffer = vec![0_u16; MAX_BYTES / 2];
    let mut size = MAX_BYTES as u32;
    // SAFETY: live request handle; `buffer` holds `size` bytes.
    unsafe {
        WinHttpQueryHeaders(
            request.0,
            info,
            PCWSTR::null(),
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
            std::ptr::null_mut(),
        )
    }
    .ok()?;
    let units = (size as usize / 2).min(buffer.len());
    String::from_utf16(&buffer[..units]).ok()
}

// ---------------------------------------------------------------------------
// Authenticode.
// ---------------------------------------------------------------------------

/// `WinVerifyTrust` (generic verify v2, whole-chain revocation, no UI) on
/// the locked staged file, then the Microsoft signer check on its chain.
#[derive(Debug, Default, Clone, Copy)]
pub struct AuthenticodeVerifier;

impl SignatureVerifier for AuthenticodeVerifier {
    fn verify(&self, file: &StagedFile) -> Result<(), SignatureError> {
        let path = wide_path(file.path());
        let handle = file
            .lock()
            .map_or(HANDLE::default(), |lock| HANDLE(lock.as_raw_handle()));
        let mut file_info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: PCWSTR::from_raw(path.as_ptr()),
            hFile: handle,
            pgKnownSubject: std::ptr::null_mut(),
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 {
                pFile: &mut file_info,
            },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT | WTD_DISABLE_MD2_MD4,
            dwUIContext: WTD_UICONTEXT_EXECUTE,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        // No interactive window: INVALID_HANDLE_VALUE.
        let no_ui = HWND(INVALID_HANDLE_VALUE.0);
        // SAFETY: `data`, `file_info`, `path` and `action` are locals that
        // outlive the call; the file handle stays open in `file`.
        let status = unsafe {
            WinVerifyTrust(
                no_ui,
                &mut action,
                std::ptr::from_mut(&mut data).cast::<c_void>(),
            )
        };
        let result = if status == 0 {
            // SAFETY: the state data is valid until the close call below.
            unsafe { signer_names(data.hWVTStateData) }.and_then(|(leaf, root)| {
                if is_microsoft_signer(&leaf, &root) {
                    Ok(())
                } else {
                    Err(SignatureError::NotMicrosoft { signer: leaf })
                }
            })
        } else {
            Err(SignatureError::Untrusted(status))
        };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: releases the state data of the verify call above.
        let _ = unsafe {
            WinVerifyTrust(
                no_ui,
                &mut action,
                std::ptr::from_mut(&mut data).cast::<c_void>(),
            )
        };
        result
    }
}

/// The leaf and root certificate names of the first signer.
///
/// # Safety
/// `state` must be the live state data of a successful verify call.
unsafe fn signer_names(state: HANDLE) -> Result<(String, String), SignatureError> {
    // SAFETY: per the contract, `state` is live; the helpers return
    // pointers into it (or null), valid until the state is closed.
    unsafe {
        let provider = WTHelperProvDataFromStateData(state);
        if provider.is_null() {
            return Err(SignatureError::NoSigner);
        }
        let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
        let Some(signer) = signer.as_ref() else {
            return Err(SignatureError::NoSigner);
        };
        if signer.csCertChain == 0 || signer.pasCertChain.is_null() {
            return Err(SignatureError::NoSigner);
        }
        let chain = std::slice::from_raw_parts(signer.pasCertChain, signer.csCertChain as usize);
        let leaf = certificate_name(chain[0].pCert).ok_or(SignatureError::NoSigner)?;
        let root =
            certificate_name(chain[chain.len() - 1].pCert).ok_or(SignatureError::NoSigner)?;
        Ok((leaf, root))
    }
}

/// # Safety
/// `certificate` must be null or a live certificate context.
unsafe fn certificate_name(certificate: *const CERT_CONTEXT) -> Option<String> {
    if certificate.is_null() {
        return None;
    }
    // SAFETY: live context per the contract; the first call sizes the
    // buffer (characters including the terminator).
    let length =
        unsafe { CertGetNameStringW(certificate, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, None) }
            as usize;
    if length <= 1 || length > 1024 {
        return None;
    }
    let mut buffer = vec![0_u16; length];
    // SAFETY: as above, with a buffer of `length` characters.
    let written = unsafe {
        CertGetNameStringW(
            certificate,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            None,
            Some(&mut buffer),
        )
    } as usize;
    let text = &buffer[..written.saturating_sub(1).min(buffer.len())];
    String::from_utf16(text).ok()
}

// ---------------------------------------------------------------------------
// Installer.
// ---------------------------------------------------------------------------

/// Runs the bootstrapper with `CreateProcessW` (never `runas`, so never
/// elevated by this code) and waits up to `timeout`.
#[derive(Debug, Clone, Copy)]
pub struct ProcessRunner {
    pub timeout: Duration,
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15 * 60),
        }
    }
}

impl InstallerRunner for ProcessRunner {
    fn run(&self, file: &StagedFile) -> Result<(), InstallError> {
        const ERROR_INVALID_NAME: u32 = 123;
        let path = file.path();
        let Some(text) = path.to_str().filter(|text| !text.contains('"')) else {
            return Err(InstallError::StartFailed(ERROR_INVALID_NAME));
        };
        let application = wide_path(path);
        let mut command_line = wide(&format!("\"{text}\" {INSTALLER_ARGUMENTS}"));
        let folder = wide_path(file.folder());
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut process = PROCESS_INFORMATION::default();
        // SAFETY: every buffer is NUL-terminated and outlives the call;
        // the command line is mutable as CreateProcessW requires; no handle
        // is inherited.
        let started = unsafe {
            CreateProcessW(
                PCWSTR::from_raw(application.as_ptr()),
                Some(PWSTR::from_raw(command_line.as_mut_ptr())),
                None,
                None,
                false,
                PROCESS_CREATION_FLAGS(0),
                None,
                PCWSTR::from_raw(folder.as_ptr()),
                &startup,
                &mut process,
            )
        };
        if let Err(error) = started {
            let code = win32_code(&error);
            return Err(if code == ERROR_ELEVATION_REQUIRED.0 {
                InstallError::ElevationRequired
            } else {
                InstallError::StartFailed(code)
            });
        }
        let wait_ms = u32::try_from(self.timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: both handles came from CreateProcessW and are closed once.
        unsafe {
            let _ = CloseHandle(process.hThread);
            let result = if WaitForSingleObject(process.hProcess, wait_ms) == WAIT_OBJECT_0 {
                let mut code = 0_u32;
                match GetExitCodeProcess(process.hProcess, &mut code) {
                    Ok(()) => classify_exit_code(code),
                    Err(error) => Err(InstallError::ExitCode(win32_code(&error))),
                }
            } else {
                Err(InstallError::TimedOut)
            };
            let _ = CloseHandle(process.hProcess);
            result
        }
    }
}
