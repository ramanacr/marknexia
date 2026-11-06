//! The standard-library file probe for startup file resolution.

use std::path::Path;

use marknexia_core::startup::StartupFileProbe;

/// Uses `std::path::absolute` (`GetFullPathNameW` on Windows, as .NET
/// `Path.GetFullPath` does) and `Path::is_file`.
///
/// `Path::is_file` follows symbolic links, so a dangling link is not a file.
/// .NET `File.Exists` reports a dangling file link as existing; opening it
/// fails later in both editions.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdStartupProbe;

impl StartupFileProbe for StdStartupProbe {
    fn full_path(&self, path: &str) -> Option<String> {
        std::path::absolute(path)
            .ok()?
            .into_os_string()
            .into_string()
            .ok()
    }

    fn is_file(&self, full_path: &str) -> bool {
        Path::new(full_path).is_file()
    }
}

/// Resolves the first existing Markdown file with [`StdStartupProbe`].
pub fn find_first_existing<'a>(
    candidates: impl IntoIterator<Item = Option<&'a str>>,
) -> Option<String> {
    marknexia_core::startup::find_first_existing(candidates, &StdStartupProbe)
}
