use marknexia_navigation::history::{HistoryEntry, NavigationHistory};

fn entry(path: &str, fragment: Option<&str>, scroll_top: u32) -> HistoryEntry {
    HistoryEntry::new(path, fragment, scroll_top)
}

#[test]
fn back_then_forward_restores_document_fragment_and_scroll() {
    let first = entry("C:/repo/README.md", None, 0);
    let second = entry("C:/repo/docs/api.md", Some("auth"), 100);
    let current = entry("C:/repo/docs/nested.md", None, 250);
    let mut history = NavigationHistory::new();
    history.push(first);
    history.push(second.clone());

    assert_eq!(history.go_back(current.clone()), Some(second.clone()));
    assert!(history.can_go_forward());
    assert_eq!(history.go_forward(second), Some(current));
    assert!(!history.can_go_forward());
}

#[test]
fn new_navigation_after_back_disposes_forward_branch() {
    let mut history = NavigationHistory::new();
    history.push(entry("C:/repo/one.md", None, 0));
    assert_eq!(
        history.go_back(entry("C:/repo/two.md", None, 0)),
        Some(entry("C:/repo/one.md", None, 0))
    );
    assert!(history.can_go_forward());

    history.push(entry("C:/repo/one.md", None, 0));
    assert!(!history.can_go_forward());
    assert_eq!(history.go_forward(entry("C:/repo/three.md", None, 0)), None);
}

#[test]
fn repeated_empty_and_nonempty_transitions_keep_history_bounded() {
    let mut history = NavigationHistory::new();
    let mut current = entry("C:/repo/current.md", None, 0);
    for index in 0..256 {
        if index % 3 == 0 {
            if let Some(previous) = history.go_back(current.clone()) {
                current = previous;
            }
        } else if index % 3 == 1 {
            if let Some(next) = history.go_forward(current.clone()) {
                current = next;
            }
        } else {
            history.push(current.clone());
            current = entry(&format!("C:/repo/{index}.md"), None, index);
        }
        assert!(history.back_len() + history.forward_len() <= index as usize + 1);
    }
    history.clear();
    assert_eq!((history.back_len(), history.forward_len()), (0, 0));
    assert!(!history.can_go_back());
    assert!(!history.can_go_forward());
}
