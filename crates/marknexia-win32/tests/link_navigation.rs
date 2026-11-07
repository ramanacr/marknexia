//! Host routing for followed links (.NET `HandleLinkClick`), per link class,
//! on the frozen navigation fixtures and on a wiki-style repository model.
//! Decision record: `compat/decisions/link-navigation.md`.

use std::path::{Path, PathBuf};

use marknexia_core::contracts::DiagnosticSeverity;
use marknexia_navigation::{ResolutionContext, VirtualFileSystem, resolve};
use marknexia_win32::chrome_model::{
    BLOCKED_LINK_NOTICE, LinkAction, TabLocation, external_scheme_allowed, fallback_link_href,
    history_offset, link_action, link_status_text, link_target_path,
};
use serde_json::Value;

fn fixtures() -> Vec<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compat/fixtures/v1/navigation");
    let mut cases = Vec::new();
    for entry in std::fs::read_dir(root).expect("navigation fixtures") {
        let path = entry.expect("fixture entry").path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            cases.push(serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap());
        }
    }
    assert!(cases.len() >= 13, "the v1 corpus has 13 navigation cases");
    cases
}

fn route(case: &Value) -> LinkAction {
    let input = &case["input"];
    let vfs = VirtualFileSystem {
        files: case["virtualFileSystem"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| file.as_str().unwrap().to_owned())
            .collect(),
    };
    let context = ResolutionContext::from_case(input);
    link_action(&resolve(input["destination"].as_str(), &context, &vfs).intent)
}

#[test]
fn every_fixture_class_routes_to_the_dotnet_action() {
    for case in fixtures() {
        let name = case["name"].as_str().unwrap();
        let intent = case["expected"].get("intent").unwrap_or(&case["expected"]);
        let action = route(&case);
        match intent["kind"].as_str().unwrap() {
            "CrossDocument" | "CrossDocumentWithAnchor" => {
                let target = intent["targetDocument"].as_str().unwrap();
                let fragment = intent["fragment"].as_str();
                assert_eq!(
                    action,
                    LinkAction::OpenDocument {
                        path: link_target_path(target, fragment),
                        fragment: fragment.map(str::to_owned),
                    },
                    "{name}"
                );
            }
            "ExternalBrowser" => assert_eq!(
                action,
                LinkAction::OpenExternal(intent["externalUri"].as_str().unwrap().to_owned()),
                "{name}"
            ),
            "BrokenTarget" => match action {
                LinkAction::Diagnostic(message) => {
                    assert_eq!(message.severity, DiagnosticSeverity::Warning, "{name}");
                    assert_eq!(message.message, intent["diagnostic"].as_str().unwrap());
                }
                other => panic!("{name}: {other:?}"),
            },
            "BlockedOrInvalid" => match action {
                LinkAction::Diagnostic(message) => {
                    assert_eq!(message.severity, DiagnosticSeverity::Error, "{name}");
                    assert_eq!(message.message, intent["diagnostic"].as_str().unwrap());
                }
                other => panic!("{name}: {other:?}"),
            },
            "SameDocumentAnchor" => assert!(
                matches!(action, LinkAction::ScrollToAnchor(_) | LinkAction::Nothing),
                "{name}"
            ),
            other => panic!("{name}: unexpected kind {other}"),
        }
    }
}

/// The owner's wiki: README.md at the root links into numbered folders.
fn wiki() -> VirtualFileSystem {
    VirtualFileSystem {
        files: [
            "C:/Project_Docs/README.md",
            "C:/Project_Docs/00-overview/product-overview.md",
            "C:/Project_Docs/00-overview/Release Notes.md",
            "C:/Project_Docs/01-design/architecture.md",
            "C:/Project_Docs/01-design/diagram.png",
            "C:/Project_Docs/01-design/README.md",
        ]
        .map(str::to_owned)
        .to_vec(),
    }
}

fn wiki_link(current: &str, href: &str) -> LinkAction {
    let context = ResolutionContext {
        current_file: current.to_owned(),
        repository_root: Some("C:/Project_Docs".to_owned()),
        allow_external_links: true,
        enforce_repository_sandbox: true,
    };
    link_action(&resolve(Some(href), &context, &wiki()).intent)
}

fn opens(path: &str, fragment: Option<&str>) -> LinkAction {
    LinkAction::OpenDocument {
        path: PathBuf::from(path),
        fragment: fragment.map(str::to_owned),
    }
}

