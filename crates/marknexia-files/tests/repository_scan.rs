//! Repository scan and tree parity with .NET `RepositoryService` and
//! `RepositoryTreeBuilder`, on temporary directories this file creates.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use marknexia_files::{
    path::PathError,
    repository::{
        RepositoryKind, RepositoryTreeNode, ScanError, ScanLimits, build_repository_tree,
        create_context, detect_repository_root, enumerate_workspace_files,
    },
};

/// A temporary folder removed on drop. Links are removed first, without
/// following them.
struct TempTree {
    root: PathBuf,
    links: Vec<PathBuf>,
}

impl TempTree {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos());
        let root = std::env::temp_dir().join(format!(
            "mnx-scan-{tag}-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        Self {
            root,
            links: Vec::new(),
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root
            .join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))
    }

    fn text(&self) -> String {
        self.root.to_str().unwrap().to_owned()
    }

    fn file(&self, relative: &str) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"# x\n").unwrap();
        path
    }

    fn dir(&self, relative: &str) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// `mklink /J`, which needs no elevation. False when it is unavailable.
    fn junction(&mut self, relative: &str, target: &Path) -> bool {
        let link = self.path(relative);
        let output = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(target)
            .output();
        let created = output.as_ref().is_ok_and(|output| output.status.success());
        if created {
            self.links.push(link);
        } else {
            eprintln!("mklink /J {}: {output:?}", link.display());
        }
        created
    }

    /// `mklink` for a file symbolic link; needs Developer Mode or elevation.
    fn file_symlink(&mut self, relative: &str, target: &Path) -> bool {
        let link = self.path(relative);
        let created = Command::new("cmd")
            .args(["/c", "mklink"])
            .arg(&link)
            .arg(target)
            .output()
            .is_ok_and(|output| output.status.success());
        if created {
            self.links.push(link);
        }
        created
    }

    fn hide(&self, relative: &str) {
        let status = Command::new("attrib")
            .arg("+h")
            .arg(self.path(relative))
            .status()
            .unwrap();
        assert!(status.success());
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        for link in self.links.drain(..).rev() {
            // A junction or directory link is removed as a directory; this
            // removes the link, not its target.
            if fs::remove_dir(&link).is_err() {
                let _ = fs::remove_file(&link);
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Writes a skip reason to the real stderr, past the test harness capture,
/// so a skipped case is visible in a normal `cargo test` run.
fn skip(reason: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "SKIPPED: {reason}");
}

fn never() -> bool {
    false
}

/// The tree as `name` and `folder/` lines in display order, relative to the
/// root, and a check that every path is its parent's path plus its name.
fn flatten(node: &RepositoryTreeNode) -> Vec<String> {
    fn walk(node: &RepositoryTreeNode, prefix: &str, out: &mut Vec<String>) {
        for child in &node.children {
            assert_eq!(child.full_path, node.full_path.join(&child.display_name));
            let line = format!("{prefix}{}", child.display_name);
            if child.is_folder {
                assert!(!child.children.is_empty(), "{line} is an empty folder");
                out.push(format!("{line}/"));
                walk(child, &format!("{line}/"), out);
            } else {
                assert!(child.children.is_empty());
                out.push(line);
            }
        }
    }
    let mut out = Vec::new();
    walk(node, "", &mut out);
    out
}

fn tree_lines(root: &str) -> Vec<String> {
    let tree = build_repository_tree(root, &ScanLimits::default(), &never).unwrap();
    assert!(!tree.limits_hit.any(), "{:?}", tree.limits_hit);
    flatten(&tree.root)
}

fn relative_files(root: &Path, files: &[PathBuf]) -> Vec<String> {
    files
        .iter()
        .map(|file| {
            file.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

#[test]
fn the_tree_keeps_dotnet_filters_pruning_and_order() {
    let tree = TempTree::new("order");
    for file in [
        "b.md",
        "a.markdown",
        "_under.md",
        "Zeta.PNG",
        ".md",
        "notes.txt",
        "README",
        "docs/guide.mdown",
        "Alpha/x.mkdn",
        "onlytxt/a.txt",
        "nested/inner/deep.md",
        "node_modules/pkg.md",
        "NODE_MODULES2/kept.md",
        "Bin/x.md",
        ".codegraph/x.md",
        "dist/x.md",
        "testresults/x.md",
        "Coverage/x.md",
        "assets/logo.svg",
        "assets/doc.PDF",
        "assets/style.css",
    ] {
        tree.file(file);
    }
    tree.dir("empty");
    tree.dir("nested/empty-too");
    let result = build_repository_tree(&tree.text(), &ScanLimits::default(), &never).unwrap();
    assert!(result.root.is_folder);
    assert_eq!(result.root.full_path, tree.root);
    assert_eq!(
        result.root.display_name,
        tree.root.file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(
        flatten(&result.root),
        [
            "Alpha/",
            "Alpha/x.mkdn",
            "assets/",
            "assets/doc.PDF",
            "assets/logo.svg",
            "docs/",
            "docs/guide.mdown",
            "nested/",
            "nested/inner/",
            "nested/inner/deep.md",
            "NODE_MODULES2/",
            "NODE_MODULES2/kept.md",
            // Files: '.' sorts first; '_' (0x5F) sorts after the uppercase
            // letters, as .NET OrdinalIgnoreCase compares uppercase forms.
            ".md",
            "a.markdown",
            "b.md",
            "Zeta.PNG",
            "_under.md",
        ]
    );
}

#[test]
fn case_variant_names_sort_ignoring_case() {
    let tree = TempTree::new("case");
    for file in [
        "Readme.md",
        "readme2.md",
        "README.markdown",
        "ab.md",
        "AA.md",
        "aC.md",
    ] {
        tree.file(file);
    }
    assert_eq!(
        tree_lines(&tree.text()),
        [
            "AA.md",
            "ab.md",
            "aC.md",
            "README.markdown",
            "Readme.md",
            "readme2.md"
        ]
    );
}

/// A per-directory case-sensitive folder can hold `a.md` and `A.md`. Both
/// are listed, next to each other, in the listing order (stable sort).
#[cfg(windows)]
#[test]
fn case_sensitive_folders_list_both_case_variants() {
    let tree = TempTree::new("casesens");
    let folder = tree.dir("cs");
    let enabled = Command::new("fsutil.exe")
        .args(["file", "setCaseSensitiveInfo"])
        .arg(&folder)
        .arg("enable")
        .output()
        .is_ok_and(|output| output.status.success());
    if !enabled {
        skip("per-directory case sensitivity is not available here");
        return;
    }
    tree.file("cs/a.md");
    tree.file("cs/A.md");
    tree.file("cs/b.md");
    let lines = tree_lines(&tree.text());
    assert_eq!(lines.len(), 4, "{lines:?}");
    assert_eq!(lines[0], "cs/");
    let mut variants = lines[1..3].to_vec();
    variants.sort();
    assert_eq!(variants, ["cs/A.md", "cs/a.md"]);
    assert_eq!(lines[3], "cs/b.md");
}

#[cfg(windows)]
#[test]
fn hidden_entries_follow_the_dotnet_rules() {
    let tree = TempTree::new("hidden");
    tree.file("visible.md");
    tree.file("secret.md");
    tree.file("hiddendir/inside.md");
    // A dot folder is not hidden on Windows; only the hidden attribute counts.
    tree.file(".github/notes.md");
    tree.hide("secret.md");
    tree.hide("hiddendir");
    assert_eq!(
        tree_lines(&tree.text()),
        [".github/", ".github/notes.md", "visible.md"]
    );
    let listed = |show_hidden| {
        let files =
            enumerate_workspace_files(&tree.text(), show_hidden, &ScanLimits::default(), &never)
                .unwrap()
                .files;
        let mut files = relative_files(&tree.root, &files);
        files.sort();
        files
    };
    assert_eq!(listed(false), [".github/notes.md", "visible.md"]);
    assert_eq!(
        listed(true),
        [
            ".github/notes.md",
            "hiddendir/inside.md",
            "secret.md",
            "visible.md"
        ]
    );
}

#[cfg(windows)]
#[test]
fn junctions_and_links_are_never_followed() {
    let outside = TempTree::new("outside");
    outside.file("leak.md");
    outside.file("deeper/leak2.md");
    let mut tree = TempTree::new("junction");
    tree.file("inner/kept.md");
    let escape = tree.junction("escape", &outside.root);
    let cycle = tree.junction("inner/cycle", &tree.root.clone());
    let inward = tree.junction("inward", &tree.path("inner"));
    if !(escape && cycle && inward) {
        skip("junction test: mklink /J is not available here");
        return;
    }
    // The links resolve: an escape would be visible if the scan followed it.
    assert!(tree.path("escape/leak.md").is_file());
    let file_link = tree.file_symlink("linked.md", &outside.path("leak.md"));
    assert_eq!(tree_lines(&tree.text()), ["inner/", "inner/kept.md"]);
    let files = enumerate_workspace_files(&tree.text(), true, &ScanLimits::default(), &never)
        .unwrap()
        .files;
    assert_eq!(relative_files(&tree.root, &files), ["inner/kept.md"]);
    if !file_link {
        skip("file symbolic link case: mklink needs Developer Mode or elevation");
    }

    // A root that is itself a junction is scanned (the user chose it), and
    // the paths stay under the junction path, as in .NET.
    let mut holder = TempTree::new("rootlink");
    if holder.junction("root", &outside.root) {
        let root = holder.path("root");
        let result =
            build_repository_tree(root.to_str().unwrap(), &ScanLimits::default(), &never).unwrap();
        assert_eq!(
            flatten(&result.root),
            ["deeper/", "deeper/leak2.md", "leak.md"]
        );
        assert!(
            result
                .root
                .children
                .iter()
                .all(|child| child.full_path.starts_with(&root))
        );
    }
}

#[test]
fn deep_trees_are_walked_without_recursion_and_bounded_by_depth() {
    let tree = TempTree::new("deep");
    let mut relative = String::new();
    for level in 1..=150 {
        relative.push_str("d/");
        if level == 10 {
            tree.file(&format!("{relative}ten.md"));
        }
    }
    // Longer than MAX_PATH (260): the walk must handle long paths.
    let bottom = tree.file(&format!("{relative}bottom.md"));
    assert!(bottom.as_os_str().len() > 260);

    let shallow = build_repository_tree(&tree.text(), &ScanLimits::default(), &never).unwrap();
    assert!(shallow.limits_hit.depth);
    let lines = flatten(&shallow.root);
    assert_eq!(lines.last().unwrap(), &format!("{}ten.md", "d/".repeat(10)));
    assert_eq!(lines.len(), 11);

    let limits = ScanLimits {
        max_depth: 200,
        ..ScanLimits::default()
    };
    let full = build_repository_tree(&tree.text(), &limits, &never).unwrap();
    assert!(!full.limits_hit.any());
    let lines = flatten(&full.root);
    assert_eq!(lines.len(), 152);
    assert!(lines.contains(&format!("{relative}bottom.md")));
    let files = enumerate_workspace_files(&tree.text(), false, &limits, &never).unwrap();
    assert_eq!(files.files.len(), 2);
    let files =
        enumerate_workspace_files(&tree.text(), false, &ScanLimits::default(), &never).unwrap();
    assert_eq!(files.files.len(), 1);
    assert!(files.limits_hit.depth);
}

#[test]
fn many_files_are_sorted_and_bounded_by_count_and_entries() {
    let tree = TempTree::new("many");
    for index in 0..2500 {
        tree.file(&format!("f{index:04}.md"));
        tree.file(&format!("sub/g{index:04}.md"));
    }
    tree.file("sub/ignored.txt");
    let result = build_repository_tree(&tree.text(), &ScanLimits::default(), &never).unwrap();
    assert!(!result.limits_hit.any());
    let lines = flatten(&result.root);
    assert_eq!(lines.len(), 5001);
    assert_eq!(lines[0], "sub/");
    assert_eq!(lines[1], "sub/g0000.md");
    assert_eq!(lines[2501], "f0000.md");
    assert_eq!(lines[5000], "f2499.md");

    let limits = ScanLimits {
        max_files: 1000,
        ..ScanLimits::default()
    };
    let capped = build_repository_tree(&tree.text(), &limits, &never).unwrap();
    assert!(capped.limits_hit.files);
    let count = flatten(&capped.root)
        .iter()
        .filter(|line| !line.ends_with('/'))
        .count();
    assert_eq!(count, 1000);
    let files = enumerate_workspace_files(&tree.text(), false, &limits, &never).unwrap();
    assert!(files.limits_hit.files);
    assert_eq!(files.files.len(), 1000);

    let limits = ScanLimits {
        max_entries: 10,
        ..ScanLimits::default()
    };
    let capped = build_repository_tree(&tree.text(), &limits, &never).unwrap();
    assert!(capped.limits_hit.entries);
    // The root lists f0000.md to f2499.md before `sub`: exactly 10 files.
    let lines = flatten(&capped.root);
    assert_eq!(lines.len(), 10);
    assert_eq!(lines[0], "f0000.md");
    assert_eq!(lines[9], "f0009.md");
}

#[test]
fn cancellation_discards_partial_results() {
    let tree = TempTree::new("cancel");
    for index in 0..300 {
        tree.file(&format!("dir{}/f{index}.md", index % 7));
    }
    assert_eq!(
        build_repository_tree(&tree.text(), &ScanLimits::default(), &|| true),
        Err(ScanError::Cancelled)
    );
    for after in [1, 2, 50, 200] {
        let polls = AtomicUsize::new(0);
        let cancel = || polls.fetch_add(1, Ordering::Relaxed) >= after;
        assert_eq!(
            build_repository_tree(&tree.text(), &ScanLimits::default(), &cancel),
            Err(ScanError::Cancelled),
            "tree, cancelled after {after} polls"
        );
        let polls = AtomicUsize::new(0);
        let cancel = || polls.fetch_add(1, Ordering::Relaxed) >= after;
        assert_eq!(
            enumerate_workspace_files(&tree.text(), false, &ScanLimits::default(), &cancel),
            Err(ScanError::Cancelled),
            "workspace, cancelled after {after} polls"
        );
    }

    // The shell's shape: a worker thread with a shared cancel flag.
    let flag = Arc::new(AtomicBool::new(false));
    let worker = {
        let flag = Arc::clone(&flag);
        let root = tree.text();
        std::thread::spawn(move || {
            build_repository_tree(&root, &ScanLimits::default(), &|| {
                flag.load(Ordering::Acquire)
            })
        })
    };
    let finished = worker.join().unwrap().unwrap();
    assert_eq!(flatten(&finished.root).len(), 307);
    flag.store(true, Ordering::Release);
    let worker = {
        let flag = Arc::clone(&flag);
        let root = tree.text();
        std::thread::spawn(move || {
            build_repository_tree(&root, &ScanLimits::default(), &|| {
                flag.load(Ordering::Acquire)
            })
        })
    };
    assert_eq!(worker.join().unwrap(), Err(ScanError::Cancelled));
}

#[test]
fn missing_and_refused_roots() {
    let tree = TempTree::new("roots");
    let missing = tree.path("missing");
    let result =
        build_repository_tree(missing.to_str().unwrap(), &ScanLimits::default(), &never).unwrap();
    assert_eq!(result.root.display_name, "missing");
    assert!(result.root.children.is_empty());
    let file = tree.file("file.md");
    let result =
        build_repository_tree(file.to_str().unwrap(), &ScanLimits::default(), &never).unwrap();
    assert!(result.root.children.is_empty());
    assert!(
        enumerate_workspace_files(
            missing.to_str().unwrap(),
            false,
            &ScanLimits::default(),
            &never
        )
        .unwrap()
        .files
        .is_empty()
    );
    for (root, error) in [
        // `\\.\` is refused for its `.` component.
        (r"\\.\C:\Repo", PathError::Traversal),
        (
            r"\\?\GLOBALROOT\Device\HarddiskVolume1",
            PathError::NetworkOrDevice,
        ),
        (r"\\server\pipe\x", PathError::NetworkOrDevice),
        ("relative\\repo", PathError::NotAbsolute),
        (r"C:\Repo\..\Other", PathError::Traversal),
        (r"C:\Repo\NUL", PathError::InvalidComponent),
    ] {
        assert_eq!(
            build_repository_tree(root, &ScanLimits::default(), &never),
            Err(ScanError::InvalidRoot(error.clone())),
            "{root}"
        );
        assert_eq!(
            enumerate_workspace_files(root, false, &ScanLimits::default(), &never),
            Err(ScanError::InvalidRoot(error)),
            "{root}"
        );
    }
}

/// A UNC root through the local administrative share, when this host
/// allows it: the same tree as the drive path, with UNC paths.
#[cfg(windows)]
#[test]
fn a_unc_root_scans_like_a_drive_root() {
    let tree = TempTree::new("unc");
    tree.file("docs/a.md");
    tree.file("b.png");
    let local = tree.text();
    let (drive, rest) = local.split_at(2);
    let unc = format!(r"\\localhost\{}${}", &drive[..1], rest);
    if fs::metadata(&unc).is_err() {
        skip(&format!("UNC test: {unc} is not reachable here"));
        return;
    }
    let result = build_repository_tree(&unc, &ScanLimits::default(), &never).unwrap();
    assert_eq!(flatten(&result.root), ["docs/", "docs/a.md", "b.png"]);
    assert!(
        result
            .root
            .full_path
            .to_str()
            .unwrap()
            .starts_with(r"\\localhost\")
    );
    let verbatim = format!(r"\\?\UNC\localhost\{}${}", &drive[..1], rest);
    let result = build_repository_tree(&verbatim, &ScanLimits::default(), &never).unwrap();
    assert_eq!(flatten(&result.root), ["docs/", "docs/a.md", "b.png"]);
}

#[cfg(windows)]
#[test]
fn workspace_files_are_listed_in_dotnet_preorder() {
    let tree = TempTree::new("workspace");
    for file in [
        "a.md",
        "b.png",
        "c.txt",
        "dist/x.md",
        "artifacts/y.md",
        "bin/no.md",
        "OBJ/no.md",
        "sub/y.md",
        "sub/z/w.md",
    ] {
        tree.file(file);
    }
    let files =
        enumerate_workspace_files(&tree.text(), false, &ScanLimits::default(), &never).unwrap();
    assert!(!files.limits_hit.any());
    // NTFS lists names in uppercase order; each folder's files come first.
    assert_eq!(
        relative_files(&tree.root, &files.files),
        [
            "a.md",
            "b.png",
            "artifacts/y.md",
            "dist/x.md",
            "sub/y.md",
            "sub/z/w.md"
        ]
    );
}

#[test]
fn repository_roots_and_contexts_match_the_dotnet_markers() {
    let detect = |path: &str| detect_repository_root(path).unwrap();
    let tree = TempTree::new("detect");
    tree.dir("git/.git");
    let doc = tree.file("git/a/b/doc.md");
    assert_eq!(detect(doc.to_str().unwrap()), Some(tree.path("git")));
    // A folder path starts the search at that folder.
    assert_eq!(
        detect(tree.path("git/a").to_str().unwrap()),
        Some(tree.path("git"))
    );
    tree.file("worktree/.git");
    let doc = tree.file("worktree/doc.md");
    assert_eq!(detect(doc.to_str().unwrap()), Some(tree.path("worktree")));
    tree.file("solution/Product.SLNX");
    let doc = tree.file("solution/docs/doc.md");
    assert_eq!(detect(doc.to_str().unwrap()), Some(tree.path("solution")));
    // A folder named like a solution is not a marker.
    tree.dir("plain/fake.sln");
    let doc = tree.file("plain/doc.md");
    let found = detect(doc.to_str().unwrap());
    assert!(
        found
            .as_ref()
            .is_none_or(|found| !found.starts_with(&tree.root)),
        "{found:?}"
    );
    assert_eq!(detect(""), None);
    assert_eq!(detect(r"\\.\pipe\x"), None);

    let git = create_context(tree.path("git").to_str().unwrap(), true).unwrap();
    assert_eq!(git.kind, RepositoryKind::GitRepository);
    assert!(git.enforce_sandbox);
    assert_eq!(git.root, tree.path("git"));
    let folder = create_context(&format!("{}/plain/", tree.text()), false).unwrap();
    assert_eq!(folder.kind, RepositoryKind::Folder);
    assert_eq!(folder.root, tree.path("plain"));
    assert!(create_context(tree.path("missing").to_str().unwrap(), true).is_none());
    assert!(create_context("", true).is_none());
}

/// Denies "list folder" to Everyone on one test folder; the drop restores it.
#[cfg(windows)]
struct DenyListing {
    path: PathBuf,
}

#[cfg(windows)]
impl DenyListing {
    /// `None` (with a SKIPPED line) when the deny ACE cannot be set or does
    /// not stop listing here (for example under backup privileges).
    fn new(path: PathBuf) -> Option<Self> {
        let denied = Command::new("icacls")
            .arg(&path)
            .args(["/deny", "*S-1-1-0:(RD)"])
            .output()
            .is_ok_and(|output| output.status.success());
        let guard = Self { path };
        if !denied {
            skip("access-denied case: icacls /deny failed");
            return None;
        }
        if fs::read_dir(&guard.path).is_ok() {
            skip("access-denied case: listing is still allowed for this account");
            return None;
        }
        Some(guard)
    }
}

#[cfg(windows)]
impl Drop for DenyListing {
    fn drop(&mut self) {
        let _ = Command::new("icacls")
            .arg(&self.path)
            .args(["/remove:d", "*S-1-1-0"])
            .output();
    }
}

#[cfg(windows)]
#[test]
fn an_access_denied_folder_gives_an_empty_listing() {
    let tree = TempTree::new("denied");
    tree.file("open/a.md");
    tree.file("locked/inside.md");
    let Some(_deny) = DenyListing::new(tree.path("locked")) else {
        return;
    };
    assert_eq!(tree_lines(&tree.text()), ["open/", "open/a.md"]);
    let files = enumerate_workspace_files(&tree.text(), true, &ScanLimits::default(), &never)
        .unwrap()
        .files;
    assert_eq!(relative_files(&tree.root, &files), ["open/a.md"]);
}

/// RS-5: access denied while looking for `*.sln` means "no marker", and the
/// search goes on upwards (.NET would throw).
#[cfg(windows)]
#[test]
fn root_detection_continues_past_an_access_denied_folder() {
    let tree = TempTree::new("rs5");
    tree.file("outer/Product.sln");
    let doc = tree.file("outer/locked/doc.md");
    let Some(_deny) = DenyListing::new(tree.path("outer/locked")) else {
        return;
    };
    assert_eq!(
        detect_repository_root(doc.to_str().unwrap()),
        Ok(Some(tree.path("outer")))
    );
}

/// A folder that disappears between its listing and its scan fails the
/// whole scan with `ScanError::Io`, as the .NET `IOException` does.
#[test]
fn a_folder_removed_during_the_scan_is_an_io_error() {
    for workspace in [false, true] {
        let tree = TempTree::new("io");
        tree.file("doomed/a.md");
        let doomed = tree.path("doomed");
        let polls = AtomicUsize::new(0);
        // Poll 0 is the scan start, poll 1 the root listing, poll 2 the
        // first root entry (`doomed`, already listed): remove it then.
        let cancel = || {
            if polls.fetch_add(1, Ordering::Relaxed) >= 2 && doomed.exists() {
                fs::remove_dir_all(&doomed).unwrap();
            }
            false
        };
        let result = if workspace {
            enumerate_workspace_files(&tree.text(), false, &ScanLimits::default(), &cancel)
                .map(drop)
        } else {
            build_repository_tree(&tree.text(), &ScanLimits::default(), &cancel).map(drop)
        };
        assert_eq!(
            result,
            Err(ScanError::Io(std::io::ErrorKind::NotFound)),
            "workspace: {workspace}"
        );
    }
}

/// RS-3: names that the document gate refuses are skipped; a refused folder
/// drops its whole subtree. The names are created through `\\?\` paths.
#[cfg(windows)]
#[test]
fn names_refused_by_the_gate_are_skipped() {
    let tree = TempTree::new("rs3");
    tree.file("keep.md");
    let verbatim = |name: &str| PathBuf::from(format!(r"\\?\{}\{name}", tree.text()));
    fs::write(verbatim("CON.md"), b"# x\n").unwrap();
    fs::write(verbatim("nul.png"), b"x").unwrap();
    fs::write(verbatim("trail.md."), b"# x\n").unwrap();
    fs::write(verbatim("space.md "), b"# x\n").unwrap();
    fs::create_dir(verbatim("dirdot.")).unwrap();
    fs::write(verbatim(r"dirdot.\inside.md"), b"# x\n").unwrap();
    assert!(verbatim("CON.md").is_file());
    assert_eq!(tree_lines(&tree.text()), ["keep.md"]);
    let files = enumerate_workspace_files(&tree.text(), true, &ScanLimits::default(), &never)
        .unwrap()
        .files;
    assert_eq!(relative_files(&tree.root, &files), ["keep.md"]);
    for name in [
        "CON.md",
        "nul.png",
        "trail.md.",
        "space.md ",
        r"dirdot.\inside.md",
    ] {
        fs::remove_file(verbatim(name)).unwrap();
    }
    fs::remove_dir(verbatim("dirdot.")).unwrap();
}

/// A path longer than the gate's 32 KiB (UTF-8) limit is skipped and
/// reported through `LimitsHit::path_length`, not dropped silently. Each
/// name is 200 CJK characters: 600 UTF-8 bytes but 200 UTF-16 units, so
/// Windows (32,767 UTF-16 units) can still hold the path.
#[test]
fn paths_over_the_gate_length_are_reported() {
    let tree = TempTree::new("long");
    tree.file("near.md");
    let name = "語".repeat(200);
    let mut relative = String::new();
    for _ in 0..56 {
        relative.push_str(&name);
        relative.push('/');
    }
    let deep = tree.file(&format!("{relative}far.md"));
    assert!(deep.as_os_str().len() > 32 * 1024);
    let limits = ScanLimits {
        max_depth: 100,
        ..ScanLimits::default()
    };
    let result = build_repository_tree(&tree.text(), &limits, &never).unwrap();
    assert!(result.limits_hit.path_length);
    assert!(result.limits_hit.any());
    let lines = flatten(&result.root);
    assert_eq!(lines.last().unwrap(), "near.md");
    assert!(!lines.iter().any(|line| line.ends_with("far.md")));
    let files = enumerate_workspace_files(&tree.text(), false, &limits, &never).unwrap();
    assert!(files.limits_hit.path_length);
    assert_eq!(relative_files(&tree.root, &files.files), ["near.md"]);
}

#[test]
fn the_path_byte_budget_bounds_what_the_scan_keeps() {
    let tree = TempTree::new("bytes");
    for index in 0..10 {
        tree.file(&format!("f{index}.md"));
    }
    let one = tree.path("f0.md").as_os_str().len();
    let limits = ScanLimits {
        max_path_bytes: 3 * one,
        ..ScanLimits::default()
    };
    let result = build_repository_tree(&tree.text(), &limits, &never).unwrap();
    assert!(result.limits_hit.bytes);
    assert_eq!(flatten(&result.root), ["f0.md", "f1.md", "f2.md"]);
    let files = enumerate_workspace_files(&tree.text(), false, &limits, &never).unwrap();
    assert!(files.limits_hit.bytes);
    assert_eq!(files.files.len(), 3);
}

#[test]
fn verbatim_roots_with_forward_slashes_are_refused() {
    assert_eq!(
        build_repository_tree(r"\\?\C:/Repo", &ScanLimits::default(), &never),
        Err(ScanError::InvalidRoot(PathError::InvalidComponent))
    );
}
