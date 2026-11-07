//! Single instance and activation hand-off: port of the .NET `App.xaml.cs`
//! (`AppInstance.FindOrRegisterForKey("Marknexia.SingleInstance")`,
//! `RedirectActivationToAsync`, `MainInstance_Activated`).
//!
//! The first instance owns a named mutex and serves a named pipe. A later
//! launch finds the mutex, sends its command line and working directory to
//! the pipe as one [`Activation`] message, waits for the acknowledgement,
//! and exits. The running instance restores and activates its window and
//! opens the first existing Markdown file among the sender's arguments and
//! then its own startup arguments (`ResolveFilePathFromActivation`), with
//! [`marknexia_core::startup::find_first_existing`].
//!
//! File-association activation of the unpackaged and the MSIX desktop
//! package starts the executable with the file path on the command line, so
//! it arrives through the same message.
//!
//! Security: the pipe accepts local clients only (`PIPE_REJECT_REMOTE_CLIENTS`),
//! its DACL grants the current user only, and both ends check that the
//! other process runs as the same user in the same session. A message is at
//! most [`MAX_MESSAGE_BYTES`], must match the exact framing, and is read
//! with a timeout. Differences from .NET are in
//! `compat/decisions/shell-input.md`.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use marknexia_core::startup::{StartupFileProbe, find_first_existing, split_command_line};

use crate::documents::check_document_path;

/// The .NET `AppInstance` key.
pub const DEFAULT_KEY: &str = "Marknexia.SingleInstance";
/// Overrides the key, so isolated test launches never hand off to each
/// other or to a user's running shell.
pub const KEY_VARIABLE: &str = "MARKNEXIA_INSTANCE_KEY";
const MAX_KEY_LENGTH: usize = 64;

const MAGIC: [u8; 4] = *b"MNXA";
/// The hand-off format version. A different version is refused.
pub const VERSION: u16 = 1;
const HEADER_BYTES: usize = 16;
/// The Windows command-line and path limit, in UTF-16 units, per field.
pub const MAX_FIELD_UNITS: usize = 32_767;
/// The largest valid message: the header and two full fields.
pub const MAX_MESSAGE_BYTES: usize = HEADER_BYTES + 4 * MAX_FIELD_UNITS;

