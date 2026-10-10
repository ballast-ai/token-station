//! Bounded, credential-free page text retrieval from previously returned search sources.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_public_addresses_including_dns_rebinding_targets() {
        for ip in [
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.0.0.1",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_address("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn reader_requires_returned_urls_and_applies_filters_to_redirects() {
        let filter = DomainFilter::new(&["example.com".into()], &[]).unwrap();
        for url in [
            "http://127.0.0.1",
            "https://example.com:8443/",
            "https://u:p@example.com/",
            "file:///a",
            "https://other.org/",
        ] {
            assert!(validate_url(url, &filter).is_err(), "{url}");
        }
        let known = BTreeSet::from(["https://example.com/page".to_owned()]);
        assert!(authorize("https://example.com/page#part", &known, &filter).is_ok());
        assert!(authorize("https://example.com/other", &known, &filter).is_err());
        assert!(
            redirect(
                &Url::parse("https://example.com/page").unwrap(),
                "http://169.254.169.254/",
                &filter
            )
            .is_err()
        );
        assert!(
            redirect(
                &Url::parse("https://example.com/page").unwrap(),
                "https://evil.org/",
                &filter
            )
            .is_err()
        );
    }

    #[test]
    fn extracts_static_text_without_scripts_and_reports_truncation() {
        let html = "<title>Docs &amp; guide</title><body><nav>Ignore navigation</nav><main><h1>Hello</h1><script>secret()</script><style>.x{}</style><p>World &amp; Rust</p></main></body>";
        let page = extract(html, "text/html").unwrap();
        assert_eq!(page.0, "Docs & guide");
        assert_eq!(page.1, "Hello World & Rust");
        assert!(!page.2);
        let page = extract(&"界".repeat(MAX_TEXT + 1), "text/plain").unwrap();
        assert_eq!(page.1.chars().count(), MAX_TEXT);
        assert!(page.2);
        assert!(extract("%PDF", "application/pdf").is_err());
    }

    #[test]
    fn rejects_structurally_excessive_page_html() {
        let html = format!(
            "<body>{}text{}</body>",
            "<div>".repeat(129),
            "</div>".repeat(129)
        );
        assert!(extract(&html, "text/html").unwrap_err().contains("depth"));
    }

    #[test]
    fn extraction_bounds_bytes_nodes_and_incomplete_tokens() {
        for (input, expected) in [
            ("x".repeat(MAX_BYTES + 1), "input limit"),
            (
                format!("<body>{}</body>", "<br>".repeat(50_001)),
                "node limit",
            ),
            (
                format!("<!--{}--><body>text</body>", "x".repeat(20_000)),
                "token byte limit",
            ),
            (
                format!("<div {}>text</div>", "a='x' ".repeat(4_000)),
                "token byte limit",
            ),
        ] {
            assert!(
                extract(&input, "text/html").unwrap_err().contains(expected),
                "{expected}"
            );
        }
    }

    #[test]
    fn parser_preserves_entities_raw_text_and_common_omitted_tags() {
        let html = "<title>Caf&eacute; &amp; &#x754C;</title><script>let x = '<main>fake</main>';</script><body><nav>skip</nav><article>fallback</article><main><p>Caf&eacute; &copy;<p>next<br>line<form>skip</form></main></body>";
        let (title, text, truncated) = extract(html, "text/html").unwrap();
        assert_eq!(title, "Café & 界");
        assert_eq!(text, "Café © next line");
        assert!(!truncated);
        assert_eq!(
            extract(&format!("<body>{}</body>", "x".repeat(2_500)), "text/html")
                .unwrap()
                .1,
            "x".repeat(2_500)
        );
        assert!(extract("<body><form><main>hidden</main></form></body>", "text/html").is_err());
        assert_eq!(
            extract("<p>Implicit body &amp; entity", "text/html")
                .unwrap()
                .1,
            "Implicit body & entity"
        );
    }

    #[test]
    fn omitted_paragraph_end_does_not_reopen_navigation_text() {
        assert_eq!(
            extract(
                "<body><p>Intro<nav><p>IGNORE NAVIGATION</p></nav>Readable</body>",
                "text/html"
            )
            .unwrap()
            .1,
            "Intro Readable"
        );
    }

    #[test]
    fn cancellation_and_deadline_interrupt_parsing_and_plain_text() {
        use std::cell::Cell;
        let body = format!("<body>{}</body>", "<p>text</p>".repeat(100));
        let calls = Cell::new(0);
        let started = Cell::new(Instant::now());
        let check = || {
            calls.set(calls.get() + 1);
            if calls.get() == 50 {
                started.set(Instant::now().checked_sub(READ_TIMEOUT).unwrap());
            }
            check_read(started.get(), &|| false)
        };
        assert!(
            extract_checked(&body, "text/html", &check)
                .unwrap_err()
                .contains("timed out")
        );
        assert_eq!(calls.get(), 50);
        for mime in ["text/html", "text/plain"] {
            calls.set(0);
            assert!(
                extract_checked(&body, mime, &|| {
                    calls.set(calls.get() + 1);
                    check_read(Instant::now(), &|| calls.get() >= 50)
                })
                .unwrap_err()
                .contains("cancelled")
            );
            assert_eq!(calls.get(), 50);
        }
    }

    #[test]
    fn character_tokens_propagate_cancellation_and_deadline_without_panicking() {
        use std::cell::Cell;
        let body = format!("<body>{}</body>", "a&amp;b".repeat(200));
        for error in ["Page reading was cancelled.", "Page reading timed out."] {
            let checks = Cell::new(0);
            let result = extract_checked(&body, "text/html", &|| {
                checks.set(checks.get() + 1);
                if checks.get() >= 10 {
                    Err(error.into())
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.unwrap_err(), error);
        }
    }

    #[test]
    fn character_tokens_propagate_node_limits_without_panicking() {
        let body = format!("<body>{}</body>", "<br>a".repeat(30_000));
        assert!(
            extract(&body, "text/html")
                .unwrap_err()
                .contains("node limit")
        );
    }

    #[test]
    fn page_parser_failures_and_cancellation_release_the_shared_queue() {
        use std::cell::Cell;
        let root = super::super::temporary_dir().unwrap();
        let controller = super::super::SearchController::shared(&root.0);
        let known = BTreeSet::from(["https://example.com/".into()]);
        let filter = DomainFilter::default();
        *controller.page_fixture.lock().unwrap() = Some((
            format!(
                "<body>{}text{}</body>",
                "<div>".repeat(129),
                "</div>".repeat(129)
            ),
            "text/html".into(),
        ));
        assert!(
            controller
                .read_page("https://example.com/", &known, &filter, None, &|| false)
                .unwrap_err()
                .contains("depth")
        );
        assert!(!controller.status().busy);
        *controller.page_fixture.lock().unwrap() = Some((
            format!("<body>{}</body>", "<p>text</p>".repeat(100)),
            "text/html".into(),
        ));
        let calls = Cell::new(0);
        assert!(
            controller
                .read_page("https://example.com/", &known, &filter, None, &|| {
                    calls.set(calls.get() + 1);
                    calls.get() >= 50
                })
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(!controller.status().busy);
        {
            // Exercise the same response extraction and queue ownership with a
            // deadline that expires during token processing, without sleeping.
            let _permit = controller
                .queue
                .acquire(&|| false, 8, Duration::ZERO)
                .unwrap();
            let started = Cell::new(Instant::now());
            calls.set(0);
            let url = Url::parse("https://example.com/").unwrap();
            let body = format!("<body>{}</body>", "<p>text</p>".repeat(100));
            assert!(
                page_response(&url, &body, "text/html", None, Instant::now(), &|| {
                    calls.set(calls.get() + 1);
                    if calls.get() == 50 {
                        started.set(Instant::now().checked_sub(READ_TIMEOUT).unwrap());
                    }
                    check_read(started.get(), &|| false)
                })
                .unwrap_err()
                .contains("timed out")
            );
        }
        assert!(!controller.status().busy);
        *controller.page_fixture.lock().unwrap() = Some((
            "<main>Allowed &amp; readable</main>".into(),
            "text/html".into(),
        ));
        assert_eq!(
            controller
                .read_page(
                    "https://example.com/",
                    &known,
                    &filter,
                    Some("readable"),
                    &|| false
                )
                .unwrap()
                .page
                .unwrap()
                .match_count,
            Some(1)
        );
        assert!(!controller.status().busy);
    }

    #[test]
    fn cancellation_happens_before_dns_or_http() {
        let root = super::super::temporary_dir().unwrap();
        let controller = super::super::SearchController::shared(&root.0);
        let known = BTreeSet::from(["https://example.com/".to_owned()]);
        assert!(
            controller
                .read_page(
                    "https://example.com/",
                    &known,
                    &DomainFilter::default(),
                    None,
                    &|| true
                )
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(!controller.status().busy);
    }

    #[test]
    #[ignore = "Requires public network access"]
    fn live_public_documentation_page() {
        let root = super::super::temporary_dir().unwrap();
        let controller = super::super::SearchController::shared(&root.0);
        let known = BTreeSet::from(["https://docs.python.org/3/".to_owned()]);
        let response = controller
            .read_page(
                "https://docs.python.org/3/",
                &known,
                &DomainFilter::new(&["docs.python.org".into()], &[]).unwrap(),
                Some("Python"),
                &|| false,
            )
            .unwrap();
        let page = response.page.unwrap();
        println!(
            "{}",
            serde_json::json!({"url":page.url,"characters":page.text.chars().count(),"truncated":page.truncated,"match_count":page.match_count,"elapsed_ms":response.elapsed_ms})
        );
        assert!(page.text.contains("Python"));
        assert!(page.match_count.is_some_and(|count| count > 0));
        assert!(page.text.len() > 100);
    }
}

use super::{DomainFilter, SearchController, SearchResponse, SearchResult};
use serde::Serialize;
use std::collections::BTreeSet;
use std::io::Read;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use url::Url;

const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_TEXT: usize = 24_000;
const READ_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Serialize)]
pub struct PageText {
    pub url: String,
    pub text: String,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub match_count: Option<usize>,
}

#[derive(Debug, Default)]
struct PublicResolver(DefaultResolver);

impl Resolver for PublicResolver {
    fn resolve(
        &self,
        uri: &ureq::http::Uri,
        config: &ureq::config::Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addresses = self.0.resolve(uri, config, timeout)?;
        if addresses
            .iter()
            .any(|address| !public_address(address.ip()))
        {
            return Err(ureq::Error::HostNotFound);
        }
        Ok(addresses)
    }
}

fn public_address(ip: IpAddr) -> bool {
    let IpAddr::V4(ip) = ip else {
        return false;
    };
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && (b == 168 || (b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99)))
        || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}

fn validate_url(raw: &str, filter: &DomainFilter) -> Result<Url, String> {
    if raw.len() > 8192 || !filter.allows(raw) {
        return Err("Page reading requires a public URL within the domain filters.".into());
    }
    let mut url = Url::parse(raw).map_err(|_| "Invalid page URL.")?;
    if url.port().is_some() {
        return Err("Page reading supports only default HTTP and HTTPS ports.".into());
    }
    if let Some(url::Host::Ipv4(ip)) = url.host()
        && !public_address(IpAddr::V4(ip))
    {
        return Err("Page reading cannot access this address.".into());
    }
    url.set_fragment(None);
    Ok(url)
}

fn authorize(raw: &str, known: &BTreeSet<String>, filter: &DomainFilter) -> Result<Url, String> {
    let url = validate_url(raw, filter)?;
    if !known
        .iter()
        .any(|source| validate_url(source, filter).is_ok_and(|source| source == url))
    {
        return Err("Open only a URL returned by search in this request.".into());
    }
    Ok(url)
}

fn redirect(from: &Url, location: &str, filter: &DomainFilter) -> Result<Url, String> {
    let url = from.join(location).map_err(|_| "Invalid page redirect.")?;
    validate_url(url.as_str(), filter)
}

fn bounded_text_checked<'a>(
    parts: impl Iterator<Item = &'a str>,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<(String, bool), String> {
    let mut text = String::new();
    let mut count = 0;
    let mut space = false;
    for part in parts {
        for character in part.chars() {
            check()?;
            if character.is_whitespace() {
                space = !text.is_empty();
                continue;
            }
            for ch in space
                .then_some(' ')
                .into_iter()
                .chain(std::iter::once(character))
            {
                if count == MAX_TEXT {
                    return Ok((text, true));
                }
                text.push(ch);
                count += 1;
            }
            space = false;
        }
        space = !text.is_empty();
    }
    Ok((text, false))
}

