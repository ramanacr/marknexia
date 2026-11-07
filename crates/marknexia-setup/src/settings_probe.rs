//! A read-only look at the settings file that the .NET edition and the
//! preview share (`%LOCALAPPDATA%\Marknexia\settings.json`).
//!
//! The setup never writes, moves or backs up this file. The shell's
//! `SettingsStore` (`marknexia-files`) does the migration on its first
//! save: it copies a schema-1 file to `settings.json.v1.<id>.bak`, writes
//! atomically and leaves a newer schema read-only. The setup only reports
//! what the shell will find, with the same parser
//! (`marknexia_core::settings::parse_settings`).

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use marknexia_core::settings::{CURRENT_SCHEMA_VERSION, MAX_SETTINGS_BYTES, parse_settings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsProbe {
    Missing,
    /// Readable by the preview. A schema-1 file is backed up by the shell
    /// before its first change.
    Compatible {
        schema_version: u32,
    },
    /// Written by a newer build; the shell reads it but does not change it.
    Newer {
        schema_version: u32,
    },
    /// Not valid settings; the shell will preserve it and use defaults.
    Invalid {
        reason: String,
    },
    Unreadable {
        error: String,
    },
}

impl fmt::Display for SettingsProbe {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => formatter.write_str("no settings file; defaults will be used"),
            Self::Compatible { schema_version } if *schema_version < CURRENT_SCHEMA_VERSION => {
                write!(
                    formatter,
                    "settings schema {schema_version} will be read and backed up before the first change"
                )
            }
            Self::Compatible { schema_version } => {
                write!(formatter, "settings schema {schema_version} will be read")
            }
            Self::Newer { schema_version } => write!(
                formatter,
                "settings schema {schema_version} is newer; it will be read but not changed"
            ),
            Self::Invalid { reason } => write!(
                formatter,
                "settings are not valid ({reason}); the shell will keep the file and use defaults"
            ),
            Self::Unreadable { error } => write!(formatter, "settings could not be read ({error})"),
        }
    }
}

/// Reads at most `MAX_SETTINGS_BYTES + 1` bytes from `path` through a
/// read-only handle and parses them. Never writes.
#[must_use]
pub fn probe(path: &Path) -> SettingsProbe {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return SettingsProbe::Missing,
        Err(error) => {
            return SettingsProbe::Unreadable {
                error: error.to_string(),
            };
        }
    };
    let mut bytes = Vec::new();
    if let Err(error) = file
        .take(MAX_SETTINGS_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
    {
        return SettingsProbe::Unreadable {
            error: error.to_string(),
        };
    }
    match parse_settings(&bytes) {
        Ok(parsed) if parsed.is_newer_schema() => SettingsProbe::Newer {
            schema_version: parsed.schema_version,
        },
        Ok(parsed) => SettingsProbe::Compatible {
            schema_version: parsed.schema_version,
        },
        Err(error) => SettingsProbe::Invalid {
            reason: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn modified(path: &Path) -> std::time::SystemTime {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .expect("metadata")
    }

    #[test]
    fn reads_dotnet_settings_without_changing_them() {
        let dir = TempDir::new("settings-v1");
        let path = dir.path().join("settings.json");
        let original = br#"{"Theme":1,"RecentFiles":["C:\\docs\\a.md"]}"#;
        std::fs::write(&path, original).expect("write");
        let before = modified(&path);
        assert_eq!(
            probe(&path),
            SettingsProbe::Compatible { schema_version: 1 }
        );
        assert_eq!(std::fs::read(&path).expect("read"), original);
        assert_eq!(modified(&path), before);
        let names: Vec<_> = std::fs::read_dir(dir.path()).expect("list").collect();
        assert_eq!(names.len(), 1, "no backup or temporary file is written");
        assert!(
            probe(&path)
                .to_string()
                .contains("backed up before the first change")
        );
    }

    #[test]
    fn reports_missing_newer_and_invalid_files() {
        let dir = TempDir::new("settings-kinds");
        let path = dir.path().join("settings.json");
        assert_eq!(probe(&path), SettingsProbe::Missing);
        std::fs::write(&path, br#"{"SchemaVersion":99}"#).expect("write");
        assert_eq!(probe(&path), SettingsProbe::Newer { schema_version: 99 });
        std::fs::write(&path, b"{ not json").expect("write");
        assert!(matches!(probe(&path), SettingsProbe::Invalid { .. }));
        assert_eq!(
            std::fs::read(&path).expect("read"),
            b"{ not json",
            "an invalid file is not moved"
        );
    }
}
