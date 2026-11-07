//! The registry plan: every key and value the preview writes or removes,
//! as data. Keys are relative to `HKEY_CURRENT_USER`; nothing goes to
//! `HKEY_LOCAL_MACHINE`.
//!
//! The installer applies a plan through [`RegistryBackend`]. The executable
//! uses the real `HKCU` adapter; tests use [`MemoryRegistry`] and never
//! touch the real registry.
//!
//! File associations are opt-in. Even then the preview only adds its ProgID
//! to each extension's `OpenWithProgids` list and registers Default Apps
//! capabilities. It never writes an extension's default value and never
//! touches `UserChoice`, so the .NET edition (or whatever the user chose)
//! stays the default app until the user changes it in Windows Settings.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::identity::{
    CAPABILITIES_KEY, MARKDOWN_EXTENSIONS, PRODUCT_NAME, PROG_ID, PUBLISHER,
    REGISTERED_APPLICATION_NAME, SHELL_FILE_NAME, UNINSTALL_KEY_NAME,
};
use crate::layout::{InstallLayout, quote};

pub const UNINSTALL_ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
pub const CLASSES_ROOT: &str = r"Software\Classes";
pub const REGISTERED_APPLICATIONS: &str = r"Software\RegisteredApplications";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegValue {
    String(String),
    Dword(u32),
    /// `REG_NONE` with no data (an `OpenWithProgids` entry).
    None,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegOp {
    /// Creates the key if needed. `name: None` is the default value.
    Set {
        key: String,
        name: Option<String>,
        value: RegValue,
    },
    /// A missing key or value is not an error.
    DeleteValue { key: String, name: String },
    /// Deletes the key and its subkeys; a missing key is not an error.
    DeleteTree { key: String },
    /// Deletes the key only when it has no values and no subkeys.
    DeleteKeyIfEmpty { key: String },
}

/// `HKCU`, real or in memory.
pub trait RegistryBackend {
    /// # Errors
    /// The registry refused the change.
    fn apply(&mut self, op: &RegOp) -> io::Result<()>;
    /// A string value, `None` when the key or value is missing.
    ///
    /// # Errors
    /// The value exists but could not be read.
    fn read_string(&self, key: &str, name: Option<&str>) -> io::Result<Option<String>>;
    /// # Errors
    /// The key could not be opened for another reason than absence.
    fn key_exists(&self, key: &str) -> io::Result<bool>;
}

/// Applies every op in order and stops at the first error.
///
/// # Errors
/// The first failed op.
pub fn apply_all(backend: &mut dyn RegistryBackend, ops: &[RegOp]) -> io::Result<()> {
    for op in ops {
        backend.apply(op)?;
    }
    Ok(())
}

pub fn uninstall_key() -> String {
    format!(r"{UNINSTALL_ROOT}\{UNINSTALL_KEY_NAME}")
}

pub fn prog_id_key() -> String {
    format!(r"{CLASSES_ROOT}\{PROG_ID}")
}

pub fn application_key() -> String {
    format!(r"{CLASSES_ROOT}\Applications\{SHELL_FILE_NAME}")
}

fn set(key: &str, name: Option<&str>, value: RegValue) -> RegOp {
    RegOp::Set {
        key: key.to_owned(),
        name: name.map(str::to_owned),
        value,
    }
}

fn text(value: impl Into<String>) -> RegValue {
    RegValue::String(value.into())
}

/// The Apps and Features entry. `UninstallString` and
/// `QuietUninstallString` run the installed copy of this setup.
#[must_use]
pub fn uninstall_entry_ops(
    layout: &InstallLayout,
    version: &str,
    estimated_kib: u32,
) -> Vec<RegOp> {
    let key = uninstall_key();
    let setup = quote(&layout.setup_exe());
    vec![
        set(&key, Some("DisplayName"), text(PRODUCT_NAME)),
        set(&key, Some("DisplayVersion"), text(version)),
        set(&key, Some("Publisher"), text(PUBLISHER)),
        set(
            &key,
            Some("Comments"),
            text("Unsigned preview of the Rust edition. It installs beside Marknexia."),
        ),
        set(
            &key,
            Some("InstallLocation"),
            text(layout.install_dir().display().to_string()),
        ),
        set(
            &key,
            Some("DisplayIcon"),
            text(format!("{},0", layout.shell_exe().display())),
        ),
        set(
            &key,
            Some("UninstallString"),
            text(format!("{setup} --uninstall")),
        ),
        set(
            &key,
            Some("QuietUninstallString"),
            text(format!("{setup} --uninstall --silent")),
        ),
        set(&key, Some("NoModify"), RegValue::Dword(1)),
        set(&key, Some("NoRepair"), RegValue::Dword(1)),
        set(&key, Some("EstimatedSize"), RegValue::Dword(estimated_kib)),
    ]
}

/// The opt-in association: a ProgID, the `Applications` entry, the
/// `OpenWithProgids` entries and Default Apps capabilities.
#[must_use]
pub fn association_ops(layout: &InstallLayout) -> Vec<RegOp> {
    let exe = layout.shell_exe();
    let command = format!("{} \"%1\"", quote(&exe));
    let icon = format!("{},0", exe.display());
    let prog_id = prog_id_key();
    let application = application_key();
    let mut ops = vec![
        set(
            &prog_id,
            None,
            text("Markdown document (Marknexia Rust preview)"),
        ),
        set(
            &prog_id,
            Some("FriendlyTypeName"),
            text("Markdown document (Marknexia Rust preview)"),
        ),
        set(&format!(r"{prog_id}\DefaultIcon"), None, text(icon.clone())),
        set(&format!(r"{prog_id}\shell"), None, text("open")),
        set(
            &format!(r"{prog_id}\shell\open\command"),
            None,
            text(command.clone()),
        ),
        set(&application, Some("FriendlyAppName"), text(PRODUCT_NAME)),
        set(&format!(r"{application}\DefaultIcon"), None, text(icon)),
        set(
            &format!(r"{application}\shell\open\command"),
            None,
            text(command),
        ),
        set(
            CAPABILITIES_KEY,
            Some("ApplicationName"),
            text(PRODUCT_NAME),
        ),
        set(
            CAPABILITIES_KEY,
            Some("ApplicationDescription"),
            text("Markdown viewer (Rust preview, unsigned)"),
        ),
    ];
    for extension in MARKDOWN_EXTENSIONS {
        ops.push(set(
            &format!(r"{application}\SupportedTypes"),
            Some(extension),
            text(""),
        ));
        ops.push(set(
            &format!(r"{CLASSES_ROOT}\{extension}\OpenWithProgids"),
            Some(PROG_ID),
            RegValue::None,
        ));
        ops.push(set(
            &format!(r"{CAPABILITIES_KEY}\FileAssociations"),
            Some(extension),
            text(PROG_ID),
        ));
    }
    ops.push(set(
        REGISTERED_APPLICATIONS,
        Some(REGISTERED_APPLICATION_NAME),
        text(CAPABILITIES_KEY),
    ));
    ops
}

/// Removes what [`association_ops`] wrote. The `Applications` entry is
/// removed only when it opens this installation, because Windows creates
/// the same key for a portable copy chosen with "Open with".
///
/// # Errors
/// The `Applications` entry could not be read.
pub fn remove_association_ops(
    backend: &dyn RegistryBackend,
    layout: &InstallLayout,
) -> io::Result<Vec<RegOp>> {
    let mut ops = vec![RegOp::DeleteTree { key: prog_id_key() }];
    for extension in MARKDOWN_EXTENSIONS {
        let open_with = format!(r"{CLASSES_ROOT}\{extension}\OpenWithProgids");
        ops.push(RegOp::DeleteValue {
            key: open_with.clone(),
            name: PROG_ID.to_owned(),
        });
        ops.push(RegOp::DeleteKeyIfEmpty { key: open_with });
        ops.push(RegOp::DeleteKeyIfEmpty {
            key: format!(r"{CLASSES_ROOT}\{extension}"),
        });
    }
    let command =
        backend.read_string(&format!(r"{}\shell\open\command", application_key()), None)?;
    if command.is_some_and(|command| opens_install(&command, layout)) {
        ops.push(RegOp::DeleteTree {
            key: application_key(),
        });
    }
    ops.push(RegOp::DeleteValue {
        key: REGISTERED_APPLICATIONS.to_owned(),
        name: REGISTERED_APPLICATION_NAME.to_owned(),
    });
    ops.push(RegOp::DeleteKeyIfEmpty {
        key: REGISTERED_APPLICATIONS.to_owned(),
    });
    ops.push(RegOp::DeleteTree {
        key: r"Software\Marknexia\RustPreview".to_owned(),
    });
    ops.push(RegOp::DeleteKeyIfEmpty {
        key: r"Software\Marknexia".to_owned(),
    });
    Ok(ops)
}

/// A command line that starts this installation's shell.
fn opens_install(command: &str, layout: &InstallLayout) -> bool {
    let expected = quote(&layout.shell_exe()).to_ascii_lowercase();
    command
        .trim_start()
        .to_ascii_lowercase()
        .starts_with(&expected)
}

/// The user opted in earlier (the ProgID exists).
///
/// # Errors
/// The key could not be opened.
pub fn is_associated(backend: &dyn RegistryBackend) -> io::Result<bool> {
    backend.key_exists(&prog_id_key())
}

/// Everything uninstall removes from the registry.
///
/// # Errors
/// See [`remove_association_ops`].
pub fn uninstall_ops(
    backend: &dyn RegistryBackend,
    layout: &InstallLayout,
) -> io::Result<Vec<RegOp>> {
    let mut ops = remove_association_ops(backend, layout)?;
    ops.push(RegOp::DeleteTree {
        key: uninstall_key(),
    });
    Ok(ops)
}

/// The install location recorded in the uninstall entry, when present.
///
/// # Errors
/// The value could not be read.
pub fn recorded_install_location(backend: &dyn RegistryBackend) -> io::Result<Option<String>> {
    backend.read_string(&uninstall_key(), Some("InstallLocation"))
}

/// Whether a path is inside (or is) a folder, case-insensitively (ASCII).
#[must_use]
pub fn path_is_within(path: &Path, folder: &Path) -> bool {
    let path = path.to_string_lossy().to_ascii_lowercase();
    let folder = folder.to_string_lossy().to_ascii_lowercase();
    let folder = folder.trim_end_matches('\\');
    path == folder || path.starts_with(&format!("{folder}\\"))
}

/// An in-memory `HKCU` for tests and dry runs. Key and value names are
/// case-insensitive, as in the registry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryRegistry {
    /// Lower-case key path to (lower-case value name, "" for the default) to value.
    keys: BTreeMap<String, BTreeMap<String, RegValue>>,
}

