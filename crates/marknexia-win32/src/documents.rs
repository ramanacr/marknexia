//! Document opening: startup paths, safe bounded reads, .NET-compatible
//! decoding, bounded off-thread rendering, and stale-result discard. The only
//! OS call is the handle-type check in [`read_source`]; no HWND or COM.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fmt, fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use marknexia_core::contracts::AppTheme;
use marknexia_files::{
    bounded_read::{BoundedReadError, BoundedReader},
    path::CanonicalPath,
};
use marknexia_rendering::{DocumentRenderer, PageIdentity, RenderContext, RenderError, Renderer};
use marknexia_webview::policy::HostDocument;

/// Paths to open from the command line (arguments after the program name).
/// Every non-empty argument is a path; one leading `--` separator is dropped.
/// `file:///` and `file://` arguments are converted like the .NET
/// `PathCanonicalizer` (prefix removed, percent-decoded, `/` to `\`).
/// Relative paths resolve against the current directory.
#[must_use]
pub fn startup_paths<I>(arguments: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = OsString>,
{
    let mut separator_seen = false;
    arguments
        .into_iter()
        .filter(|argument| {
            if !separator_seen && argument == "--" {
                separator_seen = true;
                return false;
            }
            !argument.is_empty()
        })
        .map(|argument| {
            let path = file_uri_path(&argument).unwrap_or_else(|| PathBuf::from(argument));
            std::path::absolute(&path).unwrap_or(path)
        })
        .collect()
}

/// .NET `PathCanonicalizer.CanonicalizePath` file-URI handling.
fn file_uri_path(argument: &OsString) -> Option<PathBuf> {
    let text = argument.to_str()?;
    let prefix = text
        .get(..8)
        .filter(|p| p.eq_ignore_ascii_case("file:///"))
        .map(|_| 8)
        .or_else(|| {
            text.get(..7)
                .filter(|p| p.eq_ignore_ascii_case("file://"))
                .map(|_| 7)
        })?;
    Some(PathBuf::from(
        percent_decode(&text[prefix..]).replace('/', "\\"),
    ))
}

/// `Uri.UnescapeDataString`: `%XX` escapes decode to bytes (UTF-8); a `%`
/// not followed by two hex digits stays literal.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |byte: u8| char::from(byte).to_digit(16);
        if bytes[index] == b'%'
            && let (Some(high), Some(low)) = (
                bytes.get(index + 1).copied().and_then(hex),
                bytes.get(index + 2).copied().and_then(hex),
            )
        {
            out.push((high * 16 + low) as u8);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Tab title for a document: its file name.
#[must_use]
pub fn tab_title(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The encoding `decode_source` detected (.NET `FileService.DecodeBytes`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEncoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Utf32Be,
    Latin1,
}

/// .NET `FileService.DecodeBytes`, including its quirks:
/// - a UTF-8 BOM is stripped and the rest decoded with replacement;
/// - `FF FE` always means UTF-16 LE (the UTF-32 LE branch is unreachable
///   in .NET because the UTF-16 LE check runs first);
/// - `00 00 FE FF` is labelled UTF-32 BE but decoded with little-endian
///   `Encoding.UTF32`, exactly as .NET does;
/// - otherwise strict UTF-8, falling back to Latin-1.
///
/// Invalid UTF-16/UTF-32 code units become U+FFFD, like .NET's decoders.
#[must_use]
pub fn decode_source(bytes: &[u8]) -> (String, SourceEncoding) {
    if bytes.is_empty() {
        return (String::new(), SourceEncoding::Utf8);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (
            String::from_utf8_lossy(rest).into_owned(),
            SourceEncoding::Utf8Bom,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return (
            decode_utf16(rest, u16::from_le_bytes),
            SourceEncoding::Utf16Le,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return (
            decode_utf16(rest, u16::from_be_bytes),
            SourceEncoding::Utf16Be,
        );
    }
    if let Some(rest) = bytes.strip_prefix(&[0x00, 0x00, 0xFE, 0xFF]) {
        return (decode_utf32_le(rest), SourceEncoding::Utf32Be);
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), SourceEncoding::Utf8),
        Err(_) => (
            bytes.iter().copied().map(char::from).collect(),
            SourceEncoding::Latin1,
        ),
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let (pairs, rest) = bytes.as_chunks::<2>();
    let mut text: String = char::decode_utf16(pairs.iter().map(|pair| unit(*pair)))
        .map(|character| character.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    if !rest.is_empty() {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    text
}

fn decode_utf32_le(bytes: &[u8]) -> String {
    let (quads, rest) = bytes.as_chunks::<4>();
    let mut text: String = quads
        .iter()
        .map(|quad| {
            char::from_u32(u32::from_le_bytes(*quad)).unwrap_or(char::REPLACEMENT_CHARACTER)
        })
        .collect();
    if !rest.is_empty() {
        text.push(char::REPLACEMENT_CHARACTER);
    }
    text
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpenError {
    NotFound,
    /// Not a regular disk file on a drive or UNC share: device namespaces
    /// (`\\.\`, `\\?\GLOBALROOT`, other `\\?\` forms), pipes and mailslots,
    /// reserved device names, alternate data streams and non-disk handles
    /// are refused, before opening and again on the handle's final path.
    UnsupportedPath,
    Io(String),
    Render(RenderError),
    Identity(String),
    Display(String),
    /// The render worker panicked; only this document failed.
    Panicked,
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound => f.write_str("the file does not exist"),
            Self::UnsupportedPath => {
                f.write_str("only files on a drive or network share can be opened")
            }
            Self::Io(error) => write!(f, "the file could not be read ({error})"),
            Self::Render(error) => error.fmt(f),
            Self::Identity(error) => write!(f, "no page identity ({error})"),
            Self::Display(error) => write!(f, "the page cannot be displayed ({error})"),
            Self::Panicked => f.write_str("rendering failed unexpectedly"),
        }
    }
}

/// `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`: a named-pipe server
/// reached through a path can at most identify, never impersonate, us. std
/// adds `SECURITY_SQOS_PRESENT` whenever QoS flags are set.
const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;
/// `FILE_FLAG_SEQUENTIAL_SCAN`, as .NET `FileOptions.SequentialScan`.
const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;

/// The lexical gate for a document path, applied to the requested path and
/// to the opened handle's final path. Accepted forms:
/// - a drive path `C:\...` or its verbatim form `\\?\C:\...`;
/// - a UNC path `\\server\share\...` or its verbatim form
///   `\\?\UNC\server\share\...` (as .NET allows).
///
/// Every component must pass `CanonicalPath` (no reserved device names such
/// as `CON`, `NUL`, `COM1`, no `:` so no alternate data streams, no wildcard
/// or control characters). Refused: `\\.\` and all other `\\?\` forms
/// (GLOBALROOT, volume GUIDs, pipes, devices), the `pipe` and `mailslot`
/// pseudo-shares, and `..` in UNC paths.
pub fn check_document_path(text: &str) -> Result<(), OpenError> {
    let unsupported = |_| OpenError::UnsupportedPath;
    let unc = if let Some(rest) = text.strip_prefix(r"\\?\") {
        if is_drive_path(rest) {
            return CanonicalPath::new(rest).map(drop).map_err(unsupported);
        }
        match rest.get(..4) {
            Some(prefix) if prefix.eq_ignore_ascii_case(r"UNC\") => &rest[4..],
            _ => return Err(OpenError::UnsupportedPath),
        }
    } else if let Some(rest) = text.strip_prefix(r"\\").or_else(|| text.strip_prefix("//")) {
        rest
    } else {
        return CanonicalPath::new(text).map(drop).map_err(unsupported);
    };
    let mut parts = unc.split(['\\', '/']);
    let server = parts.next().unwrap_or_default();
    let share = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    if server.is_empty()
        || share.is_empty()
        || matches!(server, "." | "?" | "..")
        || share.eq_ignore_ascii_case("pipe")
        || share.eq_ignore_ascii_case("mailslot")
        || rest.contains(&"..")
    {
        return Err(OpenError::UnsupportedPath);
    }
    // Reuse the component rules by checking the UNC components as if they
    // were below a drive root.
    let components = std::iter::once(server)
        .chain(std::iter::once(share))
        .chain(rest)
        .collect::<Vec<_>>()
        .join("/");
    CanonicalPath::new(&format!("C:/{components}"))
        .map(drop)
        .map_err(unsupported)
}

fn is_drive_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// Reads a Markdown source like .NET `FileService.ReadTextAsync`, with
/// stricter checks on what may be opened:
/// - the path passes [`check_document_path`] (drive and UNC share paths);
/// - the handle is opened for reading with identification-only QoS and
///   must be a disk file (`GetFileType == FILE_TYPE_DISK`), a regular file,
///   and have a final path (`GetFinalPathNameByHandleW`) that again passes
///   [`check_document_path`], so a reparse point cannot lead to a pipe or
///   device namespace. .NET `FileStream` also refuses non-disk handles for
///   ordinary paths (`NotSupported_FileStreamOnNonFiles`), and `File.Exists`
///   excludes devices and directories;
/// - the size is checked with `check_size` (the renderer's
///   `check_source_size`) before reading, the read is bounded by `limit`
///   (the file may grow after the check), and an over-limit read reports the
///   file's current size;
/// - the bytes are decoded by [`decode_source`].
pub fn read_source(
    path: &Path,
    check_size: impl Fn(u64) -> Result<(), RenderError>,
    limit: u64,
) -> Result<String, OpenError> {
    let text = path.to_str().ok_or(OpenError::UnsupportedPath)?;
    check_document_path(text)?;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .custom_flags(FILE_FLAG_SEQUENTIAL_SCAN);
    }
    let file = options.open(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => OpenError::NotFound,
        _ => OpenError::Io(error.kind().to_string()),
    })?;
    if !is_disk_file(&file) {
        return Err(OpenError::UnsupportedPath);
    }
    let metadata = file
        .metadata()
        .map_err(|error| OpenError::Io(error.kind().to_string()))?;
    if !metadata.is_file() {
        return Err(OpenError::UnsupportedPath);
    }
    let final_path = final_path(&file).ok_or(OpenError::UnsupportedPath)?;
    check_document_path(&final_path)?;
    check_size(metadata.len()).map_err(OpenError::Render)?;
    let bytes = BoundedReader::new(usize::try_from(limit).unwrap_or(usize::MAX))
        .read_from(&file)
        .map_err(|error| match error {
            BoundedReadError::TooLarge => {
                let current = file.metadata().map_or(0, |metadata| metadata.len());
                OpenError::Render(RenderError::SourceTooLarge {
                    // The file grew past the limit during the read.
                    size_bytes: current.max(limit.saturating_add(1)),
                    maximum_bytes: limit,
                })
            }
            BoundedReadError::Io => OpenError::Io("read failed".to_owned()),
        })?;
    Ok(decode_source(&bytes).0)
}

#[cfg(windows)]
#[allow(unsafe_code)] // One handle-type query on a handle this function owns.
fn is_disk_file(file: &fs::File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{FILE_TYPE_DISK, GetFileType},
    };
    // SAFETY: the handle belongs to `file`, which outlives this call;
    // GetFileType only queries it.
    unsafe { GetFileType(HANDLE(file.as_raw_handle())) == FILE_TYPE_DISK }
}

/// The opened file's normalized DOS-volume final path (`\\?\C:\...` or
/// `\\?\UNC\server\share\...`), or `None` when Windows cannot name it that
/// way (the file is then refused).
#[cfg(windows)]
#[allow(unsafe_code)] // One path query on a handle this function owns.
fn final_path(file: &fs::File) -> Option<String> {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            FILE_NAME_NORMALIZED, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFinalPathNameByHandleW,
            VOLUME_NAME_DOS,
        },
    };
    let handle = HANDLE(file.as_raw_handle());
    let flags = GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0);
    let mut buffer = vec![0_u16; 512];
    loop {
        // SAFETY: the handle belongs to `file`, which outlives this call, and
        // the buffer is writable for its full length.
        let length = unsafe { GetFinalPathNameByHandleW(handle, &mut buffer, flags) } as usize;
        if length == 0 {
            return None;
        }
        if length < buffer.len() {
            return String::from_utf16(&buffer[..length]).ok();
        }
        // Too small: `length` is the required size including the NUL.
        if length > 64 * 1024 {
            return None;
        }
        buffer.resize(length + 1, 0);
    }
}

