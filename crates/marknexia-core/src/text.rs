//! Text comparison helpers that approximate .NET `StringComparison.OrdinalIgnoreCase`.
//!
//! .NET folds each UTF-16 code unit with its invariant *simple* uppercase
//! mapping. Rust exposes only the full mapping (`char::to_uppercase`). This
//! module uses the full mapping when it yields exactly one character and keeps
//! the character unchanged otherwise. That matches .NET for every character
//! whose full and simple mappings agree. It differs for the few characters
//! whose simple mapping is one character but whose full mapping is several
//! (for example Greek letters with iota subscript, U+1F80 to U+1FFC). See
//! `compat/decisions/settings-and-startup-differences.md` (SET-6).

/// Folds one character for an ordinal, case-insensitive comparison.
#[must_use]
pub fn fold_char(character: char) -> char {
    let mut upper = character.to_uppercase();
    match (upper.next(), upper.next()) {
        (Some(single), None) => single,
        _ => character,
    }
}

/// Returns `true` when both strings are equal under ordinal, case-insensitive
/// comparison (the `StringComparison.OrdinalIgnoreCase` approximation).
#[must_use]
pub fn eq_ignore_case(left: &str, right: &str) -> bool {
    let mut left_chars = left.chars();
    let mut right_chars = right.chars();
    loop {
        match (left_chars.next(), right_chars.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if a == b || fold_char(a) == fold_char(b) => {}
            _ => return false,
        }
    }
}

/// Counts non-overlapping, case-insensitive occurrences of `query` in
/// `source`, scanning left to right as .NET `IndexOf` does in a loop.
///
/// An empty `query` returns 0.
#[must_use]
pub fn count_matches_ignore_case(source: &str, query: &str) -> usize {
    let needle: Vec<char> = query.chars().map(fold_char).collect();
    if needle.is_empty() {
        return 0;
    }
    let haystack: Vec<char> = source.chars().map(fold_char).collect();
    let mut count = 0;
    let mut index = 0;
    while index + needle.len() <= haystack.len() {
        if haystack[index..index + needle.len()] == needle[..] {
            count += 1;
            index += needle.len();
        } else {
            index += 1;
        }
    }
    count
}

/// .NET `char.IsWhiteSpace` and Rust `char::is_whitespace` select the same
/// set (Unicode `White_Space`), so `str::trim` matches `string.Trim()`.
#[must_use]
pub fn is_null_or_white_space(value: Option<&str>) -> bool {
    value.is_none_or(|text| text.chars().all(char::is_whitespace))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_ascii_and_simple_unicode_pairs() {
        assert!(eq_ignore_case("C:\\Docs\\Readme.MD", "c:\\docs\\README.md"));
        assert!(eq_ignore_case("ÉCOLE", "école"));
        assert!(!eq_ignore_case("abc", "abcd"));
        assert!(!eq_ignore_case("strasse", "straße"));
    }

    #[test]
    fn counts_non_overlapping_matches() {
        assert_eq!(count_matches_ignore_case("Read, reread, READER", "read"), 3);
        assert_eq!(count_matches_ignore_case("aaaa", "aa"), 2);
        assert_eq!(count_matches_ignore_case("aaa", "aa"), 1);
        assert_eq!(count_matches_ignore_case("x", ""), 0);
        assert_eq!(count_matches_ignore_case("", "x"), 0);
    }

    #[test]
    fn white_space_matches_dotnet_set() {
        assert!(is_null_or_white_space(None));
        assert!(is_null_or_white_space(Some("")));
        assert!(is_null_or_white_space(Some(
            " \t\r\n\u{a0}\u{2028}\u{3000}"
        )));
        assert!(!is_null_or_white_space(Some(" a ")));
        // U+200B ZERO WIDTH SPACE is not white space in either runtime.
        assert!(!is_null_or_white_space(Some("\u{200b}")));
    }
}
