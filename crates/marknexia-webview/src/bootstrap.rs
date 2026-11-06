//! WebView2 Evergreen missing-runtime state and the user-initiated
//! bootstrapper install (`packaging/webview/README.md`).
//!
//! Everything here is headless: the download, the staging folder, the
//! Authenticode check, the installer run and runtime detection are behind
//! traits. The native adapters (WinHTTP, `WinVerifyTrust`, `CreateProcessW`)
//! are in `bootstrap_native`. Nothing here starts work by itself: an install
//! attempt starts only from [`RuntimeSetup::begin_install`], which the shell
//! calls for an explicit user action, and a failed attempt never retries.

use std::{
    fmt,
    fs::{File, OpenOptions},
    hash::BuildHasher,
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::environment::{BrowserVersionProbe, EvergreenRuntime, RuntimeStatus};

/// Microsoft's Evergreen bootstrapper link (WebView2 distribution docs).
pub const BOOTSTRAPPER_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";
/// Microsoft's WebView2 page with the Evergreen Standalone (offline)
/// installers, shown when the online path cannot be used.
pub const OFFLINE_INSTALLER_PAGE: &str = "https://developer.microsoft.com/microsoft-edge/webview2/";
/// Name of the staged bootstrapper inside its fresh folder.
pub const BOOTSTRAPPER_FILE_NAME: &str = "MicrosoftEdgeWebview2Setup.exe";
/// Command-line arguments for the bootstrapper: install, with its own
/// progress window (so the user can follow and cancel it).
pub const INSTALLER_ARGUMENTS: &str = "/install";
/// Redirects followed from the fwlink to the download host.
pub const MAX_REDIRECTS: usize = 5;
/// Longest URL accepted from the constant or a `Location` header.
pub const MAX_URL_LENGTH: usize = 2048;

/// Bounds for one download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadLimits {
    /// The bootstrapper is about 2 MB; anything larger is refused.
    pub max_bytes: u64,
    /// Anything smaller cannot be the bootstrapper.
    pub min_bytes: u64,
    pub resolve_timeout: Duration,
    pub connect_timeout: Duration,
    pub send_timeout: Duration,
    pub receive_timeout: Duration,
    /// The whole download, redirects included.
    pub total_timeout: Duration,
}

impl Default for DownloadLimits {
    fn default() -> Self {
        Self {
            max_bytes: 8 * 1024 * 1024,
            min_bytes: 64 * 1024,
            resolve_timeout: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(15),
            send_timeout: Duration::from_secs(30),
            receive_timeout: Duration::from_secs(30),
            total_timeout: Duration::from_secs(300),
        }
    }
}

// ---------------------------------------------------------------------------
// URLs.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlError {
    TooLong,
    /// Control characters, spaces, backslashes or non-ASCII text.
    Malformed,
    NotHttps,
    UserInfo,
    /// Only the default HTTPS port is accepted.
    Port,
    /// The host is not `microsoft.com` or one of its subdomains.
    UntrustedHost,
}

/// A validated download location: HTTPS on port 443 to `microsoft.com` or
/// a subdomain, without user information or fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadUrl {
    host: String,
    path_and_query: String,
}

impl DownloadUrl {
    pub fn parse(url: &str) -> Result<Self, UrlError> {
        if url.len() > MAX_URL_LENGTH {
            return Err(UrlError::TooLong);
        }
        check_characters(url)?;
        let scheme_end = url.find("://").ok_or(UrlError::NotHttps)?;
        if !url[..scheme_end].eq_ignore_ascii_case("https") {
            return Err(UrlError::NotHttps);
        }
        let rest = &url[scheme_end + 3..];
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        if authority.contains('@') {
            return Err(UrlError::UserInfo);
        }
        let host = match authority.rsplit_once(':') {
            Some((host, "443")) => host,
            Some(_) => return Err(UrlError::Port),
            None => authority,
        };
        let host = trusted_host(host)?;
        Ok(Self {
            host,
            path_and_query: path_and_query(&rest[authority_end..]),
        })
    }

