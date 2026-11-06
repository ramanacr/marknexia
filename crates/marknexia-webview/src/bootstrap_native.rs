//! Native adapters for the bootstrapper install (`bootstrap`): a WinHTTP
//! downloader with a thread-safe abort, an Authenticode verifier
//! (`WinVerifyTrust`, the Microsoft root chain policy and the code-signing
//! usage), a version-resource reader, an owner-only staging folder, and a
//! non-elevated `CreateProcessW` runner.
//!
//! No test downloads or runs anything through these. Their pure decisions
//! (URL and redirect validation, size bounds, error and exit-code mapping,
//! signer names, usages and version values) live in `bootstrap` and are
//! tested there with in-memory fakes.

use std::{
    ffi::c_void,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ELEVATION_REQUIRED, GetLastError, HANDLE, HLOCAL, HWND,
            INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
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
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
            },
            Cryptography::{
                CERT_CHAIN_CONTEXT, CERT_CHAIN_POLICY_MICROSOFT_ROOT, CERT_CHAIN_POLICY_PARA,
                CERT_CHAIN_POLICY_STATUS, CERT_CONTEXT, CERT_NAME_SIMPLE_DISPLAY_TYPE, CTL_USAGE,
                CertGetEnhancedKeyUsage, CertGetNameStringW, CertVerifyCertificateChainPolicy,
            },
            PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
            WinTrust::{
                WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
                WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_DISABLE_MD2_MD4,
                WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT, WTD_REVOKE_WHOLECHAIN,
                WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE, WTD_UICONTEXT_EXECUTE,
                WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
            },
        },
        Storage::FileSystem::{
            CreateDirectoryW, GetFileVersionInfoSizeW, GetFileVersionInfoW,
            MOVEFILE_DELAY_UNTIL_REBOOT, MoveFileExW, VerQueryValueW,
        },
        System::{
            SystemInformation::GetSystemDirectoryW,
            Threading::{
                CreateProcessW, GetExitCodeProcess, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
                STARTUPINFOW, WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR, w},
};

use crate::bootstrap::{
    BootstrapperDownloader, BoundedBody, DownloadError, DownloadLimits, DownloadUrl,
    INSTALLER_ARGUMENTS, IdentityError, InstallError, InstallerRunner, MAX_REDIRECTS,
    SignatureError, SignatureVerifier, StagedFile, VersionInfo, VersionInfoReader,
    classify_exit_code, classify_network_error, has_code_signing_usage, is_microsoft_signer,
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
// Staging folder.
// ---------------------------------------------------------------------------

/// Creates `folder` with a protected DACL that grants full access to the
/// folder's owner only (`OW`, inherited by the staged file). Fails if the
/// folder exists.
pub fn create_owner_only_dir(folder: &Path) -> std::io::Result<()> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: static SDDL string; the descriptor is freed with LocalFree.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:P(A;OICI;FA;;;OW)"),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(|error| std::io::Error::from_raw_os_error(win32_code(&error) as i32))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let path = wide_path(folder);
    // SAFETY: `path` and `attributes` outlive the call; the descriptor is
    // freed once after it.
    let created = unsafe { CreateDirectoryW(PCWSTR::from_raw(path.as_ptr()), Some(&attributes)) };
    // SAFETY: allocated by the conversion above, freed exactly once.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    created.map_err(|error| std::io::Error::from_raw_os_error(win32_code(&error) as i32))
}

/// Schedules `file` (if given), then `folder`, for deletion at the next
/// restart (`MOVEFILE_DELAY_UNTIL_REBOOT`). Best effort: Windows accepts it
/// only from an administrator, so for a standard user the folder stays in
/// `%TEMP%` until it is cleaned.
pub fn delete_at_restart(file: Option<&Path>, folder: &Path) {
    for path in file.into_iter().chain(std::iter::once(folder)) {
        let path = wide_path(path);
        // SAFETY: NUL-terminated path that outlives the call; no target.
        let _ = unsafe {
            MoveFileExW(
                PCWSTR::from_raw(path.as_ptr()),
                PCWSTR::null(),
                MOVEFILE_DELAY_UNTIL_REBOOT,
            )
        };
    }
}

