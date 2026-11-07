//! Install, upgrade and uninstall, over injected registry and shell
//! adapters, so tests run against temp folders and a [`MemoryRegistry`]
//! (`crate::registry::MemoryRegistry`).
//!
//! Install (per user, no elevation):
//! 1. The payload is a [`VerifiedPayload`]: every hash passed before this
//!    module runs, so nothing is written for a damaged payload.
//! 2. A running installed shell (or a running payload that the updater
//!    staged) stops the install: "Close Marknexia (Rust preview) …".
//! 3. Leftover staging and backup folders of an interrupted setup are
//!    removed.
//! 4. The payload is written to a new sibling staging folder (files created
//!    new, then read back and hashed), with a copy of the setup for
//!    uninstall.
//! 5. An existing installation is renamed to a backup folder, the staging
//!    folder takes its place, and the backup is removed at the end. A
//!    failed step restores the backup.
//! 6. The uninstall entry, the Start menu shortcut and, when chosen, the
//!    "Open with" associations are registered. A registration failure
//!    restores the previous files (and, for a first install, removes the
//!    new entries).
//!
//! Uninstall removes the updater state of the Rust edition, the payload
//! files, the registry entries and the shortcut, then the setup itself
//! (moved to the temp folder when it is the running copy) and the folder
//! when it is empty. Unknown files in the install folder are kept.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::identity::{PRODUCT_NAME, SETUP_FILE_NAME, SHELL_FILE_NAME};
use crate::layout::InstallLayout;
use crate::payload::{EXPECTED_FILES, VerifiedPayload};
use crate::registry::{self, RegistryBackend};
use crate::settings_probe::{self, SettingsProbe};
use crate::update_state::{self, UpdateStateReport, is_reparse_point};

/// The Start menu shortcut and the shell's association refresh.
pub trait ShellIntegration {
    /// # Errors
    /// The shortcut could not be written.
    fn create_shortcut(
        &mut self,
        link: &Path,
        target: &Path,
        working_dir: &Path,
        description: &str,
    ) -> io::Result<()>;
    /// `SHChangeNotify(SHCNE_ASSOCCHANGED)`.
    fn associations_changed(&mut self);
}

/// What the setup needs from its process.
pub struct Host<'a> {
    pub registry: &'a mut dyn RegistryBackend,
    pub shell: &'a mut dyn ShellIntegration,
    /// The running setup: copied into the install folder on install, moved
    /// away on uninstall when it runs from there.
    pub setup_path: &'a Path,
    /// The user's temp folder.
    pub temp_dir: &'a Path,
    /// A new random token (ASCII letters and digits) for folder names.
    pub token: &'a mut dyn FnMut() -> String,
}

/// File associations on install.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Associations {
    /// Keep the user's earlier choice (none on a first install).
    Keep,
    /// The user opted in.
    Enable,
    /// The user opted out; earlier entries are removed.
    Disable,
}

#[derive(Debug)]
pub enum SetupError {
    /// A shell of this installation (or a staged update) runs.
    InUse {
        uninstall: bool,
        path: PathBuf,
    },
    /// A folder is a link or junction.
    NotSafe(PathBuf),
    Io {
        action: String,
        error: io::Error,
    },
    Registry(io::Error),
    Shortcut(io::Error),
}

impl fmt::Display for SetupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InUse {
                uninstall: false, ..
            } => {
                write!(
                    formatter,
                    "Close {PRODUCT_NAME} before installing or upgrading it."
                )
            }
            Self::InUse {
                uninstall: true, ..
            } => write!(formatter, "Close {PRODUCT_NAME} before uninstalling it."),
            Self::NotSafe(path) => write!(
                formatter,
                "The folder {} is a link or junction; setup does not change it.",
                path.display()
            ),
            Self::Io { action, error } => write!(formatter, "Setup could not {action}: {error}"),
            Self::Registry(error) => write!(
                formatter,
                "Setup could not update the per-user registration: {error}"
            ),
            Self::Shortcut(error) => write!(
                formatter,
                "Setup could not create the Start menu shortcut: {error}"
            ),
        }
    }
}

