//! Headless tests for the WebView2 bootstrapper install.
//!
//! No test downloads or runs the real bootstrapper, and no test writes an
//! executable: the downloader, stager, verifier, runner and runtime probe
//! are in-memory fakes. The real `TempStager` is exercised only with
//! payloads it rejects before writing, and the write helper only with a
//! plain-text file.
#![cfg(windows)]

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use marknexia_webview::{
    bootstrap::{
        ACCEPTED_ORIGINAL_FILENAMES, ACCEPTED_PRODUCT_NAMES, CODE_SIGNING_OID, IdentityError,
        VersionInfo, VersionInfoReader, check_version_info, has_code_signing_usage,
        is_allowed_download_host,
    },
    bootstrap::{
        BOOTSTRAPPER_URL, Bootstrap, BootstrapperDownloader, BoundedBody, DownloadError,
        DownloadLimits, DownloadUrl, InstallError, InstallerRunner, MISSING_MESSAGE,
        OFFLINE_INSTALLER_PAGE, PayloadError, RuntimeSetup, SetupFailure, SetupPhase, SetupStep,
        SignatureError, SignatureVerifier, StageError, StagedFile, Stager, TempStager, UrlError,
        classify_exit_code, classify_network_error, create_fresh_folder, failure_message,
        is_microsoft_signer, validate_payload, write_new_file,
    },
    bootstrap_native::DownloadAbort,
    environment::BrowserVersionProbe,
};

// ---------------------------------------------------------------------------
// URL validation.
// ---------------------------------------------------------------------------

#[test]
fn the_official_links_are_valid_microsoft_https_urls() {
    let url = DownloadUrl::parse(BOOTSTRAPPER_URL).expect("fwlink");
    assert_eq!(url.host(), "go.microsoft.com");
    assert_eq!(url.path_and_query(), "/fwlink/p/?LinkId=2124703");
    assert_eq!(url.to_string(), BOOTSTRAPPER_URL);
    // The offline page is only opened in the browser, never downloaded.
    assert!(OFFLINE_INSTALLER_PAGE.starts_with("https://developer.microsoft.com/"));
}

#[test]
fn only_the_fwlink_host_and_the_edge_download_cdn_are_allowed() {
    for host in [
        "go.microsoft.com",
        "msedge.sf.dl.delivery.mp.microsoft.com",
        "msedge.sb.dl.delivery.mp.microsoft.com",
        "msedge.sf.tlu.dl.delivery.mp.microsoft.com",
        "msedge.sb.tlu.dl.delivery.mp.microsoft.com",
    ] {
        assert!(is_allowed_download_host(host), "{host}");
    }
    for host in [
        "microsoft.com",
        "download.microsoft.com",
        "developer.microsoft.com",
        "dl.delivery.mp.microsoft.com",
        ".dl.delivery.mp.microsoft.com",
        "msedge.sf.dl.delivery.mp.microsoft.com.example.com",
        "evil-dl.delivery.mp.microsoft.com",
        "fwlink.go.microsoft.com",
    ] {
        assert!(!is_allowed_download_host(host), "{host}");
    }
}

#[test]
fn urls_must_be_https_to_microsoft_on_the_default_port() {
    let cases = [
        ("http://go.microsoft.com/fwlink", UrlError::NotHttps),
        ("ftp://go.microsoft.com/x", UrlError::NotHttps),
        ("go.microsoft.com/fwlink", UrlError::NotHttps),
        ("https://example.com/setup.exe", UrlError::UntrustedHost),
        (
            "https://microsoft.com.example.com/x",
            UrlError::UntrustedHost,
        ),
        ("https://evilmicrosoft.com/x", UrlError::UntrustedHost),
        ("https://microsoft.com/x", UrlError::UntrustedHost),
        ("https://download.microsoft.com/x", UrlError::UntrustedHost),
        ("https://user@go.microsoft.com/x", UrlError::UserInfo),
        ("https://go.microsoft.com@example.com/x", UrlError::UserInfo),
        ("https://go.microsoft.com:8443/x", UrlError::Port),
        ("https://go.microsoft.com:/x", UrlError::Port),
        ("https://go.microsoft.com/a b", UrlError::Malformed),
        ("https://go.microsoft.com/a\\b", UrlError::Malformed),
        ("https://go.microsoft.com/\u{e9}", UrlError::Malformed),
        ("https://.microsoft.com/x", UrlError::Malformed),
        ("https://go..microsoft.com/x", UrlError::Malformed),
        ("https:///x", UrlError::Malformed),
        ("https://go_x.microsoft.com/x", UrlError::Malformed),
    ];
    for (url, expected) in cases {
        assert_eq!(DownloadUrl::parse(url), Err(expected), "{url}");
    }
    let long = format!("https://go.microsoft.com/{}", "a".repeat(2100));
    assert_eq!(DownloadUrl::parse(&long), Err(UrlError::TooLong));
}

