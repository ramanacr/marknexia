//! Settings document parsing, normalization and serialization.

use marknexia_core::settings::{
    AppSettings, CURRENT_SCHEMA_VERSION, LEGACY_SCHEMA_VERSION, MAX_SETTINGS_BYTES,
    SettingsParseError, ThemePreference, parse_settings, serialize_settings,
};

fn parse(text: &str) -> Result<AppSettings, SettingsParseError> {
    parse_settings(text.as_bytes()).map(|parsed| parsed.settings)
}

/// The exact shape .NET `JsonSerializer` writes with `WriteIndented`.
const DOTNET_DOCUMENT: &str = r#"{
  "Theme": 2,
  "EnforceRepositorySandbox": false,
  "AllowExternalLinks": false,
  "AllowRemoteAssets": true,
  "IsSidebarOpen": false,
  "SidebarMode": 1,
  "SidebarWidth": 360,
  "RepositoryRoot": "C:\\workspace",
  "RecentFiles": [
    "C:\\docs\\README.md"
  ],
  "RecentFolders": [
    "C:\\workspace"
  ]
}"#;

#[test]
fn reads_the_dotnet_document_as_schema_version_1() {
    let parsed = parse_settings(DOTNET_DOCUMENT.as_bytes()).unwrap();
    assert_eq!(parsed.schema_version, LEGACY_SCHEMA_VERSION);
    assert!(!parsed.is_newer_schema());
    assert_eq!(
        parsed.settings,
        AppSettings {
            theme: ThemePreference::Dark,
            enforce_repository_sandbox: false,
            allow_external_links: false,
            allow_remote_assets: true,
            is_sidebar_open: false,
            sidebar_mode: 1,
            sidebar_width: 360.0,
            repository_root: Some("C:\\workspace".into()),
            recent_files: vec!["C:\\docs\\README.md".into()],
            recent_folders: vec!["C:\\workspace".into()],
        }
    );
}

#[test]
fn defaults_match_dotnet() {
    let defaults = AppSettings::default();
    assert_eq!(defaults.theme, ThemePreference::System);
    assert!(defaults.enforce_repository_sandbox);
    assert!(defaults.allow_external_links);
    assert!(!defaults.allow_remote_assets);
    assert!(defaults.is_sidebar_open);
    assert_eq!(defaults.sidebar_mode, 0);
    assert_eq!(defaults.sidebar_width, 280.0);
    assert_eq!(defaults.repository_root, None);
    assert!(defaults.recent_files.is_empty());
    assert!(defaults.recent_folders.is_empty());
    assert_eq!(parse("{}").unwrap(), defaults);
}

#[test]
fn written_document_is_dotnet_readable_schema_version_2() {
    let settings = parse(DOTNET_DOCUMENT).unwrap();
    let bytes = serialize_settings(&settings);
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.starts_with("{\n  \"SchemaVersion\": 2,\n  \"Theme\": 2,"));
    let parsed = parse_settings(&bytes).unwrap();
    assert_eq!(parsed.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(parsed.settings, settings);
}

