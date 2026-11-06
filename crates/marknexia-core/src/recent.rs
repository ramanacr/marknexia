//! Recent files and folders, as `SettingsService.cs` and `MainWindow` keep them.
//!
//! * Newest first, capped (15 files, 10 folders).
//! * Duplicates compare ordinal and case-insensitive; the first (newest)
//!   occurrence wins.
//! * Entries are trimmed; empty or white-space entries are dropped.
//! * Missing files stay in the list. The sidebar shows only the first
//!   [`RECENT_FILES_DISPLAY_LIMIT`] entries that exist.
//! * Rust also drops entries that are not absolute local or UNC paths
//!   (SET-3 in `compat/decisions/settings-and-startup-differences.md`).

use crate::text::{eq_ignore_case, is_null_or_white_space};

pub const MAX_RECENT_FILES: usize = 15;
pub const MAX_RECENT_FOLDERS: usize = 10;
/// `MainWindow.RefreshRecentFiles` shows at most this many existing files.
pub const RECENT_FILES_DISPLAY_LIMIT: usize = 5;
/// Longest accepted path, in UTF-16 code units (the Windows long-path limit).
pub const MAX_PATH_UTF16_UNITS: usize = 32_767;

/// Adds `path` to the front of `list`, as `AddRecentFile`/`AddRecentFolder`
/// do, then normalizes the list.
///
/// The steps are a literal port: remove entries equal to the raw `path`
/// (ignoring case), insert the raw `path` first, truncate to `cap`, then
/// normalize (trim, validate, de-duplicate, truncate). Returns `false`
/// without change when `path` is empty or white space; .NET does not save
/// in that case either.
pub fn push_recent(list: &mut Vec<String>, path: &str, cap: usize) -> bool {
    if is_null_or_white_space(Some(path)) {
        return false;
    }
    list.retain(|existing| !eq_ignore_case(existing, path));
    list.insert(0, path.to_owned());
    list.truncate(cap);
    normalize_recent(list, cap);
    true
}

/// .NET `NormalizePaths`: drop empty or white-space entries, trim, keep the
/// first of each case-insensitive duplicate, and keep at most `cap` entries.
/// Rust also drops entries that fail [`is_acceptable_path`].
pub fn normalize_recent(list: &mut Vec<String>, cap: usize) {
    let mut normalized: Vec<String> = Vec::with_capacity(list.len().min(cap));
    for entry in list.drain(..) {
        if normalized.len() == cap {
            break;
        }
        let trimmed = entry.trim();
        if trimmed.is_empty() || !is_acceptable_path(trimmed) {
            continue;
        }
        if normalized.iter().any(|kept| eq_ignore_case(kept, trimmed)) {
            continue;
        }
        normalized.push(trimmed.to_owned());
    }
    *list = normalized;
}

/// The recent files to show: the first `limit` entries for which `exists`
/// returns `true`, in list order.
pub fn visible_recent(
    list: &[String],
    mut exists: impl FnMut(&str) -> bool,
    limit: usize,
) -> Vec<&str> {
    list.iter()
        .map(String::as_str)
        .filter(|path| exists(path))
        .take(limit)
        .collect()
}

/// Lexical check for a stored path: an absolute drive path (`C:\...` or
/// `C:/...`) or a UNC path (`\\server\share...`), without control
/// characters, within [`MAX_PATH_UTF16_UNITS`]. Device and verbatim
/// prefixes (`\\?\`, `\\.\`) are rejected.
///
/// This check does not touch the file system. It does not prove that the
/// path exists or is safe to open; the platform path gate still applies.
#[must_use]
pub fn is_acceptable_path(path: &str) -> bool {
    if path.chars().any(char::is_control) || path.encode_utf16().count() > MAX_PATH_UTF16_UNITS {
        return false;
    }
    let bytes = path.as_bytes();
    let is_separator = |byte: u8| byte == b'\\' || byte == b'/';
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return is_separator(bytes[2]);
    }
    if bytes.len() >= 2 && is_separator(bytes[0]) && is_separator(bytes[1]) {
        let rest = &path[2..];
        let mut parts = rest.split(['\\', '/']);
        let server = parts.next().unwrap_or_default();
        let share = parts.next().unwrap_or_default();
        return !server.is_empty() && server != "?" && server != "." && !share.is_empty();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_drive_and_unc_paths_only() {
        assert!(is_acceptable_path("C:\\docs\\a.md"));
        assert!(is_acceptable_path("c:/docs/a.md"));
        assert!(is_acceptable_path("\\\\server\\share\\a.md"));
        assert!(is_acceptable_path("\\\\server\\share"));
        assert!(!is_acceptable_path("\\\\server"));
        assert!(!is_acceptable_path("\\\\?\\C:\\a.md"));
        assert!(!is_acceptable_path("\\\\.\\COM1"));
        assert!(!is_acceptable_path("docs\\a.md"));
        assert!(!is_acceptable_path("C:a.md"));
        assert!(!is_acceptable_path("\\docs\\a.md"));
        assert!(!is_acceptable_path("C:\\a\u{0}.md"));
        assert!(!is_acceptable_path("C:\\a\n.md"));
        assert!(!is_acceptable_path(&format!("C:\\{}", "a".repeat(32_765))));
        assert!(is_acceptable_path(&format!("C:\\{}", "a".repeat(32_764))));
    }

    #[test]
    fn push_matches_dotnet_order_of_operations() {
        // The raw input is compared before trimming, so a padded duplicate
        // survives the removal step. The cap then cuts the oldest entry, and
        // normalization removes the duplicate: the full list ends one entry
        // short, as in .NET.
        let mut list: Vec<String> = (0..15).map(|i| format!("C:\\d\\{i}.md")).collect();
        assert!(push_recent(&mut list, "  C:\\d\\5.md ", 15));
        assert_eq!(list.len(), 14);
        assert_eq!(list[0], "C:\\d\\5.md");
        assert_eq!(list[1], "C:\\d\\0.md");
        assert_eq!(list[13], "C:\\d\\13.md");
    }

    #[test]
    fn push_ignores_white_space_and_dedupes_case_insensitively() {
        let mut list = vec!["C:\\A.md".to_owned()];
        assert!(!push_recent(&mut list, " \t", 15));
        assert!(push_recent(&mut list, "c:\\a.MD", 15));
        assert_eq!(list, ["c:\\a.MD"]);
    }

    #[test]
    fn visible_keeps_order_skips_missing_and_limits() {
        let list: Vec<String> = (0..8).map(|i| format!("C:\\{i}.md")).collect();
        let shown = visible_recent(&list, |path| path != "C:\\1.md", RECENT_FILES_DISPLAY_LIMIT);
        assert_eq!(
            shown,
            ["C:\\0.md", "C:\\2.md", "C:\\3.md", "C:\\4.md", "C:\\5.md"]
        );
    }
}
