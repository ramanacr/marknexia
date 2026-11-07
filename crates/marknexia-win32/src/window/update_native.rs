//! Native adapters for the portable updater (`marknexia-update`): the
//! WinHTTP fetcher with the configured exact host allowlist, owner-only
//! update folders (created, checked or re-secured), the PE machine and
//! Authenticode checks of staged executables with the pinned release
//! certificate, the guarded start (forwarding), the MSIX and elevation
//! checks, the nonce source and process identities. No test drives these
//! against a server or a real installer; the decisions they feed are
//! tested headless in `marknexia-update`.

use std::{
    ffi::{OsString, c_void},
    io::{Read, Seek, SeekFrom},
    os::windows::ffi::OsStrExt,
    path::Path,
    sync::atomic::AtomicBool,
};

use marknexia_update::{
    activation::{Nonce, ProcessIdentity, StartGuard},
    config::UpdateConfig,
    manifest::{Architecture, HttpsUrl},
    payload::{AllowedExecutable, PublisherPin, Signer, SignerCertificate, pe_machine},
    stage::{FetchError, Fetcher, StagePlatform},
};
use marknexia_webview::{
    bootstrap::{
        BootstrapperDownloader, DownloadError, DownloadLimits, DownloadUrl, SignatureError,
        SignatureVerifier, StagedFile, open_locked,
    },
    bootstrap_native::{
        AuthenticodeVerifier, DownloadAbort, WinHttpDownloader, create_owner_only_dir,
        verify_signer,
    },
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE, HLOCAL, LocalFree,
            STILL_ACTIVE,
        },
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
                GetNamedSecurityInfoW, SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
            },
            Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
            DACL_SECURITY_INFORMATION, GetAce, GetSecurityDescriptorControl,
            GetSecurityDescriptorDacl, GetTokenInformation, OWNER_SECURITY_INFORMATION,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
            TOKEN_ELEVATION, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TokenElevation, TokenOwner,
            TokenUser,
        },
        Storage::{
            FileSystem::{
                FILE_ATTRIBUTE_REPARSE_POINT, GetFileAttributesW, INVALID_FILE_ATTRIBUTES,
            },
            Packaging::Appx::GetCurrentPackageFullName,
        },
        System::Threading::{
            GetCurrentProcess, GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
    core::{PCWSTR, w},
};

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

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

/// The creation time of a process handle (100 ns `FILETIME` units).
fn creation_time(process: HANDLE) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: live process handle; the four outputs are locals.
    unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
    Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

/// This process: its id and creation time.
pub(super) fn current_process() -> ProcessIdentity {
    // SAFETY: the pseudo handle needs no closing.
    let started = creation_time(unsafe { GetCurrentProcess() }).unwrap_or(0);
    ProcessIdentity {
        pid: std::process::id(),
        started,
    }
}

/// True while the recorded process runs: the id exists, has not exited,
/// and (when a creation time was recorded) was created at that time, so a
/// reused id is another process.
pub(super) fn is_process_running(process: ProcessIdentity) -> bool {
    // SAFETY: query-only access; the handle is closed below.
    let Ok(handle) =
        (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process.pid) })
    else {
        return false;
    };
    let mut code = 0_u32;
    // SAFETY: live handle opened above; `code` is a local.
    let running =
        unsafe { GetExitCodeProcess(handle, &mut code) }.is_ok() && code == STILL_ACTIVE.0 as u32;
    let same = process.started == 0 || creation_time(handle) == Some(process.started);
    // SAFETY: closes the handle opened above exactly once.
    let _ = unsafe { CloseHandle(handle) };
    running && same
}

/// True when this process token is elevated (an administrator's full
/// token). Unknown counts as elevated.
pub(super) fn is_elevated() -> bool {
    let mut token = HANDLE::default();
    // SAFETY: the pseudo handle needs no closing; `token` is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return true;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut size = 0_u32;
    // SAFETY: `elevation` is a local of the given size.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elevation).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut size,
        )
    };
    // SAFETY: closes the token opened above exactly once.
    let _ = unsafe { CloseHandle(token) };
    queried.is_err() || elevation.TokenIsElevated != 0
}

