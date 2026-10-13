//! Tavily basic search. The endpoint is fixed outside loopback tests.
use super::{
    DomainFilter, SearchError, SearchErrorKind, SearchResponse, SearchResult, TIMEOUT, public_url,
};
use std::collections::BTreeSet;
use std::io::Read;
use std::time::{Duration, Instant};

pub(super) fn search(
    endpoint: &str,
    key: &str,
    query: &str,
    filter: &DomainFilter,
    start: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<SearchResponse, SearchError> {
    use SearchErrorKind as K;
    if cancelled() {
        return Err(SearchError::new(K::Cancelled, "Search was cancelled."));
    }
    let timeout = Duration::from_secs(15).min(TIMEOUT.saturating_sub(start.elapsed()));
    if timeout.is_zero() {
        return Err(SearchError::new(K::Timeout, "Search timed out."));
    }
    let http = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .max_redirects(0)
        .http_status_as_error(false)
        .build()
        .new_agent();
    let payload = serde_json::json!({"query":query,"search_depth":"basic","topic":"general","max_results":5,"auto_parameters":false,"include_answer":false,"include_raw_content":false,"include_images":false,"include_usage":true,"include_domains":filter.allowed,"exclude_domains":filter.blocked});
    let response = http
        .post(endpoint)
        .header("authorization", &format!("Bearer {key}"))
        .header("content-type", "application/json")
        .send(payload.to_string().as_bytes())
        .map_err(|error| match error {
            ureq::Error::Timeout(_) => SearchError::new(K::Timeout, "Tavily search timed out."),
            _ => SearchError::new(K::Unavailable, "Cannot reach Tavily search."),
        })?;
    if cancelled() {
        return Err(SearchError::new(K::Cancelled, "Search was cancelled."));
    }
    let status = response.status().as_u16();
    if status != 200 {
        let (kind, message) = match status {
            400 | 422 => (
                K::InvalidArguments,
                "Tavily rejected the search parameters.",
            ),
            401 | 403 => (
                K::Authentication,
                "Tavily rejected the API key. Check the saved credential.",
            ),
            402 | 432 | 433 => (K::Quota, "Tavily search quota is exhausted."),
            429 => (K::RateLimited, "Tavily search is rate limited."),
            500..=599 => (K::Unavailable, "Tavily search is temporarily unavailable."),
            _ => (
                K::Protocol,
                "Tavily returned an unexpected status. Redirects are not followed.",
            ),
        };
        return Err(SearchError::new(kind, message));
    }
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(1024 * 1024)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut
                || error
                    .get_ref()
                    .and_then(|inner| inner.downcast_ref::<ureq::Error>())
                    .is_some_and(|inner| matches!(inner, ureq::Error::Timeout(_)))
            {
                SearchError::new(K::Timeout, "Tavily search timed out.")
            } else {
                SearchError::new(K::Protocol, "Cannot read the bounded Tavily response.")
            }
        })?;
    if cancelled() {
        return Err(SearchError::new(K::Cancelled, "Search was cancelled."));
    }
    let document: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| SearchError::new(K::Protocol, "Tavily returned invalid JSON."))?;
    parse_response(&document, filter, start)
}

fn parse_response(
    document: &serde_json::Value,
    filter: &DomainFilter,
    start: Instant,
) -> Result<SearchResponse, SearchError> {
    use SearchErrorKind as K;
    let rows = document["results"]
        .as_array()
        .filter(|rows| rows.len() <= 100)
        .ok_or_else(|| SearchError::new(K::Protocol, "Tavily returned an invalid result list."))?;
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    for row in rows {
        let (Some(url), Some(title), Some(text)) = (
            row["url"].as_str(),
            row["title"].as_str(),
            row["content"].as_str(),
        ) else {
            return Err(SearchError::new(
                K::Protocol,
                "Tavily returned a malformed source.",
            ));
        };
        if let Some(url) = public_url(url).filter(|url| filter.allows(url))
            && seen.insert(url.clone())
        {
            results.push(SearchResult {
                title: title.chars().take(500).collect(),
                url,
                snippet: text.chars().take(1500).collect(),
            });
        }
        if results.len() == 5 {
            break;
        }
    }
    if results.is_empty() {
        return Err(SearchError::new(
            K::NoResults,
            "No search results match the public-source constraints.",
        ));
    }
    Ok(SearchResponse {
        results,
        source: "tavily",
        content_type: "search_snippets",
        elapsed_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        page: None,
        credits: document["usage"]["credits"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok()),
        fallback_reason: None,
    })
}
