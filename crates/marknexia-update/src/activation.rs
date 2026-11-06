//! Activation, the nonce-bound health check, rollback and recovery.
//!
//! 1. [`begin`] writes `transaction.json` (candidate, previous pointer,
//!    SHA-256 of the nonce, coordinator process id), then switches
//!    `active.json` to the candidate. Both writes are atomic replacements.
//! 2. The coordinator starts the candidate shell with
//!    `--post-update-health <nonce>` and waits at most
//!    [`HEALTH_DEADLINE`] from the launch for `health\ack-<pid>.json`.
//! 3. The new shell writes the acknowledgement ([`write_health_ack`]) only
//!    when its window is visible, WebView2 is ready and the bundled offline
//!    document rendered, and only if the transaction names its own payload
//!    and the nonce hashes to the recorded value.
//! 4. Healthy: [`commit`] removes the transaction and old payloads, keeping
//!    the previous one as the rollback target. Anything else (timeout,
//!    exit or crash, wrong nonce or process, failed probe, launch failure):
//!    [`rollback`] restores the previous pointer and records a diagnostic.
//! 5. [`recover`] runs at every normal start: a transaction left by an
//!    interrupted switch or a crashed coordinator is rolled back before
//!    either version starts.

use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{hex, layout::Layout, stage::StagedPayload, version::Version};

/// The health-check argument (with the nonce as the next argument).
pub const HEALTH_ARGUMENT: &str = "--post-update-health";
/// The acknowledgement deadline, from the launch of the new version.
pub const HEALTH_DEADLINE: Duration = Duration::from_secs(60);
const ACTIVE_SCHEMA: &str = "marknexia-active-v1";
const TRANSACTION_SCHEMA: &str = "marknexia-update-transaction-v1";
const ACK_SCHEMA: &str = "marknexia-health-ack-v1";
const FAILURE_SCHEMA: &str = "marknexia-update-failure-v1";
/// Largest pointer, transaction or acknowledgement file read.
const MAX_RECORD_BYTES: u64 = 4096;

/// A 256-bit single-use value from the OS random generator.
#[derive(Clone, PartialEq, Eq)]
pub struct Nonce([u8; 32]);

impl Nonce {
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        hex::decode::<32>(text).map(Self)
    }

    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(&self.0)
    }

    fn digest_hex(&self) -> String {
        let digest: [u8; 32] = Sha256::digest(self.0).into();
        hex::encode(&digest)
    }
}

impl std::fmt::Debug for Nonce {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Nonce(..)")
    }
}

/// The active-version pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ActiveRecord {
    schema: String,
    /// A payload id under `versions`.
    pub payload: String,
    pub version: String,
}

impl ActiveRecord {
    #[must_use]
    pub fn new(payload: &str, version: &Version) -> Self {
        Self {
            schema: ACTIVE_SCHEMA.to_owned(),
            payload: payload.to_owned(),
            version: version.to_string(),
        }
    }

