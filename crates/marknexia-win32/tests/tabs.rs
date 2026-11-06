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

#[test]
fn closing_any_tab_keeps_or_moves_selection_like_dotnet() {
    use marknexia_win32::app::AppState;
    let mut app = AppState::new();
    let first = app.open_tab("1.md").unwrap();
    let second = app.open_tab("2.md").unwrap();
    let third = app.open_tab("3.md").unwrap();
    // An inactive tab closes without changing the selection.
    assert!(app.close_tab(first));
    assert_eq!(app.active_tab(), Some(third));
    // The active last tab hands selection to the new last tab.
    assert!(app.close_tab(third));
    assert_eq!(app.active_tab(), Some(second));
    assert!(!app.close_tab(third), "already closed");
    assert!(app.close_tab(second));
    assert_eq!(app.active_tab(), None);
}

#[test]
fn reordering_changes_the_ctrl_tab_order_and_reports_real_moves_only() {
    use marknexia_win32::{app::AppState, keyboard::ShellCommand};
    let mut app = AppState::new();
    let first = app.open_tab("1.md").unwrap();
    let second = app.open_tab("2.md").unwrap();
    let third = app.open_tab("3.md").unwrap();
    assert!(!app.reorder_tab(third, 2), "same position is not a move");
    assert!(!app.reorder_tab(third, 3), "past the end");
    assert!(app.reorder_tab(third, 0));
    let order: Vec<_> = app.tabs().tabs().iter().map(|tab| tab.id()).collect();
    assert_eq!(order, [third, first, second]);
    // Ctrl+Tab follows the visual order from the active (moved) tab.
    assert_eq!(app.active_tab(), Some(third));
    assert!(app.apply_command(ShellCommand::NextTab));
    assert_eq!(app.active_tab(), Some(first));
}

#[test]
fn the_find_bar_visibility_is_portable_state() {
    use marknexia_win32::app::AppState;
    let mut app = AppState::new();
    assert!(!app.find_bar_visible());
    app.set_find_bar_visible(true);
    assert!(app.find_bar_visible());
}
