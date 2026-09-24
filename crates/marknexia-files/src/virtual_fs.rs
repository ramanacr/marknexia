//! Deterministic probe surface for frozen cross-language navigation fixtures.

#[derive(Clone, Debug, Default)]
pub struct VirtualFileSystem {
    pub files: Vec<String>,
}

impl VirtualFileSystem {
    pub fn file_exists(&self, path: &str, probes: &mut u64) -> bool {
        *probes += 1;
        let folded_path = path.to_lowercase();
        self.files
            .iter()
            .any(|item| item.to_lowercase() == folded_path)
    }
}