    fn is_valid(&self) -> bool {
        self.schema == ACTIVE_SCHEMA
            && crate::layout::is_payload_id(&self.payload)
            && Version::parse(&self.version).is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TransactionRecord {
    schema: String,
    pub candidate: ActiveRecord,
    /// The pointer before the switch; `None` when there was none (the
    /// original unpacked installation runs itself).
    pub previous: Option<ActiveRecord>,
    nonce_sha256: String,
    pub coordinator_pid: u32,
    pub child_pid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HealthAck {
    schema: String,
    nonce: String,
    pid: u32,
    shell_visible: bool,
    web_view_ready: bool,
    offline_render: bool,
}

/// What the new shell observed before acknowledging.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Probes {
    pub shell_visible: bool,
    pub web_view_ready: bool,
    pub offline_render: bool,
}

impl Probes {
    #[must_use]
    pub fn all_passed(self) -> bool {
        self.shell_visible && self.web_view_ready && self.offline_render
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationError {
    /// Another activation is in progress.
    Busy,
    Io(std::io::ErrorKind),
}

impl From<std::io::Error> for ActivationError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}

/// Why an activation was rolled back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealthFailure {
    LaunchFailed(std::io::ErrorKind),
    TimedOut,
    /// The new process ended (or crashed) before a valid acknowledgement.
    Exited(Option<i32>),
    WrongNonce,
    WrongProcess,
    /// The acknowledgement reports a failed probe.
    ProbeFailed,
    Malformed,
    /// A transaction was left behind by an interrupted switch or a crashed
    /// coordinator.
    Interrupted,
}

impl HealthFailure {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::LaunchFailed(kind) => format!("the new version could not start ({kind})"),
            Self::TimedOut => {
                "the new version did not confirm that it works within 60 seconds".to_owned()
            }
            Self::Exited(Some(code)) => {
                format!("the new version closed unexpectedly (exit code {code})")
            }
            Self::Exited(None) => "the new version closed unexpectedly".to_owned(),
            Self::WrongNonce | Self::WrongProcess | Self::Malformed => {
                "the new version's health confirmation was not valid".to_owned()
            }
            Self::ProbeFailed => "the new version could not display documents".to_owned(),
            Self::Interrupted => "the previous update was interrupted".to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Records on disk.
// ---------------------------------------------------------------------------

fn read_bounded(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(std::io::ErrorKind::InvalidData.into());
    }
    Ok(Some(bytes))
}

/// Writes `value` to a new temporary file beside `path`, flushes it, and
/// renames it over `path` (`MoveFileEx` with replace): readers see the old
/// or the new record, never a partial one.
fn write_atomic<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&temporary);
    let written = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// The active pointer: `Ok(None)` when there is none; an unreadable or
/// invalid pointer is an error.
pub fn read_active(layout: &Layout) -> std::io::Result<Option<ActiveRecord>> {
    let Some(bytes) = read_bounded(&layout.active_file())? else {
        return Ok(None);
    };
    match serde_json::from_slice::<ActiveRecord>(&bytes) {
        Ok(record) if record.is_valid() => Ok(Some(record)),
        _ => Err(std::io::ErrorKind::InvalidData.into()),
    }
}

/// The transaction in progress, if any.
pub fn read_transaction(layout: &Layout) -> std::io::Result<Option<TransactionRecord>> {
    let Some(bytes) = read_bounded(&layout.transaction_file())? else {
        return Ok(None);
    };
    match serde_json::from_slice::<TransactionRecord>(&bytes) {
        Ok(record)
            if record.schema == TRANSACTION_SCHEMA
                && record.candidate.is_valid()
                && record.previous.as_ref().is_none_or(ActiveRecord::is_valid)
                && hex::decode::<32>(&record.nonce_sha256).is_some() =>
        {
            Ok(Some(record))
        }
        _ => Err(std::io::ErrorKind::InvalidData.into()),
    }
}

// ---------------------------------------------------------------------------
// The switch.
// ---------------------------------------------------------------------------

/// Records the transaction, then points `active.json` at `staged`.
pub fn begin(
    layout: &Layout,
    staged: &StagedPayload,
    nonce: &Nonce,
    coordinator_pid: u32,
) -> Result<TransactionRecord, ActivationError> {
    if layout.transaction_file().exists() {
        return Err(ActivationError::Busy);
    }
    let candidate = ActiveRecord::new(&staged.id, &staged.version);
    let record = TransactionRecord {
        schema: TRANSACTION_SCHEMA.to_owned(),
        candidate: candidate.clone(),
        // An invalid pointer is not a usable rollback target: rolling back
        // then removes the pointer, and the unpacked installation runs.
        previous: read_active(layout).ok().flatten(),
        nonce_sha256: nonce.digest_hex(),
        coordinator_pid,
        child_pid: None,
    };
    write_atomic(&layout.transaction_file(), &record)?;
    if let Err(error) = write_atomic(&layout.active_file(), &candidate) {
        let _ = rollback(layout, &record, &HealthFailure::Interrupted);
        return Err(error.into());
    }
    Ok(record)
}

/// Records the launched process, so a recovery knows it.
pub fn note_child(
    layout: &Layout,
    record: &mut TransactionRecord,
    pid: u32,
) -> Result<(), ActivationError> {
    record.child_pid = Some(pid);
    write_atomic(&layout.transaction_file(), record).map_err(Into::into)
}

/// The candidate is healthy: drop the transaction and acknowledgements,
/// and remove payloads other than the active one and the rollback target.
pub fn commit(layout: &Layout, record: &TransactionRecord) -> Result<(), ActivationError> {
    remove_if_present(&layout.transaction_file())?;
    let _ = std::fs::remove_dir_all(layout.health());
    let keep: Vec<&str> = std::iter::once(record.candidate.payload.as_str())
        .chain(
            record
                .previous
                .as_ref()
                .map(|previous| previous.payload.as_str()),
        )
        .collect();
    if let Ok(folders) = std::fs::read_dir(layout.versions()) {
        for folder in folders.flatten() {
            let name = folder.file_name();
            let name = name.to_string_lossy();
            if !keep.iter().any(|kept| kept.eq_ignore_ascii_case(&name)) {
                let _ = std::fs::remove_dir_all(folder.path());
            }
        }
    }
    Ok(())
}

/// Restores the previous pointer (or removes the pointer when there was
/// none), records the diagnostic, removes the failed payload and then the
/// transaction. Safe to repeat after an interruption.
pub fn rollback(
    layout: &Layout,
    record: &TransactionRecord,
    failure: &HealthFailure,
) -> Result<(), ActivationError> {
    match &record.previous {
        Some(previous) => write_atomic(&layout.active_file(), previous)?,
        None => remove_if_present(&layout.active_file())?,
    }
    let _ = write_atomic(
        &layout.failure_file(),
        &serde_json::json!({
            "schema": FAILURE_SCHEMA,
            "version": record.candidate.version,
            "reason": failure.message(),
        }),
    );
    let _ = std::fs::remove_dir_all(layout.health());
    let rollback_target = record.previous.as_ref().map(|previous| &previous.payload);
    if rollback_target != Some(&record.candidate.payload) {
        let _ = std::fs::remove_dir_all(layout.payload(&record.candidate.payload));
    }
    remove_if_present(&layout.transaction_file())?;
    Ok(())
}

/// What [`recover`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recovery {
    Nothing,
    /// The coordinator of the transaction is still running; nothing changed.
    InProgress,
    /// An interrupted transaction for this version was rolled back.
    RolledBack(String),
    /// The transaction file was unreadable: the pointer was removed, so
    /// the unpacked installation runs.
    PointerReset,
}