impl std::error::Error for SetupError {}

fn io_error(action: impl Into<String>) -> impl FnOnce(io::Error) -> SetupError {
    let action = action.into();
    move |error| SetupError::Io { action, error }
}

#[derive(Debug)]
pub struct InstallReport {
    pub install_dir: PathBuf,
    pub version: String,
    pub upgraded: bool,
    pub associated: bool,
    pub settings: SettingsProbe,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default)]
pub struct UninstallReport {
    pub removed: Vec<PathBuf>,
    pub kept: Vec<PathBuf>,
    /// Where the running setup moved itself (Windows removes it with the
    /// other temporary files).
    pub setup_moved_to: Option<PathBuf>,
    pub update_state: UpdateStateReport,
    pub install_dir_removed: bool,
}

/// A file that another process holds open without write sharing, as
/// Windows does for a running executable.
#[must_use]
pub fn is_in_use(path: &Path) -> bool {
    match OpenOptions::new().append(true).open(path) {
        Ok(_) => false,
        // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
        Err(error) => matches!(error.raw_os_error(), Some(5 | 32 | 33)),
    }
}

fn running_shells(layout: &InstallLayout) -> Result<Vec<PathBuf>, SetupError> {
    let mut shells = vec![layout.shell_exe()];
    shells.extend(
        update_state::payload_shells(layout.updates_root())
            .map_err(io_error("read the update folder"))?,
    );
    Ok(shells
        .into_iter()
        .filter(|shell| is_in_use(shell))
        .collect())
}

fn refuse_links(path: &Path) -> Result<(), SetupError> {
    match is_reparse_point(path) {
        Ok(true) => Err(SetupError::NotSafe(path.to_path_buf())),
        Ok(false) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SetupError::Io {
            action: format!("inspect {}", path.display()),
            error,
        }),
    }
}

fn sha256_of_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

/// Writes every payload file as a new file and reads it back.
fn write_payload(folder: &Path, payload: &VerifiedPayload<'_>) -> Result<(), SetupError> {
    for entry in payload.entries() {
        let path = folder.join(entry.name);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error(format!("create {}", path.display())))?;
        file.write_all(entry.bytes)
            .and_then(|()| file.sync_all())
            .map_err(io_error(format!("write {}", path.display())))?;
        drop(file);
        let written =
            sha256_of_file(&path).map_err(io_error(format!("read back {}", path.display())))?;
        if written != entry.sha256 {
            return Err(SetupError::Io {
                action: format!("verify {}", path.display()),
                error: io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the written file does not match the payload",
                ),
            });
        }
    }
    Ok(())
}

/// Renames with a few short retries (a scanner can hold a new file).
fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 4 && matches!(error.raw_os_error(), Some(5 | 32 | 33)) => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            Err(error) => return Err(error),
        }
    }
}

