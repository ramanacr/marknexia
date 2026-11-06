//! Startup file resolution.
//!
//! Port of `src/Marknexia.Core/StartupFileResolver.cs`. It selects the first
//! usable Markdown file from the candidates that Windows activation, the
//! command line, or a direct executable invocation supplies.
//!
//! The logic is portable. File-system access goes through
//! [`StartupFileProbe`]; `marknexia-files` supplies the standard adapter.

use std::borrow::Cow;

use crate::text::{eq_ignore_case, is_null_or_white_space};

/// Extensions that .NET accepts, compared case-insensitively.
pub const MARKDOWN_EXTENSIONS: [&str; 4] = [".md", ".markdown", ".mdown", ".mkdn"];

/// File-system operations that the resolver needs.
pub trait StartupFileProbe {
    /// Returns the absolute form of `path` (.NET `Path.GetFullPath`), or
    /// `None` when the path cannot be made absolute.
    fn full_path(&self, path: &str) -> Option<String>;

    /// Returns `true` when `full_path` names an existing file, not a
    /// directory (.NET `File.Exists`).
    fn is_file(&self, full_path: &str) -> bool;
}

/// Returns the full path of the first candidate that names an existing
/// Markdown file.
///
/// For each candidate that is not empty or white space, the resolver first
/// tries the whole candidate (trimmed, then with surrounding `"` removed),
/// because a file path from activation can contain spaces. It then splits
/// the candidate with Windows argv rules and tries each argument in order.
pub fn find_first_existing<'a, I, P>(candidates: I, probe: &P) -> Option<String>
where
    I: IntoIterator<Item = Option<&'a str>>,
    P: StartupFileProbe + ?Sized,
{
    for candidate in candidates {
        let Some(candidate) = candidate else {
            continue;
        };
        if is_null_or_white_space(Some(candidate)) {
            continue;
        }

        let direct = candidate.trim().trim_matches('"');
        if let Some(path) = existing_markdown(direct, probe) {
            return Some(path);
        }
        for argument in split_command_line(candidate) {
            if let Some(path) = existing_markdown(&argument, probe) {
                return Some(path);
            }
        }
    }
    None
}

fn existing_markdown<P: StartupFileProbe + ?Sized>(path: &str, probe: &P) -> Option<String> {
    let local: Cow<'_, str> = if is_file_uri(path) {
        Cow::Owned(file_uri_to_local_path(path)?)
    } else {
        Cow::Borrowed(path)
    };
    if !has_markdown_extension(&local) {
        return None;
    }
    let full_path = probe.full_path(&local)?;
    probe.is_file(&full_path).then_some(full_path)
}

/// Returns `true` when the extension (as .NET `Path.GetExtension` finds it)
/// is one of [`MARKDOWN_EXTENSIONS`], ignoring case.
#[must_use]
pub fn has_markdown_extension(path: &str) -> bool {
    let extension = extension(path);
    MARKDOWN_EXTENSIONS
        .iter()
        .any(|known| eq_ignore_case(known, extension))
}

/// .NET `Path.GetExtension` on Windows: the text from the last `.` after the
/// last directory separator, or empty when the `.` is the last character.
fn extension(path: &str) -> &str {
    for (index, character) in path.char_indices().rev() {
        match character {
            '.' if index + 1 == path.len() => return "",
            '.' => return &path[index..],
            '\\' | '/' => break,
            _ => {}
        }
    }
    ""
}

fn is_file_uri(path: &str) -> bool {
    path.get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"))
}

/// Converts a `file:` URI to a Windows path (.NET `Uri.LocalPath`).
///
/// Supported forms: `file:///C:/dir/a.md`, `file://localhost/C:/dir/a.md`
/// and `file://server/share/a.md` (UNC). The fragment is removed and the
/// path is percent-decoded as UTF-8. Other forms return `None`.
#[must_use]
pub fn file_uri_to_local_path(uri: &str) -> Option<String> {
    if !is_file_uri(uri) {
        return None;
    }
    let rest = &uri[5..];
    let rest = rest.split_once('#').map_or(rest, |(path, _)| path);
    let slashes = rest.len() - rest.trim_start_matches(['/', '\\']).len();
    let after_slashes = &rest[slashes..];

    let (host, path) = if slashes == 2 && !starts_with_drive(after_slashes) {
        match after_slashes.find(['/', '\\']) {
            Some(index) => (&after_slashes[..index], &after_slashes[index + 1..]),
            None => (after_slashes, ""),
        }
    } else {
        ("", after_slashes)
    };

    let decoded_path = percent_decode(path)?.replace('/', "\\");
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        if !starts_with_drive(&decoded_path) {
            return None;
        }
        let mut drive_path = decoded_path;
        drive_path.replace_range(1..2, ":");
        return Some(drive_path);
    }
    let host = percent_decode(host)?;
    Some(format!("\\\\{host}\\{decoded_path}"))
}

/// `X:` or `X|` (the legacy URI drive form) at the start of `text`.
fn starts_with_drive(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && (bytes[1] == b':' || bytes[1] == b'|')
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes.get(index + 1).and_then(|b| (*b as char).to_digit(16));
            let low = bytes.get(index + 2).and_then(|b| (*b as char).to_digit(16));
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push(u8::try_from(high * 16 + low).ok()?);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

/// Splits a command line with Windows argv rules, as the .NET resolver does.
///
/// White space outside quotes separates arguments. Backslashes are literal
/// except before a quote: `2n` backslashes and a quote give `n` backslashes
/// and toggle quoting; `2n+1` give `n` backslashes and a literal quote.
/// Empty arguments are dropped, and so is a final argument whose quote is
/// not closed.
#[must_use]
pub fn split_command_line(text: &str) -> Vec<String> {
    let characters: Vec<char> = text.chars().collect();
    let mut arguments = Vec::new();
    let mut argument = String::new();
    let mut quoted = false;
    let mut index = 0;
    while index < characters.len() {
        let current = characters[index];
        if current == '\\' {
            let start = index;
            while index < characters.len() && characters[index] == '\\' {
                index += 1;
            }
            let count = index - start;
            if index < characters.len() && characters[index] == '"' {
                argument.extend(std::iter::repeat_n('\\', count / 2));
                if count % 2 != 0 {
                    argument.push('"');
                } else {
                    quoted = !quoted;
                }
            } else {
                argument.extend(std::iter::repeat_n('\\', count));
                index -= 1;
            }
        } else if current == '"' {
            quoted = !quoted;
        } else if current.is_whitespace() && !quoted {
            if !argument.is_empty() {
                arguments.push(std::mem::take(&mut argument));
            }
        } else {
            argument.push(current);
        }
        index += 1;
    }
    if !quoted && !argument.is_empty() {
        arguments.push(argument);
    }
    arguments
}
