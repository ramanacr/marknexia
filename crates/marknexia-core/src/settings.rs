//! User settings: model, validation, and the JSON document format.
//!
//! Port of `AppSettings` and the load/normalize rules in
//! `src/Marknexia.Infrastructure/SettingsService.cs`. This module is pure: it
//! parses and produces bytes. `marknexia-files` owns the file, the backup,
//! and the atomic write.
//!
//! # Format
//!
//! The .NET edition writes `%LOCALAPPDATA%\Marknexia\settings.json` with
//! `System.Text.Json` defaults: PascalCase property names matched
//! case-sensitively, `Theme` as an integer (0 System, 1 Light, 2 Dark),
//! unknown properties ignored, duplicate properties last-wins, no comments,
//! no trailing commas, maximum depth 64. That is schema version 1, which has
//! no version property.
//!
//! Schema version 2 is the same object plus `"SchemaVersion": 2`. The .NET
//! edition ignores the new property, so it can still read a file that the
//! Rust edition wrote (rollback). See
//! `compat/decisions/settings-and-startup-differences.md`.

use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::contracts::AppTheme;
use crate::recent::{
    MAX_RECENT_FILES, MAX_RECENT_FOLDERS, RECENT_FILES_DISPLAY_LIMIT, is_acceptable_path,
    normalize_recent, push_recent, visible_recent,
};

/// Schema version of documents without a `SchemaVersion` property (.NET).
pub const LEGACY_SCHEMA_VERSION: u32 = 1;
/// Schema version that this build writes.
pub const CURRENT_SCHEMA_VERSION: u32 = 2;
/// Larger settings documents are rejected as corrupt (SET-4).
pub const MAX_SETTINGS_BYTES: usize = 8 * 1024 * 1024;
/// `System.Text.Json` default `MaxDepth`.
pub const MAX_JSON_DEPTH: usize = 64;

pub const DEFAULT_SIDEBAR_WIDTH: f64 = 280.0;
pub const MIN_SIDEBAR_WIDTH: f64 = 220.0;
pub const MAX_SIDEBAR_WIDTH: f64 = 520.0;
/// Sidebar modes: 0 outline, 1 repository files.
pub const MAX_SIDEBAR_MODE: i32 = 1;

/// The persisted theme choice (.NET `AppTheme`, stored as an integer).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    /// Maps the stored integer. Undefined values become `System`, as .NET
    /// `Normalize` does with `Enum.IsDefined`.
    #[must_use]
    pub fn from_stored(value: i64) -> Self {
        match value {
            1 => Self::Light,
            2 => Self::Dark,
            _ => Self::System,
        }
    }

    #[must_use]
    pub fn stored_value(self) -> i32 {
        match self {
            Self::System => 0,
            Self::Light => 1,
            Self::Dark => 2,
        }
    }

    /// The .NET enum member name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    #[must_use]
    pub fn app_theme(self) -> AppTheme {
        match self {
            Self::System => AppTheme::System,
            Self::Light => AppTheme::Light,
            Self::Dark => AppTheme::Dark,
        }
    }
}

/// User preferences and recent items. Defaults match .NET `AppSettings`.
#[derive(Clone, Debug, PartialEq)]
pub struct AppSettings {
    pub theme: ThemePreference,
    pub enforce_repository_sandbox: bool,
    pub allow_external_links: bool,
    pub allow_remote_assets: bool,
    pub is_sidebar_open: bool,
    pub sidebar_mode: i32,
    pub sidebar_width: f64,
    pub repository_root: Option<String>,
    /// Newest first.
    pub recent_files: Vec<String>,
    /// Newest first.
    pub recent_folders: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            enforce_repository_sandbox: true,
            allow_external_links: true,
            allow_remote_assets: false,
            is_sidebar_open: true,
            sidebar_mode: 0,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            repository_root: None,
            recent_files: Vec::new(),
            recent_folders: Vec::new(),
        }
    }
}

impl AppSettings {
    /// Applies the .NET `Normalize` rules and the Rust validation rules.
    ///
    /// * `repository_root`: trimmed; empty, white-space or invalid becomes
    ///   `None` (invalid: SET-3).
    /// * `sidebar_width`: clamped to 220–520; NaN becomes 280 (SET-7).
    /// * `sidebar_mode`: clamped to 0–1 (SET-7).
    /// * Recent lists: trimmed, validated, de-duplicated and capped.
    pub fn normalize(&mut self) {
        self.repository_root = self
            .repository_root
            .as_deref()
            .map(str::trim)
            .filter(|root| !root.is_empty() && is_acceptable_path(root))
            .map(str::to_owned);
        self.sidebar_width = if self.sidebar_width.is_nan() {
            DEFAULT_SIDEBAR_WIDTH
        } else {
            self.sidebar_width
                .clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
        };
        self.sidebar_mode = self.sidebar_mode.clamp(0, MAX_SIDEBAR_MODE);
        normalize_recent(&mut self.recent_files, MAX_RECENT_FILES);
        normalize_recent(&mut self.recent_folders, MAX_RECENT_FOLDERS);
    }

