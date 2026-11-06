//! Repository root discovery, workspace enumeration and the repository tree.
//!
//! Ports `RepositoryService` and `RepositoryTreeBuilder` from
//! `src/Marknexia.Files`. The filters, the ignored folder names, the sort
//! order and the hidden and reparse-point rules are the .NET ones. Differences
//! (limits, refused names, root forms) are recorded in
//! `compat/decisions/repository-scan.md`.
//!
//! Every function here does blocking file-system I/O. A UNC root contacts the
//! remote server and can block until the network times out, so the shell must
//! call these functions on a worker thread. The scan functions poll the
//! caller's `cancel` callback between directory entries; a cancelled scan
//! returns [`ScanError::Cancelled`] and drops its partial results.
//!
//! Containment:
//! - the root must pass the lexical gate [`GatedPath`] (drive or UNC share);
//! - a child path is the parent path joined with one name from the
//!   directory listing, and it must pass the gate and lie within the root;
//! - an entry with `FILE_ATTRIBUTE_REPARSE_POINT` (junction, symbolic link,
//!   mount point or other reparse point) is never listed or entered, like
//!   .NET. The root itself may be a reparse point: the user chose that path.
//!
//! The walks are iterative and bounded by [`ScanLimits`], so deep trees use
//! heap memory, not stack, and huge trees stop at a known size.

use std::{
    cmp::Ordering,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use crate::path::{GatedPath, PathError};

/// Markdown extensions, as .NET `RepositoryService.MarkdownExtensions`.
pub const MARKDOWN_EXTENSIONS: [&str; 4] = [".md", ".markdown", ".mdown", ".mkdn"];

/// Asset extensions, as .NET `RepositoryService.AssetExtensions`.
pub const ASSET_EXTENSIONS: [&str; 9] = [
    ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".bmp", ".pdf",
];

/// Folders that workspace enumeration skips (.NET `RepositoryService`).
pub const WORKSPACE_IGNORED_DIRECTORIES: [&str; 7] = [
    ".git",
    ".vs",
    "bin",
    "obj",
    "node_modules",
    ".idea",
    ".vscode",
];

/// Folders that the repository tree skips (.NET `RepositoryTreeBuilder`).
pub const TREE_IGNORED_DIRECTORIES: [&str; 13] = [
    ".git",
    ".vs",
    "bin",
    "obj",
    "node_modules",
    ".idea",
    ".vscode",
    ".codegraph",
    "artifacts",
    "dist",
    "publish",
    "TestResults",
    "coverage",
];

/// The hard ceiling for [`ScanLimits::max_depth`]. Tree nodes are dropped,
/// compared and cloned recursively, so the depth must stay small.
pub const MAX_DEPTH_CEILING: usize = 256;

/// Bounds for one scan. .NET has no limits; see the decision record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanLimits {
    /// Folder levels below the root that are entered. Deeper folders are
    /// skipped. Values above [`MAX_DEPTH_CEILING`] are clamped.
    pub max_depth: usize,
    /// Files in the result. The scan stops when one more would be added.
    pub max_files: usize,
    /// Directory entries read, including skipped ones. The scan stops when
    /// one more would be read. This bounds the work in large trees that hold
    /// few supported files.
    pub max_entries: usize,
    /// Bytes of path text the scan keeps: result file paths, folder names
    /// waiting to be visited and the paths of entered folders. The count
    /// only grows. The scan stops when one more path would pass the limit.
    /// Unsupported files and skipped entries keep no path.
    pub max_path_bytes: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_files: 100_000,
            max_entries: 1_000_000,
            max_path_bytes: 64 * 1024 * 1024,
        }
    }
}