#[test]
fn relative_links_open_in_the_same_tab_with_their_anchor() {
    let readme = "C:/Project_Docs/README.md";
    let overview = "C:/Project_Docs/00-overview/product-overview.md";
    assert_eq!(
        wiki_link(readme, "00-overview/product-overview.md"),
        opens(r"C:\Project_Docs\00-overview\product-overview.md", None)
    );
    assert_eq!(
        wiki_link(readme, "./00-overview/product-overview.md#goals"),
        opens(
            r"C:\Project_Docs\00-overview\product-overview.md",
            Some("goals")
        )
    );
    // `..`, root-relative and encoded spaces.
    assert_eq!(
        wiki_link(overview, "../01-design/architecture.md#layers"),
        opens(r"C:\Project_Docs\01-design\architecture.md", Some("layers"))
    );
    assert_eq!(
        wiki_link(overview, "/01-design/architecture.md"),
        opens(r"C:\Project_Docs\01-design\architecture.md", None)
    );
    assert_eq!(
        wiki_link(overview, "Release%20Notes.md"),
        opens(r"C:\Project_Docs\00-overview\Release Notes.md", None)
    );
    // An encoded anchor is decoded once.
    assert_eq!(
        wiki_link(readme, "00-overview/product-overview.md#next%20steps"),
        opens(
            r"C:\Project_Docs\00-overview\product-overview.md",
            Some("next steps")
        )
    );
}

#[test]
fn targets_match_case_insensitively_and_keep_the_link_spelling() {
    // NTFS matches case-insensitively; the probe does too. The path keeps
    // the link's case, as .NET `Path.GetFullPath` does.
    assert_eq!(
        wiki_link(
            "C:/Project_Docs/README.md",
            "00-OVERVIEW/Product-Overview.MD"
        ),
        opens(r"C:\Project_Docs\00-OVERVIEW\Product-Overview.MD", None)
    );
}

#[test]
fn anchor_only_links_scroll_in_the_document() {
    assert_eq!(
        wiki_link("C:/Project_Docs/README.md", "#getting-started"),
        LinkAction::ScrollToAnchor("getting-started".to_owned())
    );
    assert_eq!(
        wiki_link("C:/Project_Docs/README.md", "#"),
        LinkAction::Nothing
    );
}

#[test]
fn missing_targets_and_folders_report_the_dotnet_message() {
    for (href, message) in [
        (
            "00-overview/missing.md",
            r"Target file not found: C:\Project_Docs\00-overview\missing.md",
        ),
        // .NET probes `File.Exists`: a folder link is a missing file, it does
        // not open the folder's README.md or index.md.
        (
            "01-design/",
            r"Target file not found: C:\Project_Docs\01-design",
        ),
    ] {
        match wiki_link("C:/Project_Docs/README.md", href) {
            LinkAction::Diagnostic(diagnostic) => {
                assert_eq!(diagnostic.severity, DiagnosticSeverity::Warning, "{href}");
                assert_eq!(diagnostic.message, message, "{href}");
            }
            other => panic!("{href}: {other:?}"),
        }
    }
}

#[test]
fn links_above_the_repository_root_are_blocked() {
    for href in [
        "../outside.md",
        "../../Windows/win.ini",
        "%2e%2e/outside.md",
        "/../outside.md",
        "C:/Windows/win.ini",
        "file:///C:/Windows/win.ini",
        "//server/share/x.md",
    ] {
        match wiki_link("C:/Project_Docs/README.md", href) {
            LinkAction::Diagnostic(diagnostic) => {
                assert_eq!(diagnostic.severity, DiagnosticSeverity::Error, "{href}");
                assert!(diagnostic.message.starts_with("Access blocked"), "{href}");
            }
            other => panic!("{href}: {other:?}"),
        }
    }
}

#[test]
fn non_markdown_files_in_the_repository_open_like_documents() {
    // .NET `HandleLinkClick` opens any existing file as a document.
    assert_eq!(
        wiki_link("C:/Project_Docs/01-design/README.md", "diagram.png"),
        opens(r"C:\Project_Docs\01-design\diagram.png", None)
    );
}