// ---------------------------------------------------------------------------
// WinHTTP downloader.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct AbortState {
    aborted: bool,
    /// The request handle in use, owned here while registered.
    request: Option<usize>,
}

/// Aborts a running download from any thread: it closes the active request
/// handle (`WinHttpCloseHandle` may be called from another thread), so a
/// blocked send, receive or read returns at once, and refuses later
/// requests.
#[derive(Clone, Default)]
pub struct DownloadAbort(Arc<Mutex<AbortState>>);

impl DownloadAbort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        state.aborted = true;
        if let Some(request) = state.request.take() {
            // SAFETY: the handle was registered by its owner and is taken out
            // of the slot under the lock, so it is closed exactly once.
            let _ = unsafe { WinHttpCloseHandle(request as *mut c_void) };
        }
    }

    fn register(&self, request: *mut c_void) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.aborted {
            return false;
        }
        state.request = Some(request as usize);
        true
    }

    /// Takes the handle back; `false` once `abort` closed it.
    fn release(&self, request: *mut c_void) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if state.request == Some(request as usize) {
            state.request = None;
            true
        } else {
            false
        }
    }
}

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

/// A request handle shared with a [`DownloadAbort`]: closed on drop unless
/// the abort already closed it.
struct Request<'a> {
    raw: *mut c_void,
    abort: &'a DownloadAbort,
}

impl Drop for Request<'_> {
    fn drop(&mut self) {
        if self.abort.release(self.raw) {
            // SAFETY: released from the abort slot, so this is the only
            // close of the handle.
            let _ = unsafe { WinHttpCloseHandle(self.raw) };
        }
    }
}

enum Fetched {
    Body(Vec<u8>),
    Redirect(String),
}

/// Synchronous WinHTTP over HTTPS (TLS 1.2 or later), with the system
/// proxy, manual redirects, per-phase timeouts, an overall deadline and a
/// size bound. Cancellation is checked between calls, and
/// [`DownloadAbort::abort`] interrupts a blocked call.
#[derive(Clone, Default)]
pub struct WinHttpDownloader {
    abort: DownloadAbort,
}

impl WinHttpDownloader {
    pub fn with_abort(abort: DownloadAbort) -> Self {
        Self { abort }
    }
}

impl BootstrapperDownloader for WinHttpDownloader {
    fn download(
        &self,
        url: &DownloadUrl,
        limits: &DownloadLimits,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, DownloadError> {
        let result = self.download_inner(url, limits, cancel);
        // An aborted handle fails with a network error: report the cancel.
        result.map_err(|error| {
            if cancel.load(Ordering::Acquire) {
                DownloadError::Cancelled
            } else {
                error
            }
        })
    }
}

impl WinHttpDownloader {
    fn download_inner(
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
            match self.fetch_once(&session, &current, limits, cancel, deadline)? {
                Fetched::Body(bytes) => return Ok(bytes),
                Fetched::Redirect(location) => {
                    current = current.resolve(&location).map_err(DownloadError::Url)?;
                }
            }
        }
        Err(DownloadError::TooManyRedirects)
    }