/// Which limits shortened a scan. The result is still valid; when
/// [`LimitsHit::any`] is true the shell should tell the user that the tree
/// may be incomplete.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LimitsHit {
    /// A folder at [`ScanLimits::max_depth`] had subfolders that were not
    /// entered. They may hold no supported files, so the result *may* be
    /// incomplete; it is not known to be.
    pub depth: bool,
    /// [`ScanLimits::max_files`] stopped the scan.
    pub files: bool,
    /// [`ScanLimits::max_entries`] stopped the scan.
    pub entries: bool,
    /// [`ScanLimits::max_path_bytes`] stopped the scan.
    pub bytes: bool,
    /// An entry was skipped because its full path is longer than the gate's
    /// 32 KiB (UTF-8) limit.
    pub path_length: bool,
}

impl LimitsHit {
    pub fn any(&self) -> bool {
        self.depth || self.files || self.entries || self.bytes || self.path_length
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScanError {
    /// The `cancel` callback returned `true`. Partial results were dropped.
    Cancelled,
    /// The root failed the lexical gate.
    InvalidRoot(PathError),
    /// A directory could not be read for a reason other than access denied
    /// (for example it was deleted during the scan). .NET fails the whole
    /// tree in this case too.
    Io(io::ErrorKind),
}

impl fmt::Display for ScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("the repository scan was cancelled"),
            Self::InvalidRoot(error) => {
                write!(f, "the repository folder is not supported ({error:?})")
            }
            Self::Io(kind) => write!(f, "the repository tree could not be loaded ({kind})"),
        }
    }
}

impl std::error::Error for ScanError {}

