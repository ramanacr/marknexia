//! Atomic file replacement.
//!
//! The bytes go to a new temporary file in the target's directory. The file
//! is flushed to disk, then renamed over the target. On Windows,
//! `std::fs::rename` replaces an existing target with `MoveFileExW` and
//! `MOVEFILE_REPLACE_EXISTING` (or the equivalent POSIX-semantics rename),
//! so a reader sees either the old file or the new file, never a partial
//! one. The temporary file is removed when any step fails.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Replaces `path` with `bytes` atomically and creates missing parent
/// directories.
///
/// # Errors
///
/// Returns the I/O error of the step that failed. The target is unchanged
/// in that case.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    let (temporary_path, mut file) = create_temporary(path)?;
    let result = write_and_replace(&mut file, &temporary_path, path, bytes);
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn write_and_replace(
    file: &mut File,
    temporary_path: &Path,
    path: &Path,
    bytes: &[u8],
) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary_path, path)
}

/// Writes `bytes` to `path` atomically, but only when `path` does not exist.
///
/// The bytes go to a flushed temporary file first, which is then hard-linked
/// to `path`. Creating a hard link fails when the target exists, so an
/// existing file is never replaced.
///
/// # Errors
///
/// Returns `AlreadyExists` when `path` exists, or the I/O error of the step
/// that failed. Nothing is left at `path` after a failure.
pub fn write_new_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    let (temporary_path, mut file) = create_temporary(path)?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            fs::hard_link(&temporary_path, path)
        });
    let _ = fs::remove_file(&temporary_path);
    result
}

/// Moves `from` to `to` without replacing an existing `to`.
///
/// `std::fs::rename` always replaces on Windows, so this creates a hard link
/// (which fails when `to` exists) and then removes `from`. If `from` cannot
/// be removed, the link is removed again and `from` is unchanged.
///
/// # Errors
///
/// Returns `AlreadyExists` when `to` exists, or the I/O error of the step
/// that failed.
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::hard_link(from, to)?;
    if let Err(error) = fs::remove_file(from) {
        let _ = fs::remove_file(to);
        return Err(error);
    }
    Ok(())
}

/// Creates `<name>.<unique>.tmp` next to `path` with `create_new`, so an
/// existing file is never reused.
fn create_temporary(path: &Path) -> io::Result<(PathBuf, File)> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    for _ in 0..16 {
        let mut temporary_name = name.to_os_string();
        temporary_name.push(format!(".{}.tmp", unique_suffix()));
        let temporary_path = path.with_file_name(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not create a unique temporary file",
    ))
}

/// Process id, time and a per-process counter, in hexadecimal.
pub(crate) fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{:x}{:x}{:x}", std::process::id(), nanos, sequence)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("marknexia-atomic-{name}-{}", unique_suffix()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn entries(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn creates_and_replaces_without_leaving_temporary_files() {
        let directory = scratch("replace");
        let target = directory.join("nested").join("settings.json");
        write_atomically(&target, b"first").unwrap();
        write_atomically(&target, b"second").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");
        assert_eq!(entries(&directory.join("nested")), ["settings.json"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn write_new_never_replaces_an_existing_file() {
        let directory = scratch("new");
        let target = directory.join("backup.bak");
        write_new_atomically(&target, b"first").unwrap();
        let error = write_new_atomically(&target, b"second").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&target).unwrap(), b"first");
        assert_eq!(entries(&directory), ["backup.bak"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rename_no_replace_keeps_an_existing_target() {
        let directory = scratch("move");
        let source = directory.join("a");
        let target = directory.join("b");
        fs::write(&source, b"source").unwrap();
        fs::write(&target, b"target").unwrap();
        let error = rename_no_replace(&source, &target).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&target).unwrap(), b"target");
        fs::remove_file(&target).unwrap();
        rename_no_replace(&source, &target).unwrap();
        assert_eq!(entries(&directory), ["b"]);
        assert_eq!(fs::read(&target).unwrap(), b"source");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn replacing_a_read_only_target_fails_and_keeps_it() {
        let directory = scratch("read-only");
        let target = directory.join("settings.json");
        fs::write(&target, b"old").unwrap();
        let mut permissions = fs::metadata(&target).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&target, permissions.clone()).unwrap();

        let result = write_atomically(&target, b"new");

        let contents = fs::read(&target).unwrap();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&target, permissions).unwrap();
        assert!(result.is_err(), "a read-only target must not be replaced");
        assert_eq!(contents, b"old");
        assert_eq!(entries(&directory), ["settings.json"]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_replace_keeps_the_target_and_removes_the_temporary_file() {
        let directory = scratch("fail");
        // A directory cannot be replaced by a file rename.
        let target = directory.join("settings.json");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), b"keep").unwrap();
        assert!(write_atomically(&target, b"new").is_err());
        assert_eq!(entries(&directory), ["settings.json"]);
        assert_eq!(fs::read(target.join("keep.txt")).unwrap(), b"keep");
        fs::remove_dir_all(directory).unwrap();
    }
}