/// Resolves an interrupted transaction before any version starts.
/// `is_running(pid)` tells whether the recorded coordinator still runs.
pub fn recover(layout: &Layout, is_running: &dyn Fn(u32) -> bool) -> Recovery {
    match read_transaction(layout) {
        Ok(None) => Recovery::Nothing,
        Ok(Some(record)) => {
            if record.coordinator_pid != std::process::id() && is_running(record.coordinator_pid) {
                return Recovery::InProgress;
            }
            let _ = rollback(layout, &record, &HealthFailure::Interrupted);
            Recovery::RolledBack(record.candidate.version)
        }
        Err(_) => {
            let _ = remove_if_present(&layout.active_file());
            let _ = remove_if_present(&layout.transaction_file());
            Recovery::PointerReset
        }
    }
}

/// Where a normal start should run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    RunHere,
    /// Start this newer, active payload's shell instead and exit.
    Forward(PathBuf),
}

/// The active payload's shell when it is newer than the running version
/// and is not the running executable; otherwise run here. A missing or
/// invalid pointer, or a missing shell, runs here (never unusable).
#[must_use]
pub fn route(
    layout: &Layout,
    running_executable: &Path,
    running_version: &Version,
    shell_name: &str,
) -> Route {
    let Ok(Some(active)) = read_active(layout) else {
        return Route::RunHere;
    };
    let Ok(active_version) = Version::parse(&active.version) else {
        return Route::RunHere;
    };
    let shell = layout.payload(&active.payload).join(shell_name);
    let same = |a: &Path, b: &Path| {
        let a = std::path::absolute(a).unwrap_or_else(|_| a.to_path_buf());
        let b = std::path::absolute(b).unwrap_or_else(|_| b.to_path_buf());
        a.to_string_lossy()
            .eq_ignore_ascii_case(&b.to_string_lossy())
    };
    if active_version <= *running_version || same(&shell, running_executable) || !shell.is_file() {
        return Route::RunHere;
    }
    Route::Forward(shell)
}