    #[must_use]
    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }

    /// `SettingsService.AddRecentFile` without the save. Returns `false`
    /// when the path is empty or white space (nothing changes).
    pub fn add_recent_file(&mut self, path: &str) -> bool {
        push_recent(&mut self.recent_files, path, MAX_RECENT_FILES)
    }

    /// `SettingsService.AddRecentFolder` without the save.
    pub fn add_recent_folder(&mut self, path: &str) -> bool {
        push_recent(&mut self.recent_folders, path, MAX_RECENT_FOLDERS)
    }

    /// The recent files that the sidebar shows: the first five that exist.
    pub fn visible_recent_files(&self, exists: impl FnMut(&str) -> bool) -> Vec<&str> {
        visible_recent(&self.recent_files, exists, RECENT_FILES_DISPLAY_LIMIT)
    }
}

/// Why a settings document could not be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsParseError {
    TooLarge {
        bytes: usize,
    },
    Syntax(String),
    /// The document is valid JSON but not an object (`null` included).
    NotAnObject,
    TooDeep,
    /// A known property has a value of the wrong type.
    InvalidField(&'static str),
    InvalidSchemaVersion,
}

impl fmt::Display for SettingsParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge { bytes } => write!(
                formatter,
                "settings file is {bytes} bytes, exceeding the {MAX_SETTINGS_BYTES} byte limit"
            ),
            Self::Syntax(message) => {
                write!(formatter, "settings file is not valid JSON: {message}")
            }
            Self::NotAnObject => formatter.write_str("settings file is not a JSON object"),
            Self::TooDeep => write!(
                formatter,
                "settings file nests deeper than {MAX_JSON_DEPTH} levels"
            ),
            Self::InvalidField(name) => write!(formatter, "settings property {name} is invalid"),
            Self::InvalidSchemaVersion => formatter.write_str("settings SchemaVersion is invalid"),
        }
    }
}

impl std::error::Error for SettingsParseError {}

/// A parsed, normalized settings document.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedSettings {
    pub settings: AppSettings,
    /// [`LEGACY_SCHEMA_VERSION`] when the property is absent.
    pub schema_version: u32,
}

impl ParsedSettings {
    /// A newer build wrote this document. Its known properties were read
    /// leniently; the caller must not overwrite it.
    #[must_use]
    pub fn is_newer_schema(&self) -> bool {
        self.schema_version > CURRENT_SCHEMA_VERSION
    }
}

/// Parses a settings document from file bytes.
///
/// Text decoding follows .NET `File.ReadAllText`: a UTF-8, UTF-16 or UTF-32
/// byte order mark selects the encoding, otherwise UTF-8; invalid sequences
/// become U+FFFD. For schema versions up to [`CURRENT_SCHEMA_VERSION`], a
/// known property with the wrong type rejects the document, as
/// `JsonSerializer` does. For a newer schema such a property keeps its
/// default instead.
///
/// # Errors
///
/// Returns [`SettingsParseError`] for a document that .NET would replace with
/// defaults, and for the Rust-only limits (size, schema version).
pub fn parse_settings(bytes: &[u8]) -> Result<ParsedSettings, SettingsParseError> {
    if bytes.len() > MAX_SETTINGS_BYTES {
        return Err(SettingsParseError::TooLarge { bytes: bytes.len() });
    }
    let text = decode_text(bytes);
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| SettingsParseError::Syntax(error.to_string()))?;
    if depth(&value) > MAX_JSON_DEPTH {
        return Err(SettingsParseError::TooDeep);
    }
    let Value::Object(map) = value else {
        return Err(SettingsParseError::NotAnObject);
    };
    let schema_version = match map.get("SchemaVersion") {
        None => LEGACY_SCHEMA_VERSION,
        Some(version) => version
            .as_u64()
            .and_then(|version| u32::try_from(version).ok())
            .filter(|version| *version >= LEGACY_SCHEMA_VERSION)
            .ok_or(SettingsParseError::InvalidSchemaVersion)?,
    };
    let strict = schema_version <= CURRENT_SCHEMA_VERSION;
    let settings = read_fields(&map, strict)?.normalized();
    Ok(ParsedSettings {
        settings,
        schema_version,
    })
}