#[test]
fn an_exact_host_list_applies_to_the_url_and_every_redirect() {
    let hosts = ["updates.example.test".to_owned()];
    let start = DownloadUrl::parse_with_hosts("https://Updates.Example.TEST/m.json", &hosts)
        .expect("listed host");
    assert_eq!(start.host(), "updates.example.test");
    assert_eq!(
        start.resolve("/other.json").expect("same host").host(),
        "updates.example.test"
    );
    for (url, expected) in [
        ("https://go.microsoft.com/x", UrlError::UntrustedHost),
        (
            "https://cdn.updates.example.test/x",
            UrlError::UntrustedHost,
        ),
        (
            "https://updates.example.test.evil/x",
            UrlError::UntrustedHost,
        ),
        ("http://updates.example.test/x", UrlError::NotHttps),
        ("https://updates.example.test:8443/x", UrlError::Port),
    ] {
        assert_eq!(
            DownloadUrl::parse_with_hosts(url, &hosts),
            Err(expected),
            "{url}"
        );
    }
    // A redirect off the list fails, even to a bootstrapper host.
    for location in [
        "https://go.microsoft.com/x",
        "//msedge.sf.dl.delivery.mp.microsoft.com/x",
        "https://example.com/x",
    ] {
        assert_eq!(
            start.resolve(location),
            Err(UrlError::UntrustedHost),
            "{location}"
        );
    }
    // An empty list allows nothing.
    assert_eq!(
        DownloadUrl::parse_with_hosts("https://updates.example.test/x", &[]),
        Err(UrlError::UntrustedHost)
    );
    // The bootstrapper policy is unchanged and does not accept listed hosts.
    assert_eq!(
        DownloadUrl::parse("https://updates.example.test/x"),
        Err(UrlError::UntrustedHost)
    );
}

#[test]
fn accepted_urls_are_normalized() {
    let url = DownloadUrl::parse("HTTPS://Go.Microsoft.COM:443?x=1#frag").expect("valid");
    assert_eq!(url.host(), "go.microsoft.com");
    assert_eq!(url.path_and_query(), "/?x=1");
    let bare = DownloadUrl::parse("https://go.microsoft.com").expect("valid");
    assert_eq!(bare.path_and_query(), "/");
}

#[test]
fn redirects_are_validated_like_the_first_url() {
    let start = DownloadUrl::parse(BOOTSTRAPPER_URL).expect("fwlink");
    let absolute = start
        .resolve(
            "https://msedge.sf.dl.delivery.mp.microsoft.com/files/a/MicrosoftEdgeWebview2Setup.exe",
        )
        .expect("absolute");
    assert_eq!(absolute.host(), "msedge.sf.dl.delivery.mp.microsoft.com");
    let relative = start
        .resolve("/download/setup.exe?v=2")
        .expect("host-relative");
    assert_eq!(relative.host(), "go.microsoft.com");
    assert_eq!(relative.path_and_query(), "/download/setup.exe?v=2");
    let scheme_relative = start
        .resolve("//msedge.sb.dl.delivery.mp.microsoft.com/setup.exe")
        .expect("scheme-relative");
    assert_eq!(
        scheme_relative.host(),
        "msedge.sb.dl.delivery.mp.microsoft.com"
    );

    assert_eq!(
        start.resolve("http://msedge.sf.dl.delivery.mp.microsoft.com/setup.exe"),
        Err(UrlError::NotHttps)
    );
    assert_eq!(
        start.resolve("https://download.microsoft.com/setup.exe"),
        Err(UrlError::UntrustedHost)
    );
    assert_eq!(
        start.resolve("https://attacker.example/setup.exe"),
        Err(UrlError::UntrustedHost)
    );
    assert_eq!(
        start.resolve("//attacker.example/setup.exe"),
        Err(UrlError::UntrustedHost)
    );
    assert_eq!(start.resolve("setup.exe"), Err(UrlError::Malformed));
    assert_eq!(start.resolve("/a b"), Err(UrlError::Malformed));
    assert_eq!(start.resolve(""), Err(UrlError::Malformed));
}

// ---------------------------------------------------------------------------
// Size bounds and error mapping.
// ---------------------------------------------------------------------------

#[test]
fn a_declared_length_above_the_limit_is_refused_before_reading() {
    assert!(matches!(
        BoundedBody::new(100, Some(101)),
        Err(DownloadError::TooLarge)
    ));
    assert!(BoundedBody::new(100, Some(100)).is_ok());
    assert!(BoundedBody::new(100, None).is_ok());
}

#[test]
fn the_body_never_grows_past_its_limit() {
    let mut body = BoundedBody::new(10, None).expect("body");
    body.push(b"hello").expect("first chunk");
    body.push(b"world").expect("exactly at the limit");
    assert_eq!(body.push(b"!"), Err(DownloadError::TooLarge));
    assert_eq!(body.len(), 10);
    assert_eq!(body.finish().expect("finish"), b"helloworld");
}

