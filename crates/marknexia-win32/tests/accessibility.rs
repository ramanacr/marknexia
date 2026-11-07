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
    assert_eq!(close_button_slot(0, 200, 44, 44), Some((156, 200)));
    assert_eq!(close_button_slot(200, 400, 44, 44), Some((356, 400)));
    // Never smaller than the 44-DIP target: too narrow or too low hides it.
    assert_eq!(close_button_slot(0, 87, 44, 44), None);
    assert_eq!(close_button_slot(0, 88, 44, 44), Some((44, 88)));
    assert_eq!(close_button_slot(0, 200, 40, 44), None);
    assert_eq!(close_button_slot(0, 200, 0, 0), None);

    assert_eq!(hit_test_tab(400, 44, 44, 2, 10), Some(TabHit::Body(0)));
    assert_eq!(hit_test_tab(400, 44, 44, 2, 160), Some(TabHit::Close(0)));
    assert_eq!(hit_test_tab(400, 44, 44, 2, 200), Some(TabHit::Body(1)));
    assert_eq!(hit_test_tab(400, 44, 44, 2, 399), Some(TabHit::Close(1)));
    assert_eq!(hit_test_tab(400, 44, 44, 0, 10), None);
    // Hidden close button: the whole slot is the body.
    assert_eq!(hit_test_tab(160, 44, 44, 2, 79), Some(TabHit::Body(0)));

    // Drag destinations clamp to the first and last slot.
    assert_eq!(tab_index_at(400, 2, -50), Some(0));
    assert_eq!(tab_index_at(400, 2, 250), Some(1));
    assert_eq!(tab_index_at(400, 2, 900), Some(1));
    assert_eq!(tab_index_at(400, 0, 10), None);
}

/// Tabs are at most `Dip::TAB_MAX_WIDTH` wide and sit at the start of the
/// strip; with many tabs they shrink equally. The empty strip after the
/// last tab is no tab (no selection, no close), but drag destinations
/// still clamp to the last tab.
#[test]
fn tab_widths_are_clamped_and_the_empty_strip_is_not_a_tab() {
    use marknexia_win32::{
        accessibility::{TabHit, close_button_slot, hit_test_tab, tab_index_at, tabs_extent},
        layout::Dip,
    };
    for (dpi, max) in [(96_u32, 240_i32), (144, 360), (192, 480)] {
        assert_eq!(Dip::TAB_MAX_WIDTH.to_pixels(dpi) as i32, max);
        let height = Dip::INTERACTION_TARGET.to_pixels(dpi) as i32;
        // One document in a 1920-pixel strip: one tab of the maximum width.
        let extent = tabs_extent(1920, 1, max);
        assert_eq!(extent, max, "{dpi} DPI");
        assert_eq!(tab_slot(extent, 1, 0), Some((0, max)));
        assert_eq!(
            hit_test_tab(extent, height, height, 1, max - 1),
            Some(TabHit::Close(0))
        );
        assert_eq!(hit_test_tab(extent, height, height, 1, max), None);
        assert_eq!(hit_test_tab(extent, height, height, 1, 1500), None);
        assert_eq!(tab_index_at(extent, 1, 1500), Some(0));
        // Two documents: two tabs of the maximum width, not half the strip.
        let extent = tabs_extent(1920, 2, max);
        assert_eq!(tab_slot(extent, 2, 1), Some((max, 2 * max)));
        // The narrowest preferred tab still holds a 44-DIP close button.
        let min = Dip::TAB_MIN_WIDTH.to_pixels(dpi) as i32;
        assert!(close_button_slot(0, min, height, height).is_some());
    }
    // Many tabs shrink equally to fit the strip.
    assert_eq!(tabs_extent(1000, 10, 240), 1000);
    assert_eq!(tab_slot(1000, 10, 9), Some((900, 1000)));
    assert_eq!(tabs_extent(1000, 0, 240), 0);
    assert_eq!(tabs_extent(-5, 2, 240), 0);
}

/// Close buttons at 150 % and 200 % scale: the strip is 44 DIP high
/// (`Dip::INTERACTION_TARGET`, rounded up), the button is a square of that
/// height, and narrow tabs hide it instead of shrinking it.
#[test]
fn close_buttons_keep_the_44_dip_target_at_144_and_192_dpi() {
    use marknexia_win32::{
        accessibility::{TabHit, close_button_slot, hit_test_tab, tab_slot},
        layout::Dip,
    };
    for (dpi, size) in [(144_u32, 66_i32), (192, 88)] {
        let minimum = Dip::INTERACTION_TARGET.to_pixels(dpi) as i32;
        assert_eq!(minimum, size, "{dpi} DPI");
        let height = minimum;
        // Three tabs in a 900 px strip: 300 px slots.
        let (left, right) = tab_slot(900, 3, 1).unwrap();
        assert_eq!(
            close_button_slot(left, right, height, minimum),
            Some((right - size, right))
        );
        assert_eq!(
            hit_test_tab(900, height, minimum, 3, right - size),
            Some(TabHit::Close(1))
        );
        assert_eq!(
            hit_test_tab(900, height, minimum, 3, right - size - 1),
            Some(TabHit::Body(1))
        );
        // Slots narrower than two targets: no close button anywhere.
        let narrow = 2 * size - 1;
        let total = narrow * 4;
        for index in 0..4 {
            let (left, right) = tab_slot(total, 4, index).unwrap();
            assert_eq!(close_button_slot(left, right, height, minimum), None);
        }
        assert_eq!(
            hit_test_tab(total, height, minimum, 4, total - 1),
            Some(TabHit::Body(3))
        );
    }
}
