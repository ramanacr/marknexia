//! The file names that the setup writes, as data.
//!
//! The executable uses [`FileNames::release`] (the real `.exe` and `.dll`
//! names). Tests use [`FileNames::stand_ins`], whose names have no
//! executable extension, so no test writes a file named `*.exe` or `*.dll`
//! (the AV policy of the development machines; see the production plan).

use std::fmt;

use crate::container::is_valid_name;
use crate::identity::{SETUP_FILE_NAME, SHELL_FILE_NAME};

/// The release payload, in ordinal order.
pub const RELEASE_PAYLOAD: [&str; 5] = [
    "README.txt",
    "THIRD-PARTY-NOTICES.txt",
    "WebView2Loader.dll",
    "marknexia-sbom.spdx.json",
    SHELL_FILE_NAME,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileNames {
    shell: String,
    setup: String,
    payload: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamesError(pub String);

impl fmt::Display for NamesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for NamesError {}

impl FileNames {
    /// # Errors
    ///
    /// The names must be valid container names, the payload must be in
    /// strict ordinal order and hold the shell, and the setup copy must not
    /// collide with a payload file.
    pub fn new(shell: &str, setup: &str, payload: &[&str]) -> Result<Self, NamesError> {
        for name in payload.iter().chain([&shell, &setup]) {
            if !is_valid_name(name) {
                return Err(NamesError(format!("'{name}' is not a plain file name")));
            }
        }
        if payload
            .windows(2)
            .any(|pair| pair[0].as_bytes() >= pair[1].as_bytes())
        {
            return Err(NamesError(
                "the payload names must be in strict ordinal order".to_owned(),
            ));
        }
        if !payload.contains(&shell) {
            return Err(NamesError("the payload must hold the shell".to_owned()));
        }
        if payload.iter().any(|name| name.eq_ignore_ascii_case(setup)) {
            return Err(NamesError(
                "the setup copy collides with a payload file".to_owned(),
            ));
        }
        Ok(Self {
            shell: shell.to_owned(),
            setup: setup.to_owned(),
            payload: payload.iter().map(|name| (*name).to_owned()).collect(),
        })
    }

    /// The names that the setup executable installs.
    #[must_use]
    pub fn release() -> Self {
        Self::new(SHELL_FILE_NAME, SETUP_FILE_NAME, &RELEASE_PAYLOAD)
            .expect("the release names are valid")
    }

    /// Non-executable stand-ins for tests.
    #[cfg(test)]
    #[must_use]
    pub fn stand_ins() -> Self {
        Self::new(
            "shell.bin.txt",
            "setup.bin.txt",
            &[
                "README.txt",
                "THIRD-PARTY-NOTICES.txt",
                "loader.bin.txt",
                "marknexia-sbom.spdx.json",
                "shell.bin.txt",
            ],
        )
        .expect("the stand-in names are valid")
    }

    /// The shell executable's file name.
    #[must_use]
    pub fn shell(&self) -> &str {
        &self.shell
    }

    /// The installed copy of the setup (the uninstaller).
    #[must_use]
    pub fn setup(&self) -> &str {
        &self.setup
    }

    /// Every payload file, in ordinal order.
    #[must_use]
    pub fn payload(&self) -> &[String] {
        &self.payload
    }

    /// The extension of the setup copy (`exe` in a release), used for the
    /// copy that a running uninstaller moves to the temp folder.
    #[must_use]
    pub fn setup_extension(&self) -> &str {
        self.setup
            .rsplit_once('.')
            .map_or("", |(_, extension)| extension)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_names_are_the_shipped_files() {
        let names = FileNames::release();
        assert_eq!(names.shell(), "marknexia-win32.exe");
        assert_eq!(names.setup(), "marknexia-rust-preview-setup.exe");
        assert_eq!(names.payload(), RELEASE_PAYLOAD);
        assert_eq!(names.setup_extension(), "exe");
    }

    #[test]
    fn stand_ins_have_no_executable_extension() {
        let names = FileNames::stand_ins();
        for name in names
            .payload()
            .iter()
            .map(String::as_str)
            .chain([names.setup()])
        {
            let lower = name.to_ascii_lowercase();
            assert!(
                !lower.ends_with(".exe") && !lower.ends_with(".dll"),
                "{name}"
            );
        }
        assert_eq!(names.setup_extension(), "txt");
    }

    #[test]
    fn invalid_name_sets_are_refused() {
        assert!(FileNames::new("a.txt", "s.txt", &["b.txt", "a.txt"]).is_err());
        assert!(FileNames::new("c.txt", "s.txt", &["a.txt", "b.txt"]).is_err());
        assert!(FileNames::new("a.txt", "A.TXT", &["a.txt"]).is_err());
        assert!(FileNames::new("a.txt", "s.txt", &["../a.txt"]).is_err());
    }
}
