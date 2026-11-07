//! Parity with the frozen .NET settings fixtures in `compat/fixtures/v1/settings`.
//!
//! The exporter (`tools/Marknexia.ParityExporter`) writes `settingsJson` to
//! a file (or no file when it is `null`), constructs `SettingsService`, then
//! runs the operations. A document that .NET cannot read yields defaults.

use std::path::PathBuf;

use marknexia_core::settings::{AppSettings, parse_settings};
use serde_json::Value;

fn fixture_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat/fixtures/v1/settings")
}

fn run_case(case: &Value) -> AppSettings {
    let input = &case["input"];
    let mut settings = match input["settingsJson"].as_str() {
        None => AppSettings::default(),
        Some(json) => parse_settings(json.as_bytes())
            .map(|parsed| parsed.settings)
            .unwrap_or_default(),
    };
    for operation in input["operations"].as_array().expect("operations array") {
        let path = operation["path"].as_str().expect("operation path");
        match operation["operation"].as_str() {
            Some("addRecentFile") => {
                settings.add_recent_file(path);
            }
            Some("addRecentFolder") => {
                settings.add_recent_folder(path);
            }
            other => panic!("unknown settings operation {other:?}"),
        }
    }
    settings
}

#[test]
fn settings_fixtures_match_the_dotnet_oracle() {
    let mut cases = 0;
    let mut entries: Vec<_> = std::fs::read_dir(fixture_directory())
        .expect("settings fixture directory")
        .map(|entry| entry.expect("fixture entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".case.json"))
        .collect();
    entries.sort();
    for path in entries {
        let case: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("fixture bytes")).expect("JSON");
        assert_eq!(case["area"], "settings", "{}", path.display());
        assert_eq!(case["schemaVersion"], "marknexia-parity-v1");
        let settings = run_case(&case);
        let expected = &case["expected"];
        let name = case["name"].as_str().unwrap_or_default();

        assert_eq!(expected["theme"], settings.theme.name(), "{name}: theme");
        assert_eq!(
            expected["allowRemoteAssets"], settings.allow_remote_assets,
            "{name}: allowRemoteAssets"
        );
        let expected_recent: Vec<&str> = expected["recentFiles"]
            .as_array()
            .expect("recentFiles")
            .iter()
            .map(|item| item.as_str().expect("recent path"))
            .collect();
        assert_eq!(
            settings.recent_files, expected_recent,
            "{name}: recentFiles"
        );
        cases += 1;
    }
    assert!(
        cases >= 2,
        "expected the frozen settings cases, found {cases}"
    );
}