#[cfg(test)]
fn extract(body: &str, mime: &str) -> Result<(String, String, bool), String> {
    extract_checked(body, mime, &|| Ok(()))
}

fn extract_checked(
    body: &str,
    mime: &str,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<(String, String, bool), String> {
    check()?;
    if body.len() > MAX_BYTES {
        return Err("The page exceeded the 2 MiB input limit.".into());
    }
    if mime == "text/plain" {
        let (text, truncated) = bounded_text_checked(std::iter::once(body), check)?;
        return Ok(("Page text".into(), text, truncated));
    }
    if !matches!(mime, "text/html" | "application/xhtml+xml") {
        return Err("Page reading supports static HTML and plain text only.".into());
    }
    let document = super::html::parse(body, MAX_BYTES, check)?;
    let title_node = document.first(0, document.len(), |node| node.name == "title", check)?;
    let title = match title_node {
        Some(index) => {
            document
                .text(index + 1, document.end(index), 250, &[], check)?
                .0
        }
        None => "Page text".into(),
    };
    let mut root = None;
    for name in ["main", "article", "body"] {
        root = document.first(0, document.len(), |node| node.name == name, check)?;
        if root.is_some() {
            break;
        }
    }
    let (start, end) = root.map_or((0, document.len()), |index| {
        (index + 1, document.end(index))
    });
    let (text, truncated) = document.text(
        start,
        end,
        MAX_TEXT,
        &[
            "script", "style", "noscript", "nav", "header", "footer", "form", "template", "svg",
            "head", "title",
        ],
        check,
    )?;
    if text.is_empty() {
        return Err("The page has no readable text.".into());
    }
    Ok((title, text, truncated))
}

fn check_read(started: Instant, cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Page reading was cancelled.".into())
    } else if started.elapsed() >= READ_TIMEOUT {
        Err("Page reading timed out.".into())
    } else {
        Ok(())
    }
}

