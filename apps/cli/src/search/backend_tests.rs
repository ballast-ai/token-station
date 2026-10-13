use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

pub(crate) fn api(status: u16, body: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/search", listener.local_addr().unwrap());
    let body = body.to_owned();
    let task = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut buf = [0; 4096];
            let n = socket.read(&mut buf).unwrap();
            assert!(n > 0);
            bytes.extend_from_slice(&buf[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let len = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|s| s.parse::<usize>().ok())
                    })
                    .unwrap();
                if bytes.len() >= end + 4 + len {
                    break;
                }
            }
        }
        let _ = write!(
            socket,
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nLocation: http://127.0.0.1:1/stolen\r\n\r\n{body}",
            body.len()
        );
        String::from_utf8(bytes).unwrap()
    });
    (url, task)
}

#[test]
fn legacy_settings_keep_browser_and_credentials_stay_private() {
    let settings: SearchSettings =
        serde_json::from_str(r#"{"enabled":false,"mode":"local","engine":"bing"}"#).unwrap();
    assert_eq!(settings.backend, SearchBackend::Browser);
    assert!(!settings.browser_fallback);
    let root = temporary_dir().unwrap();
    let controller = SearchController::shared(&root.0);
    controller.save_tavily_key("fixture-secret").unwrap();
    assert!(controller.status().tavily_configured);
    assert!(
        !serde_json::to_string(&controller.status())
            .unwrap()
            .contains("fixture-secret")
    );
    controller.save(settings).unwrap();
    assert!(
        !std::fs::read_to_string(root.0.join("search-settings.json"))
            .unwrap()
            .contains("fixture-secret")
    );
    controller.save_tavily_key("").unwrap();
    assert!(!controller.status().tavily_configured);
}

#[test]
fn tavily_search_preserves_contract_filters_sources_and_reports_usage() {
    let root = temporary_dir().unwrap();
    let controller = SearchController::shared(&root.0);
    controller.save_tavily_key("fixture-secret").unwrap();
    controller
        .save(SearchSettings {
            backend: SearchBackend::Tavily,
            ..SearchSettings::default()
        })
        .unwrap();
    let (url, task) = api(
        200,
        r#"{"results":[{"title":"Docs","url":"https://docs.python.org/3/","content":"Reference"},{"title":"Duplicate","url":"https://docs.python.org/3/","content":"Other"},{"title":"Wrong","url":"https://evil.org/","content":"Wrong"},{"title":"Private","url":"http://127.0.0.1/","content":"Private"}],"usage":{"credits":1}}"#,
    );
    *controller.tavily_endpoint.lock().unwrap() = Some(url);
    let filter = DomainFilter::new(&["python.org".into()], &[]).unwrap();
    let result = controller
        .search_filtered("Python docs", &filter, &|| false)
        .unwrap();
    assert_eq!(result.results.len(), 1);
    assert_eq!(result.results[0].snippet, "Reference");
    assert_eq!(result.source, "tavily");
    assert_eq!(result.credits, Some(1));
    let request = task.join().unwrap();
    let (headers, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(
        headers
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-secret")
    );
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["query"], "Python docs");
    assert_eq!(body["include_domains"], serde_json::json!(["python.org"]));
    assert_eq!(body["search_depth"], "basic");
    assert_eq!(body["auto_parameters"], false);
    assert_eq!(body["include_answer"], false);
    assert_eq!(body["max_results"], 5);
}

#[test]
fn api_failures_are_typed_sanitized_and_do_not_silently_use_browser() {
    for (status, body, kind) in [
        (401, "fixture-secret", SearchErrorKind::Authentication),
        (432, "fixture-secret", SearchErrorKind::Quota),
        (429, "fixture-secret", SearchErrorKind::RateLimited),
        (500, "fixture-secret", SearchErrorKind::Unavailable),
        (302, "fixture-secret", SearchErrorKind::Protocol),
        (200, "invalid", SearchErrorKind::Protocol),
        (200, r#"{"results":[]}"#, SearchErrorKind::NoResults),
    ] {
        let root = temporary_dir().unwrap();
        let controller = SearchController::shared(&root.0);
        controller.save_tavily_key("fixture-secret").unwrap();
        controller
            .save(SearchSettings {
                backend: SearchBackend::Tavily,
                ..SearchSettings::default()
            })
            .unwrap();
        let (url, task) = api(status, body);
        *controller.tavily_endpoint.lock().unwrap() = Some(url);
        let error = controller
            .search_filtered("test", &DomainFilter::default(), &|| false)
            .unwrap_err();
        assert_eq!(error.kind, kind);
        assert!(!error.to_string().contains("fixture-secret"));
        task.join().unwrap();
    }
}

#[test]
fn missing_key_and_cancellation_do_not_fallback() {
    let root = temporary_dir().unwrap();
    let controller = SearchController::shared(&root.0);
    controller
        .save(SearchSettings {
            backend: SearchBackend::Tavily,
            browser_fallback: true,
            ..SearchSettings::default()
        })
        .unwrap();
    for (cancelled, expected) in [
        (false, SearchErrorKind::MissingCredential),
        (true, SearchErrorKind::Cancelled),
    ] {
        assert_eq!(
            controller
                .search_filtered("test", &DomainFilter::default(), &|| cancelled)
                .unwrap_err()
                .kind,
            expected
        );
    }
}

#[test]
fn opted_in_browser_fallback_is_bounded_and_never_hides_authentication_errors() {
    for (status, fallback) in [(503, true), (429, true), (401, false), (432, false)] {
        let root = temporary_dir().unwrap();
        let controller = SearchController::shared(&root.0);
        controller.save_tavily_key("fixture-secret").unwrap();
        controller
            .save(SearchSettings {
                backend: SearchBackend::Tavily,
                browser_fallback: true,
                ..SearchSettings::default()
            })
            .unwrap();
        *controller.fixture.lock().unwrap() = Some(Ok(vec![SearchResult {
            title: "Docs".into(),
            url: "https://docs.python.org/".into(),
            snippet: "Docs".into(),
        }]));
        let (url, task) = api(status, "private response must not leak");
        *controller.tavily_endpoint.lock().unwrap() = Some(url);
        let result = controller.search("test", &|| false);
        if fallback {
            let result = result.unwrap();
            assert_eq!(result.source, "fixture");
            assert!(result.fallback_reason.is_some());
        } else {
            assert!(result.is_err());
        }
        task.join().unwrap();
        assert!(!controller.status().busy);
    }
}

#[test]
#[ignore = "Requires Chrome, public network, a Tavily key file, and an explicit report path"]
fn live_backend_comparison() {
    use crate::search_quality::{DOCUMENTATION_CASES, matches_target};
    let key_path = std::env::var("TOKEN_STATION_TAVILY_KEY_FILE")
        .expect("Set the private Tavily key file path");
    let report_path =
        std::env::var("TOKEN_STATION_SEARCH_REPORT").expect("Set the output report path");
    let key = std::fs::read_to_string(key_path).expect("Cannot read the private key file");
    let root = temporary_dir().unwrap();
    let controller = SearchController::shared(&root.0);
    controller.save_tavily_key(&key).unwrap();
    let mut cases: Vec<_> = DOCUMENTATION_CASES
        .iter()
        .map(|c| (c.query, c.expected_source, ""))
        .collect();
    cases.extend([
        ("MDN JavaScript Array 中文文档", "https://developer.mozilla.org/zh-CN/docs/Web/JavaScript/Reference/Global_Objects/Array", ""),
        ("Rust error E0382 use of moved value", "https://doc.rust-lang.org/error_codes/E0382.html", ""),
        ("Python asyncio documentation", "https://docs.python.org/3/library/asyncio", "docs.python.org"),
    ]);
    let mut observations = Vec::new();
    for (query, expected, domain) in cases {
        for backend in [SearchBackend::Browser, SearchBackend::Tavily] {
            controller
                .save(SearchSettings {
                    backend,
                    mode: SearchMode::Local,
                    ..SearchSettings::default()
                })
                .unwrap();
            let domains = if domain.is_empty() {
                vec![]
            } else {
                vec![domain.into()]
            };
            let filter = DomainFilter::new(&domains, &[]).unwrap();
            let started = Instant::now();
            let observation = match controller.search_filtered(query, &filter, &|| false) {
                Ok(response) => {
                    let rank = response
                        .results
                        .iter()
                        .position(|r| matches_target(&r.url, expected))
                        .map(|i| i + 1);
                    serde_json::json!({"backend":backend,"query":query,"expected_source":expected,"allowed_domains":domains,"execution":"completed","target_rank":rank,"response":response})
                }
                Err(error) => {
                    serde_json::json!({"backend":backend,"query":query,"expected_source":expected,"allowed_domains":domains,"execution":"failed","elapsed_ms":started.elapsed().as_millis(),"error":error})
                }
            };
            observations.push(observation);
            let report = serde_json::json!({"scope":"Same-query top-five source presence. No model calls or factual-support assessment. Browser uses Bing; Tavily uses basic search. Currency cost is unknown; credits are provider-reported.","cases":observations});
            crate::private_fs::write_atomic_private(
                Path::new(&report_path),
                serde_json::to_string_pretty(&report).unwrap().as_bytes(),
            )
            .unwrap();
        }
    }
    println!("Saved 12 backend observations. Inspect the report for failures and source quality.");
}

#[test]
fn oversized_or_malformed_sources_fail_without_browser_fallback() {
    for body in [
        "x".repeat(1024 * 1024 + 1),
        r#"{"results":[{"url":"https://example.com"}]}"#.to_owned(),
        r#"{"results":[]}"#.to_owned(),
    ] {
        let root = temporary_dir().unwrap();
        let controller = SearchController::shared(&root.0);
        controller.save_tavily_key("fixture-secret").unwrap();
        controller
            .save(SearchSettings {
                backend: SearchBackend::Tavily,
                browser_fallback: true,
                ..SearchSettings::default()
            })
            .unwrap();
        let (url, task) = api(200, &body);
        *controller.tavily_endpoint.lock().unwrap() = Some(url);
        let error = controller
            .search_filtered("test", &DomainFilter::default(), &|| false)
            .unwrap_err();
        assert!(matches!(
            error.kind,
            SearchErrorKind::Protocol | SearchErrorKind::NoResults
        ));
        task.join().unwrap();
    }
}
