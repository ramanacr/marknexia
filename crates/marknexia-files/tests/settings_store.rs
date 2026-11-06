//! The settings file on disk: parity with `SettingsServiceTests.cs`, the
//! frozen fixtures, corrupt-file recovery, legacy backup and read-only modes.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use marknexia_core::settings::{
    AppSettings, CURRENT_SCHEMA_VERSION, MAX_SETTINGS_BYTES, ThemePreference, parse_settings,
};
use marknexia_files::settings_store::{
    ReadOnlyReason, SettingsNotice, SettingsSaveError, SettingsStore,
};
use serde_json::Value;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let directory = std::env::temp_dir().join(format!(
            "marknexia-settings-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }

    fn settings_path(&self) -> PathBuf {
        self.0.join("settings.json")
    }

    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn schema_on_disk(path: &Path) -> u32 {
    parse_settings(&fs::read(path).unwrap())
        .unwrap()
        .schema_version
}

#[test]
fn settings_round_trip_preserves_theme_and_recent_items() {
    let scratch = Scratch::new("round-trip");
    let path = scratch.settings_path();
    let (mut store, notice) = SettingsStore::open(&path);
    assert_eq!(notice, None);
    store
        .update(|settings| {
            settings.theme = ThemePreference::Dark;
            settings.is_sidebar_open = false;
            settings.sidebar_width = 360.0;
            settings.sidebar_mode = 1;
            settings.allow_remote_assets = true;
            settings.repository_root = Some("C:\\workspace".into());
        })
        .unwrap();
    store.add_recent_file("  C:\\docs\\README.md  ").unwrap();
    store.add_recent_folder("C:\\workspace").unwrap();

    let (reloaded, notice) = SettingsStore::open(&path);
    assert_eq!(notice, None);
    let current = reloaded.current();
    assert_eq!(current.theme, ThemePreference::Dark);
    assert!(!current.is_sidebar_open);
    assert_eq!(current.sidebar_width, 360.0);
    assert_eq!(current.sidebar_mode, 1);
    assert!(current.allow_remote_assets);
    assert_eq!(current.repository_root.as_deref(), Some("C:\\workspace"));
    assert_eq!(current.recent_files, ["C:\\docs\\README.md"]);
    assert_eq!(current.recent_folders, ["C:\\workspace"]);
    assert_eq!(schema_on_disk(&path), CURRENT_SCHEMA_VERSION);
    // A file that did not exist has nothing to back up.
    assert_eq!(scratch.names(), ["settings.json"]);
}

#[test]
fn settings_default_to_blocking_remote_assets_and_write_nothing_on_open() {
    let scratch = Scratch::new("defaults");
    let (store, notice) = SettingsStore::open(scratch.settings_path());
    assert_eq!(notice, None);
    assert!(!store.current().allow_remote_assets);
    assert_eq!(store.current(), &AppSettings::default());
    assert!(scratch.names().is_empty());
}

#[test]
fn white_space_recent_path_is_not_saved() {
    let scratch = Scratch::new("white-space");
    let (mut store, _) = SettingsStore::open(scratch.settings_path());
    store.add_recent_file("   ").unwrap();
    assert!(scratch.names().is_empty());
}

#[test]
fn corrupt_settings_are_preserved_and_reported() {
    let scratch = Scratch::new("corrupt");
    let path = scratch.settings_path();
    fs::write(&path, "{ not valid json").unwrap();

    let (mut store, notice) = SettingsStore::open(&path);

    let Some(SettingsNotice::Corrupt {
        reason,
        preserved_as,
    }) = notice
    else {
        panic!("expected a corrupt notice, got {notice:?}");
    };
    assert!(reason.contains("not valid JSON"), "{reason}");
    assert_eq!(fs::read(&preserved_as).unwrap(), b"{ not valid json");
    assert!(
        preserved_as
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("settings.json.corrupt-")
    );
    assert!(!path.exists());
    assert_eq!(store.current(), &AppSettings::default());
    assert_eq!(store.read_only_reason(), None);

    store.add_recent_file("C:\\docs\\a.md").unwrap();
    assert_eq!(fs::read(&preserved_as).unwrap(), b"{ not valid json");
    assert_eq!(schema_on_disk(&path), CURRENT_SCHEMA_VERSION);
    assert_eq!(scratch.names().len(), 2);
}

#[test]
fn oversized_settings_are_treated_as_corrupt() {
    let scratch = Scratch::new("oversized");
    let path = scratch.settings_path();
    fs::write(&path, vec![b' '; MAX_SETTINGS_BYTES + 1]).unwrap();
    let (store, notice) = SettingsStore::open(&path);
    assert!(
        matches!(&notice, Some(SettingsNotice::Corrupt { reason, .. }) if reason.contains("limit")),
        "{notice:?}"
    );
    assert_eq!(store.current(), &AppSettings::default());
}

#[test]
fn legacy_file_is_backed_up_once_before_the_first_write() {
    let scratch = Scratch::new("legacy");
    let path = scratch.settings_path();
    let legacy = "{\r\n  \"Theme\": 1,\r\n  \"RecentFiles\": [\"C:\\\\old.md\"],\r\n  \"FutureDotNetOnly\": true\r\n}";
    fs::write(&path, legacy).unwrap();

    let (mut store, notice) = SettingsStore::open(&path);
    assert_eq!(notice, None);
    assert_eq!(store.current().theme, ThemePreference::Light);
    // Loading alone does not migrate: the .NET edition can still use it.
    assert_eq!(scratch.names(), ["settings.json"]);
    assert_eq!(fs::read_to_string(&path).unwrap(), legacy);

    store.add_recent_file("C:\\new.md").unwrap();
    let backup = store.legacy_backup_path();
    assert_eq!(fs::read_to_string(&backup).unwrap(), legacy);
    assert_eq!(schema_on_disk(&path), CURRENT_SCHEMA_VERSION);
    assert_eq!(store.current().recent_files, ["C:\\new.md", "C:\\old.md"]);

    // Later writes keep the original backup.
    store.add_recent_file("C:\\newer.md").unwrap();
    assert_eq!(fs::read_to_string(&backup).unwrap(), legacy);
    assert_eq!(scratch.names(), ["settings.json", "settings.json.v1.bak"]);
}

#[test]
fn file_rewritten_by_dotnet_is_backed_up_again() {
    let scratch = Scratch::new("rollback");
    let path = scratch.settings_path();
    let (mut store, _) = SettingsStore::open(&path);
    store.add_recent_file("C:\\a.md").unwrap();

    // The .NET edition drops SchemaVersion when it saves.
    let dotnet = "{\"Theme\": 2, \"RecentFiles\": [\"C:\\\\a.md\"]}";
    fs::write(&path, dotnet).unwrap();
    let (mut store, notice) = SettingsStore::open(&path);
    assert_eq!(notice, None);
    assert_eq!(store.current().theme, ThemePreference::Dark);
    store.add_recent_file("C:\\b.md").unwrap();
    assert_eq!(
        fs::read_to_string(store.legacy_backup_path()).unwrap(),
        dotnet
    );
}

#[test]
fn newer_schema_is_read_but_never_overwritten() {
    let scratch = Scratch::new("newer");
    let path = scratch.settings_path();
    let newer = r#"{"SchemaVersion": 3, "Theme": 1, "AllowRemoteAssets": true}"#;
    fs::write(&path, newer).unwrap();

    let (mut store, notice) = SettingsStore::open(&path);
    assert_eq!(notice, Some(SettingsNotice::NewerSchema { found: 3 }));
    assert_eq!(store.current().theme, ThemePreference::Light);
    assert!(store.current().allow_remote_assets);

    let result = store.add_recent_file("C:\\a.md");
    assert!(matches!(
        result,
        Err(SettingsSaveError::ReadOnly(ReadOnlyReason::NewerSchema {
            found: 3
        }))
    ));
    // Memory changes as in .NET; the file does not.
    assert_eq!(store.current().recent_files, ["C:\\a.md"]);
    assert_eq!(fs::read_to_string(&path).unwrap(), newer);
}

#[test]
fn newer_file_written_after_load_stops_the_migration() {
    let scratch = Scratch::new("race");
    let path = scratch.settings_path();
    fs::write(&path, "{}").unwrap();
    let (mut store, _) = SettingsStore::open(&path);
    let newer = r#"{"SchemaVersion": 3}"#;
    fs::write(&path, newer).unwrap();
    assert!(matches!(
        store.add_recent_file("C:\\a.md"),
        Err(SettingsSaveError::ReadOnly(ReadOnlyReason::NewerSchema {
            found: 3
        }))
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), newer);
    assert!(!store.legacy_backup_path().exists());
}

#[test]
fn unreadable_settings_make_the_store_read_only() {
    let scratch = Scratch::new("unreadable");
    let path = scratch.settings_path();
    fs::create_dir(&path).unwrap();

    let (mut store, notice) = SettingsStore::open(&path);
    assert!(
        matches!(notice, Some(SettingsNotice::Unreadable { .. })),
        "{notice:?}"
    );
    assert_eq!(store.read_only_reason(), Some(&ReadOnlyReason::Unreadable));
    assert!(matches!(
        store.save(AppSettings::default()),
        Err(SettingsSaveError::ReadOnly(ReadOnlyReason::Unreadable))
    ));
    assert!(path.is_dir());
}

#[test]
fn reload_recovers_after_the_file_is_fixed() {
    let scratch = Scratch::new("reload");
    let path = scratch.settings_path();
    fs::create_dir(&path).unwrap();
    let (mut store, _) = SettingsStore::open(&path);
    fs::remove_dir(&path).unwrap();
    fs::write(&path, r#"{"Theme": 2}"#).unwrap();
    assert_eq!(store.reload(), None);
    assert_eq!(store.read_only_reason(), None);
    assert_eq!(store.current().theme, ThemePreference::Dark);
}

#[test]
fn settings_fixtures_match_the_dotnet_oracle_on_disk() {
    let directory =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat/fixtures/v1/settings");
    let mut cases = 0;
    for entry in fs::read_dir(directory).unwrap() {
        let fixture = entry.unwrap().path();
        if !fixture.to_string_lossy().ends_with(".case.json") {
            continue;
        }
        let case: Value = serde_json::from_slice(&fs::read(&fixture).unwrap()).unwrap();
        let name = case["name"].as_str().unwrap().to_owned();
        let scratch = Scratch::new(&name);
        let path = scratch.settings_path();
        if let Some(json) = case["input"]["settingsJson"].as_str() {
            fs::write(&path, json).unwrap();
        }
        let (mut store, _) = SettingsStore::open(&path);
        for operation in case["input"]["operations"].as_array().unwrap() {
            assert_eq!(operation["operation"], "addRecentFile", "{name}");
            store
                .add_recent_file(operation["path"].as_str().unwrap())
                .unwrap();
        }
        let (reloaded, _) = SettingsStore::open(&path);
        for current in [store.current(), reloaded.current()] {
            let expected = &case["expected"];
            assert_eq!(expected["theme"], current.theme.name(), "{name}");
            assert_eq!(
                expected["allowRemoteAssets"], current.allow_remote_assets,
                "{name}"
            );
            let recent: Vec<&str> = expected["recentFiles"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item.as_str().unwrap())
                .collect();
            assert_eq!(current.recent_files, recent, "{name}");
        }
        cases += 1;
    }
    assert!(cases >= 2);
}