/// A SID from this process token: `TokenUser` (the user) or `TokenOwner`
/// (the default owner of new objects: the user, or BUILTIN\Administrators
/// in an elevated token). Both structures start with the SID pointer.
fn token_sid(class: TOKEN_INFORMATION_CLASS) -> Option<Vec<u8>> {
    let mut token = HANDLE::default();
    // SAFETY: as in `is_elevated`.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
    let mut buffer = vec![0_u64; 64];
    let mut size = 0_u32;
    // SAFETY: `buffer` holds 512 bytes, aligned for TOKEN_USER/TOKEN_OWNER.
    let queried = unsafe {
        GetTokenInformation(
            token,
            class,
            Some(buffer.as_mut_ptr().cast()),
            (buffer.len() * 8) as u32,
            &mut size,
        )
    };
    // SAFETY: closes the token opened above exactly once.
    let _ = unsafe { CloseHandle(token) };
    queried.ok()?;
    // SAFETY: filled by the call above; TOKEN_USER.User.Sid and
    // TOKEN_OWNER.Owner are the first field and point into `buffer`.
    let sid = unsafe { *buffer.as_ptr().cast::<PSID>() };
    sid_bytes(sid)
}

/// The current user's SID bytes.
fn current_user_sid() -> Option<Vec<u8>> {
    token_sid(TokenUser)
}

/// The token's default owner for new objects.
fn default_owner_sid() -> Option<Vec<u8>> {
    token_sid(TokenOwner)
}

/// An acceptable folder owner: the user, or the token's default owner (an
/// elevated token creates objects owned by BUILTIN\Administrators).
fn is_own_owner(owner: &[u8]) -> bool {
    current_user_sid().is_some_and(|user| user == owner)
        || default_owner_sid().is_some_and(|default| default == owner)
}

/// Sets the folder's owner to the current user (permitted for the token's
/// own user SID), so the owner-only DACL (`OW`) means the user even when
/// an elevated token created the folder.
fn set_owner_to_user(path: &Path) -> std::io::Result<()> {
    let mut user = current_user_sid().ok_or(std::io::ErrorKind::PermissionDenied)?;
    let wide = wide_path(path);
    // SAFETY: NUL-terminated path; `user` is a valid SID buffer that
    // outlives the call.
    let status = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR::from_raw(wide.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            Some(PSID(user.as_mut_ptr().cast())),
            None,
            None,
            None,
        )
    };
    if status.is_ok() {
        Ok(())
    } else {
        Err(std::io::ErrorKind::PermissionDenied.into())
    }
}

/// Creates `path` with the protected owner-only DACL, owned by the user.
pub(super) fn create_private_dir(path: &Path) -> std::io::Result<()> {
    create_owner_only_dir(path)?;
    set_owner_to_user(path)
}

fn sid_bytes(sid: PSID) -> Option<Vec<u8>> {
    use windows::Win32::Security::{GetLengthSid, IsValidSid};
    // SAFETY: `sid` points to a SID owned by the caller's buffer.
    unsafe {
        if sid.0.is_null() || !IsValidSid(sid).as_bool() {
            return None;
        }
        let length = GetLengthSid(sid) as usize;
        Some(std::slice::from_raw_parts(sid.0.cast::<u8>(), length).to_vec())
    }
}

fn is_reparse_point(path: &Path) -> std::io::Result<bool> {
    let wide = wide_path(path);
    // SAFETY: NUL-terminated path that outlives the call.
    let attributes = unsafe { GetFileAttributesW(PCWSTR::from_raw(wide.as_ptr())) };
    if attributes == INVALID_FILE_ATTRIBUTES {
        return Err(std::io::Error::last_os_error());
    }
    Ok(attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0)
}

/// The folder's owner SID, whether its DACL is protected, and the SIDs of
/// its ACEs (with `false` for any ACE that is not a plain allow ACE).
struct FolderSecurity {
    owner: Vec<u8>,
    protected: bool,
    aces: Vec<Option<Vec<u8>>>,
}

fn folder_security(path: &Path) -> Option<FolderSecurity> {
    let wide = wide_path(path);
    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: outputs are locals; the descriptor is freed with LocalFree.
    let status = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR::from_raw(wide.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            &mut descriptor,
        )
    };
    if status.is_err() {
        return None;
    }
    let result = (|| {
        let owner = sid_bytes(owner)?;
        let mut control = 0_u16;
        let mut revision = 0_u32;
        // SAFETY: live descriptor from the call above.
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }.ok()?;
        let protected = control & SE_DACL_PROTECTED.0 != 0;
        let mut aces = Vec::new();
        if !dacl.is_null() {
            // SAFETY: `dacl` points into the live descriptor.
            let count = unsafe { (*dacl).AceCount };
            for index in 0..u32::from(count) {
                let mut ace: *mut c_void = std::ptr::null_mut();
                // SAFETY: index below the ACE count of the live ACL.
                unsafe { GetAce(dacl, index, &mut ace) }.ok()?;
                // SAFETY: GetAce returned a pointer to an ACE in the ACL.
                let header = unsafe { &*ace.cast::<ACE_HEADER>() };
                if header.AceType == 0 {
                    // ACCESS_ALLOWED_ACE_TYPE: the SID starts at SidStart.
                    // SAFETY: an allow ACE; SidStart is its embedded SID.
                    let sid = unsafe {
                        PSID(
                            std::ptr::addr_of_mut!((*ace.cast::<ACCESS_ALLOWED_ACE>()).SidStart)
                                .cast(),
                        )
                    };
                    aces.push(sid_bytes(sid));
                } else {
                    aces.push(None);
                }
            }
        } else {
            // A NULL DACL grants everyone.
            aces.push(None);
        }
        Some(FolderSecurity {
            owner,
            protected,
            aces,
        })
    })();
    // SAFETY: allocated by GetNamedSecurityInfoW, freed exactly once.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    result
}

