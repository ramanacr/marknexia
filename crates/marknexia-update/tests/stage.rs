//! Download and staging: length, hash, Authenticode and PE results,
//! interrupted downloads and staging, and side-by-side payloads. The
//! fetcher and the executable check are fakes; payloads are text files.

mod support;

use std::sync::atomic::AtomicBool;

use marknexia_update::{
    archive::ArchiveLimits,
    layout::Layout,
    manifest::{Architecture, HttpsUrl, Manifest},
    stage::{FetchError, StageError, StageRequest, download_and_stage, stage_bytes},
    version::Version,
};
use support::*;

fn manifest_for(bytes: &[u8], version: &str) -> Manifest {
    let version = Version::parse(version).unwrap();
    let name = format!("Marknexia-v{version}-win-x64.zip");
    Manifest {
        url: HttpsUrl::parse(&format!("https://{HOST}/r/{name}"), &hosts()).unwrap(),
        version,
        architecture: Architecture::X64,
        sha256: marknexia_update::hex::decode(&sha256_hex(bytes)).unwrap(),
        byte_length: bytes.len() as u64,
        minimum_os_build: 0,
        artifact_id: name,
        key_id: KEY_ID.to_owned(),
    }
}

struct Setup {
    _temp: TempDir,
    layout: Layout,
}

fn setup(label: &str) -> Setup {
    let temp = TempDir::new(label);
    let layout = Layout::new(temp.path().join("Updates"));
    Setup {
        _temp: temp,
        layout,
    }
}

fn staging_is_empty(layout: &Layout) -> bool {
    std::fs::read_dir(layout.staging()).map_or(true, |mut entries| entries.next().is_none())
}

#[test]
fn a_verified_payload_is_staged_side_by_side() {
    let s = setup("stage-ok");
    let bytes = text_payload();
    let manifest = manifest_for(&bytes, "1.2.0");
    let fetcher = FakeFetcher::default().serve(&manifest.url.to_string(), Ok(bytes.clone()));
    let platform = FakePlatform::default();
    let policy = text_policy();
    let request = StageRequest {
        manifest: &manifest,
        policy: &policy,
        limits: ArchiveLimits::default(),
        publisher: "Example Publisher",
    };
    let staged = download_and_stage(
        &request,
        &s.layout,
        &fetcher,
        &platform,
        &AtomicBool::new(false),
    )
    .expect("staged");
    assert_eq!(staged.id, "1.2.0-x64");
    assert_eq!(staged.directory, s.layout.payload("1.2.0-x64"));
    assert_eq!(staged.shell, staged.directory.join("shell.txt"));
    assert_eq!(
        std::fs::read(staged.directory.join("docs").join("README.txt")).unwrap(),
        b"read me\n".repeat(64)
    );
    // Bounded by the manifest length.
    assert_eq!(
        fetcher.requests.borrow().as_slice(),
        [(manifest.url.to_string(), bytes.len() as u64)]
    );
    // Both executables were checked on disk, with the x64 machine and the
    // configured publisher, before the move.
    let checked = platform.checked.borrow();
    assert_eq!(checked.len(), 2);
    assert!(
        checked
            .iter()
            .all(|(_, machine, publisher)| *machine == 0x8664 && publisher == "Example Publisher")
    );
    // Root, staging, versions and the pending folder were owner-only.
    let private = platform.private_dirs.borrow();
    assert!(private.contains(&s.layout.root().to_path_buf()));
    assert!(private.contains(&s.layout.staging()));
    assert!(private.contains(&s.layout.versions()));
    assert!(private.iter().any(|dir| {
        dir.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("pending-")
    }));
    assert!(staging_is_empty(&s.layout));
    // No pointer was written: staging never activates.
    assert!(!s.layout.active_file().exists());
}

