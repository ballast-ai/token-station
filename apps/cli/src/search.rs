//! Opt-in, host-owned headless Chrome search. No personal browser profile is used.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_public_search_results_and_decodes_text() {
        let html = r#"<ol><li class="b_algo"><h2><a href="https://www.rust-lang.org/">Rust &amp; tools</a></h2><p>Official language site.</p></li><li class="b_algo"><h2><a href="http://127.0.0.1/private">Private</a></h2></li><li class="b_algo"><h2><a href="javascript:alert(1)">Bad</a></h2></li></ol>"#;
        let results = parse_results(html, Engine::Bing).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust & tools");
        assert_eq!(results[0].snippet, "Official language site.");
    }

    #[test]
    fn challenge_and_unknown_layout_are_errors() {
        assert!(
            parse_results("<html>Verify you are human</html>", Engine::Bing)
                .unwrap_err()
                .contains("verification")
        );
        assert!(parse_results("<html><p>Hello</p></html>", Engine::Bing).is_err());
    }

    #[test]
    fn query_is_encoded_and_bounded() {
        let url = search_url(Engine::Bing, "rust & tokio #latest").unwrap();
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
            "rust & tokio #latest"
        );
        assert!(search_url(Engine::Bing, " ").is_err());
        assert!(search_url(Engine::Bing, &"x".repeat(501)).is_err());
    }

    #[test]
    fn legacy_preferences_migrate_to_native_first_and_modes_persist() {
        let root = temporary_dir().unwrap();
        std::fs::write(
            root.0.join("search-settings.json"),
            r#"{"enabled":true,"engine":"bing"}"#,
        )
        .unwrap();
        let controller = SearchController::shared(&root.0);
        assert_eq!(controller.settings().mode, SearchMode::Auto);
        for mode in [SearchMode::Native, SearchMode::Local, SearchMode::Auto] {
            let revision = controller.revision();
            controller
                .save(SearchSettings {
                    mode,
                    ..controller.settings()
                })
                .unwrap();
            assert!(controller.revision() > revision);
            let saved: SearchSettings = serde_json::from_slice(
                &std::fs::read(root.0.join("search-settings.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(saved.mode, mode);
        }
    }

    #[test]
    fn preview_defaults_off_and_persists_without_changing_routes() {
        let root = temporary_dir().unwrap();
        let controller = SearchController::shared(&root.0);
        assert!(!controller.settings().enabled);
        let settings = SearchSettings {
            enabled: true,
            mode: SearchMode::Local,
            engine: Engine::Duckduckgo,
        };
        controller.save(settings.clone()).unwrap();
        assert_eq!(controller.settings(), settings);
        drop(controller);
        assert_eq!(SearchController::shared(&root.0).settings(), settings);
    }

    #[test]
    fn cancelled_search_does_not_start_a_browser_or_hold_capacity() {
        let root = temporary_dir().unwrap();
        let controller = SearchController::shared(&root.0);
        assert!(
            controller
                .search("Rust", &|| true)
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(!controller.status().busy);
    }

    #[test]
    fn private_and_non_web_links_are_rejected() {
        for url in [
            "file:///tmp/a",
            "http://localhost/x",
            "http://192.168.0.1",
            "http://[::1]/",
            "https://u:p@example.com",
            "https://printer.local",
        ] {
            assert!(public_url(url).is_none(), "{url}");
        }
    }

    #[test]
    #[ignore = "Requires installed Chrome and public network access"]
    fn live_documentation_benchmark() {
        use crate::search_quality::{DOCUMENTATION_CASES, matches_target};
        let root = temporary_dir().unwrap();
        let controller = SearchController::shared(&root.0);
        let engine =
            if std::env::var("TOKEN_STATION_SEARCH_TEST_ENGINE").as_deref() == Ok("duckduckgo") {
                Engine::Duckduckgo
            } else {
                Engine::Bing
            };
        controller
            .save(SearchSettings {
                enabled: false,
                mode: SearchMode::Local,
                engine,
            })
            .unwrap();
        let mut observations = Vec::new();
        let mut hits = 0;
        let mut completed = 0;
        for case in DOCUMENTATION_CASES {
            let observation = match controller.search(case.query, &|| false) {
                Ok(response) => {
                    completed += 1;
                    let rank = response
                        .results
                        .iter()
                        .position(|result| matches_target(&result.url, case.expected_source))
                        .map(|index| index + 1);
                    hits += usize::from(rank.is_some());
                    serde_json::json!({"query":case.query,"expected_source":case.expected_source,"execution":"completed","target_rank":rank,"elapsed_ms":response.elapsed_ms,"sources":response.results})
                }
                Err(error) => {
                    serde_json::json!({"query":case.query,"expected_source":case.expected_source,"execution":"failed","error":error})
                }
            };
            observations.push(observation);
        }
        println!(
            "{}",
            serde_json::json!({"engine":engine,"cases":observations,"completed":completed,"target_hits":hits,"total":DOCUMENTATION_CASES.len(),"scope":"Source presence only. No freshness or factual-support assessment."})
        );
        assert_eq!(
            completed,
            DOCUMENTATION_CASES.len(),
            "Some browser searches failed. Inspect the report."
        );
        assert_eq!(
            hits,
            DOCUMENTATION_CASES.len(),
            "Some target sources were missing. Execution alone is not success."
        );
    }

    #[test]
    #[ignore = "Requires installed Chrome and public network access"]
    fn live_chrome_search() {
        let root = temporary_dir().unwrap();
        let controller = SearchController::shared(&root.0);
        if std::env::var("TOKEN_STATION_SEARCH_TEST_ENGINE").as_deref() == Ok("duckduckgo") {
            controller
                .save(SearchSettings {
                    enabled: false,
                    mode: SearchMode::Local,
                    engine: Engine::Duckduckgo,
                })
                .unwrap();
        }
        let result = controller
            .search("Python official documentation", &|| false)
            .unwrap();
        assert!(!result.results.is_empty());
        println!("{}", serde_json::to_string(&result).unwrap());
    }
}

use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};
use url::{Host, Url};

const MAX_HTML: u64 = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    #[default]
    Bing,
    Duckduckgo,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Auto,
    Native,
    Local,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchSettings {
    pub enabled: bool,
    #[serde(default)]
    pub mode: SearchMode,
    pub engine: Engine,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchStatus {
    pub settings: SearchSettings,
    pub chrome_available: bool,
    pub busy: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub source: &'static str,
    pub content_type: &'static str,
    pub elapsed_ms: u64,
}

pub struct SearchController {
    data_dir: PathBuf,
    settings: Mutex<SearchSettings>,
    busy: AtomicBool,
    revision: AtomicU64,
    #[cfg(test)]
    pub(crate) fixture: Mutex<Option<Result<Vec<SearchResult>, String>>>,
}

struct Permit<'a>(&'a AtomicBool);
impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
impl SearchController {
    #[must_use]
    pub fn shared(data_dir: &Path) -> Arc<Self> {
        static REGISTRY: OnceLock<Mutex<BTreeMap<PathBuf, Weak<SearchController>>>> =
            OnceLock::new();
        let mut registry = REGISTRY
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(controller) = registry.get(data_dir).and_then(Weak::upgrade) {
            return controller;
        }
        let settings = std::fs::read(data_dir.join("search-settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let controller = Arc::new(Self {
            data_dir: data_dir.to_path_buf(),
            settings: Mutex::new(settings),
            busy: AtomicBool::new(false),
            revision: AtomicU64::new(0),
            #[cfg(test)]
            fixture: Mutex::new(None),
        });
        registry.insert(data_dir.to_path_buf(), Arc::downgrade(&controller));
        controller
    }

    #[must_use]
    pub fn settings(&self) -> SearchSettings {
        self.settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn status(&self) -> SearchStatus {
        SearchStatus {
            settings: self.settings(),
            chrome_available: chrome_path().is_some(),
            busy: self.busy.load(Ordering::SeqCst),
        }
    }

    /// # Errors
    /// Returns an error when private preferences cannot be saved.
    pub fn save(&self, settings: SearchSettings) -> Result<SearchStatus, String> {
        let mut current = self
            .settings
            .lock()
            .map_err(|_| "Cannot save search settings.")?;
        crate::private_fs::ensure_private_dir(&self.data_dir)
            .map_err(|_| "Cannot create the search settings directory.")?;
        let bytes = serde_json::to_vec(&settings).map_err(|_| "Cannot encode search settings.")?;
        crate::private_fs::write_atomic_private(
            &self.data_dir.join("search-settings.json"),
            &bytes,
        )
        .map_err(|_| "Cannot save search settings.")?;
        *current = settings;
        self.revision.fetch_add(1, Ordering::Release);
        drop(current);
        Ok(self.status())
    }

    /// A manual connection test may run while automatic interception is disabled.
    /// # Errors
    /// Returns an error on invalid input, cancellation, browser failure, or result extraction failure.
    #[allow(clippy::too_many_lines)]
    pub fn search(
        &self,
        query: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SearchResponse, String> {
        if cancelled() {
            return Err("Browser search was cancelled.".into());
        }
        let engine = self.settings().engine;
        let url = search_url(engine, query)?;
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("Browser search is busy. Retry after the current search finishes.".into());
        }
        let _permit = Permit(&self.busy);
        #[cfg(test)]
        if let Some(results) = self
            .fixture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
        {
            return Ok(SearchResponse {
                results: results?,
                source: "fixture",
                content_type: "search_snippets",
                elapsed_ms: 0,
            });
        }
        let start = Instant::now();
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
        let results = browse(&mut browser, &url, engine, start, cancelled)?;
        Ok(SearchResponse {
            results,
            source: "headless_chrome",
            content_type: "search_snippets",
            elapsed_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        })
    }
}

struct OwnedBrowser {
    child: std::process::Child,
    profile: TemporaryProfile,
}
impl Drop for OwnedBrowser {
    fn drop(&mut self) {
        stop_browser(&mut self.child);
    }
}

#[allow(clippy::too_many_lines)]
fn browse(
    browser: &mut OwnedBrowser,
    url: &Url,
    engine: Engine,
    start: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<SearchResult>, String> {
    let check = || {
        if cancelled() {
            Err("Browser search was cancelled.".to_owned())
        } else if start.elapsed() >= TIMEOUT {
            Err("Browser search timed out.".to_owned())
        } else {
            Ok(())
        }
    };
    let port = loop {
        check()?;
        if let Ok(text) = std::fs::read_to_string(browser.profile.0.join("DevToolsActivePort"))
            && let Some(port) = text
                .lines()
                .next()
                .and_then(|line| line.parse::<u16>().ok())
        {
            break port;
        }
        if browser
            .child
            .try_wait()
            .map_err(|_| "Cannot wait for Chrome.")?
            .is_some()
        {
            return Err("Chrome exited before search started.".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .new_agent();
    let text = agent
        .get(format!("http://127.0.0.1:{port}/json/list"))
        .call()
        .map_err(|_| "Cannot connect to the search browser.")?
        .body_mut()
        .read_to_string()
        .map_err(|_| "Invalid browser connection response.")?;
    let pages: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "Invalid browser connection response.")?;
    let endpoint = pages
        .as_array()
        .and_then(|pages| pages.iter().find(|page| page["type"] == "page"))
        .and_then(|page| page["webSocketDebuggerUrl"].as_str())
        .ok_or("The search browser has no page.")?;
    let endpoint_url = Url::parse(endpoint).map_err(|_| "Invalid browser endpoint.")?;
    if endpoint_url.host_str() != Some("127.0.0.1") || endpoint_url.port() != Some(port) {
        return Err("Invalid browser endpoint.".into());
    }
    let socket = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_secs(2),
    )
    .map_err(|_| "Cannot connect to Chrome.")?;
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| "Cannot set browser timeout.")?;
    socket
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| "Cannot set browser timeout.")?;
    let (mut connection, _) =
        tungstenite::client(endpoint, socket).map_err(|_| "Cannot open the browser connection.")?;
    connection.set_config(|config| {
        config.max_message_size = Some(8 * 1024 * 1024);
        config.max_frame_size = Some(8 * 1024 * 1024);
    });
    cdp(
        &mut connection,
        1,
        "Page.navigate",
        &serde_json::json!({"url":url.as_str()}),
        &check,
    )?;
    let mut id = 2;
    let mut last_error = "No search results could be extracted.".to_owned();
    loop {
        check()?;
        let result = cdp(
            &mut connection,
            id,
            "Runtime.evaluate",
            &serde_json::json!({"expression":"document.documentElement.outerHTML.slice(0, 4194305)", "returnByValue":true}),
            &check,
        )?;
        id += 1;
        if let Some(html) = result["result"]["value"].as_str() {
            if html.len() as u64 > MAX_HTML {
                return Err("The search page exceeded the size limit.".into());
            }
            match parse_results(html, engine) {
                Ok(results) => return Ok(results),
                Err(error) if error.contains("verification") => return Err(error),
                Err(error) => last_error = error,
            }
        }
        if start.elapsed() > Duration::from_secs(30) {
            return Err(last_error);
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn cdp(
    connection: &mut tungstenite::WebSocket<std::net::TcpStream>,
    id: u64,
    method: &str,
    params: &serde_json::Value,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<serde_json::Value, String> {
    connection
        .send(tungstenite::Message::Text(
            serde_json::json!({"id":id,"method":method,"params":params})
                .to_string()
                .into(),
        ))
        .map_err(|_| "Cannot send a browser command.")?;
    loop {
        check()?;
        match connection.read() {
            Ok(tungstenite::Message::Text(text)) => {
                let reply: serde_json::Value =
                    serde_json::from_str(&text).map_err(|_| "Invalid browser reply.")?;
                if reply["id"].as_u64() == Some(id) {
                    if reply.get("error").is_some() {
                        return Err("The browser command failed.".into());
                    }
                    return Ok(reply["result"].clone());
                }
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(_) => return Err("The browser connection closed.".into()),
        }
    }
}

fn stop_browser(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // The child owns this new process group. Never signal a personal browser.
        let _ = Command::new("/bin/kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

struct TemporaryProfile(PathBuf);
impl Drop for TemporaryProfile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn temporary_dir() -> Result<TemporaryProfile, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Cannot create a private browser profile.")?;
    let id = u128::from_le_bytes(bytes).to_string();
    let path = std::env::temp_dir().join(format!("token-station-search-{id}"));
    crate::private_fs::ensure_private_dir(&path)
        .map_err(|_| "Cannot create a private browser profile.")?;
    Ok(TemporaryProfile(path))
}

fn chrome_path() -> Option<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        PathBuf::from("/usr/bin/google-chrome"),
        PathBuf::from("/usr/bin/chromium"),
        PathBuf::from("/usr/bin/chromium-browser"),
    ];
    #[cfg(windows)]
    for root in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
        if let Some(dir) = std::env::var_os(root) {
            paths.push(PathBuf::from(dir).join("Google/Chrome/Application/chrome.exe"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(
            PathBuf::from(home).join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        );
    }
    paths.into_iter().find(|path| path.is_file())
}

fn search_url(engine: Engine, query: &str) -> Result<Url, String> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 500 {
        return Err("Enter a search query with 1 to 500 characters.".into());
    }
    let mut url = Url::parse(match engine {
        Engine::Bing => "https://www.bing.com/search",
        Engine::Duckduckgo => "https://html.duckduckgo.com/html/",
    })
    .map_err(|_| "Invalid search engine.")?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

#[allow(clippy::case_sensitive_file_extension_comparisons)] // URL hosts are normalized domain names.
pub(crate) fn public_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    match url.host()? {
        Host::Ipv4(ip)
            if ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip.is_broadcast() =>
        {
            return None;
        }
        Host::Ipv6(_) => return None,
        Host::Domain(host)
            if !host.contains('.') || host.ends_with(".local") || host.ends_with(".localhost") =>
        {
            return None;
        }
        _ => {}
    }
    Some(url.to_string())
}

fn parse_results(html: &str, engine: Engine) -> Result<Vec<SearchResult>, String> {
    let document = Html::parse_document(html);
    let (row, link, snippet) = match engine {
        Engine::Bing => ("li.b_algo", "h2 a", "p"),
        Engine::Duckduckgo => (".result", "a.result__a", ".result__snippet"),
    };
    let select = |value| Selector::parse(value).map_err(|_| "Invalid search parser.".to_string());
    let (rows, links, snippets) = (select(row)?, select(link)?, select(snippet)?);
    let mut seen = BTreeSet::new();
    let mut results = Vec::new();
    for row in document.select(&rows) {
        let Some(link) = row.select(&links).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let resolved = if engine == Engine::Duckduckgo {
            Url::parse("https://duckduckgo.com")
                .ok()
                .and_then(|base| base.join(href).ok())
                .and_then(|url| {
                    url.query_pairs()
                        .find(|(key, _)| key == "uddg")
                        .map(|(_, value)| value.into_owned())
                })
                .unwrap_or_else(|| href.to_owned())
        } else if href.starts_with("https://www.bing.com/ck/a?") {
            use base64::Engine as _;
            Url::parse(href)
                .ok()
                .and_then(|url| {
                    url.query_pairs()
                        .find(|(key, _)| key == "u")
                        .map(|(_, value)| value.into_owned())
                })
                .and_then(|value| {
                    value.strip_prefix("a1").and_then(|encoded| {
                        base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .decode(encoded)
                            .ok()
                    })
                })
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default()
        } else {
            href.to_owned()
        };
        let Some(url) = public_url(&resolved) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        let title = link
            .text()
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if title.is_empty() {
            continue;
        }
        let snippet = row
            .select(&snippets)
            .next()
            .map(|node| node.text().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        results.push(SearchResult {
            title: title.chars().take(250).collect(),
            url,
            snippet: snippet.chars().take(1000).collect(),
        });
        if results.len() == 5 {
            break;
        }
    }
    if results.is_empty() {
        let lower = html.to_lowercase();
        if [
            "verify you are human",
            "captcha",
            "unusual traffic",
            "challenge-form",
        ]
        .iter()
        .any(|text| lower.contains(text))
        {
            return Err(
                "The search engine requires human verification. Try another engine or retry later."
                    .into(),
            );
        }
        return Err("No search results could be extracted. The page may be blocked or its layout may have changed.".into());
    }
    Ok(results)
}