#[test]
fn the_body_must_match_its_declared_length() {
    let mut short = BoundedBody::new(100, Some(6)).expect("body");
    short.push(b"abc").expect("chunk");
    assert_eq!(short.finish(), Err(DownloadError::Truncated));
    let mut long = BoundedBody::new(100, Some(2)).expect("body");
    assert_eq!(long.push(b"abc"), Err(DownloadError::TooLarge));
}

#[test]
fn network_errors_separate_offline_timeout_and_cancel() {
    assert_eq!(classify_network_error(12007), DownloadError::Offline(12007));
    assert_eq!(classify_network_error(12029), DownloadError::Offline(12029));
    assert_eq!(classify_network_error(12030), DownloadError::Offline(12030));
    assert_eq!(classify_network_error(12002), DownloadError::TimedOut);
    assert_eq!(classify_network_error(12017), DownloadError::Cancelled);
    assert_eq!(classify_network_error(12175), DownloadError::Network(12175));
}

#[test]
fn installer_exit_codes_map_to_success_cancel_or_failure() {
    assert_eq!(classify_exit_code(0), Ok(()));
    assert_eq!(classify_exit_code(1223), Err(InstallError::Cancelled));
    assert_eq!(
        classify_exit_code(0x8007_04C7),
        Err(InstallError::Cancelled)
    );
    assert_eq!(
        classify_exit_code(0x8004_0C01),
        Err(InstallError::ExitCode(0x8004_0C01))
    );
}

#[test]
fn only_a_microsoft_leaf_with_a_microsoft_root_is_accepted() {
    assert!(is_microsoft_signer(
        "Microsoft Corporation",
        "Microsoft Root Certificate Authority 2011"
    ));
    assert!(is_microsoft_signer(
        "Microsoft Corporation",
        "Microsoft Root Certificate Authority 2010"
    ));
    assert!(!is_microsoft_signer(
        "Microsoft Corporation",
        "Contoso Root CA"
    ));
    assert!(!is_microsoft_signer(
        "Contoso Ltd",
        "Microsoft Root Certificate Authority 2011"
    ));
    assert!(!is_microsoft_signer(
        "Microsoft Corporation Fake",
        "Microsoft Root Certificate Authority 2011"
    ));
    assert!(!is_microsoft_signer("", ""));
}

#[test]
fn the_leaf_must_list_code_signing_explicitly() {
    let usages = |list: &[&str]| {
        list.iter()
            .map(|usage| (*usage).to_owned())
            .collect::<Vec<_>>()
    };
    assert!(has_code_signing_usage(&usages(&[CODE_SIGNING_OID])));
    assert!(has_code_signing_usage(&usages(&[
        "1.3.6.1.4.1.311.10.3.6",
        CODE_SIGNING_OID
    ])));
    // No extension (valid for every usage), or other usages only.
    assert!(!has_code_signing_usage(&[]));
    assert!(!has_code_signing_usage(&usages(&["1.3.6.1.5.5.7.3.1"])));
    assert!(!has_code_signing_usage(&usages(&["1.3.6.1.5.5.7.3.30"])));
}

#[test]
fn the_version_resource_must_name_the_bootstrapper() {
    assert_eq!(check_version_info(&bootstrapper_info()), Ok(()));
    for name in ACCEPTED_ORIGINAL_FILENAMES {
        for product in ACCEPTED_PRODUCT_NAMES {
            let info = VersionInfo {
                original_filename: Some(format!(" {} ", name.to_ascii_lowercase())),
                product_name: Some(product.to_ascii_uppercase()),
                ..bootstrapper_info()
            };
            assert_eq!(check_version_info(&info), Ok(()), "{name} {product}");
        }
    }
    let wrong_company = VersionInfo {
        company_name: Some("Contoso Ltd".to_owned()),
        ..bootstrapper_info()
    };
    assert_eq!(
        check_version_info(&wrong_company),
        Err(IdentityError::UnexpectedCompany(Some(
            "Contoso Ltd".to_owned()
        )))
    );
    let other_tool = VersionInfo {
        original_filename: Some("MicrosoftEdgeSetup.exe".to_owned()),
        ..bootstrapper_info()
    };
    assert!(matches!(
        check_version_info(&other_tool),
        Err(IdentityError::UnexpectedOriginalFilename(_))
    ));
    let other_product = VersionInfo {
        product_name: Some("Microsoft Teams".to_owned()),
        ..bootstrapper_info()
    };
    assert!(matches!(
        check_version_info(&other_product),
        Err(IdentityError::UnexpectedProduct(_))
    ));
    assert!(check_version_info(&VersionInfo::default()).is_err());
}