/// One node of the repository tree (.NET `RepositoryTreeNode`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryTreeNode {
    pub display_name: String,
    pub full_path: PathBuf,
    pub is_folder: bool,
    pub children: Vec<RepositoryTreeNode>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryTree {
    pub root: RepositoryTreeNode,
    pub limits_hit: LimitsHit,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceFiles {
    pub files: Vec<PathBuf>,
    pub limits_hit: LimitsHit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepositoryKind {
    Folder,
    GitRepository,
}

/// .NET `RepositoryContext`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryContext {
    pub root: PathBuf,
    pub kind: RepositoryKind,
    pub enforce_sandbox: bool,
}

/// .NET `RepositoryTreeBuilder.IsSupportedMarkdownFile`.
pub fn is_supported_markdown_file(path: &str) -> bool {
    has_extension(path, &MARKDOWN_EXTENSIONS)
}

/// .NET `Path.GetExtension` for a file name: the text from the last `.`,
/// or empty when there is no `.` or the name ends with it.
fn dotnet_extension(name: &str) -> &str {
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    match name.rfind('.') {
        Some(index) if index + 1 < name.len() => &name[index..],
        _ => "",
    }
}

fn has_extension(name: &str, set: &[&str]) -> bool {
    let extension = dotnet_extension(name);
    !extension.is_empty()
        && set
            .iter()
            .any(|item| equals_ordinal_ignore_case(item, extension))
}

fn is_ignored(name: &str, set: &[&str]) -> bool {
    set.iter()
        .any(|item| equals_ordinal_ignore_case(item, name))
}

/// One character as .NET `StringComparison.OrdinalIgnoreCase` folds it: the
/// simple uppercase mapping. A character whose uppercase form is more than
/// one character (`ß`) stays as it is; U+0131 (dotless i) stays as it is,
/// as in .NET ordinal casing.
fn ordinal_upper(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_uppercase();
    }
    if c == '\u{131}' {
        return c;
    }
    let mut upper = c.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

/// The sort key of .NET `StringComparer.OrdinalIgnoreCase`: the folded
/// UTF-16 code units.
fn ordinal_ignore_case_key(text: &str) -> Vec<u16> {
    let mut key = Vec::with_capacity(text.len());
    let mut buffer = [0_u16; 2];
    for c in text.chars() {
        key.extend_from_slice(ordinal_upper(c).encode_utf16(&mut buffer));
    }
    key
}

/// Compares like .NET `StringComparer.OrdinalIgnoreCase`.
pub fn compare_ordinal_ignore_case(a: &str, b: &str) -> Ordering {
    ordinal_ignore_case_key(a).cmp(&ordinal_ignore_case_key(b))
}

fn equals_ordinal_ignore_case(a: &str, b: &str) -> bool {
    compare_ordinal_ignore_case(a, b) == Ordering::Equal
}

/// The root text as .NET `PathCanonicalizer` returns it for an absolute
/// path: `/` becomes `\` (not in a verbatim `\\?\` path), repeated
/// separators collapse, and trailing separators are removed except after a
/// drive root (`C:\`).
fn normalize_root_text(text: &str) -> String {
    let (prefix, body) = if let Some(rest) = text.strip_prefix(r"\\?\") {
        (r"\\?\", rest.to_owned())
    } else if let Some(rest) = text.strip_prefix(r"\\").or_else(|| text.strip_prefix("//")) {
        (r"\\", rest.replace('/', "\\"))
    } else {
        ("", text.replace('/', "\\"))
    };
    let mut collapsed = String::with_capacity(body.len());
    for c in body.chars() {
        if c == '\\' && collapsed.ends_with('\\') {
            continue;
        }
        collapsed.push(c);
    }
    let trimmed = collapsed.trim_end_matches('\\');
    let is_drive_root = trimmed.len() == 2 && trimmed.ends_with(':');
    if is_drive_root {
        format!("{prefix}{trimmed}\\")
    } else {
        format!("{prefix}{trimmed}")
    }
}

/// The root gate: [`GatedPath`], and no `/` in a verbatim `\\?\` path, where
/// Windows would read it as part of a name rather than as a separator.
fn gate_root(root: &str) -> Result<(PathBuf, GatedPath), ScanError> {
    if root.starts_with(r"\\?\") && root.contains('/') {
        return Err(ScanError::InvalidRoot(PathError::InvalidComponent));
    }
    let gated = GatedPath::new(root).map_err(ScanError::InvalidRoot)?;
    Ok((PathBuf::from(normalize_root_text(root)), gated))
}

fn last_name(path: &Path) -> Option<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

#[cfg(windows)]
fn hidden_and_reparse(metadata: &fs::Metadata, _name: &str) -> (bool, bool) {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let attributes = metadata.file_attributes();
    (
        attributes & FILE_ATTRIBUTE_HIDDEN != 0,
        attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    )
}

#[cfg(not(windows))]
fn hidden_and_reparse(metadata: &fs::Metadata, name: &str) -> (bool, bool) {
    // .NET on Unix reports dot files as hidden.
    (name.starts_with('.'), metadata.file_type().is_symlink())
}

fn is_supported_file(name: &str) -> bool {
    has_extension(name, &MARKDOWN_EXTENSIONS) || has_extension(name, &ASSET_EXTENSIONS)
}

/// What one folder listing keeps: supported files with their full paths,
/// and subfolder names only (their paths are built when they are entered).
#[derive(Default)]
struct Listed {
    files: Vec<(String, PathBuf)>,
    folders: Vec<String>,
}

/// The walk state shared by both scans.
struct Walk<'a> {
    root: &'a GatedPath,
    limits: ScanLimits,
    cancel: &'a dyn Fn() -> bool,
    entries_read: usize,
    files: usize,
    bytes: usize,
    hit: LimitsHit,
    stopped: bool,
}

impl<'a> Walk<'a> {
    fn new(root: &'a GatedPath, limits: &ScanLimits, cancel: &'a dyn Fn() -> bool) -> Self {
        let limits = ScanLimits {
            max_depth: limits.max_depth.min(MAX_DEPTH_CEILING),
            ..*limits
        };
        Self {
            root,
            limits,
            cancel,
            entries_read: 0,
            files: 0,
            bytes: 0,
            hit: LimitsHit::default(),
            stopped: false,
        }
    }

    fn check_cancel(&self) -> Result<(), ScanError> {
        if (self.cancel)() {
            Err(ScanError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Takes one more file slot, or stops the walk at the file limit.
    fn take_file(&mut self) -> bool {
        if self.files >= self.limits.max_files {
            self.hit.files = true;
            self.stopped = true;
            return false;
        }
        self.files += 1;
        true
    }

    /// Charges `count` bytes of path text that the scan keeps, or stops the
    /// walk at the byte limit. The count only grows.
    fn take_bytes(&mut self, count: usize) -> bool {
        match self.bytes.checked_add(count) {
            Some(total) if total <= self.limits.max_path_bytes => {
                self.bytes = total;
                true
            }
            _ => {
                self.hit.bytes = true;
                self.stopped = true;
                false
            }
        }
    }

    /// True when `path` passes the gate and lies within the root. A path that
    /// fails the gate only for its length sets `LimitsHit::path_length`.
    fn admits(&mut self, path: &Path) -> bool {
        match path.to_str().map(GatedPath::new) {
            Some(Ok(child)) => child.is_within(self.root),
            Some(Err(PathError::TooLong)) => {
                self.hit.path_length = true;
                false
            }
            _ => false,
        }
    }

    /// The kept entries of `dir`, in listing order: supported files and
    /// folders not in `ignored`, without reparse points, without hidden
    /// entries unless `show_hidden`, and without names that fail the gate or
    /// leave the root. Entries are classified before any path is built.
    /// Access denied gives an empty listing (.NET catches
    /// `UnauthorizedAccessException`).
    fn read(
        &mut self,
        dir: &Path,
        ignored: &[&str],
        show_hidden: bool,
    ) -> Result<Listed, ScanError> {
        self.check_cancel()?;
        let listing = match fs::read_dir(dir) {
            Ok(listing) => listing,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return Ok(Listed::default());
            }
            Err(error) => return Err(ScanError::Io(error.kind())),
        };
        let mut listed = Listed::default();
        for item in listing {
            self.check_cancel()?;
            if self.entries_read >= self.limits.max_entries {
                self.hit.entries = true;
                self.stopped = true;
                break;
            }
            self.entries_read += 1;
            let item = match item {
                Ok(item) => item,
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                    return Ok(Listed::default());
                }
                Err(error) => return Err(ScanError::Io(error.kind())),
            };
            // A name that is not valid UTF-16 text (an unpaired surrogate)
            // cannot pass the document gate; skip it.
            let Ok(name) = item.file_name().into_string() else {
                continue;
            };
            // On Windows this is the directory listing's data: it does not
            // follow a reparse point.
            let Ok(metadata) = item.metadata() else {
                continue;
            };
            let (hidden, reparse) = hidden_and_reparse(&metadata, &name);
            if reparse || (hidden && !show_hidden) {
                continue;
            }
            let is_dir = metadata.is_dir();
            let wanted = if is_dir {
                !is_ignored(&name, ignored)
            } else {
                is_supported_file(&name)
            };
            if !wanted {
                continue;
            }
            let path = dir.join(&name);
            if !self.admits(&path) {
                continue;
            }
            if is_dir {
                if !self.take_bytes(name.len()) {
                    break;
                }
                listed.folders.push(name);
            } else {
                if !self.take_file() || !self.take_bytes(path.as_os_str().len()) {
                    break;
                }
                listed.files.push((name, path));
            }
        }
        Ok(listed)
    }
}

struct Frame {
    name: String,
    path: PathBuf,
    depth: usize,
    children: Vec<RepositoryTreeNode>,
    /// Subfolder names still to visit, in reverse visiting order.
    pending: Vec<String>,
}

/// Folders first, then names by .NET `StringComparer.OrdinalIgnoreCase`;
/// stable, so equal names keep the directory listing order (as .NET
/// `OrderByDescending(IsFolder).ThenBy(DisplayName)`).
fn sort_children(children: &mut [RepositoryTreeNode]) {
    children
        .sort_by_cached_key(|node| (!node.is_folder, ordinal_ignore_case_key(&node.display_name)));
}

impl Walk<'_> {
    fn open_frame(
        &mut self,
        name: String,
        path: PathBuf,
        depth: usize,
    ) -> Result<Frame, ScanError> {
        let listed = self.read(&path, &TREE_IGNORED_DIRECTORIES, false)?;
        let children = listed
            .files
            .into_iter()
            .map(|(name, path)| RepositoryTreeNode {
                display_name: name,
                full_path: path,
                is_folder: false,
                children: Vec::new(),
            })
            .collect();
        let mut pending = listed.folders;
        pending.sort_by_cached_key(|name| ordinal_ignore_case_key(name));
        pending.reverse();
        Ok(Frame {
            name,
            path,
            depth,
            children,
            pending,
        })
    }
}

/// Builds the repository tree like .NET `RepositoryTreeBuilder.BuildAsync`:
/// supported Markdown and asset files, folders first and then names in
/// ordinal ignore-case order, folders without supported files left out
/// (the root is always present), hidden entries, reparse points and
/// [`TREE_IGNORED_DIRECTORIES`] skipped. A root that does not exist or is
/// not a folder gives an empty root node, as in .NET.
///
/// Call it on a worker thread. `cancel` is polled between entries; when it
/// returns `true` the scan stops and returns [`ScanError::Cancelled`].
pub fn build_repository_tree(
    root: &str,
    limits: &ScanLimits,
    cancel: &dyn Fn() -> bool,
) -> Result<RepositoryTree, ScanError> {
    if cancel() {
        return Err(ScanError::Cancelled);
    }
    let (root_path, gated) = gate_root(root)?;
    let root_text = root_path.to_string_lossy().into_owned();
    if !fs::metadata(&root_path).is_ok_and(|metadata| metadata.is_dir()) {
        // .NET: Path.GetFileName(root) ?? root, which is "" for a drive root.
        return Ok(RepositoryTree {
            root: RepositoryTreeNode {
                display_name: last_name(&root_path).unwrap_or_default(),
                full_path: root_path,
                is_folder: true,
                children: Vec::new(),
            },
            limits_hit: LimitsHit::default(),
        });
    }
    // .NET DirectoryInfo.Name: the last name, or the whole root ("C:\").
    let root_name = last_name(&root_path).unwrap_or(root_text);
    let mut walk = Walk::new(&gated, limits, cancel);
    let mut stack = vec![walk.open_frame(root_name, root_path, 0)?];
    loop {
        walk.check_cancel()?;
        let Some(top) = stack.last_mut() else {
            unreachable!("the root frame is popped last");
        };
        if !walk.stopped
            && let Some(name) = top.pending.pop()
        {
            let depth = top.depth + 1;
            if depth > walk.limits.max_depth {
                walk.hit.depth = true;
                continue;
            }
            let path = top.path.join(&name);
            if !walk.take_bytes(path.as_os_str().len()) {
                continue;
            }
            let frame = walk.open_frame(name, path, depth)?;
            stack.push(frame);
            continue;
        }
        let Some(frame) = stack.pop() else {
            unreachable!("the stack is not empty here");
        };
        let mut children = frame.children;
        sort_children(&mut children);
        let node = RepositoryTreeNode {
            display_name: frame.name,
            full_path: frame.path,
            is_folder: true,
            children,
        };
        match stack.last_mut() {
            Some(parent) => {
                if !node.children.is_empty() {
                    parent.children.push(node);
                }
            }
            None => {
                return Ok(RepositoryTree {
                    root: node,
                    limits_hit: walk.hit,
                });
            }
        }
    }
}

/// Lists supported files like .NET `RepositoryService.EnumerateWorkspaceFiles`:
/// pre-order, each folder's files in listing order before its subfolders,
/// reparse points always skipped, hidden entries skipped unless
/// `show_hidden`, and [`WORKSPACE_IGNORED_DIRECTORIES`] skipped. A root that
/// does not exist gives an empty list.
pub fn enumerate_workspace_files(
    root: &str,
    show_hidden: bool,
    limits: &ScanLimits,
    cancel: &dyn Fn() -> bool,
) -> Result<WorkspaceFiles, ScanError> {
    if cancel() {
        return Err(ScanError::Cancelled);
    }
    let (root_path, gated) = gate_root(root)?;
    if !fs::metadata(&root_path).is_ok_and(|metadata| metadata.is_dir()) {
        return Ok(WorkspaceFiles::default());
    }
    let mut walk = Walk::new(&gated, limits, cancel);
    let mut files = Vec::new();
    // Each frame: the folder path, its depth and its subfolder names still
    // to visit, in reverse listing order.
    let mut stack: Vec<(PathBuf, usize, Vec<String>)> = Vec::new();
    let mut next = Some((root_path, 0_usize));
    loop {
        if let Some((dir, depth)) = next.take() {
            let listed = walk.read(&dir, &WORKSPACE_IGNORED_DIRECTORIES, show_hidden)?;
            files.extend(listed.files.into_iter().map(|(_, path)| path));
            let mut pending = listed.folders;
            pending.reverse();
            stack.push((dir, depth, pending));
        }
        walk.check_cancel()?;
        if walk.stopped {
            break;
        }
        let Some((dir, depth, pending)) = stack.last_mut() else {
            break;
        };
        let Some(name) = pending.pop() else {
            stack.pop();
            continue;
        };
        if *depth + 1 > walk.limits.max_depth {
            walk.hit.depth = true;
            continue;
        }
        let path = dir.join(&name);
        if walk.take_bytes(path.as_os_str().len()) {
            next = Some((path, *depth + 1));
        }
    }
    walk.check_cancel()?;
    Ok(WorkspaceFiles {
        files,
        limits_hit: walk.hit,
    })
}

fn exists_as_file_or_folder(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.is_dir() || metadata.is_file())
}