fn normalize(key: &str) -> String {
    key.trim_matches('\\').to_ascii_lowercase()
}

impl MemoryRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The value, for assertions.
    #[must_use]
    pub fn value(&self, key: &str, name: Option<&str>) -> Option<&RegValue> {
        self.keys
            .get(&normalize(key))?
            .get(&name.unwrap_or_default().to_ascii_lowercase())
    }

    /// Every key path (lower case), for assertions.
    #[must_use]
    pub fn key_paths(&self) -> Vec<&str> {
        self.keys.keys().map(String::as_str).collect()
    }

    fn has_subkeys(&self, key: &str) -> bool {
        let prefix = format!("{key}\\");
        self.keys.keys().any(|other| other.starts_with(&prefix))
    }

    fn create(&mut self, key: &str) -> &mut BTreeMap<String, RegValue> {
        // Parent keys exist implicitly, as RegCreateKeyEx creates them.
        let mut path = String::new();
        for part in key.split('\\') {
            if !path.is_empty() {
                path.push('\\');
            }
            path.push_str(part);
            self.keys.entry(path.clone()).or_default();
        }
        self.keys.entry(key.to_owned()).or_default()
    }
}

impl RegistryBackend for MemoryRegistry {
    fn apply(&mut self, op: &RegOp) -> io::Result<()> {
        match op {
            RegOp::Set { key, name, value } => {
                let key = normalize(key);
                if key.is_empty() {
                    return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty key"));
                }
                self.create(&key).insert(
                    name.as_deref().unwrap_or_default().to_ascii_lowercase(),
                    value.clone(),
                );
            }
            RegOp::DeleteValue { key, name } => {
                if let Some(values) = self.keys.get_mut(&normalize(key)) {
                    values.remove(&name.to_ascii_lowercase());
                }
            }
            RegOp::DeleteTree { key } => {
                let key = normalize(key);
                let prefix = format!("{key}\\");
                self.keys
                    .retain(|other, _| *other != key && !other.starts_with(&prefix));
            }
            RegOp::DeleteKeyIfEmpty { key } => {
                let key = normalize(key);
                if self.keys.get(&key).is_some_and(BTreeMap::is_empty) && !self.has_subkeys(&key) {
                    self.keys.remove(&key);
                }
            }
        }
        Ok(())
    }

