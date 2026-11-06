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