/// The instance key from [`KEY_VARIABLE`]: 1 to 64 ASCII letters, digits,
/// `.`, `_` or `-`. Anything else, or no value, gives [`DEFAULT_KEY`].
#[must_use]
pub fn instance_key(value: Option<&OsStr>) -> String {
    value
        .and_then(OsStr::to_str)
        .filter(|key| {
            !key.is_empty()
                && key.len() <= MAX_KEY_LENGTH
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .unwrap_or(DEFAULT_KEY)
        .to_owned()
}

/// One activation request from a later launch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Activation {
    /// The sender's current directory (absolute), or empty. Relative
    /// arguments resolve against it.
    pub working_directory: String,
    /// The sender's command line after the program name, unsplit, as the
    /// .NET launch activation `Arguments`.
    pub arguments: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActivationError {
    /// Fewer bytes than the header, or a length that disagrees with it.
    Framing,
    BadMagic,
    UnsupportedVersion(u16),
    /// A field longer than [`MAX_FIELD_UNITS`].
    TooLarge,
    /// Unpaired surrogates, a NUL character, or a relative working
    /// directory.
    InvalidText,
}

fn units(text: &str) -> Result<Vec<u16>, ActivationError> {
    let units: Vec<u16> = text.encode_utf16().collect();
    if units.len() > MAX_FIELD_UNITS {
        Err(ActivationError::TooLarge)
    } else if units.contains(&0) {
        Err(ActivationError::InvalidText)
    } else {
        Ok(units)
    }
}

fn valid_directory(directory: &str) -> bool {
    directory.is_empty() || Path::new(directory).is_absolute()
}

impl Activation {
    /// The message bytes: `MNXA`, version (u16 LE), reserved zero (u16),
    /// the two field lengths in UTF-16 units (u32 LE each), then both
    /// fields as UTF-16 LE.
    ///
    /// # Errors
    ///
    /// A field over [`MAX_FIELD_UNITS`], a NUL character, or a relative
    /// working directory.
    pub fn encode(&self) -> Result<Vec<u8>, ActivationError> {
        if !valid_directory(&self.working_directory) {
            return Err(ActivationError::InvalidText);
        }
        let directory = units(&self.working_directory)?;
        let arguments = units(&self.arguments)?;
        let mut bytes = Vec::with_capacity(HEADER_BYTES + 2 * (directory.len() + arguments.len()));
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        for field in [&directory, &arguments] {
            let length = u32::try_from(field.len()).map_err(|_| ActivationError::TooLarge)?;
            bytes.extend_from_slice(&length.to_le_bytes());
        }
        for unit in directory.iter().chain(&arguments) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        Ok(bytes)
    }

    /// Parses one message strictly: exact framing, no trailing bytes,
    /// version [`VERSION`], bounded fields, valid UTF-16 without NUL, and
    /// an absolute (or empty) working directory.
    ///
    /// # Errors
    ///
    /// Any violation, as [`ActivationError`].
    pub fn decode(bytes: &[u8]) -> Result<Self, ActivationError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_MESSAGE_BYTES {
            return Err(ActivationError::Framing);
        }
        if bytes[..4] != MAGIC {
            return Err(ActivationError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != VERSION {
            return Err(ActivationError::UnsupportedVersion(version));
        }
        if bytes[6..8] != [0, 0] {
            return Err(ActivationError::Framing);
        }
        let length = |at: usize| {
            usize::try_from(u32::from_le_bytes([
                bytes[at],
                bytes[at + 1],
                bytes[at + 2],
                bytes[at + 3],
            ]))
            .map_err(|_| ActivationError::TooLarge)
        };
        let (directory_units, argument_units) = (length(8)?, length(12)?);
        if directory_units > MAX_FIELD_UNITS || argument_units > MAX_FIELD_UNITS {
            return Err(ActivationError::TooLarge);
        }
        if bytes.len() != HEADER_BYTES + 2 * (directory_units + argument_units) {
            return Err(ActivationError::Framing);
        }
        let text: Vec<u16> = bytes[HEADER_BYTES..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        if text.contains(&0) {
            return Err(ActivationError::InvalidText);
        }
        let (directory, arguments) = text.split_at(directory_units);
        let working_directory =
            String::from_utf16(directory).map_err(|_| ActivationError::InvalidText)?;
        let arguments = String::from_utf16(arguments).map_err(|_| ActivationError::InvalidText)?;
        if !valid_directory(&working_directory) {
            return Err(ActivationError::InvalidText);
        }
        Ok(Self {
            working_directory,
            arguments,
        })
    }
}

/// Resolves relative candidates against a base directory (the sender's
/// working directory) before the inner probe makes them absolute.
pub struct RelativeProbe<'a, P: ?Sized> {
    base: Option<PathBuf>,
    inner: &'a P,
}

impl<'a, P: StartupFileProbe + ?Sized> RelativeProbe<'a, P> {
    pub fn new(base: &str, inner: &'a P) -> Self {
        Self {
            base: (!base.is_empty()).then(|| PathBuf::from(base)),
            inner,
        }
    }
}

impl<P: StartupFileProbe + ?Sized> StartupFileProbe for RelativeProbe<'_, P> {
    fn full_path(&self, path: &str) -> Option<String> {
        match &self.base {
            // `join` keeps the drive of the base for a root-relative path
            // (`\docs\a.md`); a drive-relative path (`D:a.md`) replaces it.
            Some(base) if Path::new(path).is_relative() => {
                self.inner.full_path(base.join(path).to_str()?)
            }
            _ => self.inner.full_path(path),
        }
    }

    fn is_file(&self, full_path: &str) -> bool {
        self.inner.is_file(full_path)
    }

    fn expand_short_names(&self, full_path: &str) -> Option<String> {
        self.inner.expand_short_names(full_path)
    }
}

/// Applies the document path gate ([`check_document_path`]) to every path
/// before the inner probe touches the file system with it: the absolute path
/// (made lexically) before `is_file` and before short-name expansion, and
/// the expanded path again.
pub struct GatedProbe<'a, P: ?Sized> {
    inner: &'a P,
}

impl<'a, P: StartupFileProbe + ?Sized> GatedProbe<'a, P> {
    pub fn new(inner: &'a P) -> Self {
        Self { inner }
    }
}

fn gated(path: &str) -> bool {
    check_document_path(path).is_ok()
}

impl<P: StartupFileProbe + ?Sized> StartupFileProbe for GatedProbe<'_, P> {
    fn full_path(&self, path: &str) -> Option<String> {
        // `full_path` is lexical (GetFullPathNameW); the result is gated.
        self.inner.full_path(path).filter(|full| gated(full))
    }

    fn is_file(&self, full_path: &str) -> bool {
        gated(full_path) && self.inner.is_file(full_path)
    }

    fn expand_short_names(&self, full_path: &str) -> Option<String> {
        if !gated(full_path) {
            return None;
        }
        self.inner
            .expand_short_names(full_path)
            .filter(|long| gated(long))
    }
}

/// Arguments considered per source (the sender's split command line, or the
/// running instance's own arguments). A sender with more is ignored.
pub const MAX_CANDIDATES: usize = 64;

/// .NET `ResolveFilePathFromActivation`: the sender's arguments first, then
/// the running instance's own startup arguments (`Environment
/// .GetCommandLineArgs().Skip(1)`, a .NET behavior kept as is). Sender
/// candidates resolve relative to the sender's working directory; the
/// instance's own candidates relative to its own. Every path passes the
/// document path gate before any file-system access ([`GatedProbe`]). It
/// touches the file system, so the shell runs it on a worker thread.
pub fn resolve_activation<P: StartupFileProbe + ?Sized>(
    activation: &Activation,
    own_arguments: &[String],
    probe: &P,
) -> Option<String> {
    let sender = if split_command_line(&activation.arguments).len() > MAX_CANDIDATES {
        None
    } else {
        let relative = RelativeProbe::new(&activation.working_directory, probe);
        find_first_existing(
            [Some(activation.arguments.as_str())],
            &GatedProbe::new(&relative),
        )
    };
    sender.or_else(|| {
        find_first_existing(
            own_arguments
                .iter()
                .take(MAX_CANDIDATES)
                .map(|argument| Some(argument.as_str())),
            &GatedProbe::new(probe),
        )
    })
}

/// The command line after the program name, unsplit. The program name ends
/// at the first white space outside quotes; quotes inside it only toggle
/// (CommandLineToArgvW's rule for argument 0).
#[must_use]
pub fn arguments_after_program(command_line: &str) -> &str {
    let mut quoted = false;
    for (index, character) in command_line.char_indices() {
        match character {
            '"' => quoted = !quoted,
            ' ' | '\t' if !quoted => {
                return command_line[index..].trim_start_matches([' ', '\t']);
            }
            _ => {}
        }
    }
    ""
}

