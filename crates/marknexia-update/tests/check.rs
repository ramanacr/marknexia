//! Build-time configuration and the check flow, with a fake fetcher.

mod support;

use std::sync::atomic::AtomicBool;

use marknexia_update::{
    check::{CheckError, CheckOutcome, check_for_update},
    config::{BuildValues, NOT_CONFIGURED_MESSAGE, NotConfigured, UpdateConfig},
    keys::{KeyError, TEST_KEY_ID, TEST_PUBLIC_KEY_HEX},
    manifest::{Architecture, ManifestError},
    stage::FetchError,
};
use support::*;

const MANIFEST_URL: &str = "https://updates.example.test/portable/{arch}/update-manifest-v1.json";

fn release_keys() -> String {
    format!(
        "{KEY_ID}={}",
        marknexia_update::hex::encode(key_pair(SIGNING_TEST_SEED).pk.as_ref())
    )
}

fn config() -> UpdateConfig {
    let keys = release_keys();
    UpdateConfig::from_values(
        &BuildValues {
            manifest_url: Some(MANIFEST_URL),
            allowed_hosts: Some(HOST),
            publisher: Some("Example Publisher"),
            public_keys: Some(&keys),
        },
        Architecture::X64,
    )
    .expect("configured")
}

#[test]
fn this_build_is_not_configured() {
    // The repository never sets the build-time variables.
    assert_eq!(
        UpdateConfig::embedded(),
        Err(NotConfigured::MissingManifestUrl)
    );
    assert_eq!(
        NotConfigured::MissingManifestUrl.message(),
        "Updates are not configured for this build."
    );
    assert_eq!(
        NOT_CONFIGURED_MESSAGE,
        "Updates are not configured for this build."
    );
}

#[test]
fn every_value_is_required_and_the_test_key_is_never_enough() {
    let keys = release_keys();
    let full = BuildValues {
        manifest_url: Some(MANIFEST_URL),
        allowed_hosts: Some(HOST),
        publisher: Some("Example Publisher"),
        public_keys: Some(&keys),
    };
    let cases = [
        (
            BuildValues {
                manifest_url: Some("  "),
                ..full
            },
            NotConfigured::MissingManifestUrl,
        ),
        (
            BuildValues {
                allowed_hosts: None,
                ..full
            },
            NotConfigured::MissingHosts,
        ),
        (
            BuildValues {
                allowed_hosts: Some(" , "),
                ..full
            },
            NotConfigured::MissingHosts,
        ),
        (
            BuildValues {
                publisher: None,
                ..full
            },
            NotConfigured::MissingPublisher,
        ),
        // No release keys: only the TEST key, refused.
        (
            BuildValues {
                public_keys: None,
                ..full
            },
            NotConfigured::TestKey,
        ),
        (
            BuildValues {
                manifest_url: Some("http://updates.example.test/m.json"),
                ..full
            },
            NotConfigured::InvalidUrl,
        ),
        (
            BuildValues {
                manifest_url: Some("https://other.example.test/m.json"),
                ..full
            },
            NotConfigured::InvalidUrl,
        ),
        (
            BuildValues {
                public_keys: Some("k=00"),
                ..full
            },
            NotConfigured::InvalidKeys(KeyError::InvalidKey),
        ),
    ];
    for (values, expected) in cases {
        assert_eq!(
            UpdateConfig::from_values(&values, Architecture::X64),
            Err(expected),
            "{values:?}"
        );
    }
    // The TEST key under any id, or mixed with a release key, is refused.
    for keys in [
        format!("{TEST_KEY_ID}={TEST_PUBLIC_KEY_HEX}"),
        format!("renamed={TEST_PUBLIC_KEY_HEX}"),
        format!(
            "{},renamed={TEST_PUBLIC_KEY_HEX}",
            format_args!(
                "other={}",
                marknexia_update::hex::encode(key_pair(OTHER_TEST_SEED).pk.as_ref())
            )
        ),
    ] {
        assert_eq!(
            UpdateConfig::from_values(
                &BuildValues {
                    public_keys: Some(&keys),
                    ..full
                },
                Architecture::X64
            ),
            Err(NotConfigured::TestKey),
            "{keys}"
        );
    }
}