#[test]
fn external_links_follow_the_policy_and_the_scheme_allowlist() {
    assert_eq!(
        wiki_link("C:/Project_Docs/README.md", "https://example.com/a?b#c"),
        LinkAction::OpenExternal("https://example.com/a?b#c".to_owned())
    );
    assert_eq!(
        wiki_link("C:/Project_Docs/README.md", "mailto:team@example.com"),
        LinkAction::OpenExternal("mailto:team@example.com".to_owned())
    );
    // LNK-5: .NET launches ftp: and tel:; Rust blocks them.
    for href in ["ftp://example.com/x", "tel:+15550100"] {
        assert_eq!(
            wiki_link("C:/Project_Docs/README.md", href),
            LinkAction::Diagnostic(marknexia_win32::chrome_model::DiagnosticMessage::new(
                BLOCKED_LINK_NOTICE,
                DiagnosticSeverity::Error
            )),
            "{href}"
        );
    }
    for href in ["javascript:alert(1)", "data:text/html,x", "vbscript:x"] {
        assert!(
            matches!(wiki_link("C:/Project_Docs/README.md", href), LinkAction::Diagnostic(d) if d.severity == DiagnosticSeverity::Error),
            "{href}"
        );
    }
    let context = ResolutionContext {
        current_file: "C:/Project_Docs/README.md".to_owned(),
        repository_root: Some("C:/Project_Docs".to_owned()),
        allow_external_links: false,
        enforce_repository_sandbox: true,
    };
    match link_action(&resolve(Some("https://example.com"), &context, &wiki()).intent) {
        LinkAction::Diagnostic(diagnostic) => {
            assert_eq!(diagnostic.message, "External links are disabled by policy.");
        }
        other => panic!("{other:?}"),
    }
    assert!(external_scheme_allowed("HTTPS://EXAMPLE.COM"));
    assert!(!external_scheme_allowed("file:///C:/x"));
}

#[test]
fn hover_text_names_the_resolved_target() {
    let root = Path::new(r"C:\Project_Docs");
    let readme = Path::new(r"C:\Project_Docs\README.md");
    assert_eq!(
        link_status_text(
            "00-overview/product-overview.md#goals",
            readme,
            Some(root),
            true,
            true
        ),
        "00-overview/product-overview.md#goals"
    );
    assert_eq!(
        link_status_text("#intro", readme, Some(root), true, true),
        "README.md#intro"
    );
    assert_eq!(
        link_status_text("https://example.com/", readme, Some(root), true, true),
        "https://example.com/"
    );
    assert_eq!(
        link_status_text("../x.md", readme, Some(root), true, true),
        "Access blocked: Relative path escapes repository root sandbox."
    );
}

#[test]
fn navigations_that_bypass_the_adapter_map_back_to_links() {
    let origin = "https://document-00.marknexia.invalid";
    assert_eq!(
        fallback_link_href(origin, &format!("{origin}/00-overview/a%20b.md#x")),
        "./00-overview/a%20b.md#x"
    );
    assert_eq!(
        fallback_link_href(origin, &format!("{origin}/#top")),
        "#top"
    );
    assert_eq!(fallback_link_href(origin, &format!("{origin}/")), "#");
    assert_eq!(
        fallback_link_href(origin, "https://example.com/"),
        "https://example.com/"
    );
}

#[test]
fn history_restores_documents_and_scroll_offsets() {
    let readme = PathBuf::from(r"C:\Project_Docs\README.md");
    let overview = PathBuf::from(r"C:\Project_Docs\00-overview\product-overview.md");
    let mut location = TabLocation::new(readme.clone());
    location.set_scroll_top(640);
    location.navigate_document(overview.clone(), Some("goals"));
    assert_eq!(location.path(), overview);
    assert_eq!(location.fragment(), Some("goals"));
    assert_eq!(location.scroll_top(), 0);
    assert!(location.can_go_back());
    location.set_scroll_top(120);

    let back = location.go_back().expect("back");
    assert_eq!(location.path(), readme);
    assert_eq!(location.fragment(), None);
    assert_eq!(history_offset(back.scroll_top), 640);
    assert_eq!(location.scroll_top(), 640);
    assert!(location.can_go_forward());

    let forward = location.go_forward().expect("forward");
    assert_eq!(location.path(), overview);
    assert_eq!(forward.fragment.as_deref(), Some("goals"));
    assert_eq!(history_offset(forward.scroll_top), 120);

    // A new link clears forward (.NET `NavigationHistoryManager.Push`).
    location.go_back();
    location.navigate_document(overview, None);
    assert!(!location.can_go_forward());
    assert_eq!(history_offset(f64::NAN), 0);
    assert_eq!(history_offset(-5.0), 0);
}