fn has_git_marker(dir: &Path) -> bool {
    exists_as_file_or_folder(&dir.join(".git"))
}

#[cfg(windows)]
fn is_listed_directory(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

#[cfg(not(windows))]
fn is_listed_directory(metadata: &fs::Metadata) -> bool {
    metadata.is_dir()
}

fn is_solution_name(name: &str) -> bool {
    [".sln", ".slnx"].iter().any(|suffix| {
        name.len() >= suffix.len()
            && name.is_char_boundary(name.len() - suffix.len())
            && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// True when `dir` holds a file matching `*.sln` or `*.slnx` (.NET
/// `Directory.EnumerateFiles`, which includes hidden files and file links).
/// At most `max_entries` entries are read. Access denied means no marker
/// (RS-5); any other listing error is returned, as .NET throws it.
fn has_solution_marker(dir: &Path, max_entries: usize) -> Result<bool, ScanError> {
    let listing = match fs::read_dir(dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Ok(false),
        Err(error) => return Err(ScanError::Io(error.kind())),
    };
    for item in listing.take(max_entries) {
        let item = match item {
            Ok(item) => item,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => return Ok(false),
            Err(error) => return Err(ScanError::Io(error.kind())),
        };
        if is_solution_name(&item.file_name().to_string_lossy())
            && item
                .metadata()
                .is_ok_and(|metadata| !is_listed_directory(&metadata))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// .NET `RepositoryService.DetectRepositoryRoot`: from the file's folder (or
/// the folder itself) upwards, the first folder that holds `.git` (folder or
/// file) or a `*.sln` / `*.slnx` file. `Ok(None)` when the path fails the
/// gate or no folder qualifies. A folder that cannot be listed because access
/// is denied has no solution marker and the search goes on (RS-5); any other
/// listing error is returned as [`ScanError::Io`], as .NET throws it.
/// Blocking I/O: call it on a worker thread.
pub fn detect_repository_root(file_path: &str) -> Result<Option<PathBuf>, ScanError> {
    if file_path.trim().is_empty() || gate_root(file_path).is_err() {
        return Ok(None);
    }
    let path = PathBuf::from(normalize_root_text(file_path));
    let mut current = if fs::metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        match path.parent() {
            Some(parent) => parent.to_path_buf(),
            None => return Ok(None),
        }
    } else {
        path
    };
    let max_entries = ScanLimits::default().max_entries;
    while fs::metadata(&current).is_ok_and(|metadata| metadata.is_dir()) {
        if has_git_marker(&current) || has_solution_marker(&current, max_entries)? {
            return Ok(Some(current));
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    Ok(None)
}

/// .NET `RepositoryService.CreateContext`: `None` when the root fails the
/// gate or is not a folder.
pub fn create_context(root: &str, enforce_sandbox: bool) -> Option<RepositoryContext> {
    if root.trim().is_empty() {
        return None;
    }
    let (root_path, _) = gate_root(root).ok()?;
    if !fs::metadata(&root_path).is_ok_and(|metadata| metadata.is_dir()) {
        return None;
    }
    let kind = if has_git_marker(&root_path) {
        RepositoryKind::GitRepository
    } else {
        RepositoryKind::Folder
    };
    Some(RepositoryContext {
        root: root_path,
        kind,
        enforce_sandbox,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_follow_dotnet_get_extension() {
        assert_eq!(dotnet_extension("a.md"), ".md");
        assert_eq!(dotnet_extension(".md"), ".md");
        assert_eq!(dotnet_extension("a.md."), "");
        assert_eq!(dotnet_extension("README"), "");
        assert_eq!(dotnet_extension("a.tar.MD"), ".MD");
        assert!(is_supported_markdown_file(r"C:\x\NOTES.MarkDown"));
        assert!(!is_supported_markdown_file(r"C:\x.md\notes"));
        assert!(has_extension("Logo.SVG", &ASSET_EXTENSIONS));
    }

    #[test]
    fn ordinal_ignore_case_uppercases_like_dotnet() {
        // Uppercase folding puts '_' (0x5F) after the letters.
        assert_eq!(compare_ordinal_ignore_case("_a", "b"), Ordering::Greater);
        assert_eq!(compare_ordinal_ignore_case("a", "B"), Ordering::Less);
        assert_eq!(
            compare_ordinal_ignore_case("README", "readme"),
            Ordering::Equal
        );
        assert_eq!(compare_ordinal_ignore_case("é", "É"), Ordering::Equal);
        assert_ne!(compare_ordinal_ignore_case("\u{131}", "I"), Ordering::Equal);
        assert_ne!(compare_ordinal_ignore_case("ß", "SS"), Ordering::Equal);
        assert!(is_ignored("NODE_MODULES", &TREE_IGNORED_DIRECTORIES));
        assert!(is_ignored("testresults", &TREE_IGNORED_DIRECTORIES));
        assert!(!is_ignored("testresults", &WORKSPACE_IGNORED_DIRECTORIES));
    }

    #[test]
    fn root_text_is_normalized_like_the_dotnet_canonicalizer() {
        assert_eq!(normalize_root_text("C:/Repo//docs/"), r"C:\Repo\docs");
        assert_eq!(normalize_root_text(r"C:\"), r"C:\");
        assert_eq!(normalize_root_text("C:/"), r"C:\");
        assert_eq!(normalize_root_text(r"\\server\share\"), r"\\server\share");
        assert_eq!(normalize_root_text("//server/share/x"), r"\\server\share\x");
        assert_eq!(normalize_root_text(r"\\?\C:\Repo\"), r"\\?\C:\Repo");
    }
}
