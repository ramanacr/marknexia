//! What a portable payload must and may contain.

use crate::archive::Entry;

/// Who must have signed an executable in the payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signer {
    /// The configured release publisher (`UpdateConfig::publisher`).
    Release,
    /// Microsoft (the WebView2 loader), with the Microsoft root policy.
    Microsoft,
}

/// An executable the payload may contain, at its root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedExecutable {
    pub name: String,
    pub signer: Signer,
    /// The native shell, whose PE machine must match the architecture.
    pub is_shell: bool,
}

/// Required files and allowed executables, by name at the payload root.
/// Names compare case-insensitively (Windows file names).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadPolicy {
    pub required_files: Vec<String>,
    pub executables: Vec<AllowedExecutable>,
}

/// The native shell executable in a portable payload.
pub const SHELL_EXECUTABLE: &str = "marknexia-win32.exe";
/// The WebView2 loader in a portable payload.
pub const WEBVIEW_LOADER: &str = "WebView2Loader.dll";

/// Extensions treated as executable content. Any such entry that is not an
/// allowed executable at the root rejects the archive.
pub const EXECUTABLE_EXTENSIONS: [&str; 30] = [
    "exe",
    "dll",
    "com",
    "scr",
    "cpl",
    "sys",
    "ocx",
    "drv",
    "efi",
    "mui",
    "msi",
    "msp",
    "msix",
    "appx",
    "appxbundle",
    "msixbundle",
    "bat",
    "cmd",
    "ps1",
    "psm1",
    "vbs",
    "vbe",
    "js",
    "jse",
    "wsf",
    "wsh",
    "hta",
    "lnk",
    "pif",
    "reg",
];

impl PayloadPolicy {
    /// The Rust portable ZIP (`scripts/New-RustPortablePackage.ps1`): the
    /// shell, signed by the release publisher, and the Microsoft-signed
    /// WebView2 loader.
    #[must_use]
    pub fn rust_portable() -> Self {
        Self {
            required_files: vec![SHELL_EXECUTABLE.to_owned(), WEBVIEW_LOADER.to_owned()],
            executables: vec![
                AllowedExecutable {
                    name: SHELL_EXECUTABLE.to_owned(),
                    signer: Signer::Release,
                    is_shell: true,
                },
                AllowedExecutable {
                    name: WEBVIEW_LOADER.to_owned(),
                    signer: Signer::Microsoft,
                    is_shell: false,
                },
            ],
        }
    }

    /// The executable that is started for the health check.
    #[must_use]
    pub fn shell(&self) -> Option<&AllowedExecutable> {
        self.executables
            .iter()
            .find(|executable| executable.is_shell)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PayloadError {
    /// An executable that the policy does not allow (or not at the root).
    UnexpectedExecutable(String),
    /// The first required file that is missing (.NET order).
    MissingFile(String),
}

impl PayloadError {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::UnexpectedExecutable(name) => {
                format!("The update package contains an unexpected executable '{name}'.")
            }
            // .NET `ValidatePortablePayload`.
            Self::MissingFile(name) => {
                format!("The update package is missing required file '{name}'.")
            }
        }
    }
}

fn is_executable_name(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, extension)| {
        EXECUTABLE_EXTENSIONS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(extension))
    })
}

/// Checks the listed entries against `policy` before anything is written.
pub fn check(entries: &[Entry], policy: &PayloadPolicy) -> Result<(), PayloadError> {
    for entry in entries.iter().filter(|entry| !entry.is_dir) {
        if is_executable_name(&entry.path)
            && !policy
                .executables
                .iter()
                .any(|allowed| allowed.name.eq_ignore_ascii_case(&entry.path))
        {
            return Err(PayloadError::UnexpectedExecutable(entry.path.clone()));
        }
    }
    for required in &policy.required_files {
        if !entries
            .iter()
            .any(|entry| !entry.is_dir && entry.path.eq_ignore_ascii_case(required))
        {
            return Err(PayloadError::MissingFile(required.clone()));
        }
    }
    Ok(())
}

/// The `IMAGE_FILE_HEADER.Machine` of a PE image, from its first bytes
/// (at least through the PE signature and machine field).
#[must_use]
pub fn pe_machine(header: &[u8]) -> Option<u16> {
    if header.get(..2)? != b"MZ" {
        return None;
    }
    let offset = u32::from_le_bytes(header.get(0x3c..0x40)?.try_into().ok()?) as usize;
    if header.get(offset..offset.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let machine = header.get(offset + 4..offset + 6)?;
    Some(u16::from_le_bytes([machine[0], machine[1]]))
}