#[test]
fn a_hash_or_length_mismatch_is_rejected_before_anything_is_written() {
    let s = setup("stage-hash");
    let bytes = text_payload();
    let policy = text_policy();
    let platform = FakePlatform::default();
    let mut tampered = bytes.clone();
    let last = tampered.len() - 30;
    tampered[last] ^= 0xff;
    let manifest = manifest_for(&bytes, "1.2.0");
    let request = StageRequest {
        manifest: &manifest,
        policy: &policy,
        limits: ArchiveLimits::default(),
        publisher: "Example Publisher",
    };
    let cancel = AtomicBool::new(false);
    assert_eq!(
        stage_bytes(&tampered, &request, &s.layout, &platform, &cancel),
        Err(StageError::ChecksumMismatch)
    );
    assert_eq!(
        StageError::ChecksumMismatch.message(),
        "The downloaded update checksum does not match."
    );
    assert_eq!(
        stage_bytes(
            &bytes[..bytes.len() - 1],
            &request,
            &s.layout,
            &platform,
            &cancel
        ),
        Err(StageError::LengthMismatch)
    );
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(
        stage_bytes(&longer, &request, &s.layout, &platform, &cancel),
        Err(StageError::LengthMismatch)
    );
    assert!(!s.layout.root().exists(), "nothing written");
    assert!(platform.checked.borrow().is_empty());
}

