//! Headless tests for the portable chrome decisions (`chrome_model`) and
//! the chrome additions to `ShellLayout`.

use std::path::{Path, PathBuf};

use marknexia_core::contracts::DiagnosticSeverity;
use marknexia_files::repository::{LimitsHit, RepositoryTree, RepositoryTreeNode};
use marknexia_win32::{
    chrome_model::{
        ChromeCommand, DiagnosticsBar, OutlineEntry, SidebarMode, SidebarResize, TabLocation,
        TreeArena, about_text, clamp_sidebar_width, help_text, remote_assets_notice,
        repository_header, resize_sidebar, route_chrome_key, same_document_path,
        sidebar_resize_key, sidebar_width_dip,
    },
    layout::{Dip, ShellLayout, ShellLayoutRequest},
};

#[test]
fn sidebar_modes_persist_like_dotnet() {
    assert_eq!(SidebarMode::from_setting(0), SidebarMode::Outline);
    assert_eq!(SidebarMode::from_setting(1), SidebarMode::Repository);
    // .NET clamps the stored index to 0..1.
    assert_eq!(SidebarMode::from_setting(7), SidebarMode::Repository);
    assert_eq!(SidebarMode::from_setting(-3), SidebarMode::Outline);
    assert_eq!(SidebarMode::Outline.setting_value(), Some(0));
    assert_eq!(SidebarMode::Repository.setting_value(), Some(1));
    assert_eq!(SidebarMode::Recent.setting_value(), None);
    for mode in SidebarMode::ALL {
        assert_eq!(SidebarMode::from_index(mode.index()), Some(mode));
    }
    assert_eq!(SidebarMode::from_index(3), None);
    assert_eq!(SidebarMode::Outline.header(), "Document Map");
    assert_eq!(SidebarMode::Repository.list_name(), "Repository files");
}

#[test]
fn sidebar_width_clamps_and_resizes_from_the_keyboard() {
    assert_eq!(clamp_sidebar_width(f64::NAN), 280.0);
    assert_eq!(clamp_sidebar_width(10.0), 220.0);
    assert_eq!(clamp_sidebar_width(9000.0), 520.0);
    assert_eq!(resize_sidebar(280.0, SidebarResize::Wider), 296.0);
    assert_eq!(resize_sidebar(280.0, SidebarResize::Narrower), 264.0);
    assert_eq!(resize_sidebar(225.0, SidebarResize::Narrower), 220.0);
    assert_eq!(resize_sidebar(515.0, SidebarResize::Wider), 520.0);
    assert_eq!(resize_sidebar(300.0, SidebarResize::Minimum), 220.0);
    assert_eq!(resize_sidebar(300.0, SidebarResize::Maximum), 520.0);
    assert_eq!(sidebar_resize_key(0x25), Some(SidebarResize::Narrower));
    assert_eq!(sidebar_resize_key(0x27), Some(SidebarResize::Wider));
    assert_eq!(sidebar_resize_key(0x24), Some(SidebarResize::Minimum));
    assert_eq!(sidebar_resize_key(0x23), Some(SidebarResize::Maximum));
    assert_eq!(sidebar_resize_key(0x26), None);
    assert_eq!(sidebar_width_dip(300.4), 300);
    assert_eq!(sidebar_width_dip(f64::INFINITY), 520);
}

#[test]
fn dotnet_accelerators_route_to_chrome_commands() {
    let route = |key, ctrl, shift, alt| route_chrome_key(key, ctrl, shift, alt);
    assert_eq!(
        route(0x4F, true, false, false),
        Some(ChromeCommand::OpenFile)
    );
    assert_eq!(
        route(0x54, true, false, false),
        Some(ChromeCommand::OpenFile)
    );
    assert_eq!(
        route(0x4F, true, true, false),
        Some(ChromeCommand::OpenFolder)
    );
    assert_eq!(
        route(0x74, false, false, false),
        Some(ChromeCommand::Reload)
    );
    assert_eq!(route(0x52, true, false, false), Some(ChromeCommand::Reload));
    assert_eq!(route(0x25, false, false, true), Some(ChromeCommand::Back));
    assert_eq!(
        route(0x27, false, false, true),
        Some(ChromeCommand::Forward)
    );
    // Plain arrows and the tab chords belong elsewhere.
    assert_eq!(route(0x25, false, false, false), None);
    assert_eq!(route(0x09, true, false, false), None);
    assert_eq!(route(0x57, true, false, false), None);
    assert_eq!(route(0x46, true, false, false), None);
    assert_eq!(route(0x4F, false, false, false), None);
    assert_eq!(route(0x4F, true, false, true), None);
}

