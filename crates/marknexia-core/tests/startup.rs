//! Branch coverage for the `StartupFileResolver` port, with an in-memory probe.
//! `marknexia-files` repeats the .NET cases against real temporary files.

use std::cell::RefCell;

use marknexia_core::startup::{
    StartupFileProbe, file_uri_to_local_path, find_first_existing, has_markdown_extension,
    split_command_line,
};

/// Paths are absolute when they start with a drive or `\\`; others resolve
/// against `C:\work`. Records every `is_file` probe.
struct MemoryProbe {
    files: Vec<String>,
    probes: RefCell<Vec<String>>,
}

impl MemoryProbe {
    fn new(files: &[&str]) -> Self {
        Self {
            files: files.iter().map(|file| (*file).to_owned()).collect(),
            probes: RefCell::new(Vec::new()),
        }
    }
}

impl StartupFileProbe for MemoryProbe {
    fn full_path(&self, path: &str) -> Option<String> {
        if path.is_empty() || path.contains('\0') {
            return None;
        }
        let path = path.replace('/', "\\");
        let bytes = path.as_bytes();
        if path.starts_with("\\\\") || (bytes.len() >= 2 && bytes[1] == b':') {
            Some(path)
        } else {
            Some(format!("C:\\work\\{path}"))
        }
    }

    fn is_file(&self, full_path: &str) -> bool {
        self.probes.borrow_mut().push(full_path.to_owned());
        self.files
            .iter()
            .any(|file| file.eq_ignore_ascii_case(full_path))
    }
}

fn find(candidates: &[Option<&str>], probe: &MemoryProbe) -> Option<String> {
    find_first_existing(candidates.iter().copied(), probe)
}

#[test]
fn skips_null_empty_and_white_space_candidates_without_probing() {
    let probe = MemoryProbe::new(&[]);
    assert_eq!(find(&[None, Some(""), Some("  \t ")], &probe), None);
    assert!(probe.probes.borrow().is_empty());
}

#[test]
fn returns_none_when_no_candidate_is_a_file() {
    let probe = MemoryProbe::new(&[]);
    assert_eq!(find(&[None, Some(""), Some("missing.md")], &probe), None);
}

#[test]
fn returns_the_first_valid_candidate_in_activation_order() {
    let probe = MemoryProbe::new(&["C:\\docs\\a.md", "C:\\docs\\b.md"]);
    assert_eq!(
        find(
            &[
                None,
                Some(""),
                Some("missing.md"),
                Some("C:\\docs\\b.md"),
                Some("C:\\docs\\a.md")
            ],
            &probe
        ),
        Some("C:\\docs\\b.md".into())
    );
}

#[test]
fn relative_candidates_resolve_to_full_paths() {
    let probe = MemoryProbe::new(&["C:\\work\\notes.md"]);
    assert_eq!(
        find(&[Some("notes.md")], &probe),
        Some("C:\\work\\notes.md".into())
    );
}

#[test]
fn direct_candidate_with_spaces_is_tried_before_splitting() {
    let probe = MemoryProbe::new(&["C:\\my docs\\read me.md"]);
    assert_eq!(
        find(&[Some("  C:\\my docs\\read me.md  ")], &probe),
        Some("C:\\my docs\\read me.md".into())
    );
    assert_eq!(probe.probes.borrow().len(), 1);
}

#[test]
fn direct_candidate_strips_surrounding_quotes() {
    let probe = MemoryProbe::new(&["C:\\my docs\\a.md"]);
    assert_eq!(
        find(&[Some(" \"\"C:\\my docs\\a.md\" ")], &probe),
        Some("C:\\my docs\\a.md".into())
    );
}

#[test]
fn extracts_document_from_activation_command_lines() {
    let existing = "C:\\Temp\\marknexia activation.md";
    for command_line in [
        "--open \"C:\\Temp\\marknexia activation.md\"",
        "\"C:\\Program Files\\Marknexia\\Marknexia.App.exe\" \"C:\\Temp\\marknexia activation.md\"",
        "\"C:\\Temp\\marknexia activation.md\" \"missing second file.md\"",
    ] {
        let probe = MemoryProbe::new(&[existing]);
        assert_eq!(
            find(&[Some(command_line)], &probe),
            Some(existing.into()),
            "{command_line}"
        );
    }
}

