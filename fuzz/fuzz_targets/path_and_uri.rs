#![no_main]

use libfuzzer_sys::fuzz_target;
use marknexia_files::path::{CanonicalPath, RepositoryScope};
use marknexia_navigation::{ResolutionContext, VirtualFileSystem, resolve};

fuzz_target!(|bytes: &[u8]| {
    let Ok(input) = std::str::from_utf8(bytes) else {
        return;
    };
    if let Ok(candidate) = CanonicalPath::new(input) {
        let scope = RepositoryScope::new("C:/repo").expect("fixed repository root is valid");
        if scope.contains(&candidate) {
            assert!(candidate.as_str() == "c:/repo" || candidate.as_str().starts_with("c:/repo/"));
        }
    }

    let context = ResolutionContext {
        current_file: "docs/README.md".into(),
        repository_root: Some("repo".into()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    };
    let result = resolve(Some(input), &context, &VirtualFileSystem::default());
    if result.intent.kind == "BlockedOrInvalid" {
        assert_eq!(result.file_system_probe_count, 0);
    }
});