#[cfg(not(windows))]
fn is_disk_file(_file: &fs::File) -> bool {
    true
}

#[cfg(not(windows))]
fn final_path(_file: &fs::File) -> Option<String> {
    None
}

/// Reads, renders and builds the hosted page for one file. Runs on a render
/// worker: the page (up to 128 MiB) is assembled here, never on the UI
/// thread, and only the `Send` `HostDocument` goes back.
pub fn build_document(
    job: &RenderJob,
    identity: impl FnOnce() -> Result<PageIdentity, String>,
) -> Result<HostDocument, OpenError> {
    let identity = identity().map_err(OpenError::Identity)?;
    let renderer = Renderer::new();
    let source = read_source(
        &job.path,
        |size| renderer.check_source_size(size),
        renderer.limits().max_source_bytes,
    )?;
    let rendered = renderer
        .render(&source, &RenderContext::new(job.theme, identity))
        .map_err(OpenError::Render)?;
    drop(source);
    HostDocument::from_rendered(job.tab_id, 1, &job.title, rendered, &identity)
        .map_err(|error| OpenError::Display(format!("{error:?}")))
}

/// Pending renders by tab. The renderer cannot be cancelled, so a result is
/// accepted only while its ticket is still the tab's current one: closing the
/// tab (or starting a newer render for it) makes an in-flight result stale,
/// and its cancel flag lets a queued job skip the work entirely.
#[derive(Debug, Default)]
pub struct RenderTickets {
    next: u64,
    pending: BTreeMap<u64, (u64, Arc<AtomicBool>)>,
}

