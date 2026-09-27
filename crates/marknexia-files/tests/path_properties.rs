use marknexia_files::path::{CanonicalPath, RepositoryScope};

#[test]
fn repository_scope_compares_windows_components_case_insensitively() {
    let scope = RepositoryScope::new("C:/Work/Repo").expect("root");
    let inside = CanonicalPath::new("c:\\work\\repo\\Docs\\Readme.md").expect("file");
    let sibling = CanonicalPath::new("C:/Work/Repository/Readme.md").expect("sibling");
    assert!(scope.contains(&inside));
    assert!(!scope.contains(&sibling));
}

#[test]
fn untrusted_network_device_ads_and_escape_paths_are_rejected() {
    for path in [
        "//server/share.md",
        "\\\\server\\share.md",
        "C:/Repo/file.md:stream",
        "\\\\.\\C:\\Repo\\file.md",
        "C:/Repo/../../secret.md",
        "C:/Repo/<bad>.md",
    ] {
        assert!(CanonicalPath::new(path).is_err(), "{path}");
    }
}

#[test]
fn canonicalization_is_stable_for_separator_and_dot_segments() {
    let one = CanonicalPath::new("C:/Repo/docs/./a.md").unwrap();
    let two = CanonicalPath::new("c:\\repo\\docs\\a.md").unwrap();
    assert_eq!(one, two);
    assert_eq!(one.as_str(), "c:/repo/docs/a.md");
}

#[test]
fn reserved_dos_devices_and_trailing_aliases_are_rejected() {
    for path in [
        "C:/repo/CON",
        "C:/repo/NUL.txt",
        "C:/repo/docs/COM¹.md",
        "C:/repo/LPT9/report.md",
        "C:/repo/readme.md.",
        "C:/repo/readme.md ",
    ] {
        assert!(CanonicalPath::new(path).is_err(), "{path}");
    }
    for path in ["C:/repo/.temp", "C:/repo/console.md", "C:/repo/COM10.md"] {
        assert!(CanonicalPath::new(path).is_ok(), "{path}");
    }
}

#[test]
fn opened_file_scope_requires_both_lexical_and_final_handle_paths_inside_root() {
    let scope = RepositoryScope::new("C:/repo").unwrap();
    let lexical_alias = CanonicalPath::new("C:/repo/alias.md").unwrap();
    let final_outside = CanonicalPath::new("C:/private/secret.md").unwrap();
    let final_inside = CanonicalPath::new("c:/REPO/docs/actual.md").unwrap();
    let lexical_outside = CanonicalPath::new("C:/private/alias.md").unwrap();

    assert!(scope.contains(&lexical_alias));
    assert!(!scope.contains_opened_target(&lexical_alias, &final_outside));
    assert!(scope.contains_opened_target(&lexical_alias, &final_inside));
    assert!(!scope.contains_opened_target(&lexical_outside, &final_inside));
}

/// (root component, look-alike that NTFS and .NET OrdinalIgnoreCase keep
/// distinct, true case variant that both treat as equal).
const UNICODE_CASE_TRAPS: [(&str, &str, &str); 4] = [
    ("key", "\u{212A}ey", "KEY"),       // KELVIN SIGN vs k
    ("\u{3C9}", "\u{2126}", "\u{3A9}"), // OHM SIGN vs small omega; capital omega
    ("\u{E5}", "\u{212B}", "\u{C5}"),   // ANGSTROM SIGN vs a-ring; capital A-ring
    ("ix", "\u{130}x", "IX"),           // capital I with dot vs i
];

#[test]
fn containment_does_not_merge_unicode_compatibility_case_look_alikes() {
    for (root, look_alike, variant) in UNICODE_CASE_TRAPS {
        let scope = RepositoryScope::new(&format!("C:/{root}")).unwrap();
        let outside = CanonicalPath::new(&format!("C:/{look_alike}/secret.md")).unwrap();
        let inside = CanonicalPath::new(&format!("C:/{variant}/secret.md")).unwrap();
        assert!(!scope.contains(&outside), "{look_alike}");
        assert!(scope.contains(&inside), "{variant}");
        // A root spelled with the look-alike does not contain the plain name.
        let look_alike_scope = RepositoryScope::new(&format!("C:/{look_alike}")).unwrap();
        let plain = CanonicalPath::new(&format!("C:/{root}/secret.md")).unwrap();
        assert!(!look_alike_scope.contains(&plain), "{look_alike}");
    }
}

#[test]
fn opened_target_check_does_not_merge_unicode_compatibility_case_look_alikes() {
    for (root, look_alike, variant) in UNICODE_CASE_TRAPS {
        let scope = RepositoryScope::new(&format!("C:/{root}")).unwrap();
        let lexical = CanonicalPath::new(&format!("C:/{root}/alias.md")).unwrap();
        let final_outside = CanonicalPath::new(&format!("C:/{look_alike}/secret.md")).unwrap();
        let final_inside = CanonicalPath::new(&format!("C:/{variant}/actual.md")).unwrap();
        assert!(
            !scope.contains_opened_target(&lexical, &final_outside),
            "{look_alike}"
        );
        assert!(!scope.contains_opened_target(&final_outside, &lexical));
        assert!(
            scope.contains_opened_target(&lexical, &final_inside),
            "{variant}"
        );
    }
}

#[test]
fn only_simple_bmp_case_pairs_fold() {
    // Final sigma and small sigma both uppercase to capital sigma but are
    // not a 1:1 pair; dotless i uppercases to ASCII I; Deseret is outside the
    // BMP. All stay distinct, which fails closed.
    for (root, other) in [
        ("\u{3C3}", "\u{3C2}"),
        ("i", "\u{131}"),
        ("\u{10428}", "\u{10400}"),
    ] {
        let scope = RepositoryScope::new(&format!("C:/{root}")).unwrap();
        let candidate = CanonicalPath::new(&format!("C:/{other}/file.md")).unwrap();
        assert!(!scope.contains(&candidate), "{other}");
    }
    // Ordinary accented and Greek/Cyrillic case pairs still fold.
    for (root, other) in [
        ("caf\u{E9}", "CAF\u{C9}"),
        ("\u{3C3}", "\u{3A3}"),
        ("\u{434}", "\u{414}"),
    ] {
        let scope = RepositoryScope::new(&format!("C:/{root}")).unwrap();
        let candidate = CanonicalPath::new(&format!("C:/{other}/file.md")).unwrap();
        assert!(scope.contains(&candidate), "{other}");
    }
}