#[test]
fn outline_indent_follows_the_heading_level() {
    let entry = |level| OutlineEntry {
        text: "Heading".to_owned(),
        slug: "heading".to_owned(),
        level,
    };
    assert_eq!(entry(1).indent_dip(), 0);
    assert_eq!(entry(2).indent_dip(), 14);
    assert_eq!(entry(6).indent_dip(), 70);
    assert_eq!(entry(0).indent_dip(), 0);
}

#[test]
fn diagnostics_bar_keeps_the_newest_message_until_dismissed() {
    let mut bar = DiagnosticsBar::default();
    assert!(!bar.is_open());
    assert!(!bar.dismiss());
    bar.show("first", DiagnosticSeverity::Info);
    bar.show("Unable to open document: x", DiagnosticSeverity::Error);
    let current = bar.current().expect("open");
    assert_eq!(current.message, "Unable to open document: x");
    assert_eq!(
        current.accessible_name(),
        "Error: Unable to open document: x"
    );
    assert!(bar.dismiss());
    assert!(!bar.is_open());
    bar.show("w", DiagnosticSeverity::Warning);
    assert_eq!(bar.current().unwrap().accessible_name(), "Warning: w");
    bar.show("i", DiagnosticSeverity::Info);
    assert_eq!(bar.current().unwrap().accessible_name(), "Informational: i");
}

#[test]
fn dialog_and_notice_text_matches_dotnet() {
    assert_eq!(
        remote_assets_notice(true),
        "Remote images are enabled for documents rendered from now on."
    );
    assert_eq!(
        remote_assets_notice(false),
        "Remote images are blocked by default."
    );
    let help = help_text();
    for line in [
        "Ctrl+O  Open file",
        "Ctrl+Shift+O  Open repository",
        "Alt+Left / Alt+Right  Back / forward",
        "F5 / Ctrl+R  Reload",
        "Ctrl+W  Close tab",
    ] {
        assert!(help.lines().any(|candidate| candidate == line), "{line}");
    }
    let about = about_text("1.2.3");
    assert!(about.contains("Version 1.2.3"));
    assert!(about.contains("Read. Explore. Understand."));
}

#[test]
fn anchor_history_matches_the_dotnet_back_and_forward_stacks() {
    let mut location = TabLocation::new(PathBuf::from(r"C:\docs\a.md"));
    assert!(!location.can_go_back());
    assert!(location.navigate_to_anchor("intro"));
    // The same anchor, in any case, is not a new location.
    assert!(!location.navigate_to_anchor("INTRO"));
    assert!(location.navigate_to_anchor("usage"));
    assert_eq!(location.fragment(), Some("usage"));

    let back = location.go_back().expect("back");
    assert_eq!(back.fragment.as_deref(), Some("intro"));
    assert_eq!(location.fragment(), Some("intro"));
    assert!(location.can_go_forward());
    let back = location.go_back().expect("back to the top");
    assert_eq!(back.fragment, None);
    assert!(location.go_back().is_none());

    let forward = location.go_forward().expect("forward");
    assert_eq!(forward.fragment.as_deref(), Some("intro"));
    // A new location clears the forward stack.
    assert!(location.navigate_to(None));
    assert!(!location.can_go_forward());
    assert!(!location.navigate_to(None));
    assert_eq!(back.document_path, r"C:\docs\a.md");
}

#[test]
fn document_paths_compare_ignoring_case() {
    assert!(same_document_path(
        Path::new(r"C:\Docs\README.md"),
        Path::new(r"c:\docs\readme.MD")
    ));
    assert!(!same_document_path(
        Path::new(r"C:\Docs\a.md"),
        Path::new(r"C:\Docs\b.md")
    ));
}

