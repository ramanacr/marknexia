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
fn invalid_current_file_blocks_enforced_repository_navigation_before_probe() {
    let context = ResolutionContext {
        current_file: "not-an-absolute-current.md".into(),
        repository_root: Some("C:/repo".into()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    };
    let result = resolve(
        Some("target.md"),
        &context,
        &VirtualFileSystem {
            files: vec!["target.md".into()],
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

#[test]
fn virtual_root_resolves_while_invalid_roots_still_fail_closed_before_probe() {
    let vfs = VirtualFileSystem {
        files: vec!["docs/api.md".into(), "c:/repo/docs/api.md".into()],
    };
    let virtual_context = ResolutionContext {
        current_file: "README.md".into(),
        repository_root: Some("navigation".into()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    };
    let result = resolve(Some("docs/api.md"), &virtual_context, &vfs);
    assert_eq!(result.intent.kind, "CrossDocument");
    assert_eq!(
        result.intent.target_document.as_deref(),
        Some("docs/api.md")
    );
    assert_eq!(result.file_system_probe_count, 1);

    for invalid_root in [
        "",
        "..",
        "../repository",
        "repository/../..",
        "C:relative",
        r"\\?\C:\repo",
        r"repo\..\..",
        "//server/share",
        "/rooted",
        "repo:stream",
    ] {
        let context = ResolutionContext {
            repository_root: Some(invalid_root.into()),
            ..virtual_context.clone()
        };
        let result = resolve(Some("docs/api.md"), &context, &vfs);
        assert_eq!(result.intent.kind, "BlockedOrInvalid", "{invalid_root}");
        assert_eq!(
            result.intent.diagnostic.as_deref(),
            Some("Access blocked: Repository root is invalid; cannot enforce repository sandbox."),
            "{invalid_root}"
        );
        assert_eq!(result.file_system_probe_count, 0, "{invalid_root}");
    }

    for invalid_current in ["C:/repo/README.md", "/README.md", "../README.md", ""] {
        let context = ResolutionContext {
            current_file: invalid_current.into(),
            ..virtual_context.clone()
        };
        let result = resolve(Some("docs/api.md"), &context, &vfs);
        assert_eq!(result.intent.kind, "BlockedOrInvalid", "{invalid_current}");
        assert_eq!(result.file_system_probe_count, 0, "{invalid_current}");
    }

    for absolute in ["C:/repo/docs/api.md", "file:///C:/repo/docs/api.md"] {
        let result = resolve(Some(absolute), &virtual_context, &vfs);
        assert_eq!(result.intent.kind, "BlockedOrInvalid", "{absolute}");
        assert_eq!(result.file_system_probe_count, 0, "{absolute}");
    }
}

fn absolute_context(current_file: &str) -> ResolutionContext {
    ResolutionContext {
        current_file: current_file.into(),
        repository_root: Some("C:/repo".into()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    }
}

const RELATIVE_ESCAPE: &str = "Access blocked: Relative path escapes repository root sandbox.";

/// Decision NAV-1: .NET probes the current file before checking a relative
/// target's containment (one probe); Rust rejects the escape with no probe.
#[test]
fn nav_1_relative_escape_is_rejected_before_the_current_file_probe() {
    let vfs = VirtualFileSystem {
        files: vec!["c:/repo/README.md".into(), "c:/private.md".into()],
    };
    let result = resolve(
        Some("../private.md"),
        &absolute_context("C:/repo/README.md"),
        &vfs,
    );
    assert_eq!(result.intent.kind, "BlockedOrInvalid");
    assert_eq!(result.intent.diagnostic.as_deref(), Some(RELATIVE_ESCAPE));
    assert_eq!(result.file_system_probe_count, 0);
}

/// Decision NAV-1: when the current file is missing, .NET retries with the
/// current path as the base directory and would resolve this to
/// `C:/repo/docs/api.md` (two probes). Rust has already rejected the escaping
/// directory-based target before any probe.
#[test]
fn nav_1_missing_current_file_does_not_rescue_an_escaping_directory_target() {
    let vfs = VirtualFileSystem {
        files: vec!["c:/repo/docs/api.md".into()],
    };
    let result = resolve(
        Some("../docs/api.md"),
        &absolute_context("C:/repo/missing.md"),
        &vfs,
    );
    assert_eq!(result.intent.kind, "BlockedOrInvalid");
    assert_eq!(result.intent.diagnostic.as_deref(), Some(RELATIVE_ESCAPE));
    assert_eq!(result.file_system_probe_count, 0);
}

/// Decision NAV-2: .NET `Path.GetFullPath` clamps `..` at the drive root, so
/// these resolve to `C:\repo\docs\api.md` there. Rust rejects traversal above
/// the drive root before any probe.
#[test]
fn nav_2_traversal_above_the_drive_root_is_rejected_before_probe() {
    let vfs = VirtualFileSystem {
        files: vec!["c:/repo/README.md".into(), "c:/repo/docs/api.md".into()],
    };
    for (destination, diagnostic) in [
        ("../../repo/docs/api.md", RELATIVE_ESCAPE),
        (
            "/../../repo/docs/api.md",
            "Access blocked: Repository root traversal outside sandbox boundary.",
        ),
    ] {
        let result = resolve(
            Some(destination),
            &absolute_context("C:/repo/README.md"),
            &vfs,
        );
        assert_eq!(result.intent.kind, "BlockedOrInvalid", "{destination}");
        assert_eq!(
            result.intent.diagnostic.as_deref(),
            Some(diagnostic),
            "{destination}"
        );
        assert_eq!(result.file_system_probe_count, 0, "{destination}");
    }
}

#[test]
fn missing_current_file_is_used_as_the_base_directory_like_dotnet() {
    let vfs = VirtualFileSystem {
        files: vec!["c:/repo/docs/api.md".into()],
    };
    let result = resolve(Some("api.md"), &absolute_context("C:/repo/docs"), &vfs);
    assert_eq!(result.intent.kind, "CrossDocument");
    assert_eq!(
        result.intent.target_document.as_deref(),
        Some("C:/repo/docs/api.md")
    );
    assert_eq!(result.file_system_probe_count, 2);
}

#[test]
fn broken_targets_keep_the_callers_case_and_dotnet_diagnostics() {
    let vfs = VirtualFileSystem {
        files: vec!["c:/repo/README.md".into()],
    };
    for (destination, target, diagnostic, probes) in [
        (
            "docs/Missing.md",
            "C:/repo/docs/Missing.md",
            r"Target file not found: C:\repo\docs\Missing.md",
            2,
        ),
        (
            "/Docs/Missing.md",
            "C:/repo/Docs/Missing.md",
            r"Repository-relative file not found: C:\repo\Docs\Missing.md",
            1,
        ),
        (
            "C:/REPO/Missing.md",
            "C:/REPO/Missing.md",
            r"Absolute target file not found: C:\REPO\Missing.md",
            1,
        ),
    ] {
        let result = resolve(
            Some(destination),
            &absolute_context("C:/repo/README.md"),
            &vfs,
        );
        assert_eq!(result.intent.kind, "BrokenTarget", "{destination}");
        assert!(result.intent.is_safe, "{destination}");
        assert_eq!(result.intent.target_document.as_deref(), Some(target));
        assert_eq!(result.intent.diagnostic.as_deref(), Some(diagnostic));
        assert_eq!(result.file_system_probe_count, probes, "{destination}");
    }
}

#[test]
fn case_changes_never_escape_the_repository_scope() {
    let vfs = VirtualFileSystem {
        files: vec!["c:/repository/secret.md".into()],
    };
    for destination in ["C:/REPOSITORY/secret.md", "../Repository/secret.md"] {
        let result = resolve(
            Some(destination),
            &absolute_context("C:/Repo/README.md"),
            &vfs,
        );
        assert_eq!(result.intent.kind, "BlockedOrInvalid", "{destination}");
        assert_eq!(result.file_system_probe_count, 0, "{destination}");
    }
}
