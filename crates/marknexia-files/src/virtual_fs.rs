//! Deterministic probe surface for frozen cross-language navigation fixtures.
//!
//! This is a test and probe-counting model only. Its lookup folds with full
//! Unicode `to_lowercase`, which merges names that NTFS keeps distinct (for
//! example KELVIN SIGN U+212A and `k`, or Georgian U+1C90 and U+10D0). It
//! never decides repository containment; `RepositoryScope` does. No production
//! file adapter may reuse this lookup. Before one ships, gate this type to
//! tests, or replace it with a probe trait whose native implementation asks
//! the file system itself. See `compat/decisions/navigation-probe-order.md`.

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