#[test]
fn property_names_are_case_sensitive_and_unknown_properties_are_ignored() {
    let settings =
        parse(r#"{"theme": 2, "allowRemoteAssets": true, "Extra": {"a": [1]}}"#).unwrap();
    assert_eq!(settings, AppSettings::default());
}

#[test]
fn duplicate_properties_take_the_last_value() {
    assert_eq!(
        parse(r#"{"Theme": 1, "Theme": 2}"#).unwrap().theme,
        ThemePreference::Dark
    );
}

#[test]
fn undefined_theme_values_become_system() {
    for theme in ["3", "-1", "2147483647"] {
        assert_eq!(
            parse(&format!(r#"{{"Theme": {theme}}}"#)).unwrap().theme,
            ThemePreference::System,
            "{theme}"
        );
    }
}

#[test]
fn wrongly_typed_known_properties_reject_the_document() {
    for (document, field) in [
        (r#"{"Theme": "Dark"}"#, "Theme"),
        (r#"{"Theme": 1.0}"#, "Theme"),
        (r#"{"Theme": 2147483648}"#, "Theme"),
        (r#"{"Theme": null}"#, "Theme"),
        (r#"{"AllowRemoteAssets": 1}"#, "AllowRemoteAssets"),
        (r#"{"IsSidebarOpen": null}"#, "IsSidebarOpen"),
        (r#"{"SidebarMode": 1.5}"#, "SidebarMode"),
        (r#"{"SidebarWidth": "300"}"#, "SidebarWidth"),
        (r#"{"RepositoryRoot": 5}"#, "RepositoryRoot"),
        (r#"{"RecentFiles": "C:\\a.md"}"#, "RecentFiles"),
        (r#"{"RecentFolders": [1]}"#, "RecentFolders"),
    ] {
        assert_eq!(
            parse(document),
            Err(SettingsParseError::InvalidField(field)),
            "{document}"
        );
    }
}

#[test]
fn malformed_or_non_object_documents_are_rejected() {
    for document in [
        "{ not valid json",
        "",
        "{\"Theme\": 1,}",
        "// comment\n{}",
        "[1]",
        "null",
        "5",
    ] {
        assert!(parse(document).is_err(), "{document:?}");
    }
    assert_eq!(parse("null"), Err(SettingsParseError::NotAnObject));
}

#[test]
fn nesting_beyond_depth_64_is_rejected() {
    let nested = |levels: usize| {
        format!(
            "{{\"Extra\": {}{}}}",
            "[".repeat(levels - 1),
            "]".repeat(levels - 1)
        )
    };
    assert!(parse(&nested(64)).is_ok());
    assert_eq!(parse(&nested(65)), Err(SettingsParseError::TooDeep));
}

#[test]
fn oversized_documents_are_rejected() {
    let mut document = b"{\"Extra\": \"".to_vec();
    document.resize(MAX_SETTINGS_BYTES, b'a');
    document.extend_from_slice(b"\"}");
    assert!(matches!(
        parse_settings(&document),
        Err(SettingsParseError::TooLarge { .. })
    ));
}

#[test]
fn byte_order_marks_select_the_encoding() {
    let document = r#"{"Theme": 1, "RepositoryRoot": "C:\\café"}"#;
    let mut utf8 = vec![0xEF, 0xBB, 0xBF];
    utf8.extend_from_slice(document.as_bytes());
    let mut utf16le = vec![0xFF, 0xFE];
    utf16le.extend(document.encode_utf16().flat_map(u16::to_le_bytes));
    let mut utf16be = vec![0xFE, 0xFF];
    utf16be.extend(document.encode_utf16().flat_map(u16::to_be_bytes));
    let mut utf32le = vec![0xFF, 0xFE, 0x00, 0x00];
    utf32le.extend(document.chars().flat_map(|c| u32::from(c).to_le_bytes()));
    let mut utf32be = vec![0x00, 0x00, 0xFE, 0xFF];
    utf32be.extend(document.chars().flat_map(|c| u32::from(c).to_be_bytes()));
    for bytes in [utf8, utf16le, utf16be, utf32le, utf32be] {
        let settings = parse_settings(&bytes).unwrap().settings;
        assert_eq!(settings.theme, ThemePreference::Light);
        assert_eq!(settings.repository_root.as_deref(), Some("C:\\café"));
    }
}

#[test]
fn invalid_utf8_is_replaced_not_rejected() {
    let bytes = b"{\"RecentFiles\": [\"C:\\\\a\xFF.md\"]}";
    let settings = parse_settings(bytes).unwrap().settings;
    assert_eq!(settings.recent_files, ["C:\\a\u{FFFD}.md"]);
}

#[test]
fn normalization_matches_dotnet_rules() {
    let settings = parse(
        r#"{
            "SidebarWidth": 9000,
            "SidebarMode": 7,
            "RepositoryRoot": "   ",
            "RecentFiles": [null, " ", "  C:\\a.md ", "c:\\A.MD", "C:\\b.md"],
            "RecentFolders": null
        }"#,
    )
    .unwrap();
    assert_eq!(settings.sidebar_width, 520.0);
    assert_eq!(settings.sidebar_mode, 1);
    assert_eq!(settings.repository_root, None);
    assert_eq!(settings.recent_files, ["C:\\a.md", "C:\\b.md"]);
    assert!(settings.recent_folders.is_empty());

    let settings = parse(r#"{"SidebarWidth": -5, "SidebarMode": -3}"#).unwrap();
    assert_eq!(settings.sidebar_width, 220.0);
    assert_eq!(settings.sidebar_mode, 0);

    assert_eq!(
        parse(r#"{"RepositoryRoot": "  C:\\repo  "}"#)
            .unwrap()
            .repository_root
            .as_deref(),
        Some("C:\\repo")
    );
}

#[test]
fn stored_lists_are_capped_after_de_duplication() {
    let files: Vec<String> = (0..20).map(|i| format!("\"C:\\\\{i}.md\"")).collect();
    let folders: Vec<String> = (0..12).map(|i| format!("\"C:\\\\f{i}\"")).collect();
    let settings = parse(&format!(
        r#"{{"RecentFiles": ["C:\\0.md", {}], "RecentFolders": [{}]}}"#,
        files.join(","),
        folders.join(",")
    ))
    .unwrap();
    assert_eq!(settings.recent_files.len(), 15);
    assert_eq!(settings.recent_files[0], "C:\\0.md");
    assert_eq!(settings.recent_files[14], "C:\\14.md");
    assert_eq!(settings.recent_folders.len(), 10);
}

#[test]
fn invalid_paths_are_dropped() {
    let settings = parse(
        r#"{
            "RepositoryRoot": "relative\\repo",
            "RecentFiles": ["notes.md", "\\\\?\\C:\\a.md", "C:\\ok.md", "\\\\server\\share\\b.md"]
        }"#,
    )
    .unwrap();
    assert_eq!(settings.repository_root, None);
    assert_eq!(
        settings.recent_files,
        ["C:\\ok.md", "\\\\server\\share\\b.md"]
    );
}

#[test]
fn nan_width_is_normalized_before_writing() {
    let settings = AppSettings {
        sidebar_width: f64::NAN,
        ..AppSettings::default()
    };
    let parsed = parse_settings(&serialize_settings(&settings)).unwrap();
    assert_eq!(parsed.settings.sidebar_width, 280.0);
    let settings = AppSettings {
        sidebar_width: f64::INFINITY,
        ..AppSettings::default()
    };
    let parsed = parse_settings(&serialize_settings(&settings)).unwrap();
    assert_eq!(parsed.settings.sidebar_width, 520.0);
}

#[test]
fn schema_version_is_validated() {
    for version in ["0", "-1", "1.5", "\"2\"", "null", "4294967296"] {
        assert_eq!(
            parse(&format!(r#"{{"SchemaVersion": {version}}}"#)),
            Err(SettingsParseError::InvalidSchemaVersion),
            "{version}"
        );
    }
    assert_eq!(
        parse_settings(br#"{"SchemaVersion": 1}"#)
            .unwrap()
            .schema_version,
        1
    );
}

#[test]
fn newer_schema_is_read_leniently() {
    let parsed = parse_settings(
        br#"{"SchemaVersion": 9, "Theme": "Dark", "AllowRemoteAssets": true, "SidebarWidth": {"px": 300}}"#,
    )
    .unwrap();
    assert!(parsed.is_newer_schema());
    assert_eq!(parsed.schema_version, 9);
    assert_eq!(parsed.settings.theme, ThemePreference::System);
    assert!(parsed.settings.allow_remote_assets);
    assert_eq!(parsed.settings.sidebar_width, 280.0);
}

#[test]
fn recent_operations_match_dotnet_service() {
    let mut settings = AppSettings::default();
    assert!(settings.add_recent_file("  C:\\docs\\README.md  "));
    assert!(settings.add_recent_folder("C:\\workspace"));
    assert!(!settings.add_recent_file("   "));
    assert_eq!(settings.recent_files, ["C:\\docs\\README.md"]);
    assert_eq!(settings.recent_folders, ["C:\\workspace"]);

    for i in 0..12 {
        settings.add_recent_folder(&format!("C:\\f{i}"));
    }
    assert_eq!(settings.recent_folders.len(), 10);
    assert_eq!(settings.recent_folders[0], "C:\\f11");
}

#[test]
fn visible_recent_files_skip_missing_files() {
    let mut settings = AppSettings::default();
    for i in 0..7 {
        settings.add_recent_file(&format!("C:\\{i}.md"));
    }
    let shown = settings.visible_recent_files(|path| path != "C:\\5.md");
    assert_eq!(
        shown,
        ["C:\\6.md", "C:\\4.md", "C:\\3.md", "C:\\2.md", "C:\\1.md"]
    );
    // Missing files stay stored.
    assert_eq!(settings.recent_files.len(), 7);
}