#[test]
fn skips_existing_non_markdown_files_such_as_the_executable() {
    let probe = MemoryProbe::new(&["C:\\app\\Marknexia.exe", "C:\\docs\\a.md"]);
    assert_eq!(
        find(
            &[Some("C:\\app\\Marknexia.exe"), Some("C:\\docs\\a.md")],
            &probe
        ),
        Some("C:\\docs\\a.md".into())
    );
    assert!(
        !probe
            .probes
            .borrow()
            .iter()
            .any(|path| path.ends_with(".exe")),
        "non-Markdown paths are rejected before the file-system probe"
    );
}

#[test]
fn accepts_every_markdown_extension_in_any_case() {
    for name in ["a.md", "a.MARKDOWN", "a.mdown", "a.MkDn"] {
        let full = format!("C:\\docs\\{name}");
        let probe = MemoryProbe::new(&[&full]);
        assert_eq!(find(&[Some(&full)], &probe), Some(full.clone()));
    }
    let probe = MemoryProbe::new(&["C:\\docs\\a.txt"]);
    assert_eq!(find(&[Some("C:\\docs\\a.txt")], &probe), None);
}

#[test]
fn extension_follows_dotnet_get_extension() {
    assert!(has_markdown_extension("C:\\a.b\\c.md"));
    assert!(!has_markdown_extension("C:\\a.md\\c"));
    assert!(!has_markdown_extension("C:\\docs\\a.md."));
    assert!(!has_markdown_extension("C:\\docs\\a.md "));
    assert!(has_markdown_extension(".md"));
    assert!(!has_markdown_extension("md"));
}

#[test]
fn accepts_file_uris() {
    let existing = "C:\\Temp\\marknexia #activation.md";
    let probe = MemoryProbe::new(&[existing]);
    assert_eq!(
        find(
            &[
                Some("C:\\Program Files\\Marknexia\\Marknexia.exe"),
                Some("file:///C:/Temp/marknexia%20%23activation.md")
            ],
            &probe
        ),
        Some(existing.into())
    );
}

#[test]
fn full_path_failure_rejects_the_candidate() {
    let probe = MemoryProbe::new(&[]);
    assert_eq!(find(&[Some("bad\0name.md")], &probe), None);
    assert!(probe.probes.borrow().is_empty());
}

#[test]
fn file_uri_conversion_covers_drive_localhost_and_unc_forms() {
    assert_eq!(
        file_uri_to_local_path("file:///C:/a%20b/c.md").as_deref(),
        Some("C:\\a b\\c.md")
    );
    assert_eq!(
        file_uri_to_local_path("FILE://localhost/D:/x.md").as_deref(),
        Some("D:\\x.md")
    );
    assert_eq!(
        file_uri_to_local_path("file://C:/x.md").as_deref(),
        Some("C:\\x.md")
    );
    assert_eq!(
        file_uri_to_local_path("file:///C|/x.md").as_deref(),
        Some("C:\\x.md")
    );
    assert_eq!(
        file_uri_to_local_path("file://server/share/x.md#top").as_deref(),
        Some("\\\\server\\share\\x.md")
    );
    assert_eq!(
        file_uri_to_local_path("file:///C:/caf%C3%A9.md").as_deref(),
        Some("C:\\café.md")
    );
    assert_eq!(file_uri_to_local_path("file:///etc/x.md"), None);
    assert_eq!(file_uri_to_local_path("file:///C:/%FF.md"), None);
    assert_eq!(file_uri_to_local_path("http://x/a.md"), None);
}

#[test]
fn invalid_file_uri_is_not_probed() {
    let probe = MemoryProbe::new(&[]);
    assert_eq!(find(&[Some("file:///etc/readme.md")], &probe), None);
    assert!(probe.probes.borrow().is_empty());
}