impl RenderTickets {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a render for `tab_id`, superseding (and cancelling) any
    /// earlier one. Returns the ticket and the job's cancel flag.
    pub fn begin(&mut self, tab_id: u64) -> (u64, Arc<AtomicBool>) {
        self.next = self.next.wrapping_add(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        if let Some((_, previous)) = self
            .pending
            .insert(tab_id, (self.next, Arc::clone(&cancelled)))
        {
            previous.store(true, Ordering::Release);
        }
        (self.next, cancelled)
    }

    /// The tab closed; any queued or in-flight result for it is discarded.
    pub fn cancel(&mut self, tab_id: u64) {
        if let Some((_, cancelled)) = self.pending.remove(&tab_id) {
            cancelled.store(true, Ordering::Release);
        }
    }

    /// True exactly once for the current ticket of a live tab.
    pub fn complete(&mut self, tab_id: u64, ticket: u64) -> bool {
        if self.pending.get(&tab_id).map(|(current, _)| *current) == Some(ticket) {
            self.pending.remove(&tab_id);
            true
        } else {
            false
        }
    }

    #[must_use]
    pub fn is_pending(&self, tab_id: u64) -> bool {
        self.pending.contains_key(&tab_id)
    }
}

/// One file to render for one tab.
pub struct RenderJob {
    pub tab_id: u64,
    pub ticket: u64,
    pub path: PathBuf,
    pub title: String,
    pub theme: AppTheme,
    pub cancelled: Arc<AtomicBool>,
}

/// A worker's result, posted back to the UI thread.
pub struct RenderOutcome {
    pub tab_id: u64,
    pub ticket: u64,
    pub result: Result<HostDocument, OpenError>,
}

const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<RenderOutcome>();
    assert_send::<RenderJob>();
};

