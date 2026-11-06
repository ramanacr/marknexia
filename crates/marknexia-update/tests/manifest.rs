//! `update-manifest-v1` threat tests: signature, key, tampering, replay
//! and downgrade, architecture, OS, URL and artifact identity. Keys are
//! derived from published TEST seeds; nothing is downloaded.

mod support;

use marknexia_update::{
    keys::{KeySet, TEST_KEY_ID, TEST_PUBLIC_KEY_HEX},
    manifest::{
        Architecture, Expectations, ManifestDocument, ManifestError, SignatureDocument, UrlError,
        verify,
    },
    version::Version,
};
use support::*;

const ARTIFACT: &[u8] = b"TEST artifact bytes";

fn expect<'a>(current: &'a Version, hosts: &'a [String]) -> Expectations<'a> {
    Expectations {
        current_version: current,
        architecture: Architecture::X64,
        os_build: 22631,
        trusted_hosts: hosts,
        max_byte_length: 256 * 1024 * 1024,
    }
}

fn run(document: &ManifestDocument) -> Result<marknexia_update::manifest::Manifest, ManifestError> {
    let (bytes, signature) = signed(document);
    let current = Version::parse("1.0.0").unwrap();
    let hosts = hosts();
    verify(&bytes, &signature, &keys(), &expect(&current, &hosts))
}

#[test]
fn the_committed_test_key_is_derived_from_the_published_test_seed() {
    let public = key_pair(TEST_SEED).pk;
    assert_eq!(
        marknexia_update::hex::encode(public.as_ref()),
        TEST_PUBLIC_KEY_HEX
    );
    assert!(KeySet::test_only().contains(TEST_KEY_ID));
    assert!(KeySet::test_only().contains_test_key());
}

