use marknexia_navigation::{ResolutionContext, VirtualFileSystem, resolve};

fn context() -> ResolutionContext {
    ResolutionContext {
        current_file: "docs/README.md".into(),
        repository_root: Some("repository".into()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    }
}

#[test]
fn network_device_ads_and_escape_inputs_are_rejected_before_probe() {
    let vfs = VirtualFileSystem {
        files: vec!["docs/README.md".into()],
    };
    for destination in [
        "//server/share.md",
        "%2f%2fserver/share.md",
        "%5c%5cserver%5cshare.md",
        "file://server/share.md",
        "file:///%2fserver/share.md",
        "C:/docs/README.md:stream",
        "C:relative.md",
        "\\??\\C:\\repo\\private.md",
        "../../outside.md",
        "%2e%2e/%2e%2e/outside.md",
    ] {
        let result = resolve(Some(destination), &context(), &vfs);
        assert_eq!(result.intent.kind, "BlockedOrInvalid", "{destination}");
        assert_eq!(result.file_system_probe_count, 0, "{destination}");
    }
}

#[test]
fn a_literal_percent_escape_is_not_decoded_twice() {
    let vfs = VirtualFileSystem {
        files: vec!["docs/literal%20.md".into()],
    };
    let result = resolve(Some("literal%2520.md#part%2520"), &context(), &vfs);
    assert_eq!(result.intent.kind, "CrossDocumentWithAnchor");
    assert_eq!(
        result.intent.target_document.as_deref(),
        Some("docs/literal%20.md#part%20")
    );
}

#[test]
fn virtual_file_lookup_uses_windows_case_insensitive_semantics() {
    let vfs = VirtualFileSystem {
        files: vec!["docs/ReadMe.MD".into()],
    };
    let result = resolve(Some("readme.md"), &context(), &vfs);
    assert_eq!(result.intent.kind, "CrossDocument");
}

#[test]
fn virtual_file_lookup_matches_accented_windows_case_without_extra_probes() {
    let vfs = VirtualFileSystem {
        files: vec!["docs/CAFÉ.md".into()],
    };
    let result = resolve(Some("café.md"), &context(), &vfs);
    assert_eq!(result.intent.kind, "CrossDocument");
    assert_eq!(result.file_system_probe_count, 1);
}

#[test]
fn malformed_external_uri_is_blocked_without_file_probe() {
    let vfs = VirtualFileSystem::default();
    for destination in ["https://", "http://#fragment", "ftp://"] {
        let result = resolve(Some(destination), &context(), &vfs);
        assert_eq!(result.intent.kind, "BlockedOrInvalid", "{destination}");
        assert_eq!(
            result.intent.diagnostic.as_deref(),
            Some("Invalid external URI.")
        );
        assert_eq!(result.file_system_probe_count, 0);
    }
}

#[test]
fn repository_scope_blocks_relative_escape_before_virtual_file_probe() {
    let context = ResolutionContext {
        current_file: "C:/repo/docs/README.md".into(),
        repository_root: Some("C:/repo".into()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    };
    let result = resolve(
        Some("../../private.md"),
        &context,
        &VirtualFileSystem {
            files: vec!["c:/private.md".into()],
        },
    );

    assert_eq!(result.intent.kind, "BlockedOrInvalid");
    assert_eq!(result.file_system_probe_count, 0);
}

#[test]
fn invalid_repository_scope_blocks_before_virtual_file_probe() {
    let context = ResolutionContext {
        current_file: "C:/repo/docs/README.md".into(),
        repository_root: Some("\\\\?\\C:\\repo".into()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    };
    let result = resolve(
        Some("../../private.md"),
        &context,
        &VirtualFileSystem {
            files: vec!["c:/private.md".into()],
        },
    );

    assert_eq!(result.intent.kind, "BlockedOrInvalid");
    assert_eq!(result.file_system_probe_count, 0);
}

#[test]
fn safe_absolute_windows_and_file_uri_targets_resolve_inside_repository_scope() {
    let context = ResolutionContext {
        current_file: "C:/repo/README.md".into(),
        repository_root: Some("C:/repo".into()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    };
    let vfs = VirtualFileSystem {
        files: vec!["c:/repo/docs/api.md".into()],
    };

    for destination in [
        "C:\\REPO\\docs\\api.md",
        "file:///C:/repo/docs/api.md#overview",
    ] {
        let result = resolve(Some(destination), &context, &vfs);
        assert!(
            matches!(
                result.intent.kind.as_str(),
                "CrossDocument" | "CrossDocumentWithAnchor"
            ),
            "{destination}"
        );
        assert_eq!(result.file_system_probe_count, 1, "{destination}");
    }
}

#[test]
fn malformed_external_normalization_inserts_root_path_before_query_or_fragment() {
    let vfs = VirtualFileSystem::default();
    for (destination, expected) in [
        (
            "https://example.test?view=raw",
            "https://example.test/?view=raw",
        ),
        (
            "https://example.test#overview",
            "https://example.test/#overview",
        ),
    ] {
        let result = resolve(Some(destination), &context(), &vfs);
        assert_eq!(result.intent.kind, "ExternalBrowser", "{destination}");
        assert_eq!(result.intent.external_uri.as_deref(), Some(expected));
        assert_eq!(result.file_system_probe_count, 0);
    }
}