#[test]
fn an_abort_before_any_request_is_harmless_and_repeatable() {
    // No handle is registered, so nothing is closed; no network is used.
    let abort = DownloadAbort::new();
    abort.abort();
    abort.clone().abort();
}

// ---------------------------------------------------------------------------
// Payload validation and the real stager (non-executable bytes only).
// ---------------------------------------------------------------------------

/// A plain-text payload: never a Windows executable.
fn text_payload(length: usize) -> Vec<u8> {
    b"Marknexia test payload, plain text. "
        .iter()
        .copied()
        .cycle()
        .take(length)
        .collect()
}

#[test]
fn payloads_outside_the_bounds_or_not_executable_are_rejected() {
    let limits = DownloadLimits::default();
    assert_eq!(
        validate_payload(&text_payload(1024), &limits),
        Err(PayloadError::TooSmall)
    );
    assert_eq!(
        validate_payload(&text_payload(128 * 1024), &limits),
        Err(PayloadError::NotExecutable)
    );
    assert_eq!(
        validate_payload(&text_payload(9 * 1024 * 1024), &limits),
        Err(PayloadError::TooLarge)
    );
    assert_eq!(validate_payload(&[], &limits), Err(PayloadError::TooSmall));
}

/// An empty, uniquely named folder for one test, removed on drop.
struct TestFolder(PathBuf);

impl TestFolder {
    fn new() -> Self {
        let folder = create_fresh_folder(&std::env::temp_dir()).expect("test folder");
        Self(folder)
    }

    fn entries(&self) -> usize {
        std::fs::read_dir(&self.0).expect("list").count()
    }
}

impl Drop for TestFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_real_stager_rejects_non_executable_bytes_before_writing() {
    let parent = TestFolder::new();
    let stager = TempStager::in_folder(parent.0.clone(), DownloadLimits::default());
    for payload in [text_payload(16), text_payload(128 * 1024)] {
        let result = stager.stage(&payload);
        assert!(matches!(result, Err(StageError::Payload(_))), "{result:?}");
    }
    assert_eq!(parent.entries(), 0, "nothing may reach the disk");
}

#[test]
fn fresh_folders_have_a_protected_owner_only_dacl() {
    let parent = TestFolder::new();
    let folder = create_fresh_folder(&parent.0).expect("folder");
    let output = std::process::Command::new("icacls")
        .arg(&folder)
        .output()
        .expect("icacls");
    let listing = String::from_utf8_lossy(&output.stdout);
    let entries: Vec<&str> = listing
        .lines()
        .map(str::trim)
        .filter(|line| line.contains(":("))
        .collect();
    // One ACE, for OWNER RIGHTS, and no inherited ACE.
    assert_eq!(entries.len(), 1, "{listing}");
    assert!(entries[0].contains("OWNER RIGHTS"), "{listing}");
    assert!(!listing.contains("(I)"), "{listing}");
}

#[test]
fn fresh_folders_are_new_and_distinct() {
    let parent = TestFolder::new();
    let first = create_fresh_folder(&parent.0).expect("first");
    let second = create_fresh_folder(&parent.0).expect("second");
    assert_ne!(first, second);
    assert!(first.starts_with(&parent.0) && first.is_dir() && second.is_dir());
    assert_eq!(parent.entries(), 2);
}

#[test]
fn the_write_path_creates_a_new_file_and_never_overwrites() {
    let parent = TestFolder::new();
    let text = b"plain text, not an executable\n";
    let path = write_new_file(&parent.0, "payload.txt", text).expect("write");
    assert_eq!(std::fs::read(&path).expect("read"), text);
    let again = write_new_file(&parent.0, "payload.txt", b"other");
    assert_eq!(
        again.map_err(|error| error.kind()),
        Err(std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(&path).expect("read"), text);
    // The locked handle denies writers while it is open.
    let lock = marknexia_webview::bootstrap::open_locked(&path).expect("lock");
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
    drop(lock);
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_ok());
}

// ---------------------------------------------------------------------------
// One attempt, with in-memory fakes.
// ---------------------------------------------------------------------------

/// Returns a fixed result, or sets the cancel flag first.
struct FakeDownloader {
    result: Result<Vec<u8>, DownloadError>,
    calls: Cell<u32>,
    cancel_during: bool,
}

impl FakeDownloader {
    fn returning(result: Result<Vec<u8>, DownloadError>) -> Self {
        Self {
            result,
            calls: Cell::new(0),
            cancel_during: false,
        }
    }
}

