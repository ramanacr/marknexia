use marknexia_navigation::{ResolutionContext, VirtualFileSystem, resolve};
use serde_json::Value;

#[test]
fn navigation_v1_matches_frozen_contract() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../compat/fixtures/v1/navigation");
    let mut checked = 0;
    for path in std::fs::read_dir(root).expect("frozen navigation fixtures") {
        let path = path.expect("fixture entry").path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let case: Value = serde_json::from_slice(&std::fs::read(&path).expect("fixture bytes"))
            .expect("fixture JSON");
        let input = &case["input"];
        let vfs = VirtualFileSystem {
            files: case["virtualFileSystem"]["files"]
                .as_array()
                .expect("fixture file list")
                .iter()
                .map(|item| item.as_str().expect("fixture file path").to_owned())
                .collect(),
        };
        let context = ResolutionContext::from_case(input);
        let actual = resolve(input["destination"].as_str(), &context, &vfs);
        let expected = case["expected"].get("intent").unwrap_or(&case["expected"]);
        assert_eq!(
            serde_json::to_value(&actual.intent).unwrap(),
            *expected,
            "{}",
            case["name"]
        );
        if let Some(probes) = case["expected"]["fileSystemProbeCount"].as_u64() {
            assert_eq!(actual.file_system_probe_count, probes, "{}", case["name"]);
        }
        checked += 1;
    }
    assert!(
        checked >= 9,
        "navigation fixture coverage was empty or incomplete"
    );
}
