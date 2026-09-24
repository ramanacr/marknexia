use marknexia_win32::tabs::TabStore;

#[test]
fn opening_a_tab_selects_it_without_reusing_existing_identity() {
    let mut tabs = TabStore::new();
    let first = tabs.open("First.md").unwrap();
    let second = tabs.open("Second.md").unwrap();

    assert_ne!(first, second);
    assert_eq!(tabs.active_id(), Some(second));
    assert_eq!(tabs.tabs().len(), 2);
    assert_eq!(tabs.tabs()[0].title(), "First.md");
    assert_eq!(tabs.tabs()[1].title(), "Second.md");
}

#[test]
fn adjacent_selection_wraps_and_never_changes_tab_order() {
    let mut tabs = TabStore::new();
    let first = tabs.open("First.md").unwrap();
    let second = tabs.open("Second.md").unwrap();
    let third = tabs.open("Third.md").unwrap();

    assert!(tabs.select(first));
    assert_eq!(tabs.select_previous(), Some(third));
    assert_eq!(tabs.select_next(), Some(first));
    assert_eq!(tabs.select_next(), Some(second));
    assert_eq!(
        tabs.tabs()
            .iter()
            .map(|tab| tab.title())
            .collect::<Vec<_>>(),
        ["First.md", "Second.md", "Third.md"]
    );
}

#[test]
fn closing_selected_tab_prefers_right_neighbor_then_left() {
    let mut tabs = TabStore::new();
    let first = tabs.open("First.md").unwrap();
    let second = tabs.open("Second.md").unwrap();
    let third = tabs.open("Third.md").unwrap();

    assert!(tabs.select(second));
    assert!(tabs.close(second));
    assert_eq!(tabs.active_id(), Some(third));
    assert!(tabs.close(third));
    assert_eq!(tabs.active_id(), Some(first));
    assert!(tabs.close(first));
    assert_eq!(tabs.active_id(), None);
    assert!(tabs.tabs().is_empty());
    assert!(!tabs.close(first));
}

#[test]
fn closing_an_inactive_tab_keeps_the_selected_identity() {
    let mut tabs = TabStore::new();
    let first = tabs.open("First.md").unwrap();
    let second = tabs.open("Second.md").unwrap();

    assert!(tabs.close(first));
    assert_eq!(tabs.active_id(), Some(second));
    assert!(!tabs.select(first));
    assert_eq!(tabs.active_id(), Some(second));
    assert_ne!(tabs.open("New.md").unwrap(), first);
}

#[test]
fn reordering_preserves_selection_and_rejects_invalid_destination() {
    let mut tabs = TabStore::new();
    let first = tabs.open("First.md").unwrap();
    let second = tabs.open("Second.md").unwrap();
    let third = tabs.open("Third.md").unwrap();

    assert!(tabs.select(second));
    assert!(tabs.reorder(first, 2));
    assert_eq!(tabs.active_id(), Some(second));
    assert_eq!(
        tabs.tabs()
            .iter()
            .map(|tab| tab.title())
            .collect::<Vec<_>>(),
        ["Second.md", "Third.md", "First.md"]
    );
    assert!(tabs.reorder(third, 0));
    assert_eq!(
        tabs.tabs()
            .iter()
            .map(|tab| tab.title())
            .collect::<Vec<_>>(),
        ["Third.md", "Second.md", "First.md"]
    );
    assert!(!tabs.reorder(first, 3));
    assert_eq!(tabs.active_id(), Some(second));
}
