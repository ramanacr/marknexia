//! Temp folders and fakes. Nothing here writes the real registry or an
//! executable.

use std::io;
use std::path::{Path, PathBuf};

use crate::install::ShellIntegration;

/// A unique folder under the temp folder, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        use std::hash::BuildHasher;
        let token = std::collections::hash_map::RandomState::new().hash_one(label);
        let path = std::env::temp_dir().join(format!(
            "marknexia-setup-test-{label}-{}-{token:016x}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Records shortcuts as text files (never a real `.lnk`).
#[derive(Default)]
pub struct FakeShell {
    pub shortcuts: Vec<(PathBuf, PathBuf, PathBuf)>,
    pub association_notifications: usize,
    pub fail_shortcut: bool,
}

impl ShellIntegration for FakeShell {
    fn create_shortcut(
        &mut self,
        link: &Path,
        target: &Path,
        working_dir: &Path,
        _description: &str,
    ) -> io::Result<()> {
        if self.fail_shortcut {
            return Err(io::Error::other("shortcut refused by the test"));
        }
        std::fs::write(link, format!("fake shortcut to {}", target.display()))?;
        self.shortcuts.push((
            link.to_path_buf(),
            target.to_path_buf(),
            working_dir.to_path_buf(),
        ));
        Ok(())
    }

    fn associations_changed(&mut self) {
        self.association_notifications += 1;
    }
}
