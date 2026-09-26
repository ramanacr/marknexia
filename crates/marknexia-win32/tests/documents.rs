use std::{ffi::OsString, path::PathBuf};

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{PageIdentity, RenderError};
use marknexia_win32::documents::{
    OpenError, RenderTickets, SourceEncoding, decode_source, read_source, render_file,
    startup_paths, tab_title,
};

fn os(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/markdown/gfm")
        .join(name)
}

#[test]
fn startup_paths_keep_every_argument_and_resolve_relative_ones() {
    assert!(startup_paths(os(&[])).is_empty());
    let paths = startup_paths(os(&["a.md", "", "C:\\docs\\b.markdown"]));
    assert_eq!(paths.len(), 2);
    assert!(paths[0].is_absolute());
    assert!(paths[0].ends_with("a.md"));
    assert_eq!(paths[1], PathBuf::from("C:\\docs\\b.markdown"));
    // One leading `--` separator is dropped; a later one is a file name.
    let paths = startup_paths(os(&["--", "-x.md", "--"]));
    assert_eq!(paths.len(), 2);
    assert!(paths[0].ends_with("-x.md"));
    assert!(paths[1].ends_with("--"));
}

#[test]
fn tab_titles_are_file_names() {
    assert_eq!(tab_title(&PathBuf::from("C:\\docs\\notes.md")), "notes.md");
    assert_eq!(tab_title(&PathBuf::from("features.md")), "features.md");
}

#[test]
fn decoding_matches_dotnet_file_service() {
    assert_eq!(decode_source(b""), (String::new(), SourceEncoding::Utf8));
    assert_eq!(
        decode_source("# Héllo".as_bytes()),
        ("# Héllo".to_owned(), SourceEncoding::Utf8)
    );
    // The UTF-8 BOM is stripped; invalid bytes after it are replaced.
    assert_eq!(
        decode_source(b"\xEF\xBB\xBF# x"),
        ("# x".to_owned(), SourceEncoding::Utf8Bom)
    );
    assert_eq!(
        decode_source(b"\xEF\xBB\xBFa\xFF"),
        ("a\u{FFFD}".to_owned(), SourceEncoding::Utf8Bom)
    );
    // No BOM and invalid UTF-8: Latin-1, byte for byte.
    assert_eq!(
        decode_source(b"caf\xE9"),
        ("café".to_owned(), SourceEncoding::Latin1)
    );
    assert_eq!(
        decode_source(b"\xFF\xFEa\x00\xE9\x00"),
        ("aé".to_owned(), SourceEncoding::Utf16Le)
    );
    assert_eq!(
        decode_source(b"\xFE\xFF\x00a\x00\xE9"),
        ("aé".to_owned(), SourceEncoding::Utf16Be)
    );
    // Lone surrogates and a trailing odd byte become U+FFFD.
    assert_eq!(
        decode_source(b"\xFF\xFE\x00\xD8a\x00b"),
        ("\u{FFFD}a\u{FFFD}".to_owned(), SourceEncoding::Utf16Le)
    );
    // .NET checks FF FE first, so a UTF-32 LE BOM decodes as UTF-16 LE.
    assert_eq!(
        decode_source(b"\xFF\xFE\x00\x00a\x00\x00\x00").1,
        SourceEncoding::Utf16Le
    );
    // .NET labels 00 00 FE FF as UTF-32 BE but decodes little-endian.
    assert_eq!(
        decode_source(b"\x00\x00\xFE\xFFa\x00\x00\x00"),
        ("a".to_owned(), SourceEncoding::Utf32Be)
    );
}

#[test]
fn reads_are_size_checked_before_reading_and_bounded() {
    let path = fixture("features.md");
    let text = read_source(&path, |_| Ok(()), 1 << 20).unwrap();
    assert!(text.starts_with("# GFM Full Features Test"));
    let too_large = RenderError::SourceTooLarge {
        size_bytes: 1,
        maximum_bytes: 0,
    };
    assert_eq!(
        read_source(&path, |_| Err(too_large.clone()), 1 << 20),
        Err(OpenError::Render(too_large))
    );
    // A pre-check that passes cannot let a longer file through the read.
    assert!(matches!(
        read_source(&path, |_| Ok(()), 16),
        Err(OpenError::Render(RenderError::SourceTooLarge { .. }))
    ));
    assert_eq!(
        read_source(&fixture("does-not-exist.md"), |_| Ok(()), 16),
        Err(OpenError::NotFound)
    );
}

#[test]
fn render_file_produces_a_sealed_page() {
    let identity = PageIdentity::new([7; 16], [8; 16]).unwrap();
    let rendered = render_file(&fixture("features.md"), identity, AppTheme::System).unwrap();
    assert!(rendered.body().as_str().contains("GFM Full Features Test"));
    assert!(rendered.page_html().contains(&identity.nonce()));
}

#[test]
fn stale_render_results_are_discarded() {
    let mut tickets = RenderTickets::new();
    let first = tickets.begin(1);
    let other = tickets.begin(2);
    assert!(tickets.is_pending(1));
    // The tab closed while rendering: its result is dropped.
    tickets.cancel(1);
    assert!(!tickets.is_pending(1));
    assert!(!tickets.complete(1, first));
    // A newer render supersedes an older one for the same tab.
    let old = tickets.begin(3);
    let new = tickets.begin(3);
    assert!(!tickets.complete(3, old));
    assert!(tickets.complete(3, new));
    // Accepted exactly once; a ticket never matches another tab.
    assert!(!tickets.complete(3, new));
    assert!(!tickets.complete(1, other));
    assert!(tickets.complete(2, other));
    assert!(!tickets.is_pending(2));
}
