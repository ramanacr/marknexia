#![no_main]

use libfuzzer_sys::fuzz_target;
use marknexia_core::settings::{CURRENT_SCHEMA_VERSION, parse_settings, serialize_settings};

fuzz_target!(|bytes: &[u8]| {
    let Ok(parsed) = parse_settings(bytes) else {
        return;
    };
    // Parsed settings are normalized, so they must survive their own round
    // trip unchanged and serialize to a stable document.
    let written = serialize_settings(&parsed.settings);
    let again = parse_settings(&written).expect("written settings parse");
    assert_eq!(again.schema_version, CURRENT_SCHEMA_VERSION);
    assert_eq!(again.settings, parsed.settings);
    assert_eq!(serialize_settings(&again.settings), written);
});
