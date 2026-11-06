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

#[test]
fn close_buttons_sit_at_the_slot_end_and_hit_testing_agrees() {
    use marknexia_win32::accessibility::{TabHit, close_button_slot, hit_test_tab, tab_index_at};
    // A 44 px strip, two 200 px tabs: a 44 px square close button each.
    assert_eq!(close_button_slot(0, 200, 44), Some((156, 200)));
    assert_eq!(close_button_slot(200, 400, 44), Some((356, 400)));
    // Narrow slots: at most half the slot; none when empty.
    assert_eq!(close_button_slot(0, 60, 44), Some((30, 60)));
    assert_eq!(close_button_slot(0, 1, 44), None);
    assert_eq!(close_button_slot(0, 200, 0), None);

    assert_eq!(hit_test_tab(400, 44, 2, 10), Some(TabHit::Body(0)));
    assert_eq!(hit_test_tab(400, 44, 2, 160), Some(TabHit::Close(0)));
    assert_eq!(hit_test_tab(400, 44, 2, 200), Some(TabHit::Body(1)));
    assert_eq!(hit_test_tab(400, 44, 2, 399), Some(TabHit::Close(1)));
    assert_eq!(hit_test_tab(400, 44, 0, 10), None);

    // Drag destinations clamp to the first and last slot.
    assert_eq!(tab_index_at(400, 2, -50), Some(0));
    assert_eq!(tab_index_at(400, 2, 250), Some(1));
    assert_eq!(tab_index_at(400, 2, 900), Some(1));
    assert_eq!(tab_index_at(400, 0, 10), None);
}
