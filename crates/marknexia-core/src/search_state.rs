//! Find-in-page state for one document tab.
//!
//! Port of `src/Marknexia.Core/DocumentSearchState.cs`. The WebView2 bridge
//! counts matches in the rendered page and reports a position. A request id
//! ties each report to the request that caused it, so a stale report (the
//! query changed, the document changed, or a newer request started) cannot
//! overwrite newer state.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::text::count_matches_ignore_case;

/// Identifies one find-in-page request. Issued by
/// [`DocumentSearchState::begin_request`]; id 0 is never current.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SearchRequestId(pub u64);

/// A rendered result whose position disagrees with its match count.
///
/// .NET throws `ArgumentOutOfRangeException` for the same conditions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SearchPositionError {
    pub match_count: usize,
    pub current_match: usize,
}

impl fmt::Display for SearchPositionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "rendered match position {} must agree with its count {}",
            self.current_match, self.match_count
        )
    }
}

impl std::error::Error for SearchPositionError {}

/// Search state of one document. `current_match` is 1-based; 0 means none.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocumentSearchState {
    request_revision: u64,
    query: String,
    match_count: usize,
    current_match: usize,
    has_current_result: bool,
}

impl DocumentSearchState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub fn match_count(&self) -> usize {
        self.match_count
    }

    #[must_use]
    pub fn current_match(&self) -> usize {
        self.current_match
    }

    #[must_use]
    pub fn has_current_result(&self) -> bool {
        self.has_current_result
    }

    /// Starts a request and marks the current result as pending.
    pub fn begin_request(&mut self) -> SearchRequestId {
        self.has_current_result = false;
        self.request_revision += 1;
        SearchRequestId(self.request_revision)
    }

    #[must_use]
    pub fn is_current_request(&self, request: SearchRequestId) -> bool {
        request.0 > 0 && request.0 == self.request_revision
    }

    /// Applies a rendered result only when `request` is still current and
    /// `query` equals the current query (ordinal, case-sensitive).
    ///
    /// Returns `Ok(false)` for a stale or mismatched result.
    ///
    /// # Errors
    ///
    /// Returns [`SearchPositionError`] when a current result has an
    /// inconsistent position, as [`Self::apply_rendered_result`] does.
    pub fn try_apply_rendered_result(
        &mut self,
        request: SearchRequestId,
        query: &str,
        match_count: usize,
        current_match: usize,
    ) -> Result<bool, SearchPositionError> {
        if !self.is_current_request(request) || self.query != query {
            return Ok(false);
        }
        self.apply_rendered_result(query, match_count, current_match)?;
        Ok(true)
    }

    /// Stores a draft query. A changed query invalidates the old results; the
    /// same query keeps its position (restoring a tab's input).
    pub fn set_query(&mut self, query: &str) {
        if self.query == query {
            return;
        }
        query.clone_into(&mut self.query);
        self.invalidate_results();
    }

    /// Keeps the query but forgets the results, for example after the
    /// document changed. Pending requests become stale.
    pub fn invalidate_results(&mut self) {
        self.request_revision += 1;
        self.has_current_result = false;
        self.match_count = 0;
        self.current_match = 0;
    }

    /// Applies the position that the rendered page reports.
    ///
    /// # Errors
    ///
    /// Returns [`SearchPositionError`] when `current_match > match_count`,
    /// when matches exist but `current_match` is 0, or when an empty query
    /// reports matches. The state does not change in that case.
    pub fn apply_rendered_result(
        &mut self,
        query: &str,
        match_count: usize,
        current_match: usize,
    ) -> Result<(), SearchPositionError> {
        if current_match > match_count
            || (match_count > 0 && current_match == 0)
            || (query.is_empty() && match_count != 0)
        {
            return Err(SearchPositionError {
                match_count,
                current_match,
            });
        }
        query.clone_into(&mut self.query);
        self.match_count = match_count;
        self.current_match = current_match;
        self.has_current_result = true;
        self.request_revision += 1;
        Ok(())
    }

    /// Counts matches in source text (case-insensitive, non-overlapping) and
    /// selects the first match. `None` acts as an empty string.
    pub fn update(&mut self, query: Option<&str>, source: Option<&str>) {
        self.request_revision += 1;
        query.unwrap_or_default().clone_into(&mut self.query);
        self.has_current_result = true;
        let source = source.unwrap_or_default();
        if self.query.is_empty() || source.is_empty() {
            self.match_count = 0;
            self.current_match = 0;
            return;
        }
        self.match_count = count_matches_ignore_case(source, &self.query);
        self.current_match = usize::from(self.match_count != 0);
    }

    /// Selects the next match and wraps from the last match to the first.
    pub fn move_next(&mut self) {
        self.current_match = if self.match_count == 0 {
            0
        } else if self.current_match == self.match_count {
            1
        } else {
            self.current_match + 1
        };
    }

    /// Selects the previous match and wraps from the first match to the last.
    pub fn move_previous(&mut self) {
        self.current_match = if self.match_count == 0 {
            0
        } else if self.current_match <= 1 {
            self.match_count
        } else {
            self.current_match - 1
        };
    }
}
