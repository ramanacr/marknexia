use marknexia_win32::{
    accessibility::{AccessibilityTree, AccessibleRole, tab_slot},
    tabs::TabStore,
};

#[test]
fn custom_tab_strip_has_named_tab_items_and_single_selection_semantics() {
    let mut tabs = TabStore::new();
    let first = tabs.open("First.md").unwrap();
    let second = tabs.open("Second.md").unwrap();
    let tree = AccessibilityTree::from_tabs(&tabs);

    assert_eq!(tree.root().name(), "Document tabs");
    assert_eq!(tree.root().role(), AccessibleRole::TabControl);
    assert_eq!(tree.children().len(), 2);
    assert_eq!(tree.children()[0].name(), "First.md");
    assert_eq!(tree.children()[0].role(), AccessibleRole::TabItem);
    assert!(!tree.children()[0].is_selected());
    assert_eq!(tree.children()[1].name(), "Second.md");
    assert!(tree.children()[1].is_selected());
    assert_eq!(tree.selected_tab(), Some(second));

    assert!(tree.select(&mut tabs, first));
    assert_eq!(tabs.active_id(), Some(first));
}

#[test]
fn destroyed_accessibility_tree_exposes_no_stale_children() {
    let mut tabs = TabStore::new();
    tabs.open("First.md").unwrap();
    let mut tree = AccessibilityTree::from_tabs(&tabs);

    tree.disconnect();

    assert!(tree.children().is_empty());
    assert_eq!(tree.selected_tab(), None);
}

#[test]
fn tab_slots_cover_the_strip_without_gaps_and_reject_out_of_range() {
    assert_eq!(tab_slot(100, 3, 0), Some((0, 33)));
    assert_eq!(tab_slot(100, 3, 1), Some((33, 66)));
    assert_eq!(tab_slot(100, 3, 2), Some((66, 100)));
    assert_eq!(tab_slot(100, 3, 3), None);
    assert_eq!(tab_slot(100, 0, 0), None);
    assert_eq!(tab_slot(-5, 2, 1), Some((0, 0)));
}