    /// The next location from a redirect's `Location` header: absolute
    /// (`https://…`), scheme-relative (`//host/…`) or host-relative (`/…`).
    /// Every result passes the same validation as [`Self::parse`].
    pub fn resolve(&self, location: &str) -> Result<Self, UrlError> {
        let location = location.trim();
        if location.len() > MAX_URL_LENGTH {
            return Err(UrlError::TooLong);
        }
        if location.starts_with("//") {
            return Self::parse(&format!("https:{location}"));
        }
        if location.starts_with('/') {
            check_characters(location)?;
            return Ok(Self {
                host: self.host.clone(),
                path_and_query: path_and_query(location),
            });
        }
        if location.contains("://") {
            return Self::parse(location);
        }
        Err(UrlError::Malformed)
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// The request target: path and query, never empty, without fragment.
    pub fn path_and_query(&self) -> &str {
        &self.path_and_query
    }
}

impl fmt::Display for DownloadUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "https://{}{}", self.host, self.path_and_query)
    }
}

fn check_characters(text: &str) -> Result<(), UrlError> {
    if text
        .bytes()
        .any(|byte| !byte.is_ascii_graphic() || byte == b'\\')
    {
        return Err(UrlError::Malformed);
    }
    Ok(())
}

fn trusted_host(host: &str) -> Result<String, UrlError> {
    let host = host.to_ascii_lowercase();
    let valid = !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-')
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..");
    if !valid {
        return Err(UrlError::Malformed);
    }
    if host == "microsoft.com" || host.ends_with(".microsoft.com") {
        Ok(host)
    } else {
        Err(UrlError::UntrustedHost)
    }
}