#[cfg(windows)]
#[allow(unsafe_code)] // Mutex, named pipe, token and process queries stay here.
pub mod native {
    use std::{
        collections::VecDeque,
        ffi::c_void,
        sync::{Arc, Mutex, PoisonError},
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    use marknexia_core::startup::StartupFileProbe;
    use marknexia_files::startup_probe::StdStartupProbe;
    use windows::{
        Win32::{
            Foundation::{
                CloseHandle, ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND,
                ERROR_IO_PENDING, ERROR_MORE_DATA, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
                GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HLOCAL, HWND, LPARAM, LocalFree,
                WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM,
            },
            Security::{
                Authorization::{
                    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                    SDDL_REVISION_1,
                },
                EqualSid, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation,
                PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_MANDATORY_LABEL,
                TOKEN_QUERY, TOKEN_USER, TokenIntegrityLevel, TokenIsAppContainer, TokenUser,
            },
            Storage::FileSystem::{
                CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
                FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, GetLongPathNameW, OPEN_EXISTING,
                PIPE_ACCESS_DUPLEX, ReadFile, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
                WriteFile,
            },
            System::{
                IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
                Pipes::{
                    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe,
                    GetNamedPipeClientProcessId, GetNamedPipeServerProcessId,
                    PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE,
                    PIPE_WAIT, SetNamedPipeHandleState, WaitNamedPipeW,
                },
                RemoteDesktop::ProcessIdToSessionId,
                Threading::{
                    CreateEventW, CreateMutexW, GetCurrentProcess, GetCurrentProcessId,
                    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, SetEvent,
                    WaitForMultipleObjects, WaitForSingleObject,
                },
            },
            UI::WindowsAndMessaging::{
                AllowSetForegroundWindow, IsIconic, PostMessageW, SW_RESTORE, SetForegroundWindow,
                ShowWindow,
            },
        },
        core::{PCWSTR, PWSTR},
    };

    use super::{Activation, MAX_MESSAGE_BYTES};

    /// How long a later launch keeps trying to reach the running instance
    /// (it may still be starting, or shutting down).
    const HAND_OFF_DEADLINE: Duration = Duration::from_secs(5);
    /// How long the server waits for a connected client's message, and the
    /// client for the acknowledgement.
    const IO_TIMEOUT: Duration = Duration::from_secs(2);
    /// Pending activations kept for the UI thread; more are refused.
    const MAX_PENDING: usize = 16;
    const ACCEPTED: u8 = 1;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// An owned kernel handle, closed once on drop.
    struct Owned(HANDLE);

    impl Drop for Owned {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: the handle is owned here and closed exactly once.
                let _ = unsafe { CloseHandle(self.0) };
            }
        }
    }

    // SAFETY: kernel handles may be used and closed from any thread.
    unsafe impl Send for Owned {}

    /// One token information class, in u64-aligned storage, with the
    /// byte length the call reported.
    fn token_information(
        token: HANDLE,
        class: windows::Win32::Security::TOKEN_INFORMATION_CLASS,
    ) -> Option<(Vec<u64>, usize)> {
        let mut needed = 0_u32;
        // SAFETY: the size query writes only `needed`.
        let _ = unsafe { GetTokenInformation(token, class, None, 0, &mut needed) };
        if needed == 0 || needed > 4096 {
            return None;
        }
        let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
        // SAFETY: the buffer holds `needed` bytes and is 8-byte aligned.
        unsafe {
            GetTokenInformation(
                token,
                class,
                Some(buffer.as_mut_ptr().cast()),
                needed,
                &mut needed,
            )
        }
        .ok()?;
        Some((buffer, needed as usize))
    }

    /// Copies a SID that points into `buffer` (`length` valid bytes).
    fn copy_sid(buffer: &[u64], length: usize, sid: PSID) -> Option<Vec<u8>> {
        // SAFETY: GetLengthSid reads the SID header inside the token buffer.
        let size = unsafe { windows::Win32::Security::GetLengthSid(sid) } as usize;
        let start = sid.0 as usize;
        let base = buffer.as_ptr() as usize;
        if start < base || start.checked_add(size)? > base.checked_add(length)? {
            return None;
        }
        // SAFETY: bounds checked above against the token buffer.
        Some(unsafe { std::slice::from_raw_parts(sid.0 as *const u8, size) }.to_vec())
    }

    /// What the hand-off trusts about a process: its user SID, its mandatory
    /// integrity RID, and whether it is an AppContainer.
    struct TokenFacts {
        sid: Vec<u8>,
        integrity: u32,
        app_container: bool,
    }

    fn token_facts(process: HANDLE) -> Option<TokenFacts> {
        let mut token = HANDLE::default();
        // SAFETY: `process` is a live process handle; `token` receives an
        // owned handle that `Owned` closes.
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
        let token = Owned(token);
        let (user, length) = token_information(token.0, TokenUser)?;
        // SAFETY: the buffer holds a TOKEN_USER written by the call above.
        let sid = copy_sid(&user, length, unsafe {
            (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid
        })?;
        let (label, length) = token_information(token.0, TokenIntegrityLevel)?;
        // SAFETY: the buffer holds a TOKEN_MANDATORY_LABEL.
        let label_sid = unsafe { (*label.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid };
        // The copy only proves that the whole SID lies inside the buffer; the
        // RID is read in place, where the 8-byte-aligned token buffer keeps
        // the SID's sub-authorities 4-byte aligned.
        let _ = copy_sid(&label, length, label_sid)?;
        // SAFETY: the label SID lies inside `label` (checked above), which
        // stays alive; the sub-authority index is checked against its count.
        let integrity = unsafe {
            let count = *GetSidSubAuthorityCount(label_sid);
            if count == 0 {
                return None;
            }
            *GetSidSubAuthority(label_sid, u32::from(count) - 1)
        };
        let (container, length) = token_information(token.0, TokenIsAppContainer)?;
        if length < 4 {
            return None;
        }
        let app_container = (container[0] & 0xffff_ffff) != 0;
        Some(TokenFacts {
            sid,
            integrity,
            app_container,
        })
    }

    fn sid_string(sid: &[u8]) -> Option<String> {
        let mut text = PWSTR::null();
        // SAFETY: `sid` is a valid SID copy; the returned string is freed
        // with LocalFree.
        unsafe { ConvertSidToStringSidW(PSID(sid.as_ptr() as *mut c_void), &mut text) }.ok()?;
        let value = unsafe { text.to_string() }.ok();
        let _ = unsafe { LocalFree(Some(HLOCAL(text.0.cast()))) };
        value
    }

    /// This process's identity: user SID bytes, session and integrity.
    struct Identity {
        sid: Vec<u8>,
        session: u32,
        integrity: u32,
    }

    impl Identity {
        fn current() -> Option<Self> {
            // SAFETY: the pseudo handle needs no closing.
            let facts = token_facts(unsafe { GetCurrentProcess() })?;
            let mut session = 0;
            unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }.ok()?;
            Some(Self {
                sid: facts.sid,
                session,
                integrity: facts.integrity,
            })
        }

        /// True when `process_id` runs as the same user in the same session,
        /// at an integrity level at least this process's, and not in an
        /// AppContainer. A sandboxed or lower-integrity process of the same
        /// user (which could pre-create the pipe name) is never trusted.
        fn matches(&self, process_id: u32) -> bool {
            let mut session = u32::MAX;
            // SAFETY: plain query; failure leaves `session` unmatched.
            if unsafe { ProcessIdToSessionId(process_id, &mut session) }.is_err()
                || session != self.session
            {
                return false;
            }
            let Ok(process) =
                (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
            else {
                return false;
            };
            let process = Owned(process);
            token_facts(process.0).is_some_and(
                |TokenFacts {
                     sid,
                     integrity,
                     app_container,
                 }| {
                    !app_container
                    && integrity >= self.integrity
                    && sid.len() == self.sid.len()
                    // SAFETY: both buffers are valid SIDs of equal length.
                    && unsafe {
                        EqualSid(
                            PSID(sid.as_ptr() as *mut c_void),
                            PSID(self.sid.as_ptr() as *mut c_void),
                        )
                    }
                    .is_ok()
                },
            )
        }

        fn names(&self, key: &str) -> Option<(String, String)> {
            let sid = sid_string(&self.sid)?;
            Some((
                format!(r"Local\{key}.{sid}"),
                format!(r"\\.\pipe\{key}.{sid}.{}", self.session),
            ))
        }
    }

    /// Activations received by the server thread, for the UI thread.
    pub type ActivationQueue = Arc<Mutex<VecDeque<Activation>>>;

    /// The outcome of [`claim`].
    pub enum Claim {
        /// This is the first instance; start the server once the window
        /// exists.
        Primary(PrimaryInstance),
        /// A running instance accepted this launch; exit now.
        HandedOff,
        /// Single instance is unavailable (no identity, the name is held by
        /// someone else, or the running instance never answered). Run as an
        /// independent instance.
        Standalone,
    }

    pub struct PrimaryInstance {
        mutex: Owned,
        pipe_name: String,
        identity: Identity,
    }

    /// The launch's own activation: its working directory and its command
    /// line after the program name.
    #[must_use]
    pub fn current_activation() -> Activation {
        // SAFETY: GetCommandLineW returns this process's static command line.
        let command_line =
            unsafe { windows::Win32::System::Environment::GetCommandLineW().to_string() }
                .unwrap_or_default();
        let working_directory = std::env::current_dir()
            .ok()
            .and_then(|path| path.to_str().map(str::to_owned))
            .unwrap_or_default();
        Activation {
            working_directory,
            arguments: super::arguments_after_program(&command_line).to_owned(),
        }
    }

    /// Becomes the first instance for `key`, or hands `activation` to it.
    #[must_use]
    pub fn claim(key: &str, activation: &Activation) -> Claim {
        let Some(identity) = Identity::current() else {
            return Claim::Standalone;
        };
        let Some((mutex_name, pipe_name)) = identity.names(key) else {
            return Claim::Standalone;
        };
        let Ok(message) = activation.encode() else {
            // An over-long command line cannot be handed off; .NET would
            // redirect it, so run on our own rather than lose it.
            return Claim::Standalone;
        };
        let mutex_name = wide(&mutex_name);
        let deadline = Instant::now() + HAND_OFF_DEADLINE;
        loop {
            // SAFETY: the name is NUL-terminated; the handle is owned below.
            let created =
                unsafe { CreateMutexW(None, false, PCWSTR::from_raw(mutex_name.as_ptr())) };
            let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
            match created {
                Ok(handle) if !already => {
                    return Claim::Primary(PrimaryInstance {
                        mutex: Owned(handle),
                        pipe_name,
                        identity,
                    });
                }
                Ok(handle) => drop(Owned(handle)),
                // Access denied: another user's object holds the name.
                Err(_) => return Claim::Standalone,
            }
            if send(&pipe_name, &identity, &message).is_ok() {
                return Claim::HandedOff;
            }
            if Instant::now() >= deadline {
                return Claim::Standalone;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// Manual-reset event for overlapped I/O and the stop signal.
    fn event() -> Option<Owned> {
        // SAFETY: an unnamed event; the handle is owned.
        unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .ok()
            .map(Owned)
    }

    /// Completes overlapped I/O started with `start`, waiting at most
    /// `timeout` and stopping early when `stop` is signaled. Returns the
    /// transferred byte count, or the Win32 error.
    fn overlapped(
        handle: HANDLE,
        stop: Option<HANDLE>,
        timeout: Option<Duration>,
        start: impl FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
    ) -> Result<u32, u32> {
        let Some(signal) = event() else {
            return Err(0);
        };
        let mut io = OVERLAPPED {
            hEvent: signal.0,
            ..Default::default()
        };
        let pointer: *mut OVERLAPPED = &mut io;
        if let Err(error) = start(pointer) {
            let code = (error.code().0 as u32) & 0xffff;
            if code == ERROR_PIPE_CONNECTED.0 {
                return Ok(0);
            }
            if code != ERROR_IO_PENDING.0 {
                return Err(code);
            }
            let mut handles = vec![signal.0];
            handles.extend(stop);
            let milliseconds = timeout.map_or(u32::MAX, |t| {
                u32::try_from(t.as_millis()).unwrap_or(u32::MAX)
            });
            // SAFETY: the handles are live for the wait.
            let waited = unsafe { WaitForMultipleObjects(&handles, false, milliseconds) };
            if waited != WAIT_OBJECT_0 {
                // Timeout or stop: cancel, then wait for the cancellation so
                // `io` and the buffer are no longer used by the kernel.
                unsafe {
                    let _ = CancelIoEx(handle, Some(pointer));
                    let _ = WaitForSingleObject(signal.0, u32::MAX);
                }
                return Err(if waited == WAIT_TIMEOUT {
                    WAIT_TIMEOUT.0
                } else {
                    0
                });
            }
        }
        let mut transferred = 0;
        // SAFETY: the operation completed; `io` is still live.
        match unsafe { GetOverlappedResult(handle, pointer, &mut transferred, false) } {
            Ok(()) => Ok(transferred),
            Err(error) => Err((error.code().0 as u32) & 0xffff),
        }
    }

    /// Sends one activation and waits for the acknowledgement.
    fn send(pipe_name: &str, identity: &Identity, message: &[u8]) -> Result<(), ()> {
        let name = wide(pipe_name);
        let pipe = loop {
            // SAFETY: NUL-terminated name; identification-only QoS, so the
            // server can never impersonate this process.
            match unsafe {
                CreateFileW(
                    PCWSTR::from_raw(name.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(
                        FILE_FLAG_OVERLAPPED.0
                            | SECURITY_SQOS_PRESENT.0
                            | SECURITY_IDENTIFICATION.0,
                    ),
                    None,
                )
            } {
                Ok(pipe) => break Owned(pipe),
                Err(error) if (error.code().0 as u32 & 0xffff) == ERROR_PIPE_BUSY.0 => {
                    // SAFETY: NUL-terminated name.
                    if !unsafe { WaitNamedPipeW(PCWSTR::from_raw(name.as_ptr()), 500) }.as_bool() {
                        return Err(());
                    }
                }
                Err(error) if (error.code().0 as u32 & 0xffff) == ERROR_FILE_NOT_FOUND.0 => {
                    return Err(());
                }
                Err(_) => return Err(()),
            }
        };
        let mut server = 0;
        // SAFETY: live pipe handle; plain queries.
        unsafe { GetNamedPipeServerProcessId(pipe.0, &mut server) }.map_err(drop)?;
        if !identity.matches(server) {
            return Err(());
        }
        let mode = PIPE_READMODE_MESSAGE;
        unsafe { SetNamedPipeHandleState(pipe.0, Some(&mode), None, None) }.map_err(drop)?;
        // The running instance may take the foreground (.NET BringToFront).
        let _ = unsafe { AllowSetForegroundWindow(server) };
        let written = overlapped(pipe.0, None, Some(IO_TIMEOUT), |io| unsafe {
            WriteFile(pipe.0, Some(message), None, Some(io))
        })
        .map_err(drop)?;
        if written as usize != message.len() {
            return Err(());
        }
        let mut ack = [0_u8; 1];
        let read = overlapped(pipe.0, None, Some(IO_TIMEOUT), |io| unsafe {
            ReadFile(pipe.0, Some(&mut ack), None, Some(io))
        })
        .map_err(drop)?;
        (read == 1 && ack[0] == ACCEPTED).then_some(()).ok_or(())
    }

    /// The running pipe server. Drop stops the thread, closes the pipe, and
    /// then releases the mutex, so a new launch never finds the mutex
    /// without a server for longer than the shutdown takes.
    pub struct ActivationServer {
        stop: Owned,
        thread: Option<JoinHandle<()>>,
        _mutex: Owned,
    }

    impl Drop for ActivationServer {
        fn drop(&mut self) {
            // SAFETY: the event is owned by this server.
            let _ = unsafe { SetEvent(self.stop.0) };
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    impl PrimaryInstance {
        /// Serves activations for `window`: each accepted one is queued and
        /// `message` is posted to the window. `None` when the pipe cannot be
        /// created (another process holds the name). The mutex is then
        /// released with `self`, so later launches stop handing off to a
        /// name this instance does not serve; the caller reports it.
        pub fn serve(
            self,
            window: HWND,
            message: u32,
            token: isize,
            queue: ActivationQueue,
        ) -> Option<ActivationServer> {
            let stop = event()?;
            let sddl = wide(&format!("D:P(A;;GA;;;{})", sid_string(&self.identity.sid)?));
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            // SAFETY: NUL-terminated SDDL; the descriptor is freed below.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR::from_raw(sddl.as_ptr()),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    None,
                )
            }
            .ok()?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            let name = wide(&self.pipe_name);
            // One instance only: while it serves a client, others wait
            // (ERROR_PIPE_BUSY), and no other process can add an instance.
            let pipe = unsafe {
                CreateNamedPipeW(
                    PCWSTR::from_raw(name.as_ptr()),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_MESSAGE
                        | PIPE_READMODE_MESSAGE
                        | PIPE_WAIT
                        | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    64,
                    u32::try_from(MAX_MESSAGE_BYTES).unwrap_or(u32::MAX),
                    0,
                    Some(&attributes),
                )
            };
            // SAFETY: allocated by ConvertStringSecurityDescriptor...; the
            // pipe holds its own copy of the security descriptor.
            let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
            if pipe.is_invalid() {
                return None;
            }
            let pipe = Owned(pipe);
            let stop_handle = Owned(duplicate(stop.0)?);
            let window = window.0 as isize;
            let identity = self.identity;
            let thread = thread::Builder::new()
                .name("marknexia-activation".to_owned())
                .spawn(move || {
                    serve_loop(
                        &pipe,
                        &stop_handle,
                        &identity,
                        window,
                        message,
                        token,
                        &queue,
                    );
                })
                .ok()?;
            Some(ActivationServer {
                stop,
                thread: Some(thread),
                _mutex: self.mutex,
            })
        }
    }

    fn duplicate(handle: HANDLE) -> Option<HANDLE> {
        let mut copy = HANDLE::default();
        // SAFETY: duplicates a handle of this process for the server thread.
        unsafe {
            windows::Win32::Foundation::DuplicateHandle(
                GetCurrentProcess(),
                handle,
                GetCurrentProcess(),
                &mut copy,
                0,
                false,
                windows::Win32::Foundation::DUPLICATE_SAME_ACCESS,
            )
        }
        .ok()?;
        Some(copy)
    }

    fn serve_loop(
        pipe: &Owned,
        stop: &Owned,
        identity: &Identity,
        window: isize,
        message: u32,
        token: isize,
        queue: &ActivationQueue,
    ) {
        let mut buffer = vec![0_u8; MAX_MESSAGE_BYTES + 1];
        loop {
            // Wait for a client, or stop.
            match overlapped(pipe.0, Some(stop.0), None, |io| unsafe {
                ConnectNamedPipe(pipe.0, Some(io))
            }) {
                Ok(_) => {}
                Err(_) => {
                    // SAFETY: plain wait on an owned event.
                    if unsafe { WaitForSingleObject(stop.0, 0) } == WAIT_OBJECT_0 {
                        return;
                    }
                    let _ = unsafe { DisconnectNamedPipe(pipe.0) };
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
            }
            let accepted = serve_client(pipe, stop, identity, &mut buffer, queue);
            if accepted {
                // SAFETY: posting to the shell window; no pointers. After the
                // window is destroyed the post just fails.
                let _ = unsafe {
                    PostMessageW(
                        Some(HWND(window as *mut c_void)),
                        message,
                        WPARAM(0),
                        LPARAM(token),
                    )
                };
            }
            let _ = unsafe { DisconnectNamedPipe(pipe.0) };
            if unsafe { WaitForSingleObject(stop.0, 0) } == WAIT_OBJECT_0 {
                return;
            }
        }
    }

    /// Validates one connected client and its message. Returns true when
    /// an activation was queued and acknowledged.
    fn serve_client(
        pipe: &Owned,
        stop: &Owned,
        identity: &Identity,
        buffer: &mut [u8],
        queue: &ActivationQueue,
    ) -> bool {
        let mut client = 0;
        // SAFETY: live, connected pipe.
        if unsafe { GetNamedPipeClientProcessId(pipe.0, &mut client) }.is_err()
            || !identity.matches(client)
        {
            return false;
        }
        let read = overlapped(pipe.0, Some(stop.0), Some(IO_TIMEOUT), |io| unsafe {
            ReadFile(pipe.0, Some(buffer), None, Some(io))
        });
        let length = match read {
            Ok(length) => length as usize,
            // ERROR_MORE_DATA: the message is larger than the limit.
            Err(code) if code == ERROR_MORE_DATA.0 => return false,
            Err(_) => return false,
        };
        let Ok(activation) = Activation::decode(&buffer[..length.min(buffer.len())]) else {
            return false;
        };
        // Shutting down: no acknowledgement, so the sender retries and
        // becomes the first instance instead of losing its file.
        // SAFETY: plain wait on an owned event.
        if unsafe { WaitForSingleObject(stop.0, 0) } == WAIT_OBJECT_0 {
            return false;
        }
        {
            let mut pending = queue.lock().unwrap_or_else(PoisonError::into_inner);
            if pending.len() >= MAX_PENDING {
                return false;
            }
            pending.push_back(activation);
        }
        let ack = [ACCEPTED];
        let _ = overlapped(pipe.0, Some(stop.0), Some(IO_TIMEOUT), |io| unsafe {
            WriteFile(pipe.0, Some(&ack), None, Some(io))
        });
        // Wait until the client has read the acknowledgement and closed its
        // end (ERROR_BROKEN_PIPE); disconnecting earlier discards it.
        let mut drain = [0_u8; 1];
        let closed = overlapped(pipe.0, Some(stop.0), Some(IO_TIMEOUT), |io| unsafe {
            ReadFile(pipe.0, Some(&mut drain), None, Some(io))
        });
        let _ = matches!(closed, Err(code) if code == ERROR_BROKEN_PIPE.0);
        true
    }

    /// .NET `BringToFront` calls `ShowWindow(SW_RESTORE)` then
    /// `SetForegroundWindow`, which also un-maximizes a maximized window.
    /// Corrected here (owner test 2026-11-07): only a minimized window is
    /// restored, then it comes to the foreground.
    pub fn bring_to_front(window: HWND) {
        // SAFETY: this thread's own top-level window.
        // A minimized window is restored; a maximized or normal one keeps
        // its show state (SW_RESTORE would un-maximize it).
        unsafe {
            if IsIconic(window).as_bool() {
                let _ = ShowWindow(window, SW_RESTORE);
            }
            let _ = SetForegroundWindow(window);
        }
    }

    /// [`StdStartupProbe`]'s lexical `full_path`, an `is_file` that opens
    /// with identification-only QoS (so a path that reaches a pipe can never
    /// be used to impersonate this process), and the Win32 8.3 short-name
    /// expansion that .NET `Path.GetFullPath` performs (SET-8c). Use it
    /// through `GatedProbe` (as `resolve_activation` does), off the UI thread.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct ShellStartupProbe;

    impl StartupFileProbe for ShellStartupProbe {
        fn full_path(&self, path: &str) -> Option<String> {
            StdStartupProbe.full_path(path)
        }

        fn is_file(&self, full_path: &str) -> bool {
            crate::documents::probe_is_file(std::path::Path::new(full_path))
        }

        fn expand_short_names(&self, full_path: &str) -> Option<String> {
            let path = wide(full_path);
            let mut buffer = vec![0_u16; 32_768];
            // SAFETY: NUL-terminated input; the output buffer length is
            // passed. A failure or truncation keeps the short form.
            let length =
                unsafe { GetLongPathNameW(PCWSTR::from_raw(path.as_ptr()), Some(&mut buffer)) }
                    as usize;
            (length > 0 && length < buffer.len())
                .then(|| String::from_utf16(&buffer[..length]).ok())
                .flatten()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn instance_keys_are_validated() {
        assert_eq!(instance_key(None), DEFAULT_KEY);
        assert_eq!(instance_key(Some(OsStr::new("test.1_a-b"))), "test.1_a-b");
        for bad in ["", "a b", "a\\b", "a/b", &"k".repeat(65), "é"] {
            assert_eq!(instance_key(Some(OsStr::new(bad))), DEFAULT_KEY, "{bad:?}");
        }
    }

    fn sample() -> Activation {
        Activation {
            working_directory: r"C:\work".to_owned(),
            arguments: r#""C:\docs\read me.md" --x"#.to_owned(),
        }
    }

    #[test]
    fn activation_round_trips() {
        let bytes = sample().encode().unwrap();
        assert_eq!(&bytes[..4], b"MNXA");
        assert_eq!(Activation::decode(&bytes), Ok(sample()));
        let empty = Activation {
            working_directory: String::new(),
            arguments: String::new(),
        };
        assert_eq!(Activation::decode(&empty.encode().unwrap()), Ok(empty));
    }

    #[test]
    fn activation_framing_is_strict() {
        let bytes = sample().encode().unwrap();
        assert_eq!(
            Activation::decode(&bytes[..15]),
            Err(ActivationError::Framing)
        );
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(Activation::decode(&trailing), Err(ActivationError::Framing));
        let mut short = bytes.clone();
        short.truncate(bytes.len() - 2);
        assert_eq!(Activation::decode(&short), Err(ActivationError::Framing));
        let mut magic = bytes.clone();
        magic[0] = b'X';
        assert_eq!(Activation::decode(&magic), Err(ActivationError::BadMagic));
        let mut version = bytes.clone();
        version[4] = 2;
        assert_eq!(
            Activation::decode(&version),
            Err(ActivationError::UnsupportedVersion(2))
        );
        let mut reserved = bytes.clone();
        reserved[7] = 1;
        assert_eq!(Activation::decode(&reserved), Err(ActivationError::Framing));
        let mut huge = bytes;
        huge[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Activation::decode(&huge), Err(ActivationError::TooLarge));
        assert_eq!(
            Activation::decode(&vec![0; MAX_MESSAGE_BYTES + 1]),
            Err(ActivationError::Framing)
        );
    }

    #[test]
    fn activation_text_is_validated() {
        let relative = Activation {
            working_directory: "work".to_owned(),
            arguments: String::new(),
        };
        assert_eq!(relative.encode(), Err(ActivationError::InvalidText));
        let nul = Activation {
            working_directory: String::new(),
            arguments: "a\0b".to_owned(),
        };
        assert_eq!(nul.encode(), Err(ActivationError::InvalidText));
        let long = Activation {
            working_directory: String::new(),
            arguments: "a".repeat(MAX_FIELD_UNITS + 1),
        };
        assert_eq!(long.encode(), Err(ActivationError::TooLarge));
        let full = Activation {
            working_directory: format!(r"C:\{}", "d".repeat(MAX_FIELD_UNITS - 3)),
            arguments: "a".repeat(MAX_FIELD_UNITS),
        };
        let bytes = full.encode().unwrap();
        assert_eq!(bytes.len(), MAX_MESSAGE_BYTES);
        assert_eq!(Activation::decode(&bytes), Ok(full));
        // An unpaired surrogate in the arguments.
        let mut bytes = Activation {
            working_directory: String::new(),
            arguments: "ab".to_owned(),
        }
        .encode()
        .unwrap();
        let at = bytes.len() - 2;
        bytes[at..].copy_from_slice(&0xD800_u16.to_le_bytes());
        assert_eq!(
            Activation::decode(&bytes),
            Err(ActivationError::InvalidText)
        );
    }

    #[test]
    fn the_program_name_is_removed_like_argument_zero() {
        assert_eq!(
            arguments_after_program(r#""C:\Program Files\m.exe" a.md"#),
            "a.md"
        );
        assert_eq!(arguments_after_program(r"C:\m.exe   a.md b"), "a.md b");
        assert_eq!(
            arguments_after_program(r#"C:\m.exe "x y.md""#),
            r#""x y.md""#
        );
        assert_eq!(arguments_after_program(r"C:\m.exe"), "");
        assert_eq!(arguments_after_program(r#""C:\a b\m.exe""#), "");
    }

    /// Files exist only in the set; paths are made absolute lexically.
    struct FakeProbe(BTreeSet<&'static str>);

    impl StartupFileProbe for FakeProbe {
        fn full_path(&self, path: &str) -> Option<String> {
            Path::new(path)
                .is_absolute()
                .then(|| path.to_owned())
                .or_else(|| Some(format!(r"C:\primary\{path}")))
        }

        fn is_file(&self, full_path: &str) -> bool {
            self.0.contains(full_path)
        }
    }

    #[test]
    fn activation_resolves_sender_arguments_first_then_the_instance_arguments() {
        let probe = FakeProbe([r"C:\sender\a.md", r"C:\primary\own.md", r"C:\primary\a.md"].into());
        let activation = Activation {
            working_directory: r"C:\sender".to_owned(),
            arguments: "notes.txt a.md".to_owned(),
        };
        // Relative to the sender's directory, not the running instance's.
        assert_eq!(
            resolve_activation(&activation, &["own.md".to_owned()], &probe),
            Some(r"C:\sender\a.md".to_owned())
        );
        // No sender match: the running instance's own startup file (.NET
        // appends Environment.GetCommandLineArgs().Skip(1)).
        let none = Activation {
            working_directory: r"C:\sender".to_owned(),
            arguments: String::new(),
        };
        assert_eq!(
            resolve_activation(&none, &["own.md".to_owned()], &probe),
            Some(r"C:\primary\own.md".to_owned())
        );
        assert_eq!(resolve_activation(&none, &[], &probe), None);
        // A file path with spaces is tried whole first.
        let probe = FakeProbe([r"C:\docs\read me.md"].into());
        let spaced = Activation {
            working_directory: String::new(),
            arguments: r"C:\docs\read me.md".to_owned(),
        };
        assert_eq!(
            resolve_activation(&spaced, &[], &probe),
            Some(r"C:\docs\read me.md".to_owned())
        );
    }

    #[test]
    fn root_relative_sender_paths_keep_the_sender_drive() {
        let probe = FakeProbe([r"D:\docs\a.md"].into());
        let activation = Activation {
            working_directory: r"D:\work".to_owned(),
            arguments: r"\docs\a.md".to_owned(),
        };
        assert_eq!(
            resolve_activation(&activation, &[], &probe),
            Some(r"D:\docs\a.md".to_owned())
        );
    }

    /// Records every path that reaches a file-system call.
    struct RecordingProbe(std::cell::RefCell<Vec<String>>);

    impl StartupFileProbe for RecordingProbe {
        fn full_path(&self, path: &str) -> Option<String> {
            Some(path.to_owned())
        }

        fn is_file(&self, full_path: &str) -> bool {
            self.0.borrow_mut().push(full_path.to_owned());
            false
        }

        fn expand_short_names(&self, full_path: &str) -> Option<String> {
            self.0.borrow_mut().push(full_path.to_owned());
            None
        }
    }

    #[test]
    fn gated_candidates_never_reach_the_file_system() {
        let probe = RecordingProbe(std::cell::RefCell::new(Vec::new()));
        let activation = Activation {
            working_directory: String::new(),
            arguments:
                r#"\\.\pipe\x.md "\\?\GLOBALROOT\a.md" C:\a\..\b.md C:\ok.md C:\PROGRA~1\c.md"#
                    .to_owned(),
        };
        assert_eq!(resolve_activation(&activation, &[], &probe), None);
        assert_eq!(
            *probe.0.borrow(),
            [r"C:\ok.md", r"C:\PROGRA~1\c.md", r"C:\PROGRA~1\c.md"],
            "only gated paths are probed"
        );
    }

    #[test]
    fn senders_with_too_many_arguments_are_ignored() {
        let probe = FakeProbe([r"C:\a.md", r"C:\own.md"].into());
        let many = std::iter::repeat_n("x", MAX_CANDIDATES)
            .chain([r"C:\a.md"])
            .collect::<Vec<_>>()
            .join(" ");
        let activation = Activation {
            working_directory: String::new(),
            arguments: many,
        };
        assert_eq!(
            resolve_activation(&activation, &[r"C:\own.md".to_owned()], &probe),
            Some(r"C:\own.md".to_owned())
        );
    }
}
