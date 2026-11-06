//! The strict ZIP reader and payload policy, including the frozen .NET
//! update-archive fixtures. Fixture archives are read in memory; none of
//! them reaches extraction (each is rejected before anything is written),
//! and the test asserts that no fixture entry holds PE data.

mod support;

use std::path::PathBuf;

use marknexia_update::{
    archive::{self, ArchiveError, ArchiveLimits},
    payload::{self, PayloadError, PayloadPolicy, pe_machine},
};
use support::*;

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compat/fixtures/v1/update-archives")
}

fn base64(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buffer = 0_u32;
    let mut bits = 0;
    for byte in text.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => panic!("base64"),
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    out
}

fn list(bytes: &[u8]) -> Result<Vec<archive::Entry>, ArchiveError> {
    archive::list(bytes, &ArchiveLimits::default())
}

// ---------------------------------------------------------------------------
// Frozen fixtures.
// ---------------------------------------------------------------------------

#[test]
fn every_fixture_archive_is_free_of_pe_data() {
    let mut count = 0;
    for entry in std::fs::read_dir(fixture_dir()).expect("fixtures") {
        let path = entry.unwrap().path();
        let case: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let bytes = base64(case["input"]["archiveBytesBase64"].as_str().unwrap());
        assert!(
            !bytes.windows(2).any(|pair| pair == b"MZ"),
            "{} contains an MZ marker",
            path.display()
        );
        for entry in case["input"]["packageEntries"].as_array().unwrap() {
            if let Some(content) = entry["contentBase64"].as_str() {
                assert!(base64(content).len() < 2, "{}", path.display());
            }
        }
        count += 1;
    }
    assert_eq!(count, 5);
}

/// The fixtures' .NET payload names, with the .NET required-file order.
pub fn dotnet_policy() -> PayloadPolicy {
    use marknexia_update::payload::{AllowedExecutable, Signer};
    PayloadPolicy {
        required_files: [
            "Marknexia.App.exe",
            "Marknexia.App.dll",
            "Marknexia.App.pri",
            "marknexia-sbom.spdx.json",
        ]
        .map(str::to_owned)
        .to_vec(),
        optional_files: Vec::new(),
        executables: vec![
            AllowedExecutable {
                name: "Marknexia.App.exe".to_owned(),
                signer: Signer::Release,
                is_shell: true,
            },
            AllowedExecutable {
                name: "Marknexia.App.dll".to_owned(),
                signer: Signer::Release,
                is_shell: false,
            },
        ],
    }
}

