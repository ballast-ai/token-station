//! Observable search evidence. Target presence does not prove factual support.
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

/// A stable query and an expected documentation URL prefix.
pub struct SearchBenchmarkCase {
    pub query: &'static str,
    pub expected_source: &'static str,
}

pub const DOCUMENTATION_CASES: [SearchBenchmarkCase; 3] = [
    SearchBenchmarkCase {
        query: "Python official documentation",
        expected_source: "https://docs.python.org/",
    },
    SearchBenchmarkCase {
        query: "Rust Cargo official documentation",
        expected_source: "https://doc.rust-lang.org/cargo/",
    },
    SearchBenchmarkCase {
        query: "MDN JavaScript Array documentation",
        expected_source: "https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Array",
    },
];

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SearchQuality {
    pub expected_source: String,
    pub source_count: usize,
    pub target_found: bool,
    pub cited_source_count: usize,
}

fn normalized_url(raw: &str) -> Option<String> {
    let public = crate::search::public_url(raw)?;
    let mut url = url::Url::parse(&public).ok()?;
    url.set_fragment(None);
    Some(url.to_string())
}

/// Match the exact host and a path boundary. Never use domain substring matching.
#[must_use]
pub fn matches_target(source: &str, expected: &str) -> bool {
    let (Some(source), Some(expected)) = (normalized_url(source), normalized_url(expected)) else {
        return false;
    };
    let (Ok(source), Ok(expected)) = (url::Url::parse(&source), url::Url::parse(&expected)) else {
        return false;
    };
    let prefix = expected.path().trim_end_matches('/');
    source.host_str() == expected.host_str()
        && source.port().is_none()
        && (source.path() == prefix || source.path().starts_with(&format!("{prefix}/")))
}

fn text_urls(text: &str) -> impl Iterator<Item = String> + '_ {
    // Scan each token once. Repeated URL prefixes must not cause quadratic work.
    text.split(|c: char| {
        c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`' | '(' | ')' | '[' | ']')
    })
    .filter_map(|token| {
        let start = token.find("http")?;
        normalized_url(
            token[start..].trim_end_matches(['.', ',', ';', '!', '。', '，', '；', '！']),
        )
    })
}

/// Require completed execution and report independent source and citation observations.
/// Only URLs in completed search calls count as retrieved evidence.
///
/// # Errors
/// Returns an error when execution is incomplete or public sources or final text are missing.
pub fn evaluate_response(document: &Value, expected_source: &str) -> Result<SearchQuality, String> {
    let output = document["output"]
        .as_array()
        .ok_or("The model did not return search evidence.")?;
    let sources: BTreeSet<String> = output
        .iter()
        .filter(|item| item["type"] == "web_search_call" && item["status"] == "completed")
        .flat_map(|item| item["action"]["sources"].as_array().into_iter().flatten())
        .filter_map(|source| source["url"].as_str().and_then(normalized_url))
        .collect();
    let mut has_text = false;
    let mut cited = BTreeSet::new();
    for part in output
        .iter()
        .filter(|item| item["type"] == "message")
        .flat_map(|item| item["content"].as_array().into_iter().flatten())
        .filter(|part| part["type"] == "output_text")
    {
        let text = part["text"].as_str().unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        has_text = true;
        cited.extend(text_urls(text));
        cited.extend(
            part["annotations"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|annotation| annotation["type"] == "url_citation")
                .filter_map(|annotation| annotation["url"].as_str().and_then(normalized_url)),
        );
    }
    if document["status"] != "completed" || sources.is_empty() || !has_text {
        return Err("The model did not complete a search with public sources and final text. Check the search path and model tool support.".into());
    }
    Ok(SearchQuality {
        expected_source: expected_source.to_owned(),
        source_count: sources.len(),
        target_found: sources
            .iter()
            .any(|source| matches_target(source, expected_source)),
        cited_source_count: sources.intersection(&cited).count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn response(url: &str, text: &str) -> Value {
        json!({"status":"completed","output":[
            {"type":"web_search_call","status":"completed","action":{"sources":[{"url":url},{"url":url}]}},
            {"type":"message","content":[{"type":"output_text","text":text}]}
        ]})
    }

    #[test]
    fn execution_target_and_citations_are_independent() {
        let target = DOCUMENTATION_CASES[0].expected_source;
        let quality =
            evaluate_response(&response("https://example.com/", "An answer"), target).unwrap();
        assert_eq!(quality.source_count, 1);
        assert!(!quality.target_found);
        assert_eq!(quality.cited_source_count, 0);
        let quality = evaluate_response(&response(target, "An answer"), target).unwrap();
        assert!(quality.target_found);
        assert_eq!(quality.cited_source_count, 0);
        let quality = evaluate_response(
            &response(target, "See [docs](https://docs.python.org/#contents)."),
            target,
        )
        .unwrap();
        assert!(quality.target_found);
        assert_eq!(quality.cited_source_count, 1);
    }

    #[test]
    fn failed_private_and_invented_sources_do_not_prove_evidence() {
        let target = DOCUMENTATION_CASES[0].expected_source;
        for url in [
            "file:///tmp/docs",
            "http://127.0.0.1/",
            "https://user:pass@docs.python.org/",
        ] {
            assert!(evaluate_response(&response(url, "text"), target).is_err());
        }
        let mut document = response(target, "text");
        document["output"][0]["status"] = json!("failed");
        assert!(evaluate_response(&document, target).is_err());
        let document = response("https://example.com/", "https://docs.python.org/");
        let quality = evaluate_response(&document, target).unwrap();
        assert!(!quality.target_found);
        assert_eq!(quality.cited_source_count, 0);
        assert!(evaluate_response(&json!({"status":"completed","output":[]}), target).is_err());
    }

    #[test]
    fn target_matching_rejects_spoofed_hosts_and_path_prefixes() {
        let target = DOCUMENTATION_CASES[0].expected_source;
        for url in [
            "https://docs.python.org.evil.example/",
            "https://evil.example/docs.python.org/",
            "https://docs.python.org:8443/",
        ] {
            assert!(!matches_target(url, target));
        }
        assert!(matches_target("https://docs.python.org/3/library/", target));
        let target = DOCUMENTATION_CASES[1].expected_source;
        assert!(matches_target(
            "https://doc.rust-lang.org/cargo/reference/",
            target
        ));
        assert!(!matches_target(
            "https://doc.rust-lang.org/cargo-other/",
            target
        ));
    }

    #[test]
    fn citations_require_returned_urls_and_nonempty_answer_text() {
        let target = DOCUMENTATION_CASES[0].expected_source;
        let mut document = response(target, "文档说明");
        document["output"][1]["content"][0]["annotations"] = json!([
            {"type":"url_citation","url":target}, {"type":"url_citation","url":"https://invented.example/"}
        ]);
        assert_eq!(
            evaluate_response(&document, target)
                .unwrap()
                .cited_source_count,
            1
        );
        document["output"][1]["content"][0]["text"] = json!(" ");
        assert!(evaluate_response(&document, target).is_err());
        document["output"][1]["content"][0]["text"] =
            json!("https://docs.python.org.evil.example/");
        document["output"][1]["content"][0]["annotations"] = json!([]);
        assert_eq!(
            evaluate_response(&document, target)
                .unwrap()
                .cited_source_count,
            0
        );
        document["output"][1]["content"][0]["text"] =
            json!("https://evil.example/?next=https://docs.python.org/");
        assert_eq!(
            evaluate_response(&document, target)
                .unwrap()
                .cited_source_count,
            0
        );
        document["status"] = json!("incomplete");
        assert!(evaluate_response(&document, target).is_err());
    }
}