#[test]
fn a_valid_signed_manifest_is_accepted_with_every_field() {
    let document = document("1.2.0", ARTIFACT);
    let manifest = run(&document).expect("valid");
    assert_eq!(manifest.version.to_string(), "1.2.0");
    assert_eq!(manifest.architecture, Architecture::X64);
    assert_eq!(manifest.byte_length, ARTIFACT.len() as u64);
    assert_eq!(manifest.artifact_id, "Marknexia-v1.2.0-win-x64.zip");
    assert_eq!(manifest.url.host(), HOST);
    assert_eq!(manifest.minimum_os_build, 19041);
    assert_eq!(manifest.key_id, KEY_ID);
    assert_eq!(
        marknexia_update::hex::encode(&manifest.sha256),
        sha256_hex(ARTIFACT)
    );
    // The canonical form is exactly JCS: sorted keys, no white space.
    let text = String::from_utf8(document.to_canonical_json()).unwrap();
    assert!(text.starts_with(r#"{"architecture":"x64","artifactId":"Marknexia-v1.2.0-win-x64.zip","artifactKind":"portable-zip","byteLength":19,"#));
    assert!(!text.contains(' '));
}

#[test]
fn a_bad_signature_is_rejected() {
    let document = document("1.2.0", ARTIFACT);
    let bytes = document.to_canonical_json();
    let mut signature = key_pair(TEST_SEED).sk.sign(&bytes, None);
    signature[0] ^= 1;
    let envelope = SignatureDocument::new(KEY_ID, &signature).to_canonical_json();
    let current = Version::parse("1.0.0").unwrap();
    let hosts = hosts();
    assert_eq!(
        verify(&bytes, &envelope, &keys(), &expect(&current, &hosts)),
        Err(ManifestError::BadSignature)
    );
    // A zero signature and a malformed one.
    let zero = SignatureDocument::new(KEY_ID, &[0; 64]).to_canonical_json();
    assert_eq!(
        verify(&bytes, &zero, &keys(), &expect(&current, &hosts)),
        Err(ManifestError::BadSignature)
    );
    let short = br#"{"algorithm":"Ed25519","keyId":"test-release-key","signature":"abcd"}"#;
    assert_eq!(
        verify(&bytes, short, &keys(), &expect(&current, &hosts)),
        Err(ManifestError::Malformed)
    );
}

#[test]
fn a_signature_from_the_wrong_key_is_rejected() {
    let document = document("1.2.0", ARTIFACT);
    let bytes = document.to_canonical_json();
    let current = Version::parse("1.0.0").unwrap();
    let hosts = hosts();
    // Signed with another key but claiming the trusted key id.
    let forged = sign(&bytes, OTHER_TEST_SEED, KEY_ID);
    assert_eq!(
        verify(&bytes, &forged, &keys(), &expect(&current, &hosts)),
        Err(ManifestError::BadSignature)
    );
    // A key id that is not embedded.
    let unknown = sign(&bytes, OTHER_TEST_SEED, "attacker-key");
    assert_eq!(
        verify(&bytes, &unknown, &keys(), &expect(&current, &hosts)),
        Err(ManifestError::UnknownKey)
    );
    // No keys at all: nothing verifies (no unsigned fallback).
    assert_eq!(
        verify(
            &bytes,
            &sign(&bytes, TEST_SEED, KEY_ID),
            &KeySet::new(),
            &expect(&current, &hosts)
        ),
        Err(ManifestError::UnknownKey)
    );
    // The manifest names another key than the signature.
    let mut other = document.clone();
    other.key_id = "second-key".to_owned();
    let mut keys = keys();
    keys.insert("second-key", *key_pair(OTHER_TEST_SEED).pk)
        .unwrap();
    let bytes = other.to_canonical_json();
    assert_eq!(
        verify(
            &bytes,
            &sign(&bytes, TEST_SEED, KEY_ID),
            &keys,
            &expect(&current, &hosts)
        ),
        Err(ManifestError::KeyMismatch)
    );
}

#[test]
fn any_field_changed_after_signing_is_rejected() {
    let original = document("1.2.0", ARTIFACT);
    let (_, signature) = signed(&original);
    let current = Version::parse("1.0.0").unwrap();
    let hosts = hosts();
    let mutations: Vec<Box<dyn Fn(&mut ManifestDocument)>> = vec![
        Box::new(|d| d.version = "9.9.9".to_owned()),
        Box::new(|d| d.architecture = "ARM64".to_owned()),
        Box::new(|d| d.url = d.url.replace("/releases/", "/evil/")),
        Box::new(|d| d.sha256 = "0".repeat(64)),
        Box::new(|d| d.byte_length += 1),
        Box::new(|d| d.minimum_os_build = 1),
        Box::new(|d| d.artifact_id.push('x')),
        Box::new(|d| d.artifact_kind = "msix".to_owned()),
        Box::new(|d| d.channel = "store".to_owned()),
        Box::new(|d| d.schema_version = "update-manifest-v2".to_owned()),
    ];
    for (index, mutate) in mutations.iter().enumerate() {
        let mut tampered = original.clone();
        mutate(&mut tampered);
        assert_eq!(
            verify(
                &tampered.to_canonical_json(),
                &signature,
                &keys(),
                &expect(&current, &hosts)
            ),
            Err(ManifestError::BadSignature),
            "mutation {index}"
        );
    }
    // A single flipped byte anywhere in the signed bytes.
    let bytes = original.to_canonical_json();
    for position in [0, 10, bytes.len() / 2, bytes.len() - 1] {
        let mut flipped = bytes.clone();
        flipped[position] ^= 0x20;
        assert!(
            verify(&flipped, &signature, &keys(), &expect(&current, &hosts)).is_err(),
            "byte {position}"
        );
    }
}

#[test]
fn noncanonical_duplicate_or_unknown_json_is_rejected_even_when_signed() {
    let document = document("1.2.0", ARTIFACT);
    let canonical = String::from_utf8(document.to_canonical_json()).unwrap();
    let current = Version::parse("1.0.0").unwrap();
    let hosts = hosts();
    let variants = [
        (
            canonical.replace(",\"", ", \""),
            ManifestError::NotCanonical,
        ),
        (format!("{canonical}\n"), ManifestError::NotCanonical),
        (format!("\u{feff}{canonical}"), ManifestError::Malformed),
        (
            canonical.replace("\"byteLength\":19", "\"byteLength\":19.0"),
            ManifestError::Malformed,
        ),
        (
            canonical.replace("\"byteLength\":19", "\"byteLength\":1.9e1"),
            ManifestError::Malformed,
        ),
        (
            canonical.replace("{\"architecture\"", "{\"aaa\":\"x\",\"architecture\""),
            ManifestError::Malformed,
        ),
        (
            canonical.replace(
                "\"channel\":\"portable\"",
                "\"channel\":\"portable\",\"channel\":\"portable\"",
            ),
            ManifestError::Malformed,
        ),
        (
            // Keys out of order.
            canonical.replacen(
                "\"artifactId\":\"Marknexia-v1.2.0-win-x64.zip\",\"artifactKind\":\"portable-zip\"",
                "\"artifactKind\":\"portable-zip\",\"artifactId\":\"Marknexia-v1.2.0-win-x64.zip\"",
                1,
            ),
            ManifestError::NotCanonical,
        ),
        (
            canonical.replace("portable-zip", "portable\\u002dzip"),
            ManifestError::NotCanonical,
        ),
        (
            canonical.replace(",\"keyId\":\"test-release-key\"", ""),
            ManifestError::Malformed,
        ),
    ];
    for (text, expected) in variants {
        let bytes = text.into_bytes();
        // Validly signed by the trusted key: the format alone must refuse.
        let signature = sign(&bytes, TEST_SEED, KEY_ID);
        assert_eq!(
            verify(&bytes, &signature, &keys(), &expect(&current, &hosts)),
            Err(expected),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
    }
}

#[test]
fn signature_documents_are_strict() {
    let document = document("1.2.0", ARTIFACT);
    let bytes = document.to_canonical_json();
    let current = Version::parse("1.0.0").unwrap();
    let hosts = hosts();
    let signature = key_pair(TEST_SEED).sk.sign(&bytes, None);
    let mut envelope = SignatureDocument::new(KEY_ID, &signature);
    envelope.algorithm = "ed25519ph".to_owned();
    assert_eq!(
        verify(
            &bytes,
            &envelope.to_canonical_json(),
            &keys(),
            &expect(&current, &hosts)
        ),
        Err(ManifestError::UnsupportedAlgorithm)
    );
    let good = SignatureDocument::new(KEY_ID, &signature).to_canonical_json();
    let spaced = String::from_utf8(good.clone()).unwrap().replace(':', ": ");
    assert_eq!(
        verify(
            &bytes,
            spaced.as_bytes(),
            &keys(),
            &expect(&current, &hosts)
        ),
        Err(ManifestError::NotCanonical)
    );
    let _ = good;
    // Upper-case hex has a second spelling: refused.
    let mut upper = SignatureDocument::new(KEY_ID, &signature);
    upper.signature = upper.signature.to_uppercase();
    assert_eq!(
        verify(
            &bytes,
            &upper.to_canonical_json(),
            &keys(),
            &expect(&current, &hosts)
        ),
        Err(ManifestError::Malformed)
    );
    assert_eq!(
        verify(&bytes, b"", &keys(), &expect(&current, &hosts)),
        Err(ManifestError::Malformed)
    );
    assert_eq!(
        verify(&vec![b' '; 9000], b"{}", &keys(), &expect(&current, &hosts)),
        Err(ManifestError::TooLarge)
    );
}

#[test]
fn replay_and_downgrade_are_rejected() {
    // The same, correctly signed manifest replayed to a build that is
    // already at that version, or newer.
    for current in ["1.2.0", "1.3.0", "2.0.0"] {
        let document = document("1.2.0", ARTIFACT);
        let (bytes, signature) = signed(&document);
        let current = Version::parse(current).unwrap();
        let hosts = hosts();
        assert_eq!(
            verify(&bytes, &signature, &keys(), &expect(&current, &hosts)),
            Err(ManifestError::NotNewer)
        );
    }
    // A pre-release of the running version is older.
    assert_eq!(
        run(&document("1.0.0-rc.1", ARTIFACT)),
        Err(ManifestError::NotNewer)
    );
    for version in ["v1.2.0", "1.2", "1.2.0+build.5", "01.2.0"] {
        let mut document = document("1.2.0", ARTIFACT);
        document.version = version.to_owned();
        assert_eq!(
            run(&document),
            Err(ManifestError::InvalidVersion),
            "{version}"
        );
    }
}

#[test]
fn a_wrong_architecture_or_too_new_os_is_rejected() {
    let mut arm = document("1.2.0", ARTIFACT);
    arm.architecture = "ARM64".to_owned();
    arm.artifact_id = "Marknexia-v1.2.0-win-arm64.zip".to_owned();
    arm.url = format!("https://{HOST}/releases/v1.2.0/{}", arm.artifact_id);
    assert_eq!(run(&arm), Err(ManifestError::WrongArchitecture));
    for name in ["arm64", "X64", "x86", "AnyCPU", ""] {
        let mut document = document("1.2.0", ARTIFACT);
        document.architecture = name.to_owned();
        assert_eq!(
            run(&document),
            Err(ManifestError::WrongArchitecture),
            "{name}"
        );
    }
    // The same ARM64 manifest is valid on ARM64.
    let (bytes, signature) = signed(&arm);
    let current = Version::parse("1.0.0").unwrap();
    let hosts = hosts();
    let mut on_arm = expect(&current, &hosts);
    on_arm.architecture = Architecture::Arm64;
    assert!(verify(&bytes, &signature, &keys(), &on_arm).is_ok());
    let mut newer_os = document("1.2.0", ARTIFACT);
    newer_os.minimum_os_build = 99999;
    assert_eq!(run(&newer_os), Err(ManifestError::OsTooOld));
}

#[test]
fn url_and_artifact_identity_are_bound() {
    let base = document("1.2.0", ARTIFACT);
    let cases: Vec<(Box<dyn Fn(&mut ManifestDocument)>, ManifestError)> = vec![
        (
            Box::new(|d| d.url = d.url.replace("https://", "http://")),
            ManifestError::Url(UrlError::Malformed),
        ),
        (
            Box::new(|d| d.url = d.url.replace(HOST, "evil.example.test")),
            ManifestError::Url(UrlError::UntrustedHost),
        ),
        (
            Box::new(|d| d.url = d.url.replace(HOST, "cdn.updates.example.test")),
            ManifestError::Url(UrlError::UntrustedHost),
        ),
        (
            Box::new(|d| d.url = d.url.replace(HOST, &format!("{HOST}:8443"))),
            ManifestError::Url(UrlError::Malformed),
        ),
        (
            Box::new(|d| d.url = d.url.replace(HOST, &format!("user@{HOST}"))),
            ManifestError::Url(UrlError::Malformed),
        ),
        (
            Box::new(|d| d.url.push_str("?x=1")),
            ManifestError::Url(UrlError::Malformed),
        ),
        (
            Box::new(|d| d.url = d.url.replace("/releases/", "/releases/../")),
            ManifestError::Url(UrlError::Malformed),
        ),
        (
            Box::new(|d| d.url = d.url.replace("Marknexia-v1.2.0", "Marknexia%2Dv1.2.0")),
            ManifestError::Url(UrlError::Malformed),
        ),
        (
            Box::new(|d| d.url = d.url.replace("win-x64.zip", "win-x64-other.zip")),
            ManifestError::ArtifactMismatch,
        ),
        (
            Box::new(|d| {
                d.artifact_id = "Marknexia-v1.1.0-win-x64.zip".to_owned();
                d.url = format!("https://{HOST}/r/{}", d.artifact_id);
            }),
            ManifestError::ArtifactMismatch,
        ),
        (
            Box::new(|d| d.sha256 = d.sha256.to_uppercase()),
            ManifestError::InvalidHash,
        ),
        (
            Box::new(|d| d.sha256.truncate(63)),
            ManifestError::InvalidHash,
        ),
        (
            Box::new(|d| d.byte_length = 0),
            ManifestError::InvalidLength,
        ),
        (
            Box::new(|d| d.byte_length = 256 * 1024 * 1024 + 1),
            ManifestError::InvalidLength,
        ),
        (
            Box::new(|d| d.channel = "store".to_owned()),
            ManifestError::WrongChannel,
        ),
        (
            Box::new(|d| d.artifact_kind = "msix".to_owned()),
            ManifestError::UnsupportedArtifactKind,
        ),
        (
            Box::new(|d| d.schema_version = "update-manifest-v2".to_owned()),
            ManifestError::UnsupportedSchema,
        ),
    ];
    for (index, (mutate, expected)) in cases.iter().enumerate() {
        let mut document = base.clone();
        mutate(&mut document);
        assert_eq!(run(&document), Err(*expected), "case {index}: {document:?}");
    }
}