/// `S-1-3-4` (OWNER RIGHTS), which the `OW` SDDL alias grants.
fn owner_rights_sid() -> Option<Vec<u8>> {
    let mut sid = PSID::default();
    // SAFETY: static SID string; the SID is freed with LocalFree.
    unsafe { ConvertStringSidToSidW(w!("S-1-3-4"), &mut sid) }.ok()?;
    let bytes = sid_bytes(sid);
    // SAFETY: allocated by the conversion above, freed exactly once.
    unsafe { LocalFree(Some(HLOCAL(sid.0))) };
    bytes
}

/// Not a reparse point, owned by the current user (or the token's default
/// owner, when an elevated token created it), with a protected DACL
/// whose every ACE allows only the current user or OWNER RIGHTS.
pub(super) fn folder_is_private(path: &Path) -> bool {
    if is_reparse_point(path).unwrap_or(true) {
        return false;
    }
    let (Some(user), Some(owner_rights), Some(security)) = (
        current_user_sid(),
        owner_rights_sid(),
        folder_security(path),
    ) else {
        return false;
    };
    is_own_owner(&security.owner)
        && security.protected
        && !security.aces.is_empty()
        && security.aces.iter().all(|ace| {
            ace.as_ref()
                .is_some_and(|sid| *sid == user || *sid == owner_rights)
        })
}

/// An existing update folder: refused if it is a reparse point or owned by
/// another account; otherwise its DACL is replaced by the protected
/// owner-only DACL (`D:P(A;OICI;FA;;;OW)`, inherited by its contents).
pub(super) fn secure_existing_dir(path: &Path) -> std::io::Result<()> {
    if is_reparse_point(path)? {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    let security = folder_security(path).ok_or(std::io::ErrorKind::PermissionDenied)?;
    if !is_own_owner(&security.owner) {
        return Err(std::io::ErrorKind::PermissionDenied.into());
    }
    set_owner_to_user(path)?;
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
    .map_err(|_| std::io::Error::from(std::io::ErrorKind::PermissionDenied))?;
    let applied = (|| {
        let mut present = false.into();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut defaulted = false.into();
        // SAFETY: live descriptor; outputs are locals.
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) }
            .ok()?;
        let wide = wide_path(path);
        // SAFETY: NUL-terminated path; the DACL lives in `descriptor`.
        let status = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR::from_raw(wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl),
                None,
            )
        };
        status.is_ok().then_some(())
    })();
    // SAFETY: allocated by the conversion above, freed exactly once.
    unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    applied.ok_or_else(|| std::io::ErrorKind::PermissionDenied.into())
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

/// Opens `path` denying writers and deleters, checks its PE machine and
/// its Authenticode signer through that handle, and returns the still-open
/// file.
fn open_and_verify(
    path: &Path,
    signer: &Signer,
    machine: u16,
    publisher: &PublisherPin,
) -> Result<StagedFile, String> {
    let mut file =
        open_locked(path).map_err(|error| format!("it could not be opened ({}).", error.kind()))?;
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
    match signer {
        Signer::Release => {
            let identity = verify_signer(&staged).map_err(|error| signature_reason(&error))?;
            publisher
                .accepts(&SignerCertificate {
                    name: identity.name,
                    certificate: identity.certificate,
                })
                .map_err(|error| error.message().to_owned())?;
        }
        Signer::Microsoft => AuthenticodeVerifier
            .verify(&staged)
            .map_err(|error| signature_reason(&error))?,
    }
    Ok(staged)
}

/// Owner-only folders and the executable checks on a write-locked handle.
pub(super) struct NativeStagePlatform;

impl StagePlatform for NativeStagePlatform {
    fn create_private_dir(&self, path: &Path) -> std::io::Result<()> {
        create_private_dir(path)
    }

    fn secure_existing_dir(&self, path: &Path) -> std::io::Result<()> {
        secure_existing_dir(path)
    }

    fn check_executable(
        &self,
        path: &Path,
        executable: &AllowedExecutable,
        machine: u16,
        publisher: &PublisherPin,
    ) -> Result<(), String> {
        open_and_verify(path, &executable.signer, machine, publisher).map(drop)
    }
}