#[test]
fn the_fixture_archives_are_rejected_with_the_dotnet_messages() {
    use marknexia_update::{
        layout::Layout,
        manifest::{Architecture, HttpsUrl, Manifest},
        stage::{StageRequest, stage_bytes},
        version::Version,
    };
    for name in [
        "archive-traversal",
        "checksum-mismatch",
        "expansion-limits",
        "missing-payload-files",
        "stale-stage-cleanup",
    ] {
        let path = fixture_dir().join(format!("{name}.case.json"));
        let case: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let input = &case["input"];
        let expected = &case["expected"];
        let bytes = base64(input["archiveBytesBase64"].as_str().unwrap());
        assert_eq!(bytes.len() as u64, input["packageBytes"].as_u64().unwrap());
        // The fixture's checksum file holds the expected hash.
        let checksum_text = input["checksumText"].as_str().unwrap();
        let expected_hash: [u8; 32] =
            marknexia_update::hex::decode(&checksum_text[..64]).expect("hash");
        assert_eq!(
            sha256_hex(&bytes) == checksum_text[..64],
            input["checksumMatches"].as_bool().unwrap(),
            "{name}"
        );
        let version = Version::parse("1.2.0").unwrap();
        let manifest = Manifest {
            url: HttpsUrl::parse(
                &format!("https://{HOST}/r/Marknexia-v1.2.0-win-x64.zip"),
                &hosts(),
            )
            .unwrap(),
            version,
            architecture: Architecture::X64,
            sha256: expected_hash,
            byte_length: bytes.len() as u64,
            minimum_os_build: 0,
            artifact_id: "Marknexia-v1.2.0-win-x64.zip".to_owned(),
            key_id: KEY_ID.to_owned(),
        };
        let temp = TempDir::new(name);
        let layout = Layout::new(temp.path().join("updates"));
        std::fs::create_dir_all(layout.staging()).unwrap();
        let stale = input["preExistingStage"]
            .as_str()
            .map(|stale| layout.staging().join(stale));
        if let Some(stale) = &stale {
            std::fs::create_dir(stale).unwrap();
        }
        let policy = dotnet_policy();
        let request = StageRequest {
            manifest: &manifest,
            policy: &policy,
            limits: ArchiveLimits::default(),
            publisher: &pin(),
        };
        let platform = FakePlatform::default();
        let result = stage_bytes(
            &bytes,
            &request,
            &layout,
            &platform,
            &std::sync::atomic::AtomicBool::new(false),
        );
        let error = result.expect_err(name);
        assert_eq!(
            error.message(),
            expected["message"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(expected["outcome"], "rejected");
        // Nothing was extracted or checked; only a stale stage remains.
        assert!(platform.checked.borrow().is_empty(), "{name}");
        let pending = std::fs::read_dir(layout.staging())
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(
            pending as u64,
            expected["pendingStageDirectories"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            stale.is_some_and(|stale| stale.is_dir()),
            expected["preExistingStageStillExists"].as_bool().unwrap(),
            "{name}"
        );
        assert!(!layout.versions().join("1.2.0-x64").exists(), "{name}");
    }
}

// ---------------------------------------------------------------------------
// Crafted archives (text payloads, in memory).
// ---------------------------------------------------------------------------

#[test]
fn a_valid_archive_lists_and_inflates() {
    let bytes = text_payload();
    let entries = list(&bytes).expect("valid");
    let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
    assert_eq!(paths, ["shell.txt", "loader.txt", "docs/README.txt"]);
    let readme = archive::read(&bytes, &entries[2]).expect("inflate");
    assert_eq!(readme, b"read me\n".repeat(64));
    payload::check(&entries, &text_policy()).expect("policy");
}

#[test]
fn traversal_absolute_and_invalid_names_are_rejected() {
    for (name, expected) in [
        ("../escaped.txt", ArchiveError::Traversal),
        ("docs/../../escaped.txt", ArchiveError::Traversal),
        ("/etc/passwd", ArchiveError::InvalidPath),
        ("C:/Windows/x.txt", ArchiveError::InvalidPath),
        ("..\\escaped.txt", ArchiveError::InvalidPath),
        ("file.txt:ads", ArchiveError::InvalidPath),
        ("aux.txt", ArchiveError::InvalidPath),
        ("dir./x.txt", ArchiveError::InvalidPath),
    ] {
        let bytes = zip(&[ZipEntry::file(name, b"x")]);
        assert_eq!(list(&bytes).unwrap_err(), expected, "{name}");
    }
    // A non-ASCII name without the UTF-8 flag has no single meaning.
    let bytes = zip(&[ZipEntry::file("caf\u{e9}.txt", b"x")]);
    assert_eq!(list(&bytes).unwrap_err(), ArchiveError::InvalidPath);
    let mut flagged = ZipEntry::file("caf\u{e9}.txt", b"x");
    flagged.flags = 0x0800;
    assert!(list(&zip(&[flagged])).is_ok());
}

#[test]
fn duplicates_and_file_folder_conflicts_are_rejected() {
    for names in [
        ["a.txt", "a.txt"],
        ["a.txt", "A.TXT"],
        ["dir/a.txt", "DIR/A.txt"],
        ["dir", "dir/a.txt"],
    ] {
        let bytes = zip(&[
            ZipEntry::file(names[0], b"1"),
            ZipEntry::file(names[1], b"2"),
        ]);
        assert_eq!(
            list(&bytes).unwrap_err(),
            ArchiveError::Duplicate,
            "{names:?}"
        );
    }
}

#[test]
fn links_and_reparse_points_are_rejected() {
    let mut symlink = ZipEntry::file("link", b"target.txt");
    symlink.made_by = (3 << 8) | 20;
    symlink.external = 0o120_777 << 16;
    assert_eq!(list(&zip(&[symlink])).unwrap_err(), ArchiveError::Link);
    let mut reparse = ZipEntry::file("junction", b"");
    reparse.external = 0x400;
    assert_eq!(list(&zip(&[reparse])).unwrap_err(), ArchiveError::Link);
    // A regular Unix file is fine.
    let mut regular = ZipEntry::file("file.txt", b"x");
    regular.made_by = (3 << 8) | 20;
    regular.external = 0o100_644 << 16;
    assert!(list(&zip(&[regular])).is_ok());
}

#[test]
fn encryption_and_unknown_formats_are_rejected() {
    let mut encrypted = ZipEntry::file("a.txt", b"x");
    encrypted.flags = 1;
    assert_eq!(
        list(&zip(&[encrypted])).unwrap_err(),
        ArchiveError::Unsupported
    );
    // Trailing bytes after the end record, prepended data, truncation.
    let bytes = text_payload();
    let mut trailing = bytes.clone();
    trailing.extend_from_slice(b"junk");
    assert_eq!(list(&trailing).unwrap_err(), ArchiveError::Corrupt);
    let mut prepended = b"junk".to_vec();
    prepended.extend_from_slice(&bytes);
    assert_eq!(list(&prepended).unwrap_err(), ArchiveError::Corrupt);
    for cut in [0, 10, bytes.len() / 2, bytes.len() - 1] {
        assert!(list(&bytes[..cut]).is_err(), "cut {cut}");
    }
    assert_eq!(
        list(b"not a zip at all").unwrap_err(),
        ArchiveError::Corrupt
    );
}

#[test]
fn entry_count_and_sizes_are_bounded_before_inflation() {
    let many: Vec<ZipEntry> = (0..5)
        .map(|index| ZipEntry::file(&format!("f{index}.txt"), b"x"))
        .collect();
    let limits = ArchiveLimits {
        max_entries: 4,
        ..ArchiveLimits::default()
    };
    assert_eq!(
        archive::list(&zip(&many), &limits).unwrap_err(),
        ArchiveError::TooManyEntries
    );
    let small = ArchiveLimits {
        max_entry_bytes: 10,
        max_total_bytes: 15,
        ..ArchiveLimits::default()
    };
    assert_eq!(
        archive::list(&zip(&[ZipEntry::file("big.txt", &[b'a'; 11])]), &small).unwrap_err(),
        ArchiveError::TooLarge
    );
    assert_eq!(
        archive::list(
            &zip(&[
                ZipEntry::file("a.txt", &[b'a'; 8]),
                ZipEntry::file("b.txt", &[b'b'; 8])
            ]),
            &small
        )
        .unwrap_err(),
        ArchiveError::TooLarge
    );
    // A declared size over the default limit (the .NET fixture shape).
    let mut huge = ZipEntry::file("oversized.bin", b"");
    huge.declared_size = Some(1_000_000_001);
    assert_eq!(list(&zip(&[huge])).unwrap_err(), ArchiveError::TooLarge);
}

#[test]
fn inflation_stops_at_the_declared_size_and_checks_the_crc() {
    // A deflate stream that expands past its declared size (a bomb).
    let mut bomb = ZipEntry::deflated("bomb.txt", &[b'a'; 100_000]);
    bomb.declared_size = Some(100);
    let bytes = zip(&[bomb]);
    let entries = list(&bytes).expect("listed by declared size");
    assert_eq!(
        archive::read(&bytes, &entries[0]).unwrap_err(),
        ArchiveError::TooLarge
    );
    let mut wrong_crc = ZipEntry::deflated("a.txt", b"hello hello hello");
    wrong_crc.crc = Some(0x1234_5678);
    let bytes = zip(&[wrong_crc]);
    let entries = list(&bytes).unwrap();
    assert_eq!(
        archive::read(&bytes, &entries[0]).unwrap_err(),
        ArchiveError::Checksum
    );
    // A stored entry whose recorded size differs from its data.
    let mut short = ZipEntry::file("a.txt", b"hello");
    short.declared_size = Some(4);
    assert_eq!(list(&zip(&[short])).unwrap_err(), ArchiveError::Unsupported);
}

// ---------------------------------------------------------------------------
// Payload policy (in memory; names only, text content).
// ---------------------------------------------------------------------------

#[test]
fn the_rust_payload_needs_the_shell_and_loader_and_nothing_else_executable() {
    let policy = PayloadPolicy::rust_portable();
    let good = zip(&[
        ZipEntry::file("marknexia-win32.exe", b"TEST text, not PE"),
        ZipEntry::file("WebView2Loader.dll", b"TEST text, not PE"),
        ZipEntry::file("README.txt", b"read me"),
        ZipEntry::file("THIRD-PARTY-NOTICES.txt", b"notices"),
    ]);
    payload::check(&list(&good).unwrap(), &policy).expect("valid payload");
    for (extra, expected) in [
        ("helper.exe", "helper.exe"),
        ("tools/marknexia-win32.exe", "tools/marknexia-win32.exe"),
        ("evil.DLL", "evil.DLL"),
        ("run.ps1", "run.ps1"),
        ("start.cmd", "start.cmd"),
        ("shortcut.lnk", "shortcut.lnk"),
    ] {
        let bytes = zip(&[
            ZipEntry::file("marknexia-win32.exe", b"TEST text"),
            ZipEntry::file("WebView2Loader.dll", b"TEST text"),
            ZipEntry::file(extra, b"TEST text"),
        ]);
        assert_eq!(
            payload::check(&list(&bytes).unwrap(), &policy),
            Err(PayloadError::UnexpectedExecutable(expected.to_owned())),
            "{extra}"
        );
    }
    let missing = zip(&[ZipEntry::file("marknexia-win32.exe", b"TEST text")]);
    let error = payload::check(&list(&missing).unwrap(), &policy).unwrap_err();
    assert_eq!(
        error.message(),
        "The update package is missing required file 'WebView2Loader.dll'."
    );
}

#[test]
fn the_pe_machine_reader_matches_this_test_binary() {
    // Reads the real, already-built test executable; no PE bytes are made.
    let exe = std::env::current_exe().unwrap();
    let mut header = vec![0_u8; 4096];
    let read = {
        use std::io::Read;
        std::fs::File::open(exe).unwrap().read(&mut header).unwrap()
    };
    let machine = pe_machine(&header[..read]).expect("PE image");
    let expected = marknexia_update::manifest::Architecture::current()
        .expect("x64 or ARM64")
        .pe_machine();
    assert_eq!(machine, expected);
    assert_eq!(pe_machine(b"TEST text"), None);
    assert_eq!(pe_machine(b""), None);
}

#[test]
fn only_the_exact_payload_file_set_is_accepted() {
    let policy = PayloadPolicy::rust_portable();
    let base = [
        ZipEntry::file("marknexia-win32.exe", b"TEST text"),
        ZipEntry::file("WebView2Loader.dll", b"TEST text"),
    ];
    // DLL redirection and configuration files are refused, as is anything
    // not in the set (files or folders).
    for extra in [
        "marknexia-win32.exe.local",
        "marknexia-win32.exe.manifest",
        "marknexia-win32.exe.config",
        "WebView2Loader.dll.local",
        "notes.txt",
        "docs/README.txt",
        "docs/",
    ] {
        let mut entries = base.to_vec();
        entries.push(ZipEntry::file(extra, b""));
        let error = payload::check(&list(&zip(&entries)).unwrap(), &policy).unwrap_err();
        assert_eq!(
            error,
            PayloadError::UnexpectedFile(extra.trim_end_matches('/').to_owned()),
            "{extra}"
        );
        assert!(error.message().contains("unexpected file"));
    }
    // A folder that holds an allowed file is fine (test policy).
    payload::check(&list(&text_payload()).unwrap(), &text_policy()).unwrap();
}

#[test]
fn the_publisher_is_pinned_by_certificate_thumbprint_and_name() {
    use marknexia_update::payload::{PinError, PublisherPin, SignerCertificate};
    let pin = pin();
    let signer = |name: &str, certificate: &[u8]| SignerCertificate {
        name: name.to_owned(),
        certificate: certificate.to_vec(),
    };
    assert_eq!(
        pin.accepts(&signer("Example Publisher", TEST_CERTIFICATE)),
        Ok(())
    );
    // The right name on another certificate (a look-alike CN) is refused.
    assert_eq!(
        pin.accepts(&signer("Example Publisher", b"another certificate")),
        Err(PinError::UnknownCertificate)
    );
    // The pinned certificate under another name is refused too.
    assert_eq!(
        pin.accepts(&signer("Example Publisher Ltd", TEST_CERTIFICATE)),
        Err(PinError::NameMismatch)
    );
    let unnamed = PublisherPin {
        name: String::new(),
        ..pin.clone()
    };
    assert_eq!(
        unnamed.accepts(&signer("", TEST_CERTIFICATE)),
        Err(PinError::NameMismatch)
    );
    assert_eq!(PublisherPin::parse_thumbprints(""), None);
    assert_eq!(PublisherPin::parse_thumbprints("abcd"), None);
    assert_eq!(PublisherPin::parse_thumbprints(&"A".repeat(64)), None);
    assert_eq!(
        PublisherPin::parse_thumbprints(&format!("{},{}", "a".repeat(64), "b".repeat(64)))
            .map(|list| list.len()),
        Some(2)
    );
}