// ---------------------------------------------------------------------------
// Health check.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AckError {
    /// The executable is not a staged payload, or no transaction names it.
    NotAnUpdate,
    WrongNonce,
    Io(std::io::ErrorKind),
}

/// Called by the new shell in health mode once its probes ran.
pub fn write_health_ack(
    own_executable: &Path,
    nonce: &str,
    pid: u32,
    probes: Probes,
) -> Result<(), AckError> {
    let (layout, id) =
        Layout::from_payload_executable(own_executable).ok_or(AckError::NotAnUpdate)?;
    let record = read_transaction(&layout)
        .ok()
        .flatten()
        .filter(|record| record.candidate.payload.eq_ignore_ascii_case(&id))
        .ok_or(AckError::NotAnUpdate)?;
    let nonce = Nonce::parse(nonce).ok_or(AckError::WrongNonce)?;
    if !hex::constant_time_eq(
        nonce.digest_hex().as_bytes(),
        record.nonce_sha256.as_bytes(),
    ) {
        return Err(AckError::WrongNonce);
    }
    let health = layout.health();
    match std::fs::create_dir(&health) {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => {
            return Err(AckError::Io(error.kind()));
        }
        _ => {}
    }
    let ack = HealthAck {
        schema: ACK_SCHEMA.to_owned(),
        nonce: nonce.to_hex(),
        pid,
        shell_visible: probes.shell_visible,
        web_view_ready: probes.web_view_ready,
        offline_render: probes.offline_render,
    };
    write_atomic(&ack_path(&layout, pid), &ack).map_err(|error| AckError::Io(error.kind()))
}

fn ack_path(layout: &Layout, pid: u32) -> PathBuf {
    layout.health().join(format!("ack-{pid}.json"))
}

/// Checks an acknowledgement against the nonce and the launched process.
fn check_ack(bytes: &[u8], nonce: &Nonce, pid: u32) -> Result<(), HealthFailure> {
    let ack: HealthAck = serde_json::from_slice(bytes).map_err(|_| HealthFailure::Malformed)?;
    if ack.schema != ACK_SCHEMA {
        return Err(HealthFailure::Malformed);
    }
    if !hex::constant_time_eq(ack.nonce.as_bytes(), nonce.to_hex().as_bytes()) {
        return Err(HealthFailure::WrongNonce);
    }
    if ack.pid != pid {
        return Err(HealthFailure::WrongProcess);
    }
    let probes = Probes {
        shell_visible: ack.shell_visible,
        web_view_ready: ack.web_view_ready,
        offline_render: ack.offline_render,
    };
    if !probes.all_passed() {
        return Err(HealthFailure::ProbeFailed);
    }
    Ok(())
}

/// The started candidate process.
pub trait LaunchedChild {
    fn id(&self) -> u32;
    /// `Some(exit code)` once it ended (`None` inside when unknown).
    fn exited(&mut self) -> Option<Option<i32>>;
    fn terminate(&mut self);
}

/// Starts a payload's shell in health mode.
pub trait Launcher {
    fn launch(&self, shell: &Path, nonce: &Nonce) -> std::io::Result<Box<dyn LaunchedChild>>;
}

/// Time for the health wait (a fake clock in tests).
pub trait Clock {
    /// Time since the launch.
    fn elapsed(&self) -> Duration;
    /// Waits a short poll interval.
    fn pause(&self);
}

