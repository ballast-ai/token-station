//! Isolated Chrome retrieval adapter.
#[cfg(test)]
use super::filter_results;
use super::{
    DomainFilter, Engine, OwnedBrowser, SearchController, SearchResponse, TIMEOUT, browse,
    chrome_path, search_url, temporary_dir,
};
use std::process::{Command, Stdio};
use std::time::Instant;

impl SearchController {
    pub(super) fn browser_search(
        &self,
        query: &str,
        engine: Engine,
        start: Instant,
        filter: &DomainFilter,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SearchResponse, String> {
        if cancelled() {
            return Err("Browser search was cancelled.".into());
        }
        let mut url = search_url(engine, query)?;
        url.query_pairs_mut()
            .clear()
            .append_pair("q", &filter.query(query)?);
        let _permit = self
            .queue
            .acquire(cancelled, 8, TIMEOUT.saturating_sub(start.elapsed()))?;
        #[cfg(test)]
        if let Some(results) = self
            .fixture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Ok(SearchResponse {
                results: filter_results(results?, filter)?,
                source: "fixture",
                content_type: "search_snippets",
                elapsed_ms: 0,
                page: None,
                credits: None,
                fallback_reason: None,
            });
        }
        let chrome = chrome_path().ok_or("Chrome is not installed in a supported location.")?;
        let profile = temporary_dir()?;
        let mut command = Command::new(chrome);
        command
            .args([
                "--headless=new",
                "--disable-gpu",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-extensions",
                "--disable-background-networking",
                "--disable-sync",
                "--use-mock-keychain",
                "--password-store=basic",
                "--disable-component-update",
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
            ])
            .arg(format!("--user-data-dir={}", profile.0.display()))
            .arg("about:blank")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let child = command
            .spawn()
            .map_err(|_| "Cannot start headless Chrome.")?;
        let mut browser = OwnedBrowser { child, profile };
        let results = browse(&mut browser, &url, engine, filter, start, cancelled)?;
        Ok(SearchResponse {
            results,
            source: "headless_chrome",
            content_type: "search_snippets",
            elapsed_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            page: None,
            credits: None,
            fallback_reason: None,
        })
    }
}