/// The guarded start of `activation::start`.
struct NativeStartGuard {
    config: Option<UpdateConfig>,
}

impl StartGuard for NativeStartGuard {
    fn configured(&self) -> bool {
        self.config.is_some()
    }

    fn elevated(&self) -> bool {
        is_elevated()
    }

    fn folder_is_private(&self, root: &Path) -> bool {
        folder_is_private(root)
    }

    fn is_running(&self, process: ProcessIdentity) -> bool {
        is_process_running(process)
    }

    fn open_verified(&self, shell: &Path) -> Result<Box<dyn std::any::Any>, String> {
        let config = self.config.as_ref().ok_or("updates are not configured.")?;
        let machine = Architecture::current()
            .ok_or("unsupported architecture.")?
            .pe_machine();
        open_and_verify(shell, &Signer::Release, machine, &config.publisher)
            .map(|staged| Box::new(staged) as Box<dyn std::any::Any>)
    }

    fn start(&self, shell: &Path, arguments: &[OsString]) -> std::io::Result<()> {
        let folder = shell.parent().ok_or(std::io::ErrorKind::NotFound)?;
        std::process::Command::new(shell)
            .args(arguments)
            .current_dir(folder)
            .spawn()
            .map(drop)
    }
}

/// A normal (non-health) start through `activation::start`: only in a
/// configured, unpackaged and unelevated process, and only through a
/// private update folder, it resolves an interrupted update and starts a
/// newer active payload's shell after verifying it on a locked handle.
/// `true` when that shell started (this process then exits).
pub(super) fn forward_to_active_version(arguments: &[OsString]) -> bool {
    use marknexia_update::{
        activation::{self, StartOutcome},
        layout::Layout,
        payload::SHELL_EXECUTABLE,
        version::Version,
    };
    if is_packaged() {
        return false;
    }
    let guard = NativeStartGuard {
        config: UpdateConfig::embedded().ok(),
    };
    let Some(root) = Layout::default_root() else {
        return false;
    };
    let (Ok(executable), Ok(version)) = (
        std::env::current_exe(),
        Version::parse(env!("CARGO_PKG_VERSION")),
    ) else {
        return false;
    };
    matches!(
        activation::start(
            &Layout::new(root),
            &executable,
            &version,
            SHELL_EXECUTABLE,
            arguments,
            &guard,
        ),
        StartOutcome::Forwarded(_)
    )
}

fn signature_reason(error: &SignatureError) -> String {
    match error {
        SignatureError::Untrusted(code) => {
            format!("its signature is not trusted (0x{:08X}).", *code as u32)
        }
        SignatureError::NoSigner => "it is not signed.".to_owned(),
        SignatureError::NotMicrosoft { signer } => {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_folder(label: &str) -> std::path::PathBuf {
        use std::hash::BuildHasher;
        let token = std::collections::hash_map::RandomState::new().hash_one(label);
        std::env::temp_dir().join(format!(
            "marknexia-update-native-{label}-{}-{token:016x}",
            std::process::id()
        ))
    }

    #[test]
    fn owner_only_folders_are_private_and_inherited_ones_are_not() {
        let private = temp_folder("private");
        // Works elevated too (GitHub runners): the owner is set to the user.
        create_private_dir(&private).expect("owner-only folder");
        assert!(folder_is_private(&private));
        assert_eq!(
            folder_security(&private).unwrap().owner,
            current_user_sid().unwrap()
        );
        let plain = temp_folder("plain");
        std::fs::create_dir(&plain).unwrap();
        // An inherited %TEMP% DACL (SYSTEM, Administrators) is not private:
        // a pointer in such a folder is never followed.
        assert!(!folder_is_private(&plain));
        // A folder the user owns is re-secured (the .NET updater's case).
        std::fs::create_dir(plain.join("pending-old")).unwrap();
        secure_existing_dir(&plain).expect("re-secured");
        assert!(folder_is_private(&plain));
        assert_eq!(
            folder_security(&plain).unwrap().owner,
            current_user_sid().unwrap()
        );
        // Elevated or not, the test process never forwards (UPD-12).
        let _ = is_elevated();
        assert!(!folder_is_private(&plain.join("missing")));
        assert!(secure_existing_dir(&plain.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&private);
        let _ = std::fs::remove_dir_all(&plain);
    }

    #[test]
    fn a_process_is_identified_by_id_and_creation_time() {
        let me = current_process();
        assert_ne!(me.started, 0);
        assert!(is_process_running(me));
        assert!(!is_process_running(ProcessIdentity {
            started: me.started + 1,
            ..me
        }));
        assert!(is_process_running(ProcessIdentity { started: 0, ..me }));
    }
}