/// Wakes the UI thread after a result was queued (the shell posts a message).
pub type Notify = Arc<dyn Fn() + Send + Sync>;
/// Draws a fresh page identity (the shell uses the OS RNG).
pub type IdentitySource = fn() -> Result<PageIdentity, String>;

/// A bounded pool of render workers fed by one FIFO queue. Workers start on
/// demand up to `max_workers` and stop when the pool is dropped. A panic
/// while rendering is caught and fails only that job's tab; a drop guard
/// still reports the tab if a panic ever escapes the catch.
pub struct RenderPool {
    jobs: mpsc::Sender<RenderJob>,
    queue: Arc<Mutex<mpsc::Receiver<RenderJob>>>,
    results: mpsc::Sender<RenderOutcome>,
    notify: Notify,
    identity: IdentitySource,
    workers: usize,
    max_workers: usize,
    stack_bytes: usize,
}

impl RenderPool {
    #[must_use]
    pub fn new(
        max_workers: usize,
        stack_bytes: usize,
        identity: IdentitySource,
        results: mpsc::Sender<RenderOutcome>,
        notify: Notify,
    ) -> Self {
        let (jobs, queue) = mpsc::channel();
        Self {
            jobs,
            queue: Arc::new(Mutex::new(queue)),
            results,
            notify,
            identity,
            workers: 0,
            max_workers: max_workers.max(1),
            stack_bytes,
        }
    }

