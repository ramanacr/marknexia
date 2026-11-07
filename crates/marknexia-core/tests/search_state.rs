//! Parity with `tests/Marknexia.Core.Tests/DocumentSearchStateTests.cs`.

use marknexia_core::search_state::{DocumentSearchState, SearchPositionError, SearchRequestId};

#[test]
fn pending_result_for_inactive_tab_remains_valid_when_another_tab_searches() {
    let mut first = DocumentSearchState::new();
    let mut second = DocumentSearchState::new();
    first.apply_rendered_result("read", 3, 1).unwrap();
    let request = first.begin_request();
    second.set_query("other tab");
    second.begin_request();

    first.set_query("read"); // restoring its input must not cancel the request
    assert_eq!(
        first.try_apply_rendered_result(request, "read", 3, 2),
        Ok(true)
    );

    assert_eq!(first.current_match(), 2);
    assert!(first.has_current_result());
    assert_eq!(second.query(), "other tab");
    assert!(!second.has_current_result());
}

#[test]
fn stale_request_cannot_overwrite_changed_search_state() {
    for invalidation in ["query", "document", "newer-request"] {
        let mut state = DocumentSearchState::new();
        state.set_query("read");
        let request = state.begin_request();
        match invalidation {
            "query" => state.set_query("different"),
            "document" => state.invalidate_results(),
            _ => {
                state.begin_request();
            }
        }

        assert_eq!(
            state.try_apply_rendered_result(request, "read", 3, 2),
            Ok(false),
            "{invalidation}"
        );
        assert!(!state.has_current_result(), "{invalidation}");
    }
}

#[test]
fn current_request_rejects_mismatched_query_and_accepts_matching_result_once() {
    let mut state = DocumentSearchState::new();
    state.set_query("read");
    let request = state.begin_request();

    assert_eq!(
        state.try_apply_rendered_result(request, "other", 1, 1),
        Ok(false)
    );
    assert_eq!(
        state.try_apply_rendered_result(request, "read", 3, 2),
        Ok(true)
    );
    assert_eq!(
        state.try_apply_rendered_result(request, "read", 3, 3),
        Ok(false)
    );
    assert_eq!(state.current_match(), 2);
}

#[test]
fn query_comparison_for_rendered_results_is_case_sensitive() {
    let mut state = DocumentSearchState::new();
    state.set_query("read");
    let request = state.begin_request();
    assert_eq!(
        state.try_apply_rendered_result(request, "READ", 1, 1),
        Ok(false)
    );
}

#[test]
fn request_id_zero_is_never_current() {
    let state = DocumentSearchState::new();
    assert!(!state.is_current_request(SearchRequestId(0)));
}

#[test]
fn current_request_with_inconsistent_position_is_an_error() {
    let mut state = DocumentSearchState::new();
    state.set_query("read");
    let request = state.begin_request();
    assert_eq!(
        state.try_apply_rendered_result(request, "read", 2, 3),
        Err(SearchPositionError {
            match_count: 2,
            current_match: 3
        })
    );
    assert!(!state.has_current_result());
}

#[test]
fn apply_rendered_result_accepts_literal_whitespace_query() {
    let mut state = DocumentSearchState::new();
    state.apply_rendered_result("  ", 1, 1).unwrap();
    assert_eq!(state.query(), "  ");
    assert!(state.has_current_result());
    assert_eq!(state.current_match(), 1);
}

#[test]
fn set_query_preserves_draft_and_invalidates_old_results() {
    let mut state = DocumentSearchState::new();
    state.apply_rendered_result("read", 3, 2).unwrap();

    state.set_query("new draft");

    assert_eq!(state.query(), "new draft");
    assert!(!state.has_current_result());
    assert_eq!(state.match_count(), 0);
    assert_eq!(state.current_match(), 0);
}

#[test]
fn set_query_restoring_same_tab_query_keeps_its_position() {
    let mut state = DocumentSearchState::new();
    state.apply_rendered_result("read", 3, 2).unwrap();

    state.set_query("read");

    assert!(state.has_current_result());
    assert_eq!(state.current_match(), 2);
}

#[test]
fn invalidate_results_retains_query_without_claiming_no_matches() {
    let mut state = DocumentSearchState::new();
    state.apply_rendered_result("read", 3, 2).unwrap();

    state.invalidate_results();

    assert_eq!(state.query(), "read");
    assert!(!state.has_current_result());
    assert_eq!(state.match_count(), 0);
    assert_eq!(state.current_match(), 0);
    state.apply_rendered_result("read", 0, 0).unwrap();
    assert!(state.has_current_result());
}

#[test]
fn apply_rendered_result_uses_browser_position_instead_of_recounting_markdown() {
    let mut state = DocumentSearchState::new();

    state.apply_rendered_result("read", 3, 2).unwrap();

    assert_eq!(state.query(), "read");
    assert_eq!(state.match_count(), 3);
    assert_eq!(state.current_match(), 2);
    state.apply_rendered_result("", 0, 0).unwrap();
    assert_eq!(state.current_match(), 0);
}

#[test]
fn apply_rendered_result_rejects_inconsistent_positions() {
    // The .NET case (-1, 0) cannot be expressed: counts are unsigned.
    for (count, position) in [(0, 1), (2, 0), (2, 3)] {
        let mut state = DocumentSearchState::new();
        let before = state.clone();
        assert!(
            state
                .apply_rendered_result("read", count, position)
                .is_err(),
            "({count}, {position})"
        );
        assert_eq!(state, before);
    }
    let mut state = DocumentSearchState::new();
    assert!(state.apply_rendered_result("", 1, 1).is_err());
}

#[test]
fn update_counts_matches_case_insensitively_and_wraps() {
    let mut state = DocumentSearchState::new();

    state.update(Some("read"), Some("Read, reread, READER"));

    assert_eq!(state.match_count(), 3);
    assert_eq!(state.current_match(), 1);
    state.move_next();
    assert_eq!(state.current_match(), 2);
    state.move_previous();
    assert_eq!(state.current_match(), 1);
    state.move_previous();
    assert_eq!(state.current_match(), 3);
    state.move_next();
    assert_eq!(state.current_match(), 1);
}

#[test]
fn update_empty_query_clears_state() {
    let mut state = DocumentSearchState::new();
    state.update(Some("x"), Some("x"));

    state.update(Some(""), Some("x"));

    assert_eq!(state.match_count(), 0);
    assert_eq!(state.current_match(), 0);
    assert!(state.has_current_result());

    state.update(None, None);
    assert_eq!(state.query(), "");
    assert_eq!(state.match_count(), 0);
}

#[test]
fn update_without_matches_selects_nothing_and_moves_stay_at_zero() {
    let mut state = DocumentSearchState::new();
    state.update(Some("zzz"), Some("abc"));
    assert_eq!(state.match_count(), 0);
    assert_eq!(state.current_match(), 0);
    state.move_next();
    assert_eq!(state.current_match(), 0);
    state.move_previous();
    assert_eq!(state.current_match(), 0);
}

#[test]
fn update_invalidates_pending_requests() {
    let mut state = DocumentSearchState::new();
    state.set_query("a");
    let request = state.begin_request();
    state.update(Some("a"), Some("aaa"));
    assert!(!state.is_current_request(request));
}