impl BootstrapperDownloader for FakeDownloader {
    fn download(
        &self,
        url: &DownloadUrl,
        _limits: &DownloadLimits,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, DownloadError> {
        assert_eq!(url.to_string(), BOOTSTRAPPER_URL);
        self.calls.set(self.calls.get() + 1);
        if self.cancel_during {
            cancel.store(true, Ordering::Release);
        }
        self.result.clone()
    }
}

/// Stages in memory: records the bytes, never touches the disk.
#[derive(Default)]
struct MemoryStager {
    staged: RefCell<Vec<Vec<u8>>>,
    discarded: Cell<u32>,
    fail: Option<StageError>,
}

impl Stager for MemoryStager {
    fn stage(&self, bytes: &[u8]) -> Result<StagedFile, StageError> {
        if let Some(error) = self.fail {
            return Err(error);
        }
        self.staged.borrow_mut().push(bytes.to_vec());
        Ok(StagedFile::unlocked(
            PathBuf::from(r"Z:\memory\MicrosoftEdgeWebview2Setup.exe"),
            PathBuf::from(r"Z:\memory"),
        ))
    }

    fn discard(&self, _file: StagedFile) {
        self.discarded.set(self.discarded.get() + 1);
    }
}

struct FakeVerifier {
    result: Result<(), SignatureError>,
    calls: Cell<u32>,
    cancel: Option<&'static AtomicBool>,
}

impl SignatureVerifier for FakeVerifier {
    fn verify(&self, _file: &StagedFile) -> Result<(), SignatureError> {
        self.calls.set(self.calls.get() + 1);
        if let Some(cancel) = self.cancel {
            cancel.store(true, Ordering::Release);
        }
        self.result.clone()
    }
}

struct FakeRunner {
    result: Result<(), InstallError>,
    calls: Cell<u32>,
    /// What the runtime probe reports after this runner ran.
    installs: Option<&'static str>,
    installed: Cell<Option<&'static str>>,
}

impl InstallerRunner for FakeRunner {
    fn run(&self, _file: &StagedFile) -> Result<(), InstallError> {
        self.calls.set(self.calls.get() + 1);
        self.installed.set(self.installs);
        self.result
    }
}

/// Reports the version the fake runner "installed", else missing.
struct FakeProbe<'a> {
    runner: &'a FakeRunner,
    calls: Cell<u32>,
}

impl BrowserVersionProbe for FakeProbe<'_> {
    fn query_version(&self) -> Result<String, i32> {
        self.calls.set(self.calls.get() + 1);
        match self.runner.installed.get() {
            Some(version) => Ok(version.to_owned()),
            None => Err(0x8007_0002_u32 as i32),
        }
    }
}

/// In-memory bytes for the fake pipeline. They never reach a file: the
/// memory stager only records them.
fn downloaded_bytes() -> Vec<u8> {
    text_payload(256)
}

/// Returns a fixed version resource; never reads a file.
struct FakeIdentity {
    result: Result<VersionInfo, IdentityError>,
    calls: Cell<u32>,
}

impl VersionInfoReader for FakeIdentity {
    fn read(&self, _file: &StagedFile) -> Result<VersionInfo, IdentityError> {
        self.calls.set(self.calls.get() + 1);
        self.result.clone()
    }
}

fn bootstrapper_info() -> VersionInfo {
    VersionInfo {
        company_name: Some("Microsoft Corporation".to_owned()),
        product_name: Some("Microsoft Edge Update".to_owned()),
        original_filename: Some("MicrosoftEdgeWebview2Setup.exe".to_owned()),
        file_version: Some("1.3.195.61".to_owned()),
    }
}

struct Harness {
    downloader: FakeDownloader,
    stager: MemoryStager,
    verifier: FakeVerifier,
    identity: FakeIdentity,
    runner: FakeRunner,
}

impl Harness {
    fn new() -> Self {
        Self {
            downloader: FakeDownloader::returning(Ok(downloaded_bytes())),
            stager: MemoryStager::default(),
            verifier: FakeVerifier {
                result: Ok(()),
                calls: Cell::new(0),
                cancel: None,
            },
            identity: FakeIdentity {
                result: Ok(bootstrapper_info()),
                calls: Cell::new(0),
            },
            runner: FakeRunner {
                result: Ok(()),
                calls: Cell::new(0),
                installs: Some("153.0.4234.32"),
                installed: Cell::new(None),
            },
        }
    }

    fn run(&self, cancel: &AtomicBool) -> (Result<String, SetupFailure>, Vec<SetupStep>, u32) {
        let probe = FakeProbe {
            runner: &self.runner,
            calls: Cell::new(0),
        };
        let bootstrap = Bootstrap {
            downloader: &self.downloader,
            stager: &self.stager,
            verifier: &self.verifier,
            identity: &self.identity,
            runner: &self.runner,
            probe: &probe,
            limits: DownloadLimits::default(),
        };
        let mut steps = Vec::new();
        let result = bootstrap.run(cancel, &mut |step| steps.push(step));
        (result, steps, probe.calls.get())
    }
}

