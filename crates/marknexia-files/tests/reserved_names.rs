use marknexia_files::path::CanonicalPath;

#[test]
fn port_zero_console_devices_and_space_padded_names_are_reserved() {
    for path in [
        "C:/repo/COM0",
        "C:/repo/lpt0.md",
        "C:/repo/CONIN$",
        "C:/repo/conout$.txt",
        "C:/repo/COM1 .md",
        "C:/repo/NUL  .txt",
        "C:/repo/docs/CON .md",
    ] {
        assert!(CanonicalPath::new(path).is_err(), "{path}");
    }
    for path in [
        "C:/repo/COM00.md",
        "C:/repo/CONIN.md",
        "C:/repo/CONOUT$X.md",
        "C:/repo/ COM1.md",
        // Only U+0020 is trimmed, not other Unicode whitespace.
        "C:/repo/COM1\u{a0}.md",
        "C:/repo/NUL\u{3000}.md",
    ] {
        assert!(CanonicalPath::new(path).is_ok(), "{path}");
    }
}
