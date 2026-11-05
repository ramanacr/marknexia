use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

use marknexia_core::contracts::AppTheme;
use marknexia_rendering::{PageIdentity, RenderError};
use marknexia_win32::documents::{
    OpenError, RenderJob, RenderOutcome, RenderPool, RenderTickets, SourceEncoding, StartupSignal,
    StartupTracker, build_document, check_document_path, decode_source, read_source, startup_paths,
    tab_title,
};

fn os(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

/// The fixture path without `..` components (the document gate refuses
/// them); `std::path::absolute` normalizes like GetFullPathNameW.
fn fixture(name: &str) -> PathBuf {
    std::path::absolute(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../test-fixtures/markdown/gfm")
            .join(name),
    )
    .unwrap()
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
fn file_uri_arguments_convert_like_the_dotnet_canonicalizer() {
    let paths = startup_paths(os(&[
        "file:///C:/Docs/My%20Notes.md",
        "FILE://D:/x/y.md",
        "file:///C:/a%2",
    ]));
    assert_eq!(paths[0], PathBuf::from(r"C:\Docs\My Notes.md"));
    assert_eq!(paths[1], PathBuf::from(r"D:\x\y.md"));
    // A malformed escape stays literal (Uri.UnescapeDataString).
    assert_eq!(paths[2], PathBuf::from(r"C:\a%2"));
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
    // A pre-check that passes cannot let a longer file through the read,
    // and the error reports the file's real size.
    let size = std::fs::metadata(&path).unwrap().len();
    assert_eq!(
        read_source(&path, |_| Ok(()), 16),
        Err(OpenError::Render(RenderError::SourceTooLarge {
            size_bytes: size,
            maximum_bytes: 16,
        }))
    );
    assert_eq!(
        read_source(&fixture("does-not-exist.md"), |_| Ok(()), 16),
        Err(OpenError::NotFound)
    );
    // The verbatim form of an ordinary drive path is still a disk file.
    let verbatim = std::fs::canonicalize(&path).unwrap();
    assert!(verbatim.to_string_lossy().starts_with(r"\\?\"));
    assert!(read_source(&verbatim, |_| Ok(()), 1 << 20).is_ok());
}

#[test]
fn device_pipe_and_directory_paths_are_refused_before_reading() {
    let opened = std::cell::Cell::new(false);
    let check = |_| {
        opened.set(true);
        Ok(())
    };
    for path in [
        r"\\.\pipe\marknexia-test",
        r"\\.\PhysicalDrive0",
        r"\\.\C:",
        r"\\?\GLOBALROOT\Device\Null",
        r"\\?\pipe\marknexia-test",
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x.md",
        r"\\server\pipe\marknexia-test",
        r"\\?\UNC\server\PIPE\marknexia-test",
        r"\\server\mailslot\x",
        r"\\.\share\x.md",
        r"\\server\share\..\..\x.md",
        r"\\server\share\CON.md",
        r"\\server\share\x.md:stream",
        r"\\server",
        r"\\server\..\pipe\x",
        r"\\server\.\pipe\x",
        r"\\?\UNC\server\..\pipe\x",
        r"\\?\UNC\server\.\mailslot\x",
        r"\\server\share\.\x.md",
        r"\\server@SSL\share\x.md",
        r"\\server@8080\share\x.md",
        r"\\server\pipe.\x",
        r"\\server\mailslot .\x",
        r"\\?\UNC\server\pipe.\x",
        r"\\server\PIPE. .\x",
        r"\\server.\share\x.md",
        r"\\server\share \x.md",
        r"\\server\DavWWWRoot\x.md",
        r"\\server\davwwwroot.\x.md",
        r"\\server\IPC$\x",
        r"\\server\ipc$. \x",
        r"\\?\UNC\server\IPC$\x",
        "\\\\s\u{e9}rver\\share\\x.md",
        "\\\\server\\sh\u{e4}re\\x.md",
        "\\\\server\\share\u{200b}\\x.md",
        r"C:\docs\..\x.md",
        r"C:\docs\.\x.md",
        r"\\?\C:\docs\..\x.md",
        r"C:\NUL",
        r"C:\docs\COM1 .md",
        r"C:\docs\CON.md",
        r"C:\docs\COM1",
        r"C:\docs\x.md:stream",
        r"relative\x.md",
    ] {
        assert_eq!(
            read_source(&PathBuf::from(path), check, 1 << 20),
            Err(OpenError::UnsupportedPath),
            "{path}"
        );
    }
    assert!(!opened.get(), "no refused path reached the size check");
    // A directory is not a regular file.
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        read_source(&directory, |_| Ok(()), 1 << 20),
        Err(OpenError::UnsupportedPath)
    );
}

/// The final-path check: a directory junction (no admin rights needed)
/// whose target has a reserved device name. The requested path passes the
/// lexical gate; the handle's final path does not.
#[test]
fn a_junction_to_a_refused_final_path_is_refused_after_opening() {
    let root = std::env::temp_dir().join(format!("marknexia-junction-{}", std::process::id()));
    let root = std::path::absolute(root).unwrap();
    let verbatim_target = format!(r"\\?\{}\CON", root.display());
    std::fs::create_dir_all(&verbatim_target).unwrap();
    std::fs::write(format!(r"{verbatim_target}\x.md"), b"# refused").unwrap();
    let link = root.join("link");
    let created = std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&verbatim_target)
        .output()
        .is_ok_and(|output| output.status.success());
    let result = created.then(|| read_source(&link.join("x.md"), |_| Ok(()), 1 << 20));
    let _ = std::fs::remove_dir(&link);
    let _ = std::fs::remove_file(format!(r"{verbatim_target}\x.md"));
    let _ = std::fs::remove_dir(&verbatim_target);
    let _ = std::fs::remove_dir(&root);
    match result {
        Some(result) => assert_eq!(result, Err(OpenError::UnsupportedPath)),
        None => eprintln!("skipped: mklink /J is not available here"),
    }
}

#[test]
fn drive_and_unc_share_paths_pass_the_lexical_gate() {
    for path in [
        r"C:\docs\notes.md",
        r"\\?\C:\docs\notes.md",
        r"\\server\share\docs\notes.md",
        r"//server/share/notes.md",
        r"\\?\UNC\server\share\notes.md",
        r"\\?\unc\server\share\notes.md",
        r"\\server\c$\notes.md",
        // Server and share are names: device-name rules apply only below them.
        r"\\con\share\notes.md",
        r"\\server\nul\notes.md",
    ] {
        assert_eq!(check_document_path(path), Ok(()), "{path}");
    }
}

/// A real UNC open through the local administrative share, when this host
/// allows it: the final path is `\\?\UNC\...` and must pass the gate too.
#[test]
fn a_local_admin_share_path_opens_like_a_drive_path() {
    let local = std::path::absolute(fixture("features.md")).unwrap();
    let local = local.to_string_lossy().into_owned();
    let (drive, rest) = local.split_at(2);
    let unc = format!(r"\\localhost\{}${}", &drive[..1], rest);
    match read_source(&PathBuf::from(&unc), |_| Ok(()), 1 << 20) {
        Ok(text) => assert!(text.starts_with("# GFM Full Features Test")),
        // Admin shares can be disabled or need elevation on this host.
        Err(OpenError::Io(_) | OpenError::NotFound) => {
            eprintln!("skipped: {unc} is not reachable here");
        }
        Err(error) => panic!("{unc}: {error:?}"),
    }
}

fn job(tab_id: u64, path: PathBuf) -> (RenderJob, Arc<AtomicBool>) {
    let cancelled = Arc::new(AtomicBool::new(false));
    (
        RenderJob {
            tab_id,
            ticket: tab_id,
            path,
            title: format!("doc-{tab_id}.md"),
            theme: AppTheme::System,
            cancelled: Arc::clone(&cancelled),
        },
        cancelled,
    )
}

fn fixed_identity() -> Result<PageIdentity, String> {
    PageIdentity::new([7; 16], [8; 16]).map_err(|error| error.to_string())
}

#[test]
fn build_document_produces_the_hosted_page_off_the_ui_thread() {
    let (job, _) = job(3, fixture("features.md"));
    let document = build_document(&job, fixed_identity).unwrap();
    let html = String::from_utf8_lossy(document.html());
    assert!(html.contains("GFM Full Features Test"));
    assert!(html.contains("<title>doc-3.md</title>"));
    assert_eq!(document.tab_id(), 3);
    assert!(
        build_document(&job, || Err("no entropy".to_owned()))
            .is_err_and(|error| matches!(error, OpenError::Identity(_)))
    );
}

fn panicking_identity() -> Result<PageIdentity, String> {
    panic!("simulated renderer panic")
}

fn collect(results: &mpsc::Receiver<RenderOutcome>, count: usize) -> Vec<RenderOutcome> {
    (0..count)
        .map(|_| {
            results
                .recv_timeout(Duration::from_secs(60))
                .expect("outcome")
        })
        .collect()
}

#[test]
fn the_pool_is_bounded_and_answers_every_live_job() {
    let (sender, results) = mpsc::channel();
    let notified = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&notified);
    let mut pool = RenderPool::new(
        2,
        8 << 20,
        fixed_identity,
        sender,
        Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let mut cancelled = None;
    for tab_id in 1..=6 {
        let (job, flag) = job(tab_id, fixture("features.md"));
        if tab_id == 6 {
            // Cancelled before a worker takes it: no work, no outcome.
            flag.store(true, Ordering::SeqCst);
            cancelled = Some(flag);
        }
        pool.submit(job).unwrap();
    }
    assert_eq!(pool.worker_count(), 2, "never more than the limit");
    let mut outcomes = collect(&results, 5);
    outcomes.sort_by_key(|outcome| outcome.tab_id);
    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| outcome.tab_id)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert!(outcomes.iter().all(|outcome| outcome.result.is_ok()));
    assert!(results.recv_timeout(Duration::from_millis(500)).is_err());
    assert!(cancelled.is_some());
    assert_eq!(notified.load(Ordering::SeqCst), 5);
    let clamp = RenderPool::default_workers();
    assert!((2..=4).contains(&clamp));
}

#[test]
fn a_panicking_render_fails_only_its_own_tab() {
    let (sender, results) = mpsc::channel();
    let mut pool = RenderPool::new(1, 8 << 20, panicking_identity, sender, Arc::new(|| {}));
    let (first, _) = job(1, fixture("features.md"));
    let (second, _) = job(2, fixture("features.md"));
    pool.submit(first).unwrap();
    pool.submit(second).unwrap();
    // The single worker survives the first panic and answers both tabs.
    let outcomes = collect(&results, 2);
    assert!(
        outcomes
            .iter()
            .all(|outcome| matches!(outcome.result, Err(OpenError::Panicked)))
    );
}

#[test]
fn stale_render_results_are_discarded_and_queued_jobs_cancelled() {
    let mut tickets = RenderTickets::new();
    let (first, first_flag) = tickets.begin(1);
    let (other, _) = tickets.begin(2);
    assert!(tickets.is_pending(1));
    // The tab closed while rendering: its result is dropped and a queued
    // job sees its cancel flag.
    tickets.cancel(1);
    assert!(first_flag.load(Ordering::SeqCst));
    assert!(!tickets.is_pending(1));
    assert!(!tickets.complete(1, first));
    // A newer render supersedes (and cancels) an older one for the tab.
    let (old, old_flag) = tickets.begin(3);
    let (new, new_flag) = tickets.begin(3);
    assert!(old_flag.load(Ordering::SeqCst));
    assert!(!new_flag.load(Ordering::SeqCst));
    assert!(!tickets.complete(3, old));
    assert!(tickets.complete(3, new));
    // Accepted exactly once; a ticket never matches another tab.
    assert!(!tickets.complete(3, new));
    assert!(!tickets.complete(1, other));
    assert!(tickets.complete(2, other));
    assert!(!tickets.is_pending(2));
}

#[test]
fn exactly_one_of_first_render_or_startup_failed_is_signaled() {
    use StartupSignal::{FirstRender, StartupFailed};
    // No paths: the active placeholder is the startup tab.
    let mut tracker = StartupTracker::new(false, [2]);
    assert_eq!(tracker.evaluate(), None);
    assert_eq!(tracker.rendered(2), Some(FirstRender));
    assert_eq!(tracker.webview_unavailable(), None, "never both");
    // No paths and no WebView2: StartupFailed, so empty-shell fails fast.
    let mut tracker = StartupTracker::new(false, [2]);
    assert_eq!(tracker.webview_unavailable(), Some(StartupFailed));
    assert_eq!(tracker.rendered(2), None);
    // No paths, the placeholder's controller fails.
    let mut tracker = StartupTracker::new(false, [2]);
    assert_eq!(tracker.failed(2), Some(StartupFailed));
    // Paths requested but no tab opened.
    let mut tracker = StartupTracker::new(true, []);
    assert_eq!(tracker.evaluate(), Some(StartupFailed));
    assert_eq!(tracker.rendered(1), None, "never both");
    // One tab fails, the other renders: FirstRender only.
    let mut tracker = StartupTracker::new(true, [1, 2]);
    assert_eq!(tracker.failed(1), None);
    assert_eq!(tracker.rendered(2), Some(FirstRender));
    assert_eq!(tracker.failed(2), None);
    assert_eq!(tracker.webview_unavailable(), None);
    // A transient controller failure is final for that tab: StartupFailed
    // only, and a later navigation completion cannot add FirstRender.
    let mut tracker = StartupTracker::new(true, [1]);
    assert_eq!(tracker.failed(1), Some(StartupFailed));
    assert_eq!(tracker.rendered(1), None);
    assert_eq!(tracker.signaled(), Some(StartupFailed));
    // WebView2 unavailable while a render is pending.
    let mut tracker = StartupTracker::new(true, [1]);
    assert_eq!(tracker.webview_unavailable(), Some(StartupFailed));
    assert_eq!(tracker.rendered(1), None);
}