#[test]
fn a_successful_attempt_downloads_verifies_installs_and_detects_once() {
    let harness = Harness::new();
    let (result, steps, probes) = harness.run(&AtomicBool::new(false));
    assert_eq!(result, Ok("153.0.4234.32".to_owned()));
    assert_eq!(
        steps,
        [
            SetupStep::Downloading,
            SetupStep::Verifying,
            SetupStep::Installing,
            SetupStep::Detecting
        ]
    );
    assert_eq!(harness.downloader.calls.get(), 1);
    assert_eq!(
        harness.stager.staged.borrow().as_slice(),
        [downloaded_bytes()]
    );
    assert_eq!(harness.verifier.calls.get(), 1);
    assert_eq!(harness.runner.calls.get(), 1);
    assert_eq!(harness.stager.discarded.get(), 1);
    assert_eq!(probes, 1);
}

#[test]
fn offline_and_timeouts_fail_without_staging_or_retrying() {
    for error in [
        DownloadError::Offline(12007),
        DownloadError::Offline(12029),
        DownloadError::TimedOut,
    ] {
        let mut harness = Harness::new();
        harness.downloader = FakeDownloader::returning(Err(error));
        let (result, steps, probes) = harness.run(&AtomicBool::new(false));
        assert_eq!(result, Err(SetupFailure::Offline));
        assert_eq!(steps, [SetupStep::Downloading]);
        assert_eq!(harness.downloader.calls.get(), 1, "no retry loop");
        assert!(harness.stager.staged.borrow().is_empty());
        assert_eq!(harness.runner.calls.get(), 0);
        assert_eq!(probes, 0);
    }
}

#[test]
fn a_failed_download_never_runs_anything() {
    for error in [
        DownloadError::Http(404),
        DownloadError::TooLarge,
        DownloadError::Truncated,
        DownloadError::TooManyRedirects,
        DownloadError::Url(UrlError::UntrustedHost),
        DownloadError::Network(12175),
    ] {
        let mut harness = Harness::new();
        harness.downloader = FakeDownloader::returning(Err(error));
        let (result, _, _) = harness.run(&AtomicBool::new(false));
        assert_eq!(result, Err(SetupFailure::Download(error)));
        assert_eq!(harness.verifier.calls.get(), 0);
        assert_eq!(harness.runner.calls.get(), 0);
    }
}

#[test]
fn a_payload_the_stager_rejects_never_reaches_verification() {
    let mut harness = Harness::new();
    harness.stager.fail = Some(StageError::Payload(PayloadError::NotExecutable));
    let (result, _, _) = harness.run(&AtomicBool::new(false));
    assert_eq!(
        result,
        Err(SetupFailure::Payload(PayloadError::NotExecutable))
    );
    assert_eq!(harness.verifier.calls.get(), 0);
    assert_eq!(harness.runner.calls.get(), 0);
}

#[test]
fn a_failed_signature_discards_the_file_and_never_runs_it() {
    for error in [
        SignatureError::Untrusted(0x800B_0100_u32 as i32),
        SignatureError::NoSigner,
        SignatureError::NotMicrosoftRoot(0x800B_0109),
        SignatureError::NoCodeSigningUsage,
        SignatureError::NotMicrosoft {
            signer: "Contoso Ltd".to_owned(),
        },
    ] {
        let mut harness = Harness::new();
        harness.verifier.result = Err(error.clone());
        let (result, steps, probes) = harness.run(&AtomicBool::new(false));
        assert_eq!(result, Err(SetupFailure::Signature(error)));
        assert_eq!(steps, [SetupStep::Downloading, SetupStep::Verifying]);
        assert_eq!(harness.runner.calls.get(), 0);
        assert_eq!(harness.stager.discarded.get(), 1);
        assert_eq!(probes, 0);
    }
}

#[test]
fn a_signed_file_that_is_not_the_bootstrapper_is_never_run() {
    for error in [
        IdentityError::Unreadable(1813),
        IdentityError::UnexpectedOriginalFilename(Some("other.exe".to_owned())),
    ] {
        let mut harness = Harness::new();
        harness.identity.result = Err(error.clone());
        let (result, steps, probes) = harness.run(&AtomicBool::new(false));
        assert_eq!(result, Err(SetupFailure::Identity(error)));
        assert_eq!(steps, [SetupStep::Downloading, SetupStep::Verifying]);
        assert_eq!(harness.verifier.calls.get(), 1);
        assert_eq!(harness.runner.calls.get(), 0);
        assert_eq!(harness.stager.discarded.get(), 1);
        assert_eq!(probes, 0);
    }
    // A rejected identity from a good reader: the check runs on its values.
    let mut harness = Harness::new();
    harness.identity.result = Ok(VersionInfo {
        company_name: Some("Contoso Ltd".to_owned()),
        ..bootstrapper_info()
    });
    let (result, _, _) = harness.run(&AtomicBool::new(false));
    assert!(matches!(
        result,
        Err(SetupFailure::Identity(IdentityError::UnexpectedCompany(_)))
    ));
    assert_eq!(harness.runner.calls.get(), 0);
}