fn page_response(
    url: &Url,
    body: &str,
    mime: &str,
    pattern: Option<&str>,
    started: Instant,
    check: &dyn Fn() -> Result<(), String>,
) -> Result<SearchResponse, String> {
    let (title, text, truncated) = extract_checked(body, mime, check)?;
    check()?;
    let match_count = pattern.map(|pattern| text.matches(pattern).count());
    Ok(SearchResponse {
        results: vec![SearchResult {
            title,
            url: url.to_string(),
            snippet: text.chars().take(300).collect(),
        }],
        source: "public_http",
        content_type: "page_text",
        elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        page: Some(PageText {
            url: url.to_string(),
            text,
            truncated,
            pattern: pattern.map(str::to_owned),
            match_count,
        }),
    })
}

impl SearchController {
    #[allow(clippy::too_many_lines)] // One bounded redirect and body-read lifecycle.
    pub(crate) fn read_page(
        &self,
        raw: &str,
        known: &BTreeSet<String>,
        filter: &DomainFilter,
        pattern: Option<&str>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<SearchResponse, String> {
        if cancelled() {
            return Err("Page reading was cancelled.".into());
        }
        let mut url = authorize(raw, known, filter)?;
        if pattern.is_some_and(|pattern| pattern.is_empty() || pattern.chars().count() > 200) {
            return Err("The page pattern must contain 1 to 200 characters.".into());
        }
        let _permit = self.queue.acquire(cancelled, 8, Duration::from_mins(1))?;
        let started = Instant::now();
        let check = || check_read(started, cancelled);
        #[cfg(test)]
        if let Some((body, mime)) = self.page_fixture.lock().unwrap().clone() {
            return page_response(&url, &body, &mime, pattern, started, &check);
        }
        for attempt in 0..=3 {
            check()?;
            let config = ureq::Agent::config_builder()
                .proxy(None)
                .max_redirects(0)
                .http_status_as_error(false)
                .ip_family(ureq::config::IpFamily::Ipv4Only)
                .timeout_global(Some(READ_TIMEOUT.saturating_sub(started.elapsed())))
                .timeout_resolve(Some(Duration::from_secs(2)))
                .timeout_connect(Some(Duration::from_secs(2)))
                .timeout_recv_response(Some(Duration::from_secs(2)))
                .timeout_recv_body(Some(Duration::from_secs(2)))
                .build();
            let agent = ureq::Agent::with_parts(
                config,
                DefaultConnector::default(),
                PublicResolver::default(),
            );
            let mut response = agent.get(url.as_str()).header("Accept", "text/html,application/xhtml+xml,text/plain").call()
                .map_err(|_| "Cannot read this public page. Check DNS, network access, and address restrictions.")?;
            check()?;
            if response.status().is_redirection() {
                if attempt == 3 {
                    return Err("The page exceeded the redirect limit.".into());
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or("The page redirect has no location.")?;
                url = redirect(&url, location, filter)?;
                continue;
            }
            if !response.status().is_success() {
                return Err(format!(
                    "Page reading failed with HTTP {}.",
                    response.status().as_u16()
                ));
            }
            let mime = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            if !matches!(
                mime.as_str(),
                "text/html" | "application/xhtml+xml" | "text/plain"
            ) {
                return Err("Page reading supports static HTML and plain text only.".into());
            }
            let mut bytes = Vec::new();
            let mut reader = response.body_mut().as_reader();
            let mut buffer = [0; 8192];
            loop {
                check()?;
                let count = reader
                    .read(&mut buffer)
                    .map_err(|_| "The page transfer failed or timed out.")?;
                if count == 0 {
                    break;
                }
                if bytes.len() + count > MAX_BYTES {
                    return Err("The page exceeded the 2 MiB transfer limit.".into());
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            check()?;
            let body = std::str::from_utf8(&bytes)
                .map_err(|_| "Page reading supports UTF-8 text only.")?;
            return page_response(&url, body, &mime, pattern, started, &check);
        }
        unreachable!("redirect limit returns above")
    }
}