#[test]
fn command_line_splitting_follows_windows_argv_rules() {
    assert_eq!(split_command_line("a b\t c"), ["a", "b", "c"]);
    assert_eq!(split_command_line("\"a b\" c\"d e\"f"), ["a b", "cd ef"]);
    // Backslashes are literal unless they precede a quote.
    assert_eq!(split_command_line("C:\\dir\\a.md"), ["C:\\dir\\a.md"]);
    // 2n backslashes + quote: n backslashes, the quote toggles quoting.
    assert_eq!(split_command_line("\"C:\\dir\\\\\" x"), ["C:\\dir\\", "x"]);
    // 2n+1 backslashes + quote: n backslashes and a literal quote.
    assert_eq!(split_command_line("a\\\"b"), ["a\"b"]);
    assert_eq!(split_command_line("a\\\\\\\"b"), ["a\\\"b"]);
    // Empty arguments are dropped.
    assert_eq!(split_command_line("\"\" a"), ["a"]);
    // An unterminated quote drops the final argument.
    assert_eq!(split_command_line("a \"b c"), ["a"]);
    assert!(split_command_line("   ").is_empty());
}

#[test]
fn quoted_path_with_trailing_backslash_does_not_swallow_the_next_argument() {
    let probe = MemoryProbe::new(&["C:\\work\\x.md"]);
    assert_eq!(
        find(&[Some("\"C:\\dir\\\\\" x.md")], &probe),
        Some("C:\\work\\x.md".into())
    );
}

#[test]
fn file_uri_query_and_fragment_are_removed_but_encoded_ones_stay() {
    assert_eq!(
        file_uri_to_local_path("file:///C:/docs/a.md?x=1#top").as_deref(),
        Some("C:\\docs\\a.md")
    );
    assert_eq!(
        file_uri_to_local_path("file:///C:/docs/a.md#top?x").as_deref(),
        Some("C:\\docs\\a.md")
    );
    assert_eq!(
        file_uri_to_local_path("file:///C:/docs/what%3F%23.md").as_deref(),
        Some("C:\\docs\\what?#.md")
    );
    let probe = MemoryProbe::new(&["C:\\docs\\a.md"]);
    assert_eq!(
        find(&[Some("file:///C:/docs/a.md?view=raw")], &probe),
        Some("C:\\docs\\a.md".into())
    );
}

/// Expands `PROGRA~1` the way `GetLongPathNameW` would.
struct ShortNameProbe {
    inner: MemoryProbe,
    expansions: RefCell<usize>,
}

impl StartupFileProbe for ShortNameProbe {
    fn full_path(&self, path: &str) -> Option<String> {
        self.inner.full_path(path)
    }

    fn is_file(&self, full_path: &str) -> bool {
        self.inner.is_file(full_path)
    }

    fn expand_short_names(&self, full_path: &str) -> Option<String> {
        *self.expansions.borrow_mut() += 1;
        full_path
            .contains("PROGRA~1")
            .then(|| full_path.replace("PROGRA~1", "Program Files"))
    }
}

#[test]
fn short_names_are_expanded_through_the_probe_hook() {
    let probe = ShortNameProbe {
        inner: MemoryProbe::new(&["C:\\Program Files\\a.md", "C:\\x~y\\b.md"]),
        expansions: RefCell::new(0),
    };
    assert_eq!(
        find_first_existing([Some("C:\\PROGRA~1\\a.md")], &probe),
        Some("C:\\Program Files\\a.md".into())
    );
    // A `~` that is not a short name keeps the path.
    assert_eq!(
        find_first_existing([Some("C:\\x~y\\b.md")], &probe),
        Some("C:\\x~y\\b.md".into())
    );
    assert_eq!(*probe.expansions.borrow(), 2);
    // Paths without `~` never call the hook.
    assert_eq!(find_first_existing([Some("C:\\docs\\c.md")], &probe), None);
    assert_eq!(*probe.expansions.borrow(), 2);
}

#[test]
fn the_default_hook_keeps_short_names() {
    let probe = MemoryProbe::new(&["C:\\PROGRA~1\\a.md"]);
    assert_eq!(
        find(&[Some("C:\\PROGRA~1\\a.md")], &probe),
        Some("C:\\PROGRA~1\\a.md".into())
    );
}