/// Serializes settings as an indented schema-version-2 document. The input
/// is normalized first.
#[must_use]
pub fn serialize_settings(settings: &AppSettings) -> Vec<u8> {
    let settings = settings.clone().normalized();
    let document = SettingsDocument {
        schema_version: CURRENT_SCHEMA_VERSION,
        theme: settings.theme.stored_value(),
        enforce_repository_sandbox: settings.enforce_repository_sandbox,
        allow_external_links: settings.allow_external_links,
        allow_remote_assets: settings.allow_remote_assets,
        is_sidebar_open: settings.is_sidebar_open,
        sidebar_mode: settings.sidebar_mode,
        sidebar_width: settings.sidebar_width,
        repository_root: settings.repository_root.as_deref(),
        recent_files: &settings.recent_files,
        recent_folders: &settings.recent_folders,
    };
    serde_json::to_vec_pretty(&document).expect("a settings document always serializes")
}

/// Property order follows the .NET class, with `SchemaVersion` first.
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct SettingsDocument<'a> {
    schema_version: u32,
    theme: i32,
    enforce_repository_sandbox: bool,
    allow_external_links: bool,
    allow_remote_assets: bool,
    is_sidebar_open: bool,
    sidebar_mode: i32,
    sidebar_width: f64,
    repository_root: Option<&'a str>,
    recent_files: &'a [String],
    recent_folders: &'a [String],
}

fn read_fields(map: &Map<String, Value>, strict: bool) -> Result<AppSettings, SettingsParseError> {
    let mut settings = AppSettings::default();
    let invalid = |name: &'static str| {
        if strict {
            Err(SettingsParseError::InvalidField(name))
        } else {
            Ok(())
        }
    };

    if let Some(value) = map.get("Theme") {
        match as_i32(value) {
            Some(theme) => settings.theme = ThemePreference::from_stored(theme.into()),
            None => invalid("Theme")?,
        }
    }
    for (name, target) in [
        (
            "EnforceRepositorySandbox",
            &mut settings.enforce_repository_sandbox,
        ),
        ("AllowExternalLinks", &mut settings.allow_external_links),
        ("AllowRemoteAssets", &mut settings.allow_remote_assets),
        ("IsSidebarOpen", &mut settings.is_sidebar_open),
    ] {
        if let Some(value) = map.get(name) {
            match value {
                Value::Bool(flag) => *target = *flag,
                _ => invalid(name)?,
            }
        }
    }
    if let Some(value) = map.get("SidebarMode") {
        match as_i32(value) {
            Some(mode) => settings.sidebar_mode = mode,
            None => invalid("SidebarMode")?,
        }
    }
    if let Some(value) = map.get("SidebarWidth") {
        match value.as_f64() {
            Some(width) => settings.sidebar_width = width,
            None => invalid("SidebarWidth")?,
        }
    }
    if let Some(value) = map.get("RepositoryRoot") {
        match value {
            Value::Null => settings.repository_root = None,
            Value::String(root) => settings.repository_root = Some(root.clone()),
            _ => invalid("RepositoryRoot")?,
        }
    }
    for (name, target) in [
        ("RecentFiles", &mut settings.recent_files),
        ("RecentFolders", &mut settings.recent_folders),
    ] {
        if let Some(value) = map.get(name) {
            match string_list(value) {
                Some(list) => *target = list,
                None => invalid(name)?,
            }
        }
    }
    Ok(settings)
}

/// A JSON integer literal within `i32`, as `Utf8JsonReader.TryGetInt32`
/// accepts. Fractions and exponents are rejected.
fn as_i32(value: &Value) -> Option<i32> {
    value.as_i64().and_then(|number| i32::try_from(number).ok())
}

/// `null` (no list) or an array of strings and `null`s. `null` elements are
/// dropped here; .NET drops them in `NormalizePaths`.
fn string_list(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Null => Some(Vec::new()),
        Value::Array(items) => {
            let mut list = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(text) => list.push(text.clone()),
                    Value::Null => {}
                    _ => return None,
                }
            }
            Some(list)
        }
        _ => None,
    }
}

/// Container nesting depth; a top-level object is depth 1.
fn depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => 1 + items.iter().map(depth).max().unwrap_or(0),
        Value::Object(map) => 1 + map.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

fn decode_text(bytes: &[u8]) -> String {
    match bytes {
        [0x00, 0x00, 0xFE, 0xFF, rest @ ..] => decode_utf32(rest, u32::from_be_bytes),
        [0xFF, 0xFE, 0x00, 0x00, rest @ ..] => decode_utf32(rest, u32::from_le_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units = bytes.chunks(2).map(|pair| match pair {
        [a, b] => unit([*a, *b]),
        _ => 0xFFFD,
    });
    char::decode_utf16(units)
        .map(|result| result.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

fn decode_utf32(bytes: &[u8], unit: fn([u8; 4]) -> u32) -> String {
    bytes
        .chunks(4)
        .map(|chunk| match chunk {
            [a, b, c, d] => char::from_u32(unit([*a, *b, *c, *d])),
            _ => None,
        })
        .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}
