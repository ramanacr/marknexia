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

use std::borrow::Cow;

use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

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
    let shape = scan_shape(&text);
    if shape.max_depth > MAX_JSON_DEPTH {
        return Err(SettingsParseError::TooDeep);
    }
    if shape.first != Some('{') {
        // Distinguish valid JSON that is not an object from a syntax error,
        // without building the value.
        return Err(match serde_json::from_str::<IgnoredAny>(&text) {
            Ok(_) => SettingsParseError::NotAnObject,
            Err(error) => SettingsParseError::Syntax(error.to_string()),
        });
    }
    let document: RawDocument<'_> = serde_json::from_str(&text)
        .map_err(|error| SettingsParseError::Syntax(error.to_string()))?;
    let mut schema_version = LEGACY_SCHEMA_VERSION;
    for raw in document.occurrences(Field::SchemaVersion) {
        schema_version = serde_json::from_str::<u32>(raw)
            .ok()
            .filter(|version| *version >= LEGACY_SCHEMA_VERSION)
            .ok_or(SettingsParseError::InvalidSchemaVersion)?;
    }
    let strict = schema_version <= CURRENT_SCHEMA_VERSION;
    let settings = read_fields(&document, strict)?.normalized();
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

/// The known properties, in document order of the .NET class.
#[derive(Clone, Copy)]
enum Field {
    SchemaVersion,
    Theme,
    EnforceRepositorySandbox,
    AllowExternalLinks,
    AllowRemoteAssets,
    IsSidebarOpen,
    SidebarMode,
    SidebarWidth,
    RepositoryRoot,
    RecentFiles,
    RecentFolders,
}

const FIELD_NAMES: [&str; 11] = [
    "SchemaVersion",
    "Theme",
    "EnforceRepositorySandbox",
    "AllowExternalLinks",
    "AllowRemoteAssets",
    "IsSidebarOpen",
    "SidebarMode",
    "SidebarWidth",
    "RepositoryRoot",
    "RecentFiles",
    "RecentFolders",
];

/// The top-level object, with the raw text of each known property.
///
/// Unknown properties are skipped with [`IgnoredAny`], so they are validated
/// but never materialized. A repeated property replaces the earlier one
/// (last wins, as `System.Text.Json` deserialization does), but every
/// occurrence is still validated.
struct RawDocument<'a> {
    /// Every occurrence of each known property, in document order.
    values: [Vec<&'a RawValue>; FIELD_NAMES.len()],
}

impl RawDocument<'_> {
    /// Each occurrence of `field`, in order. `System.Text.Json` converts
    /// every occurrence, so an invalid earlier duplicate still fails.
    fn occurrences(&self, field: Field) -> impl Iterator<Item = &str> {
        self.values[field as usize]
            .iter()
            .map(|raw| raw.get().trim())
    }
}

impl<'de> Deserialize<'de> for RawDocument<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct DocumentVisitor;

        impl<'de> Visitor<'de> for DocumentVisitor {
            type Value = RawDocument<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a settings object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values: [Vec<&RawValue>; FIELD_NAMES.len()] = Default::default();
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    match FIELD_NAMES.iter().position(|name| *name == key) {
                        Some(index) => values[index].push(map.next_value::<&'de RawValue>()?),
                        None => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(RawDocument { values })
            }
        }

        deserializer.deserialize_map(DocumentVisitor)
    }
}

fn read_fields(
    document: &RawDocument<'_>,
    strict: bool,
) -> Result<AppSettings, SettingsParseError> {
    let mut settings = AppSettings::default();
    let invalid = |name: Field| {
        if strict {
            Err(SettingsParseError::InvalidField(FIELD_NAMES[name as usize]))
        } else {
            Ok(())
        }
    };

    for raw in document.occurrences(Field::Theme) {
        match int32_literal(raw) {
            Some(theme) => settings.theme = ThemePreference::from_stored(theme.into()),
            None => invalid(Field::Theme)?,
        }
    }
    for (field, target) in [
        (
            Field::EnforceRepositorySandbox,
            &mut settings.enforce_repository_sandbox,
        ),
        (
            Field::AllowExternalLinks,
            &mut settings.allow_external_links,
        ),
        (Field::AllowRemoteAssets, &mut settings.allow_remote_assets),
        (Field::IsSidebarOpen, &mut settings.is_sidebar_open),
    ] {
        for raw in document.occurrences(field) {
            match raw {
                "true" => *target = true,
                "false" => *target = false,
                _ => invalid(field)?,
            }
        }
    }
    for raw in document.occurrences(Field::SidebarMode) {
        match int32_literal(raw) {
            Some(mode) => settings.sidebar_mode = mode,
            None => invalid(Field::SidebarMode)?,
        }
    }
    for raw in document.occurrences(Field::SidebarWidth) {
        match serde_json::from_str::<f64>(raw) {
            Ok(width) => settings.sidebar_width = width,
            Err(_) => invalid(Field::SidebarWidth)?,
        }
    }
    for raw in document.occurrences(Field::RepositoryRoot) {
        match serde_json::from_str::<Option<String>>(raw) {
            Ok(root) => settings.repository_root = root,
            Err(_) => invalid(Field::RepositoryRoot)?,
        }
    }
    for (field, target) in [
        (Field::RecentFiles, &mut settings.recent_files),
        (Field::RecentFolders, &mut settings.recent_folders),
    ] {
        for raw in document.occurrences(field) {
            // `null` (no list) or an array of strings and `null`s. `null`
            // elements are dropped here; .NET drops them in `NormalizePaths`.
            match serde_json::from_str::<Option<Vec<Option<String>>>>(raw) {
                Ok(list) => *target = list.into_iter().flatten().flatten().collect(),
                Err(_) => invalid(field)?,
            }
        }
    }
    Ok(settings)
}

/// A JSON number token that `Utf8JsonReader.TryGetInt32` accepts: an
/// optional `-` and digits only, within `i32`. `-0` is accepted (as 0);
/// fractions and exponents (`0.0`, `1e2`) are rejected.
fn int32_literal(raw: &str) -> Option<i32> {
    let digits = raw.strip_prefix('-').unwrap_or(raw);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

struct Shape {
    /// The first character that is not JSON white space.
    first: Option<char>,
    /// Container nesting depth; a top-level object is depth 1.
    max_depth: usize,
}

/// Measures nesting without building a value. Brackets inside strings do
/// not count. The result is meaningful only for valid JSON; the parser
/// rejects anything else.
fn scan_shape(text: &str) -> Shape {
    let first = text
        .chars()
        .find(|character| !matches!(character, ' ' | '\t' | '\n' | '\r'));
    let mut depth = 0_usize;
    let mut max_depth = 0;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => {
                depth += 1;
                max_depth = max_depth.max(depth);
            }
            b'}' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Shape { first, max_depth }
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
