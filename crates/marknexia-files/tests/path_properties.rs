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

/// Pairs that must never compare equal for containment. The first four are
/// look-alikes that NTFS and .NET keep distinct. The rest are real Unicode
/// case pairs, some added after older NTFS `$UpCase` tables were fixed.
/// Rust keeps all of them distinct (a false block, never an escape).
const NEVER_MERGED: [(&str, &str); 9] = [
    ("key", "\u{212A}ey"),      // KELVIN SIGN vs k
    ("\u{3C9}", "\u{2126}"),    // OHM SIGN vs small omega
    ("\u{E5}", "\u{212B}"),     // ANGSTROM SIGN vs a-ring
    ("ix", "\u{130}x"),         // capital I with dot vs i
    ("\u{10D0}", "\u{1C90}"),   // Georgian Mkhedruli vs Mtavruli (Unicode 11)
    ("\u{AB70}", "\u{13A0}"),   // Cherokee small vs capital (Unicode 8)
    ("\u{A7C1}", "\u{A7C0}"),   // Latin Extended-D old Polish o (Unicode 14)
    ("\u{3C3}", "\u{3A3}"),     // Greek sigma
    ("caf\u{E9}", "CAF\u{C9}"), // Latin-1 accented letter
];

#[test]
fn containment_folds_only_ascii_letters() {
    for (root, other) in NEVER_MERGED {
        for (scope_root, candidate) in [(root, other), (other, root)] {
            let scope = RepositoryScope::new(&format!("C:/src/{scope_root}")).unwrap();
            let outside = CanonicalPath::new(&format!("C:/src/{candidate}/secret.md")).unwrap();
            let exact = CanonicalPath::new(&format!("C:/SRC/{scope_root}/secret.md")).unwrap();
            assert!(!scope.contains(&outside), "{scope_root} vs {candidate}");
            assert!(scope.contains(&exact), "{scope_root}");
        }
    }
    for (root, variant) in [("key", "KEY"), ("ix", "IX"), ("Repo", "rEPO")] {
        let scope = RepositoryScope::new(&format!("C:/{root}")).unwrap();
        let inside = CanonicalPath::new(&format!("c:/{variant}/secret.md")).unwrap();
        assert!(scope.contains(&inside), "{variant}");
    }
}

#[test]
fn opened_target_check_folds_only_ascii_letters() {
    for (root, other) in NEVER_MERGED {
        let scope = RepositoryScope::new(&format!("C:/src/{root}")).unwrap();
        let lexical = CanonicalPath::new(&format!("C:/src/{root}/alias.md")).unwrap();
        let final_outside = CanonicalPath::new(&format!("C:/src/{other}/secret.md")).unwrap();
        let final_inside = CanonicalPath::new(&format!("c:/SRC/{root}/actual.md")).unwrap();
        assert!(
            !scope.contains_opened_target(&lexical, &final_outside),
            "{other}"
        );
        assert!(
            !scope.contains_opened_target(&final_outside, &lexical),
            "{other}"
        );
        assert!(
            scope.contains_opened_target(&lexical, &final_inside),
            "{root}"
        );
    }
}