    fn fetch_once(
        &self,
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
        // SAFETY: `target` is NUL-terminated and outlives the call; no
        // referrer and no accept-type list (null).
        let raw = unsafe {
            WinHttpOpenRequest(
                connection.0,
                w!("GET"),
                PCWSTR::from_raw(target.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                std::ptr::null(),
                WINHTTP_FLAG_SECURE,
            )
        };
        if raw.is_null() {
            return Err(classify_network_error(last_error()));
        }
        if !self.abort.register(raw) {
            // SAFETY: never registered, so this is the only close.
            let _ = unsafe { WinHttpCloseHandle(raw) };
            return Err(DownloadError::Cancelled);
        }
        let request = Request {
            raw,
            abort: &self.abort,
        };
        // Redirects are followed here, one validated hop at a time.
        // SAFETY: the request handle is live (or closed by an abort, which
        // makes the call fail); the option buffer is a local u32.
        unsafe {
            WinHttpSetOption(
                Some(request.raw),
                WINHTTP_OPTION_DISABLE_FEATURE,
                Some(&WINHTTP_DISABLE_REDIRECTS.to_ne_bytes()),
            )
        }
        .map_err(network)?;
        // SAFETY: as above; no headers and no body.
        unsafe { WinHttpSendRequest(request.raw, None, None, 0, 0, 0) }.map_err(network)?;
        check_running(cancel, deadline)?;
        // SAFETY: as above; reserved pointer is null.
        unsafe { WinHttpReceiveResponse(request.raw, std::ptr::null_mut()) }.map_err(network)?;
        let status =
            query_u32(request.raw, WINHTTP_QUERY_STATUS_CODE).ok_or(DownloadError::Http(0))?;
        match status {
            301 | 302 | 303 | 307 | 308 => {
                return query_text(request.raw, WINHTTP_QUERY_LOCATION)
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
        let declared = query_u64(request.raw, WINHTTP_QUERY_CONTENT_LENGTH);
        let mut body = BoundedBody::new(limits.max_bytes, declared)?;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            check_running(cancel, deadline)?;
            let mut available = 0_u32;
            // SAFETY: as above; `available` is a local.
            unsafe { WinHttpQueryDataAvailable(request.raw, &mut available) }.map_err(network)?;
            if available == 0 {
                break;
            }
            let wanted = available.min(buffer.len() as u32);
            let mut read = 0_u32;
            // SAFETY: `buffer` holds at least `wanted` bytes; `read` is a
            // local.
            unsafe { WinHttpReadData(request.raw, buffer.as_mut_ptr().cast(), wanted, &mut read) }
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

fn query_u32(request: *mut c_void, info: u32) -> Option<u32> {
    let mut value = 0_u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: request handle from `fetch_once`; the buffer is a local u32.
    unsafe {
        WinHttpQueryHeaders(
            request,
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

fn query_u64(request: *mut c_void, info: u32) -> Option<u64> {
    let mut value = 0_u64;
    let mut size = std::mem::size_of::<u64>() as u32;
    // SAFETY: request handle from `fetch_once`; the buffer is a local u64.
    unsafe {
        WinHttpQueryHeaders(
            request,
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

fn query_text(request: *mut c_void, info: u32) -> Option<String> {
    // Bytes, including the terminator; longer values are refused.
    const MAX_BYTES: usize = (crate::bootstrap::MAX_URL_LENGTH + 1) * 2;
    let mut buffer = vec![0_u16; MAX_BYTES / 2];
    let mut size = MAX_BYTES as u32;
    // SAFETY: request handle from `fetch_once`; `buffer` holds `size` bytes.
    unsafe {
        WinHttpQueryHeaders(
            request,
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
/// the locked staged file. Then, before the state is closed: the Microsoft
/// root chain policy on the signer's verified chain, an explicit
/// code-signing usage on the leaf, and the signer and root names.
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
            unsafe { check_signer(data.hWVTStateData) }
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

/// The first signer's checks.
///
/// # Safety
/// `state` must be the live state data of a successful verify call.
unsafe fn check_signer(state: HANDLE) -> Result<(), SignatureError> {
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
        microsoft_root_policy(signer.pChainContext)?;
        let chain = std::slice::from_raw_parts(signer.pasCertChain, signer.csCertChain as usize);
        let leaf_certificate = chain[0].pCert;
        if !has_code_signing_usage(&enhanced_key_usages(leaf_certificate)) {
            return Err(SignatureError::NoCodeSigningUsage);
        }
        let leaf = certificate_name(leaf_certificate).ok_or(SignatureError::NoSigner)?;
        let root =
            certificate_name(chain[chain.len() - 1].pCert).ok_or(SignatureError::NoSigner)?;
        if is_microsoft_signer(&leaf, &root) {
            Ok(())
        } else {
            Err(SignatureError::NotMicrosoft { signer: leaf })
        }
    }
}

/// `CERT_CHAIN_POLICY_MICROSOFT_ROOT` on the chain `WinVerifyTrust` built.
///
/// # Safety
/// `chain` must be null or a live chain context.
unsafe fn microsoft_root_policy(chain: *const CERT_CHAIN_CONTEXT) -> Result<(), SignatureError> {
    const CRYPT_E_NOT_FOUND: u32 = 0x8009_2004;
    if chain.is_null() {
        return Err(SignatureError::NotMicrosoftRoot(CRYPT_E_NOT_FOUND));
    }
    let para = CERT_CHAIN_POLICY_PARA {
        cbSize: std::mem::size_of::<CERT_CHAIN_POLICY_PARA>() as u32,
        ..Default::default()
    };
    let mut status = CERT_CHAIN_POLICY_STATUS {
        cbSize: std::mem::size_of::<CERT_CHAIN_POLICY_STATUS>() as u32,
        ..Default::default()
    };
    // SAFETY: live chain per the contract; `para` and `status` are locals.
    let called = unsafe {
        CertVerifyCertificateChainPolicy(
            CERT_CHAIN_POLICY_MICROSOFT_ROOT,
            chain,
            &para,
            &mut status,
        )
    };
    if !called.as_bool() {
        return Err(SignatureError::NotMicrosoftRoot(last_error()));
    }
    if status.dwError != 0 {
        return Err(SignatureError::NotMicrosoftRoot(status.dwError));
    }
    Ok(())
}

/// The certificate's explicit enhanced key usages (none when the extension
/// is absent or unreadable).
///
/// # Safety
/// `certificate` must be null or a live certificate context.
unsafe fn enhanced_key_usages(certificate: *const CERT_CONTEXT) -> Vec<String> {
    if certificate.is_null() {
        return Vec::new();
    }
    let mut size = 0_u32;
    // SAFETY: sizing call on a live context.
    if unsafe { CertGetEnhancedKeyUsage(certificate, 0, None, &mut size) }.is_err()
        || size == 0
        || size > 64 * 1024
    {
        return Vec::new();
    }
    // u64 storage keeps the CTL_USAGE header aligned.
    let mut buffer = vec![0_u64; (size as usize).div_ceil(8)];
    let usage = buffer.as_mut_ptr().cast::<CTL_USAGE>();
    // SAFETY: `buffer` holds `size` bytes, suitably aligned.
    if unsafe { CertGetEnhancedKeyUsage(certificate, 0, Some(usage), &mut size) }.is_err() {
        return Vec::new();
    }
    // SAFETY: filled by the call above; the identifiers point into `buffer`.
    unsafe {
        let usage = &*usage;
        if usage.cUsageIdentifier == 0 || usage.rgpszUsageIdentifier.is_null() {
            return Vec::new();
        }
        std::slice::from_raw_parts(usage.rgpszUsageIdentifier, usage.cUsageIdentifier as usize)
            .iter()
            .filter(|oid| !oid.is_null())
            .filter_map(|oid| oid.to_string().ok())
            .collect()
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
// Version resource.
// ---------------------------------------------------------------------------

/// `GetFileVersionInfoW` and `VerQueryValueW` on the staged file (opened
/// with read sharing, which the staging lock allows; writers stay denied).
#[derive(Debug, Default, Clone, Copy)]
pub struct FileVersionReader;

impl VersionInfoReader for FileVersionReader {
    fn read(&self, file: &StagedFile) -> Result<VersionInfo, IdentityError> {
        const MAX_RESOURCE: u32 = 1024 * 1024;
        let path = wide_path(file.path());
        // SAFETY: NUL-terminated path that outlives the call.
        let size = unsafe { GetFileVersionInfoSizeW(PCWSTR::from_raw(path.as_ptr()), None) };
        if size == 0 || size > MAX_RESOURCE {
            return Err(IdentityError::Unreadable(last_error()));
        }
        let mut block = vec![0_u8; size as usize];
        // SAFETY: `block` holds `size` bytes.
        unsafe {
            GetFileVersionInfoW(
                PCWSTR::from_raw(path.as_ptr()),
                None,
                size,
                block.as_mut_ptr().cast(),
            )
        }
        .map_err(|error| IdentityError::Unreadable(win32_code(&error)))?;
        let translations = translations(&block);
        let value = |name: &str| {
            translations
                .iter()
                .find_map(|(language, code_page)| {
                    query_string(
                        &block,
                        &format!("\\StringFileInfo\\{language:04x}{code_page:04x}\\{name}"),
                    )
                })
                .filter(|value| !value.is_empty())
        };
        Ok(VersionInfo {
            company_name: value("CompanyName"),
            product_name: value("ProductName"),
            original_filename: value("OriginalFilename"),
            file_version: value("FileVersion"),
        })
    }
}

/// The resource's `(language, code page)` pairs, then the common US English
/// Unicode pair as a fallback.
fn translations(block: &[u8]) -> Vec<(u16, u16)> {
    let mut pairs = Vec::new();
    let mut pointer = std::ptr::null_mut::<c_void>();
    let mut length = 0_u32;
    // SAFETY: `block` is a version resource from GetFileVersionInfoW; the
    // returned pointer points into it and is read within its bounds.
    let found = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            w!("\\VarFileInfo\\Translation"),
            &mut pointer,
            &mut length,
        )
    };
    if found.as_bool() && !pointer.is_null() {
        let count = (length as usize / 4).min(16);
        // SAFETY: `length` bytes of WORD pairs inside `block`.
        let words = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), count * 2) };
        pairs.extend(
            words
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[language, code_page]| (*language, *code_page)),
        );
    }
    pairs.push((0x0409, 0x04B0));
    pairs
}

fn query_string(block: &[u8], sub_block: &str) -> Option<String> {
    let sub_block = wide(sub_block);
    let mut pointer = std::ptr::null_mut::<c_void>();
    let mut length = 0_u32;
    // SAFETY: as in `translations`; `length` counts UTF-16 units including
    // the terminator.
    let found = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR::from_raw(sub_block.as_ptr()),
            &mut pointer,
            &mut length,
        )
    };
    if !found.as_bool() || pointer.is_null() || length == 0 {
        return None;
    }
    let units = (length as usize).min(1024);
    // SAFETY: `units` UTF-16 units inside `block`.
    let text = unsafe { std::slice::from_raw_parts(pointer.cast::<u16>(), units) };
    let end = text.iter().position(|unit| *unit == 0).unwrap_or(units);
    String::from_utf16(&text[..end]).ok()
}

// ---------------------------------------------------------------------------
// Installer.
// ---------------------------------------------------------------------------

/// Runs the bootstrapper with `CreateProcessW` (never `runas`, so never
/// elevated by this code), with System32 as its current directory, and
/// waits up to `timeout`. The staged paths of a running installer are
/// published in `running` so shutdown can schedule their removal.
#[derive(Clone)]
pub struct ProcessRunner {
    pub timeout: Duration,
    pub running: Arc<Mutex<Option<(PathBuf, PathBuf)>>>,
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15 * 60),
            running: Arc::default(),
        }
    }
}

fn system_directory() -> Option<Vec<u16>> {
    let mut buffer = vec![0_u16; 260];
    // SAFETY: the buffer length is passed with the slice.
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    buffer.truncate(length);
    buffer.push(0);
    Some(buffer)
}

impl InstallerRunner for ProcessRunner {
    fn run(&self, file: &StagedFile) -> Result<(), InstallError> {
        const ERROR_INVALID_NAME: u32 = 123;
        const ERROR_PATH_NOT_FOUND: u32 = 3;
        let path = file.path();
        let Some(text) = path.to_str().filter(|text| !text.contains('"')) else {
            return Err(InstallError::StartFailed(ERROR_INVALID_NAME));
        };
        let application = wide_path(path);
        let mut command_line = wide(&format!("\"{text}\" {INSTALLER_ARGUMENTS}"));
        let Some(directory) = system_directory() else {
            return Err(InstallError::StartFailed(ERROR_PATH_NOT_FOUND));
        };
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
                PCWSTR::from_raw(directory.as_ptr()),
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
        let publish = |value: Option<(PathBuf, PathBuf)>| {
            *self
                .running
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = value;
        };
        publish(Some((path.to_path_buf(), file.folder().to_path_buf())));
        let wait_ms = u32::try_from(self.timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: both handles came from CreateProcessW and are closed once.
        let result = unsafe {
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
        };
        // A timed-out installer keeps running: the stager's discard then
        // schedules the removal (`delete_at_restart`).
        publish(None);
        result
    }
}