#[test]
fn the_manifest_url_is_per_architecture_with_a_detached_signature() {
    let config = config();
    assert_eq!(
        config.manifest_url.to_string(),
        "https://updates.example.test/portable/x64/update-manifest-v1.json"
    );
    assert_eq!(
        config.signature_url.to_string(),
        "https://updates.example.test/portable/x64/update-manifest-v1.json.ed25519"
    );
}

fn serve(document: &marknexia_update::manifest::ManifestDocument) -> FakeFetcher {
    let config = config();
    let (bytes, signature) = signed(document);
    FakeFetcher::default()
        .serve(&config.manifest_url.to_string(), Ok(bytes))
        .serve(&config.signature_url.to_string(), Ok(signature))
}

#[test]
fn a_newer_signed_release_is_available_and_an_older_one_is_up_to_date() {
    let config = config();
    let newer = serve(&document("1.2.0", b"zip"));
    let outcome = check_for_update(
        &config,
        &newer,
        "1.1.0",
        Architecture::X64,
        22631,
        &AtomicBool::new(false),
    )
    .unwrap();
    let CheckOutcome::Available(manifest) = outcome else {
        panic!("available");
    };
    assert_eq!(manifest.version.to_string(), "1.2.0");
    // Both documents are fetched with small bounds.
    let requests = newer.requests.borrow();
    assert_eq!(requests[0].1, 8 * 1024);
    assert_eq!(requests[1].1, 1024);
    // A replayed older or equal manifest is "up to date", never installed.
    for current in ["1.2.0", "1.5.0"] {
        assert_eq!(
            check_for_update(
                &config,
                &serve(&document("1.2.0", b"zip")),
                current,
                Architecture::X64,
                22631,
                &AtomicBool::new(false)
            ),
            Ok(CheckOutcome::UpToDate)
        );
    }
}

#[test]
fn tampered_or_unreachable_update_information_is_an_error() {
    let config = config();
    let document = document("1.2.0", b"zip");
    let (bytes, signature) = signed(&document);
    let mut tampered = bytes.clone();
    tampered[5] ^= 1;
    let fetcher = FakeFetcher::default()
        .serve(&config.manifest_url.to_string(), Ok(tampered))
        .serve(&config.signature_url.to_string(), Ok(signature.clone()));
    let error = check_for_update(
        &config,
        &fetcher,
        "1.1.0",
        Architecture::X64,
        22631,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(error, CheckError::Manifest(_)), "{error:?}");
    assert_eq!(
        error.message(),
        "The update information is not valid or is not signed for this build."
    );
    // Signature missing (404).
    let fetcher = FakeFetcher::default().serve(&config.manifest_url.to_string(), Ok(bytes.clone()));
    assert_eq!(
        check_for_update(
            &config,
            &fetcher,
            "1.1.0",
            Architecture::X64,
            22631,
            &AtomicBool::new(false)
        ),
        Err(CheckError::Download(FetchError::Http(404)))
    );
    // A manifest larger than the bound is refused by the fetcher bound.
    let fetcher = FakeFetcher::default()
        .serve(&config.manifest_url.to_string(), Ok(vec![b' '; 9000]))
        .serve(&config.signature_url.to_string(), Ok(signature));
    assert_eq!(
        check_for_update(
            &config,
            &fetcher,
            "1.1.0",
            Architecture::X64,
            22631,
            &AtomicBool::new(false)
        ),
        Err(CheckError::Download(FetchError::TooLarge))
    );
    // The wrong architecture's manifest.
    let error = check_for_update(
        &config,
        &serve(&document),
        "1.1.0",
        Architecture::Arm64,
        22631,
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(
        error,
        CheckError::Manifest(ManifestError::WrongArchitecture)
    );
}
