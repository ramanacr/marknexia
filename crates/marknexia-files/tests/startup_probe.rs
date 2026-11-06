//! Parity with `StartupFileResolverTests.cs`, against real temporary files.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use marknexia_files::startup_probe::find_first_existing;

struct TempFile(PathBuf);

impl TempFile {
    fn new(prefix: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "{prefix} {}-{}.md",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, "# Activation").unwrap();
        Self(path)
    }

    fn as_str(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn file_uri(path: &str) -> String {
    let mut uri = String::from("file:///");
    for byte in path.replace('\\', "/").bytes() {
        if byte.is_ascii_alphanumeric() || b"/:-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

#[test]
fn extracts_document_from_activation_command_line() {
    for template in [
        "--open \"{0}\"",
        "\"C:\\Program Files\\Marknexia\\Marknexia.App.exe\" \"{0}\"",
        "\"{0}\" \"missing second file.md\"",
    ] {
        let existing = TempFile::new("marknexia activation");
        let command_line = template.replace("{0}", existing.as_str());
        assert_eq!(
            find_first_existing([Some(command_line.as_str())]).as_deref(),
            Some(existing.as_str()),
            "{template}"
        );
    }
}

#[test]
fn accepts_file_uri_and_skips_existing_executable() {
    let existing = TempFile::new("marknexia #activation");
    let executable = std::env::current_exe().unwrap();
    let uri = file_uri(existing.as_str());
    assert_eq!(
        find_first_existing([executable.to_str(), Some(uri.as_str())]).as_deref(),
        Some(existing.as_str())
    );
}

#[test]
fn returns_the_first_valid_candidate_in_activation_order() {
    let existing = TempFile::new("marknexia-startup");
    assert_eq!(
        find_first_existing([None, Some(""), Some("missing.md"), Some(existing.as_str())])
            .as_deref(),
        Some(existing.as_str())
    );
}

#[test]
fn returns_none_when_no_candidate_is_a_file() {
    assert_eq!(
        find_first_existing([None, Some(""), Some("missing.md")]),
        None
    );
}

#[test]
fn accepts_quoted_paths_from_command_line_activation() {
    let existing = TempFile::new("marknexia startup");
    let quoted = format!("\"{}\"", existing.as_str());
    assert_eq!(
        find_first_existing([Some(quoted.as_str())]).as_deref(),
        Some(existing.as_str())
    );
}

#[test]
fn a_directory_with_a_markdown_name_is_not_a_file() {
    let directory = std::env::temp_dir().join(format!("marknexia-dir-{}.md", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    assert_eq!(find_first_existing([directory.to_str()]), None);
    fs::remove_dir(&directory).unwrap();
}

#[test]
fn a_path_with_a_nul_character_is_rejected() {
    assert_eq!(find_first_existing([Some("C:\\a\0b.md")]), None);
}