#[test]
fn a_failed_signature_never_reads_the_version_resource() {
    let mut harness = Harness::new();
    harness.verifier.result = Err(SignatureError::NotMicrosoftRoot(0x800B_0109));
    let (result, _, _) = harness.run(&AtomicBool::new(false));
    assert_eq!(
        result,
        Err(SetupFailure::Signature(SignatureError::NotMicrosoftRoot(
            0x800B_0109
        )))
    );
    assert_eq!(harness.identity.calls.get(), 0);
    assert_eq!(harness.runner.calls.get(), 0);
}

#[test]
fn cancelling_before_or_during_download_stops_early() {
    let harness = Harness::new();
    let (result, steps, _) = harness.run(&AtomicBool::new(true));
    assert_eq!(result, Err(SetupFailure::Cancelled));
    assert!(steps.is_empty());
    assert_eq!(harness.downloader.calls.get(), 0);

    let mut harness = Harness::new();
    harness.downloader.cancel_during = true;
    let (result, _, _) = harness.run(&AtomicBool::new(false));
    assert_eq!(result, Err(SetupFailure::Cancelled));
    assert!(harness.stager.staged.borrow().is_empty());

    let mut harness = Harness::new();
    harness.downloader = FakeDownloader::returning(Err(DownloadError::Cancelled));
    let (result, _, _) = harness.run(&AtomicBool::new(false));
    assert_eq!(result, Err(SetupFailure::Cancelled));
}

#[test]
fn cancelling_during_verification_never_runs_the_installer() {
    static CANCEL: AtomicBool = AtomicBool::new(false);
    CANCEL.store(false, Ordering::Release);
    let mut harness = Harness::new();
    harness.verifier.cancel = Some(&CANCEL);
    let (result, _, probes) = harness.run(&CANCEL);
    assert_eq!(result, Err(SetupFailure::Cancelled));
    assert_eq!(harness.runner.calls.get(), 0);
    assert_eq!(harness.stager.discarded.get(), 1);
    assert_eq!(probes, 0);
}

#[test]
fn installer_failures_are_reported_after_one_detection() {
    let cases = [
        (InstallError::Cancelled, SetupFailure::Cancelled),
        (
            InstallError::ExitCode(0x8004_0C01),
            SetupFailure::Install(InstallError::ExitCode(0x8004_0C01)),
        ),
        (
            InstallError::ElevationRequired,
            SetupFailure::Install(InstallError::ElevationRequired),
        ),
        (
            InstallError::StartFailed(2),
            SetupFailure::Install(InstallError::StartFailed(2)),
        ),
        (
            InstallError::TimedOut,
            SetupFailure::Install(InstallError::TimedOut),
        ),
    ];
    for (error, expected) in cases {
        let mut harness = Harness::new();
        harness.runner.result = Err(error);
        harness.runner.installs = None;
        let (result, steps, probes) = harness.run(&AtomicBool::new(false));
        assert_eq!(result, Err(expected));
        assert_eq!(steps.last(), Some(&SetupStep::Detecting));
        assert_eq!(probes, 1, "detection runs once, never in a loop");
        assert_eq!(harness.stager.discarded.get(), 1);
    }
}

#[test]
fn a_runtime_found_after_a_failed_exit_code_counts_as_installed() {
    let mut harness = Harness::new();
    harness.runner.result = Err(InstallError::ExitCode(1));
    let (result, _, _) = harness.run(&AtomicBool::new(false));
    assert_eq!(result, Ok("153.0.4234.32".to_owned()));
}

#[test]
fn a_successful_installer_without_a_runtime_reports_still_missing() {
    let mut harness = Harness::new();
    harness.runner.installs = None;
    let (result, _, probes) = harness.run(&AtomicBool::new(false));
    assert_eq!(result, Err(SetupFailure::StillMissing));
    assert_eq!(probes, 1);
    // A preview channel or an empty version is not the shared runtime.
    for version in ["153.0.4234.32 Canary", ""] {
        let mut harness = Harness::new();
        harness.runner.installs = Some(version);
        let (result, _, _) = harness.run(&AtomicBool::new(false));
        assert_eq!(result, Err(SetupFailure::StillMissing), "{version:?}");
    }
}

// ---------------------------------------------------------------------------
// The shell's state machine.
// ---------------------------------------------------------------------------

#[test]
fn the_missing_state_offers_install_and_the_offline_link_only() {
    let setup = RuntimeSetup::new();
    assert_eq!(setup.phase(), &SetupPhase::Missing);
    assert_eq!(setup.attempt(), 0, "nothing starts by itself");
    assert!(setup.can_install());
    assert!(!setup.can_cancel());
    assert!(setup.shows_offline_link());
    assert_eq!(setup.message(), MISSING_MESSAGE);
}