/// Waits for a valid acknowledgement from `child` until `deadline`.
pub fn await_health(
    layout: &Layout,
    child: &mut dyn LaunchedChild,
    nonce: &Nonce,
    clock: &dyn Clock,
    deadline: Duration,
) -> Result<(), HealthFailure> {
    let path = ack_path(layout, child.id());
    loop {
        if let Ok(Some(bytes)) = read_bounded(&path) {
            check_ack(&bytes, nonce, child.id())?;
            // The acknowledging process must still be running.
            return match child.exited() {
                Some(code) => Err(HealthFailure::Exited(code)),
                None => Ok(()),
            };
        }
        if let Some(code) = child.exited() {
            return Err(HealthFailure::Exited(code));
        }
        if clock.elapsed() >= deadline {
            return Err(HealthFailure::TimedOut);
        }
        clock.pause();
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivateError {
    Begin(ActivationError),
    /// Rolled back; the previous version stays active.
    RolledBack(HealthFailure),
    /// Rolled back, but the rollback itself reported an error. The next
    /// start's recovery retries it.
    RollbackIncomplete(HealthFailure),
}

impl ActivateError {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Begin(ActivationError::Busy) => "another update is being installed.".to_owned(),
            Self::Begin(ActivationError::Io(kind)) => {
                format!("the update could not be activated ({kind}).")
            }
            Self::RolledBack(failure) | Self::RollbackIncomplete(failure) => {
                format!("{}; the previous version was kept.", failure.message())
            }
        }
    }
}

/// Switches to `staged`, starts it in health mode and commits on a valid
/// acknowledgement within [`HEALTH_DEADLINE`]; otherwise ends the child
/// and rolls back. On success the caller (the old version) exits.
pub fn activate(
    layout: &Layout,
    staged: &StagedPayload,
    nonce: &Nonce,
    launcher: &dyn Launcher,
    clock_from_launch: &dyn Fn() -> Box<dyn Clock>,
) -> Result<(), ActivateError> {
    let mut record =
        begin(layout, staged, nonce, std::process::id()).map_err(ActivateError::Begin)?;
    let fail = |record: &TransactionRecord, failure: HealthFailure| {
        if rollback(layout, record, &failure).is_ok() {
            ActivateError::RolledBack(failure)
        } else {
            ActivateError::RollbackIncomplete(failure)
        }
    };
    let mut child = match launcher.launch(&staged.shell, nonce) {
        Ok(child) => child,
        Err(error) => return Err(fail(&record, HealthFailure::LaunchFailed(error.kind()))),
    };
    let clock = clock_from_launch();
    let _ = note_child(layout, &mut record, child.id());
    match await_health(
        layout,
        child.as_mut(),
        nonce,
        clock.as_ref(),
        HEALTH_DEADLINE,
    ) {
        Ok(()) => commit(layout, &record).map_err(ActivateError::Begin),
        Err(failure) => {
            child.terminate();
            Err(fail(&record, failure))
        }
    }
}

// ---------------------------------------------------------------------------
// Native process adapters (std only).
// ---------------------------------------------------------------------------

/// Starts the shell with `std::process::Command` from its own folder.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessLauncher;

struct ProcessChild(std::process::Child);

impl LaunchedChild for ProcessChild {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn exited(&mut self) -> Option<Option<i32>> {
        match self.0.try_wait() {
            Ok(Some(status)) => Some(status.code()),
            Ok(None) => None,
            Err(_) => Some(None),
        }
    }

    fn terminate(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Launcher for ProcessLauncher {
    fn launch(&self, shell: &Path, nonce: &Nonce) -> std::io::Result<Box<dyn LaunchedChild>> {
        let folder = shell.parent().ok_or(std::io::ErrorKind::NotFound)?;
        let child = std::process::Command::new(shell)
            .arg(HEALTH_ARGUMENT)
            .arg(nonce.to_hex())
            .current_dir(folder)
            .spawn()?;
        Ok(Box::new(ProcessChild(child)))
    }
}

/// Real time since construction; polls every 250 ms.
#[derive(Debug)]
pub struct SystemClock(std::time::Instant);

impl SystemClock {
    #[must_use]
    pub fn start() -> Self {
        Self(std::time::Instant::now())
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }

    fn pause(&self) {
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// The health-mode arguments: exactly `--post-update-health <nonce>`.
#[must_use]
pub fn health_nonce_from_arguments(arguments: &[std::ffi::OsString]) -> Option<String> {
    match arguments {
        [flag, nonce] if flag == HEALTH_ARGUMENT => {
            let nonce = nonce.to_str()?;
            Nonce::parse(nonce).map(|_| nonce.to_owned())
        }
        _ => None,
    }
}