/// Removes staging and backup folders that an interrupted setup left.
fn remove_leftovers(layout: &InstallLayout, temp_dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Ok(entries) = fs::read_dir(layout.programs()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let path = entry.path();
            if name.to_str().is_some_and(InstallLayout::is_leftover_name)
                && matches!(is_reparse_point(&path), Ok(false))
                && fs::remove_dir_all(&path).is_err()
            {
                warnings.push(format!(
                    "A leftover folder could not be removed: {}",
                    path.display()
                ));
            }
        }
    }
    // Copies that earlier uninstalls moved to the temp folder.
    if let Ok(entries) = fs::read_dir(temp_dir) {
        for entry in entries.flatten() {
            if entry.file_name().to_str().is_some_and(|name| {
                name.starts_with(UNINSTALL_COPY_PREFIX) && name.ends_with(".exe")
            }) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    warnings
}

const UNINSTALL_COPY_PREFIX: &str = "marknexia-rust-preview-uninstall-";

/// Installs or upgrades. See the module documentation for the steps.
///
/// # Errors
/// [`SetupError`]; the previous installation is kept or restored.
pub fn install(
    layout: &InstallLayout,
    payload: &VerifiedPayload<'_>,
    host: &mut Host<'_>,
    version: &str,
    associations: Associations,
) -> Result<InstallReport, SetupError> {
    let install_dir = layout.install_dir();
    refuse_links(install_dir)?;
    if let Some(path) = running_shells(layout)?.into_iter().next() {
        return Err(SetupError::InUse {
            uninstall: false,
            path,
        });
    }
    let settings = settings_probe::probe(layout.settings_file());
    let mut warnings = remove_leftovers(layout, host.temp_dir);
    fs::create_dir_all(layout.programs())
        .map_err(io_error(format!("create {}", layout.programs().display())))?;

    // Stage.
    let staging = layout.staging_dir(&(host.token)());
    fs::create_dir(&staging).map_err(io_error(format!("create {}", staging.display())))?;
    let staged = write_payload(&staging, payload).and_then(|()| {
        let target = staging.join(SETUP_FILE_NAME);
        fs::copy(host.setup_path, &target)
            .map(|_| ())
            .map_err(io_error(format!("copy the setup to {}", target.display())))
    });
    if let Err(error) = staged {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }

    // Swap.
    let backup = if install_dir.exists() {
        let backup = layout.backup_dir(&(host.token)());
        if let Err(error) = rename_with_retry(install_dir, &backup) {
            let _ = fs::remove_dir_all(&staging);
            return Err(if matches!(error.raw_os_error(), Some(5 | 32 | 33)) {
                SetupError::InUse {
                    uninstall: false,
                    path: install_dir.to_path_buf(),
                }
            } else {
                SetupError::Io {
                    action: format!(
                        "move the previous installation from {}",
                        install_dir.display()
                    ),
                    error,
                }
            });
        }
        Some(backup)
    } else {
        None
    };
    if let Err(error) = rename_with_retry(&staging, install_dir) {
        if let Some(backup) = &backup {
            let _ = rename_with_retry(backup, install_dir);
        }
        let _ = fs::remove_dir_all(&staging);
        return Err(SetupError::Io {
            action: format!("move the new files to {}", install_dir.display()),
            error,
        });
    }

    // Register.
    let was_associated = registry::is_associated(host.registry).unwrap_or(false);
    let associate = match associations {
        Associations::Keep => was_associated,
        Associations::Enable => true,
        Associations::Disable => false,
    };
    let setup_bytes = fs::metadata(layout.setup_exe())
        .map(|meta| meta.len())
        .unwrap_or(0);
    let estimated_kib =
        u32::try_from((payload.total_bytes() + setup_bytes).div_ceil(1024)).unwrap_or(u32::MAX);
    let registration = (|| {
        let mut ops = registry::uninstall_entry_ops(layout, version, estimated_kib);
        if associate {
            ops.extend(registry::association_ops(layout));
        } else if was_associated {
            ops.extend(
                registry::remove_association_ops(host.registry, layout)
                    .map_err(SetupError::Registry)?,
            );
        }
        registry::apply_all(host.registry, &ops).map_err(SetupError::Registry)?;
        if let Some(parent) = layout.shortcut().parent() {
            fs::create_dir_all(parent).map_err(SetupError::Shortcut)?;
        }
        host.shell
            .create_shortcut(
                layout.shortcut(),
                &layout.shell_exe(),
                install_dir,
                "Marknexia Markdown viewer (Rust preview)",
            )
            .map_err(SetupError::Shortcut)
    })();
    if let Err(error) = registration {
        let _ = fs::remove_dir_all(install_dir);
        match &backup {
            Some(backup) => {
                let _ = rename_with_retry(backup, install_dir);
            }
            None => {
                if let Ok(ops) = registry::uninstall_ops(host.registry, layout) {
                    let _ = registry::apply_all(host.registry, &ops);
                }
                let _ = fs::remove_file(layout.shortcut());
            }
        }
        if associate || was_associated {
            host.shell.associations_changed();
        }
        return Err(error);
    }
    if associate || was_associated {
        host.shell.associations_changed();
    }
    let upgraded = backup.is_some();
    if let Some(backup) = backup
        && fs::remove_dir_all(&backup).is_err()
    {
        warnings.push(format!(
            "The previous installation could not be removed completely; the next setup removes {}",
            backup.display()
        ));
    }
    Ok(InstallReport {
        install_dir: install_dir.to_path_buf(),
        version: version.to_owned(),
        upgraded,
        associated: associate,
        settings,
        warnings,
    })
}

fn remove_file_if_present(path: &Path) -> io::Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
        || matches!(
            (fs::canonicalize(left), fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left.to_string_lossy().eq_ignore_ascii_case(&right.to_string_lossy())
        )
}

/// Uninstalls. Safe to run again after a partial uninstall.
///
/// # Errors
/// [`SetupError`]. A running shell stops the uninstall before anything is
/// removed.
pub fn uninstall(
    layout: &InstallLayout,
    host: &mut Host<'_>,
) -> Result<UninstallReport, SetupError> {
    let install_dir = layout.install_dir();
    refuse_links(install_dir)?;
    if let Some(path) = running_shells(layout)?.into_iter().next() {
        return Err(SetupError::InUse {
            uninstall: true,
            path,
        });
    }
    let mut report = UninstallReport {
        update_state: update_state::remove(layout.updates_root())
            .map_err(io_error("remove the update folder"))?,
        ..UninstallReport::default()
    };

    for name in EXPECTED_FILES {
        let path = install_dir.join(name);
        match remove_file_if_present(&path) {
            Ok(true) => report.removed.push(path),
            Ok(false) => {}
            Err(error)
                if matches!(error.raw_os_error(), Some(5 | 32 | 33)) && name == SHELL_FILE_NAME =>
            {
                return Err(SetupError::InUse {
                    uninstall: true,
                    path,
                });
            }
            Err(error) => return Err(io_error(format!("remove {}", path.display()))(error)),
        }
    }

    let ops = registry::uninstall_ops(host.registry, layout).map_err(SetupError::Registry)?;
    registry::apply_all(host.registry, &ops).map_err(SetupError::Registry)?;
    if remove_file_if_present(layout.shortcut()).map_err(SetupError::Shortcut)? {
        report.removed.push(layout.shortcut().to_path_buf());
    }
    host.shell.associations_changed();

    let setup = layout.setup_exe();
    if same_path(host.setup_path, &setup) {
        // A running executable can be renamed but not deleted.
        let moved = host
            .temp_dir
            .join(format!("{UNINSTALL_COPY_PREFIX}{}.exe", (host.token)()));
        match fs::rename(&setup, &moved) {
            Ok(()) => report.setup_moved_to = Some(moved),
            Err(_) => report.kept.push(setup),
        }
    } else {
        match remove_file_if_present(&setup) {
            Ok(true) => report.removed.push(setup),
            Ok(false) => {}
            Err(_) => report.kept.push(setup),
        }
    }

    match fs::read_dir(install_dir) {
        Ok(entries) => {
            let leftovers: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
            if leftovers.is_empty() {
                fs::remove_dir(install_dir)
                    .map_err(io_error(format!("remove {}", install_dir.display())))?;
                report.install_dir_removed = true;
            } else {
                report.kept.extend(leftovers);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => report.install_dir_removed = true,
        Err(error) => return Err(io_error(format!("list {}", install_dir.display()))(error)),
    }
    Ok(report)
}