    fn read_string(&self, key: &str, name: Option<&str>) -> io::Result<Option<String>> {
        Ok(match self.value(key, name) {
            Some(RegValue::String(value)) => Some(value.clone()),
            _ => None,
        })
    }

    fn key_exists(&self, key: &str) -> io::Result<bool> {
        Ok(self.keys.contains_key(&normalize(key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::dotnet;

    fn layout() -> InstallLayout {
        InstallLayout::new(
            Path::new(r"C:\Users\u\AppData\Local"),
            Path::new(r"C:\Users\u\AppData\Roaming\Microsoft\Windows\Start Menu\Programs"),
        )
        .expect("layout")
    }

    fn keys_written(ops: &[RegOp]) -> Vec<String> {
        ops.iter()
            .filter_map(|op| match op {
                RegOp::Set { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_uninstall_entry_runs_the_installed_setup() {
        let layout = layout();
        let mut registry = MemoryRegistry::new();
        apply_all(&mut registry, &uninstall_entry_ops(&layout, "0.1.0", 4096)).expect("apply");
        let key = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\MarknexiaRustPreview";
        assert_eq!(
            registry
                .read_string(key, Some("DisplayName"))
                .unwrap()
                .as_deref(),
            Some("Marknexia (Rust preview)")
        );
        assert_eq!(
            registry
                .read_string(key, Some("QuietUninstallString"))
                .unwrap()
                .as_deref(),
            Some(
                "\"C:\\Users\\u\\AppData\\Local\\Programs\\Marknexia Rust Preview\\marknexia-rust-preview-setup.exe\" --uninstall --silent"
            )
        );
        assert_eq!(
            registry.value(key, Some("NoModify")),
            Some(&RegValue::Dword(1))
        );
        assert_eq!(
            registry.value(key, Some("EstimatedSize")),
            Some(&RegValue::Dword(4096))
        );
        assert_eq!(
            recorded_install_location(&registry).unwrap().as_deref(),
            Some(r"C:\Users\u\AppData\Local\Programs\Marknexia Rust Preview")
        );
    }

    #[test]
    fn nothing_is_written_outside_hkcu_paths_the_preview_owns() {
        let layout = layout();
        let mut ops = uninstall_entry_ops(&layout, "0.1.0", 1);
        ops.extend(association_ops(&layout));
        for key in keys_written(&ops) {
            let lower = key.to_ascii_lowercase();
            assert!(
                !lower.starts_with("hkey") && !lower.starts_with("system"),
                "{key}"
            );
            assert_ne!(lower, dotnet::APPLICATION_KEY.to_ascii_lowercase());
            assert!(
                !lower.contains(&format!(r"\{}", dotnet::PROG_ID.to_ascii_lowercase())),
                "{key}"
            );
            assert!(!lower.ends_with(r"\uninstall\marknexia"), "{key}");
        }
    }

    #[test]
    fn associations_never_take_the_default() {
        let ops = association_ops(&layout());
        for op in &ops {
            if let RegOp::Set { key, name, .. } = op {
                for extension in MARKDOWN_EXTENSIONS {
                    let extension_key = format!(r"{CLASSES_ROOT}\{extension}");
                    // No default value and no UserChoice on the extension key.
                    assert!(
                        !(key.eq_ignore_ascii_case(&extension_key) && name.is_none()),
                        "{key}"
                    );
                    assert!(!key.to_ascii_lowercase().contains("userchoice"), "{key}");
                    assert!(!key.to_ascii_lowercase().contains("fileexts"), "{key}");
                }
            }
        }
        let mut registry = MemoryRegistry::new();
        apply_all(&mut registry, &ops).expect("apply");
        assert_eq!(
            registry.value(r"Software\Classes\.md\OpenWithProgids", Some(PROG_ID)),
            Some(&RegValue::None)
        );
        assert_eq!(
            registry.read_string(r"Software\Classes\.md", None).unwrap(),
            None
        );
        assert_eq!(
            registry
                .read_string(
                    r"Software\Classes\Marknexia.RustPreview.MarkdownFile\shell\open\command",
                    None
                )
                .unwrap()
                .as_deref(),
            Some(
                "\"C:\\Users\\u\\AppData\\Local\\Programs\\Marknexia Rust Preview\\marknexia-win32.exe\" \"%1\""
            )
        );
        assert_eq!(
            registry
                .read_string(REGISTERED_APPLICATIONS, Some(REGISTERED_APPLICATION_NAME))
                .unwrap()
                .as_deref(),
            Some(CAPABILITIES_KEY)
        );
        assert!(is_associated(&registry).unwrap());
    }

    #[test]
    fn dotnet_registrations_survive_install_and_uninstall() {
        let layout = layout();
        let mut registry = MemoryRegistry::new();
        // What the .NET setup writes.
        let dotnet_ops = vec![
            set(r"Software\Classes\.md", None, text(dotnet::PROG_ID)),
            set(
                r"Software\Classes\.md\OpenWithProgids",
                Some(dotnet::PROG_ID),
                RegValue::None,
            ),
            set(
                &format!(r"{CLASSES_ROOT}\{}\shell\open\command", dotnet::PROG_ID),
                None,
                text("x"),
            ),
            set(
                dotnet::APPLICATION_KEY,
                Some("FriendlyAppName"),
                text("Marknexia"),
            ),
            set(
                r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Marknexia",
                Some("DisplayName"),
                text("Marknexia"),
            ),
        ];
        apply_all(&mut registry, &dotnet_ops).expect("dotnet");
        let before = registry.clone();

        apply_all(&mut registry, &uninstall_entry_ops(&layout, "0.1.0", 1)).expect("entry");
        apply_all(&mut registry, &association_ops(&layout)).expect("associate");
        assert_eq!(
            registry
                .read_string(r"Software\Classes\.md", None)
                .unwrap()
                .as_deref(),
            Some(dotnet::PROG_ID)
        );

        let ops = uninstall_ops(&registry, &layout).expect("plan");
        apply_all(&mut registry, &ops).expect("uninstall");
        assert_eq!(registry, before);
    }

    #[test]
    fn uninstall_leaves_no_preview_key_behind() {
        let layout = layout();
        let mut registry = MemoryRegistry::new();
        apply_all(&mut registry, &uninstall_entry_ops(&layout, "0.1.0", 1)).expect("entry");
        apply_all(&mut registry, &association_ops(&layout)).expect("associate");
        let ops = uninstall_ops(&registry, &layout).expect("plan");
        apply_all(&mut registry, &ops).expect("uninstall");
        // Only the shared parents that RegCreateKeyEx creates may stay.
        for key in registry.key_paths() {
            assert!(
                [
                    "software",
                    r"software\classes",
                    r"software\classes\applications",
                    r"software\microsoft",
                    r"software\microsoft\windows",
                    r"software\microsoft\windows\currentversion",
                    r"software\microsoft\windows\currentversion\uninstall",
                    r"software\registeredapplications",
                ]
                .contains(&key),
                "{key}"
            );
        }
        assert!(!is_associated(&registry).unwrap());
    }

    #[test]
    fn a_foreign_applications_entry_is_kept() {
        let layout = layout();
        let mut registry = MemoryRegistry::new();
        // Windows wrote this for a portable copy chosen with "Open with".
        apply_all(
            &mut registry,
            &[set(
                r"Software\Classes\Applications\marknexia-win32.exe\shell\open\command",
                None,
                text("\"D:\\portable\\marknexia-win32.exe\" \"%1\""),
            )],
        )
        .expect("portable");
        let ops = uninstall_ops(&registry, &layout).expect("plan");
        assert!(!ops.contains(&RegOp::DeleteTree {
            key: application_key()
        }));
        apply_all(&mut registry, &ops).expect("uninstall");
        assert!(registry.key_exists(&application_key()).unwrap());
    }

    #[test]
    fn delete_if_empty_keeps_keys_with_content() {
        let mut registry = MemoryRegistry::new();
        apply_all(
            &mut registry,
            &[
                set(
                    r"Software\Classes\.md\OpenWithProgids",
                    Some("Other.App"),
                    RegValue::None,
                ),
                RegOp::DeleteKeyIfEmpty {
                    key: r"Software\Classes\.md\OpenWithProgids".to_owned(),
                },
                RegOp::DeleteKeyIfEmpty {
                    key: r"Software\Classes\.md".to_owned(),
                },
            ],
        )
        .expect("apply");
        assert!(
            registry
                .key_exists(r"Software\Classes\.md\OpenWithProgids")
                .unwrap()
        );
    }

    #[test]
    fn paths_within_folders_fold_ascii_case() {
        let folder = Path::new(r"C:\Users\u\AppData\Local\Programs\Marknexia Rust Preview");
        assert!(path_is_within(
            Path::new(r"c:\users\u\appdata\local\programs\marknexia rust preview\x.exe"),
            folder
        ));
        assert!(path_is_within(folder, folder));
        assert!(!path_is_within(
            Path::new(r"C:\Users\u\AppData\Local\Programs\Marknexia Rust Preview2\x.exe"),
            folder
        ));
    }
}
