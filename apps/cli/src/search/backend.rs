//! Managed retrieval selection. Native provider routing stays in the gateway.
use super::{DomainFilter, SearchController, SearchResponse, SearchStatus, TIMEOUT, tavily};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchBackend {
    #[default]
    Browser,
    Tavily,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchErrorKind {
    InvalidArguments,
    MissingCredential,
    Authentication,
    Quota,
    RateLimited,
    Timeout,
    NoResults,
    Blocked,
    Cancelled,
    Unavailable,
    BrowserUnavailable,
    Protocol,
    Busy,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchError {
    pub kind: SearchErrorKind,
    message: String,
}
impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for SearchError {}
impl SearchError {
    pub(crate) fn new(kind: SearchErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    // The browser predates typed errors. Keep its compatibility mapping at this boundary.
    pub(crate) fn browser(message: String) -> Self {
        use SearchErrorKind as K;
        let kind = if message.contains("cancelled") {
            K::Cancelled
        } else if message.contains("timed out") {
            K::Timeout
        } else if message.contains("queue is full") || message.contains("busy") {
            K::Busy
        } else if message.contains("verification") {
            K::Blocked
        } else if message.contains("No search results") {
            K::NoResults
        } else if message.contains("not installed") {
            K::BrowserUnavailable
        } else if message.contains("query") || message.contains("arguments") {
            K::InvalidArguments
        } else {
            K::Unavailable
        };
        Self::new(kind, message)
    }
    pub(super) fn permits_browser_fallback(&self) -> bool {
        matches!(
            self.kind,
            SearchErrorKind::RateLimited | SearchErrorKind::Timeout | SearchErrorKind::Unavailable
        )
    }
}

impl SearchController {
    pub(super) fn tavily_key(&self) -> Result<String, SearchError> {
        crate::secrets::store_get(&self.data_dir, "search-tavily", "api_key")
            .ok()
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                SearchError::new(
                    SearchErrorKind::MissingCredential,
                    "Save a Tavily API key before testing API search.",
                )
            })
    }

    /// Save or remove the managed search credential without enabling search.
    /// # Errors
    /// Returns an error for invalid input or private-store failures.
    pub fn save_tavily_key(&self, value: &str) -> Result<SearchStatus, String> {
        let value = value.trim();
        if value.len() > 512
            || value.chars().any(char::is_whitespace)
            || !value.is_ascii()
            || value.chars().any(char::is_control)
        {
            return Err("Enter a valid Tavily API key without whitespace.".into());
        }
        crate::private_fs::ensure_private_dir(&self.data_dir)
            .map_err(|_| "Cannot create the private search directory.")?;
        let result = if value.is_empty() {
            crate::secrets::store_remove(&self.data_dir, "search-tavily", "api_key")
        } else {
            crate::secrets::store_set(&self.data_dir, "search-tavily", "api_key", value)
        };
        result.map_err(|_| "Cannot update the private search credential.")?;
        self.revision.fetch_add(1, Ordering::Release);
        Ok(self.status())
    }

    pub(crate) fn search_filtered(
        &self,
        query: &str,
        filter: &DomainFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SearchResponse, SearchError> {
        if cancelled() {
            return Err(SearchError::new(
                SearchErrorKind::Cancelled,
                "Search was cancelled.",
            ));
        }
        filter
            .query(query)
            .map_err(|e| SearchError::new(SearchErrorKind::InvalidArguments, e))?;
        let settings = self.settings();
        let start = Instant::now();
        if settings.backend == SearchBackend::Browser {
            return self
                .browser_search(query, settings.engine, start, filter, cancelled)
                .map_err(SearchError::browser);
        }
        let key = self.tavily_key()?;
        let result = {
            let _permit = self
                .api_queue
                .acquire(cancelled, 8, TIMEOUT.saturating_sub(start.elapsed()))
                .map_err(SearchError::browser)?;
            let endpoint = "https://api.tavily.com/search".to_owned();
            #[cfg(test)]
            let endpoint = self
                .tavily_endpoint
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(endpoint);
            tavily::search(&endpoint, &key, query.trim(), filter, start, cancelled)
        };
        if cancelled() {
            return Err(SearchError::new(
                SearchErrorKind::Cancelled,
                "Search was cancelled.",
            ));
        }
        match result {
            Err(error)
                if settings.browser_fallback
                    && error.permits_browser_fallback()
                    && start.elapsed() < TIMEOUT =>
            {
                let mut response = self
                    .browser_search(query, settings.engine, start, filter, cancelled)
                    .map_err(|fallback| {
                        SearchError::new(
                            SearchError::browser(fallback.clone()).kind,
                            format!("{error} Browser fallback failed: {fallback}"),
                        )
                    })?;
                response.fallback_reason = Some(error.kind);
                Ok(response)
            }
            other => other,
        }
    }
}