fn node(name: &str, folder: bool, children: Vec<RepositoryTreeNode>) -> RepositoryTreeNode {
    RepositoryTreeNode {
        display_name: name.to_owned(),
        full_path: PathBuf::from(format!(r"C:\repo\{name}")),
        is_folder: folder,
        children,
    }
}

#[test]
fn tree_arena_keeps_order_and_parent_links() {
    let tree = RepositoryTree {
        root: RepositoryTreeNode {
            display_name: String::new(),
            full_path: PathBuf::from(r"C:\"),
            is_folder: true,
            children: vec![
                node("docs", true, vec![node("guide.md", false, Vec::new())]),
                node("README.md", false, Vec::new()),
            ],
        },
        limits_hit: LimitsHit::default(),
    };
    let arena = TreeArena::from_tree(tree);
    assert!(!arena.incomplete());
    let root = arena.root().expect("root");
    // A drive root has no file name; the path is shown instead.
    assert_eq!(root.name, r"C:\");
    let names: Vec<&str> = root
        .children
        .iter()
        .map(|index| arena.node(*index).unwrap().name.as_str())
        .collect();
    assert_eq!(names, ["docs", "README.md"]);
    let docs = arena.node(root.children[0]).unwrap();
    assert!(docs.is_folder);
    assert_eq!(arena.node(docs.children[0]).unwrap().name, "guide.md");
    assert_eq!(arena.nodes().len(), 4);
}

#[test]
fn tree_arena_flattens_deep_trees_without_recursion_and_reports_limits() {
    let mut deepest = node("leaf.md", false, Vec::new());
    for depth in 0..10_000 {
        deepest = node(&format!("d{depth}"), true, vec![deepest]);
    }
    let arena = TreeArena::from_tree(RepositoryTree {
        root: deepest,
        limits_hit: LimitsHit {
            depth: true,
            ..LimitsHit::default()
        },
    });
    assert_eq!(arena.nodes().len(), 10_001);
    assert!(arena.incomplete());
    assert_eq!(repository_header(true), "Repository (incomplete)");
    assert_eq!(repository_header(false), "Repository");
}

#[test]
fn layout_uses_the_persisted_sidebar_width_and_carves_the_diagnostics_bar() {
    let base = ShellLayoutRequest::new(1200, 800, 144).with_sidebar_visible(true);
    let default = ShellLayout::compute(base);
    assert_eq!(default.sidebar.width, Dip::SIDEBAR_WIDTH.to_pixels(144));
    assert_eq!(default.diagnostics.height, 0);

    let wide = ShellLayout::compute(
        base.with_sidebar_width(Dip(400))
            .with_diagnostics_visible(true),
    );
    let target = Dip::INTERACTION_TARGET.to_pixels(144);
    assert_eq!(wide.sidebar.width, 600);
    assert_eq!(wide.webview.x, 600);
    // The splitter is the sidebar's right edge, 6 DIPs wide.
    assert_eq!(wide.splitter.width, Dip::SPLITTER_WIDTH.to_pixels(144));
    assert_eq!(wide.splitter.right(), wide.sidebar.right());
    assert_eq!(wide.splitter.height, wide.sidebar.height);
    // The diagnostics bar sits above the document and meets the target.
    assert_eq!(wide.diagnostics.height, target);
    assert_eq!(wide.diagnostics.y, wide.sidebar.y);
    assert_eq!(wide.diagnostics.x, wide.webview.x);
    assert_eq!(wide.webview.y, wide.diagnostics.bottom());
    assert_eq!(wide.webview.bottom(), default.webview.bottom());

    let hidden = ShellLayout::compute(
        ShellLayoutRequest::new(1200, 800, 96)
            .with_sidebar_width(Dip(400))
            .with_diagnostics_visible(true),
    );
    assert_eq!(hidden.sidebar.width, 0);
    assert_eq!(hidden.splitter.width, 0);
    assert_eq!(hidden.diagnostics.width, 1200);
}
