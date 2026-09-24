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
