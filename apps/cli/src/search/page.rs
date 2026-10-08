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
use scraper::{Html, Selector};
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

fn bounded_text<'a>(parts: impl Iterator<Item = &'a str>) -> (String, bool) {
    let mut text = String::new();
    let mut count = 0;
    let mut space = false;
    for part in parts {
        for character in part.chars() {
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
                    return (text, true);
                }
                text.push(ch);
                count += 1;
            }
            space = false;
        }
        space = !text.is_empty();
    }
    (text, false)
}

fn extract(body: &str, mime: &str) -> Result<(String, String, bool), String> {
    if mime == "text/plain" {
        let (text, truncated) = bounded_text(std::iter::once(body));
        return Ok(("Page text".into(), text, truncated));
    }
    if !matches!(mime, "text/html" | "application/xhtml+xml") {
        return Err("Page reading supports static HTML and plain text only.".into());
    }
    let document = Html::parse_document(body);
    let title = document
        .select(&Selector::parse("title").unwrap())
        .next()
        .map_or_else(
            || "Page text".into(),
            |node| node.text().collect::<String>().chars().take(250).collect(),
        );
    let root = ["main", "article", "body"]
        .iter()
        .find_map(|selector| document.select(&Selector::parse(selector).unwrap()).next())
        .ok_or("The page has no readable body.")?;
    let parts = root.descendants().filter_map(|node| {
        if node.ancestors().any(|parent| {
            parent.value().as_element().is_some_and(|element| {
                matches!(
                    element.name(),
                    "script"
                        | "style"
                        | "noscript"
                        | "nav"
                        | "header"
                        | "footer"
                        | "form"
                        | "template"
                        | "svg"
                )
            })
        }) {
            return None;
        }
        node.value().as_text().map(|text| text.as_ref())
    });
    let (text, truncated) = bounded_text(parts);
    if text.is_empty() {
        return Err("The page has no readable text.".into());
    }
    Ok((title, text, truncated))
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
        let check = || {
            if cancelled() {
                Err("Page reading was cancelled.")
            } else if started.elapsed() >= READ_TIMEOUT {
                Err("Page reading timed out.")
            } else {
                Ok(())
            }
        };
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
            let (title, text, truncated) = extract(body, &mime)?;
            check()?;
            let match_count = pattern.map(|pattern| text.matches(pattern).count());
            return Ok(SearchResponse {
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
            });
        }
        unreachable!("redirect limit returns above")
    }
}