fn path_and_query(rest: &str) -> String {
    let without_fragment = rest.split('#').next().unwrap_or_default();
    if without_fragment.is_empty() {
        "/".to_owned()
    } else if without_fragment.starts_with('?') {
        format!("/{without_fragment}")
    } else {
        without_fragment.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Download.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadError {
    /// The host could not be resolved or reached (Win32 error code).
    Offline(u32),
    TimedOut,
    Cancelled,
    Url(UrlError),
    TooManyRedirects,
    /// A redirect without a usable `Location` header.
    MissingLocation,
    /// A final status other than 200.
    Http(u16),
    /// Declared or received length above the limit.
    TooLarge,
    /// The body ended before its declared length.
    Truncated,
    /// Any other network failure (Win32 error code).
    Network(u32),
}

/// Maps a WinHTTP error code to a download error.
pub fn classify_network_error(code: u32) -> DownloadError {
    const ERROR_WINHTTP_TIMEOUT: u32 = 12002;
    const ERROR_WINHTTP_NAME_NOT_RESOLVED: u32 = 12007;
    const ERROR_WINHTTP_OPERATION_CANCELLED: u32 = 12017;
    const ERROR_WINHTTP_CANNOT_CONNECT: u32 = 12029;
    const ERROR_WINHTTP_CONNECTION_ERROR: u32 = 12030;
    match code {
        ERROR_WINHTTP_TIMEOUT => DownloadError::TimedOut,
        ERROR_WINHTTP_OPERATION_CANCELLED => DownloadError::Cancelled,
        ERROR_WINHTTP_NAME_NOT_RESOLVED
        | ERROR_WINHTTP_CANNOT_CONNECT
        | ERROR_WINHTTP_CONNECTION_ERROR => DownloadError::Offline(code),
        other => DownloadError::Network(other),
    }
}

/// A response body that never grows past its limit.
#[derive(Debug)]
pub struct BoundedBody {
    bytes: Vec<u8>,
    max_bytes: u64,
    declared: Option<u64>,
}

impl BoundedBody {
    /// Refuses a declared `Content-Length` above `max_bytes` before any
    /// byte is read.
    pub fn new(max_bytes: u64, declared: Option<u64>) -> Result<Self, DownloadError> {
        if declared.is_some_and(|length| length > max_bytes) {
            return Err(DownloadError::TooLarge);
        }
        let capacity = declared.unwrap_or(0).min(max_bytes);
        Ok(Self {
            bytes: Vec::with_capacity(usize::try_from(capacity).unwrap_or(0)),
            max_bytes,
            declared,
        })
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), DownloadError> {
        let total = self.bytes.len() as u64 + chunk.len() as u64;
        if total > self.max_bytes || self.declared.is_some_and(|length| total > length) {
            return Err(DownloadError::TooLarge);
        }
        self.bytes.extend_from_slice(chunk);
        Ok(())
    }

    pub fn len(&self) -> u64 {
        self.bytes.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn finish(self) -> Result<Vec<u8>, DownloadError> {
        if self
            .declared
            .is_some_and(|length| length != self.bytes.len() as u64)
        {
            return Err(DownloadError::Truncated);
        }
        Ok(self.bytes)
    }
}

/// Downloads the bootstrapper. Implementations follow at most
/// [`MAX_REDIRECTS`] redirects, each through [`DownloadUrl::resolve`], keep
/// the body in a [`BoundedBody`], apply the timeouts in `limits`, and stop
/// with [`DownloadError::Cancelled`] once `cancel` is set.
pub trait BootstrapperDownloader {
    fn download(
        &self,
        url: &DownloadUrl,
        limits: &DownloadLimits,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, DownloadError>;
}

// ---------------------------------------------------------------------------
// Payload and staging.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadError {
    TooSmall,
    TooLarge,
    /// The bytes do not start like a Windows executable.
    NotExecutable,
}

/// Checks downloaded bytes before anything is written to disk. This is a
/// shape check only; trust comes from the Authenticode signature.
pub fn validate_payload(bytes: &[u8], limits: &DownloadLimits) -> Result<(), PayloadError> {
    let length = bytes.len() as u64;
    if length > limits.max_bytes {
        return Err(PayloadError::TooLarge);
    }
    if length < limits.min_bytes {
        return Err(PayloadError::TooSmall);
    }
    if !bytes.starts_with(b"MZ") {
        return Err(PayloadError::NotExecutable);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageError {
    Payload(PayloadError),
    Io(std::io::ErrorKind),
}

/// A staged bootstrapper. A real staged file is held open with read-only
/// sharing from staging until it is discarded, so it cannot be replaced
/// between the signature check and the run.
#[derive(Debug)]
pub struct StagedFile {
    path: PathBuf,
    folder: PathBuf,
    lock: Option<File>,
}

impl StagedFile {
    /// A staged file without a lock (in-memory test doubles).
    pub fn unlocked(path: PathBuf, folder: PathBuf) -> Self {
        Self {
            path,
            folder,
            lock: None,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The open, write-locked file, when there is one.
    pub fn lock(&self) -> Option<&File> {
        self.lock.as_ref()
    }
}

pub trait Stager {
    fn stage(&self, bytes: &[u8]) -> Result<StagedFile, StageError>;
    /// Removes the staged file and its folder (best effort).
    fn discard(&self, file: StagedFile);
}

/// Stages into a fresh folder under the per-user temporary folder.
#[derive(Debug, Clone)]
pub struct TempStager {
    parent: PathBuf,
    limits: DownloadLimits,
}

impl TempStager {
    /// Uses `%TEMP%` (per user).
    pub fn new(limits: DownloadLimits) -> Self {
        Self::in_folder(std::env::temp_dir(), limits)
    }

    pub fn in_folder(parent: PathBuf, limits: DownloadLimits) -> Self {
        Self { parent, limits }
    }
}

impl Stager for TempStager {
    fn stage(&self, bytes: &[u8]) -> Result<StagedFile, StageError> {
        // Rejected payloads never reach the disk.
        validate_payload(bytes, &self.limits).map_err(StageError::Payload)?;
        let folder =
            create_fresh_folder(&self.parent).map_err(|error| StageError::Io(error.kind()))?;
        let staged = write_new_file(&folder, BOOTSTRAPPER_FILE_NAME, bytes)
            .and_then(|path| open_locked(&path).map(|lock| (path, lock)));
        match staged {
            Ok((path, lock)) => Ok(StagedFile {
                path,
                folder,
                lock: Some(lock),
            }),
            Err(error) => {
                let _ = std::fs::remove_file(folder.join(BOOTSTRAPPER_FILE_NAME));
                let _ = std::fs::remove_dir(&folder);
                Err(StageError::Io(error.kind()))
            }
        }
    }

    fn discard(&self, file: StagedFile) {
        let StagedFile { path, folder, lock } = file;
        drop(lock);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(folder);
    }
}

/// Creates a new folder with an unpredictable name under `parent`. An
/// existing folder is never reused.
pub fn create_fresh_folder(parent: &Path) -> std::io::Result<PathBuf> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let mut last_error = None;
    for _ in 0..8 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let token = std::collections::hash_map::RandomState::new().hash_one((
            std::process::id(),
            nanos,
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let folder = parent.join(format!("Marknexia-WebView2-{token:016x}"));
        match std::fs::create_dir(&folder) {
            Ok(()) => return Ok(folder),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::from(std::io::ErrorKind::AlreadyExists)))
}

/// Writes `bytes` to a new file `name` in `folder`; an existing file is
/// never overwritten.
pub fn write_new_file(folder: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let path = folder.join(name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(path)
}

/// Opens `path` for reading and denies other writers and deleters while the
/// handle is open.
pub fn open_locked(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        options.share_mode(FILE_SHARE_READ);
    }
    options.open(path)
}

// ---------------------------------------------------------------------------
// Signature and installer.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureError {
    /// `WinVerifyTrust` rejected the file (its HRESULT-like status).
    Untrusted(i32),
    NoSigner,
    /// Trusted, but not signed by Microsoft.
    NotMicrosoft {
        signer: String,
    },
}

pub trait SignatureVerifier {
    fn verify(&self, file: &StagedFile) -> Result<(), SignatureError>;
}

/// The leaf certificate must name Microsoft Corporation and the chain must
/// end in a Microsoft root.
pub fn is_microsoft_signer(leaf: &str, root: &str) -> bool {
    leaf == "Microsoft Corporation" && root.starts_with("Microsoft Root Certificate Authority")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallError {
    /// `CreateProcessW` failed (Win32 error code).
    StartFailed(u32),
    /// The bootstrapper asked for elevation, which is not requested.
    ElevationRequired,
    /// The user cancelled the installer.
    Cancelled,
    /// The installer exited with this code.
    ExitCode(u32),
    /// The installer did not exit within the wait limit; it keeps running.
    TimedOut,
}

/// Runs the staged bootstrapper without elevation and waits for it.
pub trait InstallerRunner {
    fn run(&self, file: &StagedFile) -> Result<(), InstallError>;
}

/// Maps the bootstrapper's exit code.
pub fn classify_exit_code(code: u32) -> Result<(), InstallError> {
    const ERROR_CANCELLED: u32 = 1223;
    const HRESULT_ERROR_CANCELLED: u32 = 0x8007_04C7;
    match code {
        0 => Ok(()),
        ERROR_CANCELLED | HRESULT_ERROR_CANCELLED => Err(InstallError::Cancelled),
        other => Err(InstallError::ExitCode(other)),
    }
}

// ---------------------------------------------------------------------------
// One install attempt.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SetupStep {
    Downloading,
    Verifying,
    Installing,
    Detecting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupFailure {
    /// No connection to Microsoft's servers (or a connection timeout).
    Offline,
    Download(DownloadError),
    Payload(PayloadError),
    Staging(std::io::ErrorKind),
    Signature(SignatureError),
    Cancelled,
    Install(InstallError),
    /// The installer finished, but detection still finds no runtime.
    StillMissing,
}

/// The dependencies of one attempt.
pub struct Bootstrap<'a> {
    pub downloader: &'a dyn BootstrapperDownloader,
    pub stager: &'a dyn Stager,
    pub verifier: &'a dyn SignatureVerifier,
    pub runner: &'a dyn InstallerRunner,
    pub probe: &'a dyn BrowserVersionProbe,
    pub limits: DownloadLimits,
}

impl Bootstrap<'_> {
    /// Download, stage, verify, run, then detect once. Returns the detected
    /// runtime version. The staged file is discarded before detection; a
    /// rejected signature or a cancellation never runs the installer.
    pub fn run(
        &self,
        cancel: &AtomicBool,
        progress: &mut dyn FnMut(SetupStep),
    ) -> Result<String, SetupFailure> {
        let cancelled = || cancel.load(Ordering::Acquire);
        if cancelled() {
            return Err(SetupFailure::Cancelled);
        }
        progress(SetupStep::Downloading);
        let url = DownloadUrl::parse(BOOTSTRAPPER_URL)
            .map_err(|error| SetupFailure::Download(DownloadError::Url(error)))?;
        let bytes = self
            .downloader
            .download(&url, &self.limits, cancel)
            .map_err(download_failure)?;
        if cancelled() {
            return Err(SetupFailure::Cancelled);
        }
        let staged = self.stager.stage(&bytes).map_err(|error| match error {
            StageError::Payload(error) => SetupFailure::Payload(error),
            StageError::Io(kind) => SetupFailure::Staging(kind),
        })?;
        drop(bytes);
        let installed = (|| {
            progress(SetupStep::Verifying);
            self.verifier
                .verify(&staged)
                .map_err(SetupFailure::Signature)?;
            if cancelled() {
                return Err(SetupFailure::Cancelled);
            }
            progress(SetupStep::Installing);
            Ok(self.runner.run(&staged))
        })();
        self.stager.discard(staged);
        // The installer did not run: nothing to detect.
        let installed = installed?;
        progress(SetupStep::Detecting);
        match EvergreenRuntime::detect_with(self.probe) {
            Ok(RuntimeStatus::Available { version }) => Ok(version),
            _ => Err(match installed {
                Ok(()) => SetupFailure::StillMissing,
                Err(InstallError::Cancelled) => SetupFailure::Cancelled,
                Err(error) => SetupFailure::Install(error),
            }),
        }
    }
}

fn download_failure(error: DownloadError) -> SetupFailure {
    match error {
        DownloadError::Offline(_) | DownloadError::TimedOut => SetupFailure::Offline,
        DownloadError::Cancelled => SetupFailure::Cancelled,
        other => SetupFailure::Download(other),
    }
}

// ---------------------------------------------------------------------------
// The shell's missing-runtime state.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupPhase {
    /// No runtime; nothing is running.
    Missing,
    Working(SetupStep),
    Installed {
        version: String,
    },
    Failed(SetupFailure),
}

/// The missing-runtime state machine. Attempts start only from
/// [`Self::begin_install`] (an explicit user action); results of earlier
/// attempts are ignored; a failure stays until the user acts again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSetup {
    phase: SetupPhase,
    attempt: u64,
    cancel_requested: bool,
}

impl Default for RuntimeSetup {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeSetup {
    pub fn new() -> Self {
        Self {
            phase: SetupPhase::Missing,
            attempt: 0,
            cancel_requested: false,
        }
    }

    pub fn phase(&self) -> &SetupPhase {
        &self.phase
    }

    /// The current attempt number (0 before the first).
    pub fn attempt(&self) -> u64 {
        self.attempt
    }

    pub fn can_install(&self) -> bool {
        matches!(self.phase, SetupPhase::Missing | SetupPhase::Failed(_))
    }

    /// Cancellation is offered until the installer starts; after that the
    /// installer's own window offers it.
    pub fn can_cancel(&self) -> bool {
        !self.cancel_requested
            && matches!(
                self.phase,
                SetupPhase::Working(SetupStep::Downloading | SetupStep::Verifying)
            )
    }

    pub fn is_working(&self) -> bool {
        matches!(self.phase, SetupPhase::Working(_))
    }

    /// The offline installer link stays available until the runtime is
    /// installed.
    pub fn shows_offline_link(&self) -> bool {
        !matches!(self.phase, SetupPhase::Installed { .. })
    }

    /// Starts an attempt for an explicit user action; `None` while one runs
    /// or after the runtime was installed.
    pub fn begin_install(&mut self) -> Option<u64> {
        if !self.can_install() {
            return None;
        }
        self.attempt += 1;
        self.cancel_requested = false;
        self.phase = SetupPhase::Working(SetupStep::Downloading);
        Some(self.attempt)
    }

    /// Records a cancel request for the running attempt.
    pub fn request_cancel(&mut self) -> bool {
        if !self.can_cancel() {
            return false;
        }
        self.cancel_requested = true;
        true
    }

    /// Progress of `attempt`; stale or backward steps are ignored.
    pub fn progress(&mut self, attempt: u64, step: SetupStep) -> bool {
        match self.phase {
            SetupPhase::Working(current) if attempt == self.attempt && step > current => {
                self.phase = SetupPhase::Working(step);
                true
            }
            _ => false,
        }
    }

    /// The result of `attempt`; a stale result is ignored.
    pub fn finish(&mut self, attempt: u64, outcome: Result<String, SetupFailure>) -> bool {
        if attempt != self.attempt || !self.is_working() {
            return false;
        }
        self.phase = match outcome {
            Ok(version) => SetupPhase::Installed { version },
            Err(failure) => SetupPhase::Failed(failure),
        };
        self.cancel_requested = false;
        true
    }

    /// The text shown in the document area.
    pub fn message(&self) -> String {
        match &self.phase {
            SetupPhase::Missing => MISSING_MESSAGE.to_owned(),
            SetupPhase::Working(_) if self.cancel_requested => "Cancelling…".to_owned(),
            SetupPhase::Working(SetupStep::Downloading) => {
                "Downloading the WebView2 Runtime installer from Microsoft…".to_owned()
            }
            SetupPhase::Working(SetupStep::Verifying) => {
                "Checking the installer's Microsoft signature…".to_owned()
            }
            SetupPhase::Working(SetupStep::Installing) => {
                "Installing the WebView2 Runtime. Follow the steps in the installer window."
                    .to_owned()
            }
            SetupPhase::Working(SetupStep::Detecting) => {
                "Checking for the WebView2 Runtime…".to_owned()
            }
            SetupPhase::Installed { version } => {
                format!("WebView2 Runtime {version} is installed. Documents are loading.")
            }
            SetupPhase::Failed(failure) => failure_message(failure),
        }
    }
}

/// The missing-runtime explanation.
pub const MISSING_MESSAGE: &str = "Microsoft Edge WebView2 Runtime is not installed. Marknexia needs it to display documents. Select Install WebView2 Runtime to download and run Microsoft's installer, or get Microsoft's offline installer.";

/// The diagnostics bar text for a missing runtime at startup.
pub const MISSING_DIAGNOSTIC: &str =
    "Microsoft Edge WebView2 Runtime is not installed; documents cannot be displayed.";

/// The explanation of a failed attempt (also the diagnostics bar text).
pub fn failure_message(failure: &SetupFailure) -> String {
    const OFFLINE: &str = "or use Microsoft's offline installer.";
    match failure {
        SetupFailure::Offline => format!(
            "Microsoft's download server could not be reached. Check the connection and try again, {OFFLINE}"
        ),
        SetupFailure::Download(_) | SetupFailure::Payload(_) => {
            format!("The WebView2 Runtime installer could not be downloaded. Try again, {OFFLINE}")
        }
        SetupFailure::Staging(_) => format!(
            "The WebView2 Runtime installer could not be saved to the temporary folder. Try again, {OFFLINE}"
        ),
        SetupFailure::Signature(_) => "The downloaded installer does not have a valid Microsoft signature, so it was not run. Use Microsoft's offline installer.".to_owned(),
        SetupFailure::Cancelled => format!(
            "The installation was cancelled. The WebView2 Runtime is still not installed. Try again, {OFFLINE}"
        ),
        SetupFailure::Install(InstallError::ElevationRequired) => {
            "The WebView2 Runtime installer needs administrator rights. Use Microsoft's offline installer.".to_owned()
        }
        SetupFailure::Install(InstallError::TimedOut) => format!(
            "The WebView2 Runtime installer is taking too long. When it finishes, restart Marknexia, {OFFLINE}"
        ),
        SetupFailure::Install(InstallError::StartFailed(code)) => format!(
            "The WebView2 Runtime installer could not start (error {code}). Try again, {OFFLINE}"
        ),
        SetupFailure::Install(InstallError::ExitCode(code)) => format!(
            "The WebView2 Runtime installer failed (code 0x{code:08X}). Try again, {OFFLINE}"
        ),
        SetupFailure::Install(InstallError::Cancelled) => format!(
            "The installation was cancelled. The WebView2 Runtime is still not installed. Try again, {OFFLINE}"
        ),
        SetupFailure::StillMissing => format!(
            "The installer finished, but the WebView2 Runtime was not found. Restart Marknexia, {OFFLINE}"
        ),
    }
}