#[test]
fn an_authenticode_or_architecture_failure_discards_the_stage() {
    for rejected in ["shell.txt", "loader.txt"] {
        let s = setup("stage-sig");
        let bytes = text_payload();
        let manifest = manifest_for(&bytes, "1.2.0");
        let policy = text_policy();
        let mut platform = FakePlatform::default();
        platform.reject.insert(
            rejected.to_owned(),
            "signed by 'Someone Else', not the release publisher".to_owned(),
        );
        let request = StageRequest {
            manifest: &manifest,
            policy: &policy,
            limits: ArchiveLimits::default(),
            publisher: "Example Publisher",
        };
        let error = stage_bytes(
            &bytes,
            &request,
            &s.layout,
            &platform,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(
            error,
            StageError::Executable {
                name: rejected.to_owned(),
                reason: "signed by 'Someone Else', not the release publisher".to_owned()
            }
        );
        assert!(staging_is_empty(&s.layout), "{rejected}");
        assert!(!s.layout.payload("1.2.0-x64").exists(), "{rejected}");
    }
}

#[test]
fn interrupted_downloads_leave_nothing_behind() {
    for failure in [
        FetchError::Truncated,
        FetchError::TimedOut,
        FetchError::Offline,
        FetchError::Http(503),
        FetchError::TooLarge,
        FetchError::UntrustedLocation,
    ] {
        let s = setup("stage-download");
        let bytes = text_payload();
        let manifest = manifest_for(&bytes, "1.2.0");
        let fetcher = FakeFetcher::default().serve(&manifest.url.to_string(), Err(failure));
        let policy = text_policy();
        let request = StageRequest {
            manifest: &manifest,
            policy: &policy,
            limits: ArchiveLimits::default(),
            publisher: "Example Publisher",
        };
        assert_eq!(
            download_and_stage(
                &request,
                &s.layout,
                &fetcher,
                &FakePlatform::default(),
                &AtomicBool::new(false)
            ),
            Err(StageError::Download(failure))
        );
        assert!(!s.layout.root().exists(), "{failure:?}");
    }
    // A response longer than the manifest length is refused by the bound.
    let s = setup("stage-oversize");
    let bytes = text_payload();
    let manifest = manifest_for(&bytes[..100], "1.2.0");
    let fetcher = FakeFetcher::default().serve(&manifest.url.to_string(), Ok(bytes));
    let policy = text_policy();
    let request = StageRequest {
        manifest: &manifest,
        policy: &policy,
        limits: ArchiveLimits::default(),
        publisher: "p",
    };
    assert_eq!(
        download_and_stage(
            &request,
            &s.layout,
            &fetcher,
            &FakePlatform::default(),
            &AtomicBool::new(false)
        ),
        Err(StageError::Download(FetchError::TooLarge))
    );
    // A cancelled download is reported as a cancellation.
    let cancelled =
        FakeFetcher::default().serve(&manifest.url.to_string(), Err(FetchError::Cancelled));
    assert_eq!(
        download_and_stage(
            &request,
            &s.layout,
            &cancelled,
            &FakePlatform::default(),
            &AtomicBool::new(true)
        ),
        Err(StageError::Cancelled)
    );
}

#[test]
fn interrupted_staging_removes_the_pending_folder_and_keeps_others() {
    let s = setup("stage-interrupt");
    let bytes = text_payload();
    let manifest = manifest_for(&bytes, "1.2.0");
    let policy = text_policy();
    // An earlier, stale stage and an existing payload are kept.
    std::fs::create_dir_all(s.layout.staging().join("pending-stale")).unwrap();
    std::fs::create_dir_all(s.layout.payload("1.1.0-x64")).unwrap();
    std::fs::write(s.layout.payload("1.1.0-x64").join("shell.txt"), b"old").unwrap();
    // The executable check sets the cancel flag: staging is interrupted
    // after extraction, before the move.
    struct CancellingPlatform<'a> {
        inner: FakePlatform,
        cancel: &'a AtomicBool,
    }
    impl marknexia_update::stage::StagePlatform for CancellingPlatform<'_> {
        fn create_private_dir(&self, path: &std::path::Path) -> std::io::Result<()> {
            self.inner.create_private_dir(path)
        }
        fn check_executable(
            &self,
            path: &std::path::Path,
            executable: &marknexia_update::payload::AllowedExecutable,
            machine: u16,
            publisher: &str,
        ) -> Result<(), String> {
            self.cancel
                .store(true, std::sync::atomic::Ordering::Release);
            self.inner
                .check_executable(path, executable, machine, publisher)
        }
    }
    let cancel = AtomicBool::new(false);
    let platform = CancellingPlatform {
        inner: FakePlatform::default(),
        cancel: &cancel,
    };
    let request = StageRequest {
        manifest: &manifest,
        policy: &policy,
        limits: ArchiveLimits::default(),
        publisher: "p",
    };
    assert_eq!(
        stage_bytes(&bytes, &request, &s.layout, &platform, &cancel),
        Err(StageError::Cancelled)
    );
    let left: Vec<String> = std::fs::read_dir(s.layout.staging())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["pending-stale"]);
    assert!(!s.layout.payload("1.2.0-x64").exists());
    assert_eq!(
        std::fs::read(s.layout.payload("1.1.0-x64").join("shell.txt")).unwrap(),
        b"old"
    );
    // A retry then succeeds and replaces nothing else.
    let staged = stage_bytes(
        &bytes,
        &request,
        &s.layout,
        &FakePlatform::default(),
        &AtomicBool::new(false),
    )
    .expect("retry");
    assert!(staged.directory.is_dir());
    assert!(s.layout.payload("1.1.0-x64").is_dir());
}

#[test]
fn a_leftover_folder_for_the_same_version_is_replaced() {
    let s = setup("stage-leftover");
    let bytes = text_payload();
    let manifest = manifest_for(&bytes, "1.2.0");
    let policy = text_policy();
    std::fs::create_dir_all(s.layout.payload("1.2.0-x64")).unwrap();
    std::fs::write(s.layout.payload("1.2.0-x64").join("planted.txt"), b"x").unwrap();
    let request = StageRequest {
        manifest: &manifest,
        policy: &policy,
        limits: ArchiveLimits::default(),
        publisher: "p",
    };
    let staged = stage_bytes(
        &bytes,
        &request,
        &s.layout,
        &FakePlatform::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!staged.directory.join("planted.txt").exists());
    assert!(staged.directory.join("shell.txt").is_file());
}