    /// 2–4 workers: rendering is CPU-bound, and the UI thread plus WebView2
    /// need the remaining cores.
    #[must_use]
    pub fn default_workers() -> usize {
        std::thread::available_parallelism()
            .map_or(2, |count| count.get().saturating_sub(1))
            .clamp(2, 4)
    }

    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.workers
    }

    /// Queues `job`, starting another worker when below the limit.
    pub fn submit(&mut self, job: RenderJob) -> Result<(), String> {
        if self.workers < self.max_workers {
            match self.spawn_worker() {
                Ok(()) => self.workers += 1,
                Err(error) if self.workers == 0 => return Err(error),
                Err(_) => {}
            }
        }
        self.jobs
            .send(job)
            .map_err(|_| "the render queue is closed".to_owned())
    }

    fn spawn_worker(&self) -> Result<(), String> {
        let queue = Arc::clone(&self.queue);
        let results = self.results.clone();
        let notify = Arc::clone(&self.notify);
        let identity = self.identity;
        std::thread::Builder::new()
            .name("marknexia-render".to_owned())
            .stack_size(self.stack_bytes)
            .spawn(move || {
                loop {
                    let next = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
                    let Ok(job) = next else { break };
                    if job.cancelled.load(Ordering::Acquire) {
                        continue;
                    }
                    let mut reply = Reply {
                        tab_id: job.tab_id,
                        ticket: job.ticket,
                        results: &results,
                        notify: &notify,
                        done: false,
                    };
                    let result = catch_unwind(AssertUnwindSafe(|| build_document(&job, identity)))
                        .unwrap_or(Err(OpenError::Panicked));
                    if job.cancelled.load(Ordering::Acquire) {
                        reply.done = true;
                    } else {
                        reply.send(result);
                    }
                }
            })
            .map(drop)
            .map_err(|error| format!("could not start a render worker: {error}"))
    }
}

/// Always answers its tab: dropped without `send` (a panic escaping the
/// catch), it reports `Panicked` so the tab never stays "Rendering…".
struct Reply<'a> {
    tab_id: u64,
    ticket: u64,
    results: &'a mpsc::Sender<RenderOutcome>,
    notify: &'a Notify,
    done: bool,
}

impl Reply<'_> {
    fn send(&mut self, result: Result<HostDocument, OpenError>) {
        self.done = true;
        let outcome = RenderOutcome {
            tab_id: self.tab_id,
            ticket: self.ticket,
            result,
        };
        if self.results.send(outcome).is_ok() {
            (self.notify)();
        }
    }
}

impl Drop for Reply<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.send(Err(OpenError::Panicked));
        }
    }
}