#[test]
fn an_attempt_starts_only_on_request_and_never_twice() {
    let mut setup = RuntimeSetup::new();
    assert_eq!(setup.begin_install(), Some(1));
    assert_eq!(setup.phase(), &SetupPhase::Working(SetupStep::Downloading));
    assert_eq!(setup.begin_install(), None, "one attempt at a time");
    assert!(!setup.can_install());
    assert!(setup.can_cancel());
}

#[test]
fn progress_moves_forward_and_ignores_stale_attempts() {
    let mut setup = RuntimeSetup::new();
    let attempt = setup.begin_install().expect("start");
    assert!(setup.progress(attempt, SetupStep::Verifying));
    assert!(
        !setup.progress(attempt, SetupStep::Downloading),
        "no going back"
    );
    assert!(
        !setup.progress(attempt + 1, SetupStep::Installing),
        "unknown attempt"
    );
    assert!(setup.progress(attempt, SetupStep::Installing));
    assert!(!setup.can_cancel(), "the installer window owns cancel now");
    assert!(!setup.request_cancel());
    assert!(setup.progress(attempt, SetupStep::Detecting));
}

#[test]
fn every_failure_keeps_the_shell_in_a_retryable_state_without_retrying() {
    let failures = [
        SetupFailure::Offline,
        SetupFailure::Download(DownloadError::Http(500)),
        SetupFailure::Payload(PayloadError::NotExecutable),
        SetupFailure::Staging(std::io::ErrorKind::PermissionDenied),
        SetupFailure::Signature(SignatureError::NoSigner),
        SetupFailure::Identity(IdentityError::Unreadable(1813)),
        SetupFailure::Cancelled,
        SetupFailure::Install(InstallError::ExitCode(1)),
        SetupFailure::Install(InstallError::ElevationRequired),
        SetupFailure::Install(InstallError::TimedOut),
        SetupFailure::StillMissing,
    ];
    for failure in failures {
        let mut setup = RuntimeSetup::new();
        let attempt = setup.begin_install().expect("start");
        assert!(setup.finish(attempt, Err(failure.clone())));
        assert_eq!(setup.phase(), &SetupPhase::Failed(failure.clone()));
        assert_eq!(setup.attempt(), attempt, "no automatic retry");
        assert!(!setup.is_working());
        assert!(setup.can_install(), "the user may try again");
        assert!(setup.shows_offline_link(), "{failure:?}");
        let message = setup.message();
        assert_eq!(message, failure_message(&failure));
        assert!(
            message.contains("offline installer") || message.contains("Restart"),
            "{message}"
        );
        // A second request is a new, explicit attempt.
        assert_eq!(setup.begin_install(), Some(attempt + 1));
    }
}

#[test]
fn a_signature_failure_says_the_file_was_not_run() {
    let message = failure_message(&SetupFailure::Signature(SignatureError::NotMicrosoft {
        signer: "Contoso Ltd".to_owned(),
    }));
    assert!(message.contains("not run"), "{message}");
    assert!(message.contains("offline installer"), "{message}");
}

#[test]
fn stale_results_are_ignored() {
    let mut setup = RuntimeSetup::new();
    let first = setup.begin_install().expect("start");
    assert!(setup.finish(first, Err(SetupFailure::Offline)));
    let second = setup.begin_install().expect("again");
    assert!(!setup.finish(first, Ok("1.0".to_owned())), "stale attempt");
    assert_eq!(setup.phase(), &SetupPhase::Working(SetupStep::Downloading));
    assert!(setup.finish(second, Ok("153.0.4234.32".to_owned())));
    assert!(
        !setup.finish(second, Err(SetupFailure::Offline)),
        "already finished"
    );
}

#[test]
fn an_installed_runtime_ends_the_missing_state() {
    let mut setup = RuntimeSetup::new();
    let attempt = setup.begin_install().expect("start");
    assert!(setup.finish(attempt, Ok("153.0.4234.32".to_owned())));
    assert_eq!(
        setup.phase(),
        &SetupPhase::Installed {
            version: "153.0.4234.32".to_owned()
        }
    );
    assert!(!setup.can_install());
    assert!(!setup.shows_offline_link());
    assert_eq!(setup.begin_install(), None);
    assert!(setup.message().contains("153.0.4234.32"));
}

#[test]
fn cancel_is_requested_once_and_shown() {
    let mut setup = RuntimeSetup::new();
    assert!(!setup.request_cancel(), "nothing to cancel");
    let attempt = setup.begin_install().expect("start");
    assert!(setup.request_cancel());
    assert!(!setup.request_cancel(), "already requested");
    assert_eq!(setup.message(), "Cancelling…");
    assert!(setup.finish(attempt, Err(SetupFailure::Cancelled)));
    assert!(setup.can_install());
}
