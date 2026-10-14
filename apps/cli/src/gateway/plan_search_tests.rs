//! `ChatGPT` plan managed search through the installed Responses component.
use super::{Gateway, Reply};
use crate::config::ClientConfig;
use crate::search::{SearchBackend, SearchMode, SearchResult, SearchSettings};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use token_station_metrics::{BrowserSearchOutcome, CostKind, Recorder, RequestRecord};

const INTERNAL: &str = "token_station_browser_search";
const SOURCE: &str = "https://www.rust-lang.org/learn";
const SNIPPET: &str = "Fixture source: Rust provides ownership without a garbage collector.";
const INVENTED: &str = "UNVERIFIED_MODEL_SUMMARY";

#[derive(Default)]
struct Records(Mutex<Vec<RequestRecord>>);
impl Recorder for Records {
    fn record(&self, record: &RequestRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ts-plan-search-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read_request(stream: &mut TcpStream) -> (String, Value) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 64 * 1024);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse().unwrap())
        })
        .unwrap();
    assert!(length < 1024 * 1024);
    let mut body = vec![0; length];
    stream.read_exact(&mut body).unwrap();
    (headers, serde_json::from_slice(&body).unwrap())
}

struct Server {
    endpoint: String,
    seen: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(handler: impl Fn(usize, &Value) -> (u16, String) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let capture = Arc::clone(&seen);
        let stopped = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("fixture listener: {error}"),
                };
                let (headers, body) = read_request(&mut stream);
                assert!(headers.starts_with("POST "));
                let mut requests = capture.lock().unwrap();
                let round = requests.len();
                requests.push(body.clone());
                drop(requests);
                assert!(round < 12, "host loop must remain bounded");
                let (status, response) = handler(round, &body);
                let mime = if response.starts_with("event:") {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                    response.len()
                );
            }
        });
        Self {
            endpoint,
            seen,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let result = worker.join();
            if !std::thread::panicking() {
                result.unwrap();
            }
        }
    }
}

fn events(events: Vec<Value>) -> String {
    use std::fmt::Write as _;
    events.into_iter().fold(String::new(), |mut output, event| {
        write!(
            output,
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap()
        )
        .unwrap();
        output
    })
}

fn searches(round: usize, arguments: &[Value]) -> String {
    let mut frames = vec![
        json!({"type":"response.created","response":{"id":format!("resp_{round}"),"model":"gpt-5.5","status":"in_progress"}}),
    ];
    let mut output = Vec::new();
    for (index, args) in arguments.iter().enumerate() {
        let id = format!("fc_{round}_{index}");
        let arguments = args.to_string();
        let item = json!({"type":"function_call","id":id,"call_id":format!("call_{round}_{index}"),"name":INTERNAL,"arguments":"","status":"in_progress"});
        let mut done = item.clone();
        done["arguments"] = json!(arguments);
        done["status"] = json!("completed");
        frames.push(json!({"type":"response.output_item.added","output_index":index,"item":item}));
        frames.push(json!({"type":"response.function_call_arguments.delta","output_index":index,"item_id":id,"delta":arguments}));
        frames.push(json!({"type":"response.function_call_arguments.done","output_index":index,"item_id":id,"arguments":arguments}));
        frames.push(json!({"type":"response.output_item.done","output_index":index,"item":done}));
        output.push(done);
    }
    frames.push(json!({"type":"response.completed","response":{"id":format!("resp_{round}"),"model":"gpt-5.5","status":"completed","output":output,"usage":{"input_tokens":10,"output_tokens":5}}}));
    events(frames)
}

fn final_answer(terminal: &str) -> String {
    let mut frames = vec![
        json!({"type":"response.created","response":{"id":"resp_final","model":"gpt-5.5","status":"in_progress"}}),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":INVENTED}),
    ];
    if terminal != "truncated" {
        frames.push(json!({"type":format!("response.{terminal}"),"response":{"id":"resp_final","model":"gpt-5.5","status":terminal,"output":[],"usage":{"input_tokens":10,"output_tokens":5},"error":if terminal == "completed" {Value::Null} else {json!({"code":"server_error","message":"Fixture model failure"})}}}));
    }
    events(frames)
}

struct Harness {
    gateway: Gateway,
    records: Arc<Records>,
    model: Server,
    paid: TcpListener,
    scratch: Scratch,
}
impl Harness {
    fn new(handler: impl Fn(usize, &Value) -> (u16, String) + Send + 'static) -> Self {
        let scratch = Scratch::new();
        let model = Server::new(handler);
        let paid = TcpListener::bind("127.0.0.1:0").unwrap();
        paid.set_nonblocking(true).unwrap();
        crate::secrets::store_set(
            &scratch.0,
            "plan",
            "chatgpt_access_token",
            "local-plan-fixture",
        )
        .unwrap();
        let config: ClientConfig = serde_json::from_value(json!({
            "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":scratch.0,"metrics":false},
            "plugins":{"dir":Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins-dist"),"allow_unsigned":true,"agents":["agent-anthropic","agent-openai-responses"],"providers":{"chatgpt-plan":"provider-chatgpt-plan-v2","openai-compatible":"provider-openai-compatible-v2"}},
            "upstreams":{"plan":{"provider":"chatgpt-plan","base_url":format!("{}/v1",model.endpoint),"auth":{"slot":"chatgpt_access_token","store":true},"models":[{"model":"gpt-5.5","tool":true,"tool_state":"verified","context_window":200_000}]},"paid":{"provider":"openai-compatible","base_url":format!("http://{}/v1",paid.local_addr().unwrap()),"models":[{"model":"gpt-5.5","tool":true,"tool_state":"verified","context_window":200_000}]}},
            "pricing":{"version":77,"models":{"gpt-5.5":{"input_per_mtok":5_000_000,"output_per_mtok":30_000_000}}},
            "router":{"version":1,"pools":{"main":[{"upstream":"plan","model":"gpt-5.5"},{"upstream":"paid","model":"gpt-5.5"}]},"default_pool":"main"}
        })).unwrap();
        config.validate().unwrap();
        let records = Arc::new(Records::default());
        let gateway = Gateway::new(&config, records.clone()).unwrap();
        gateway
            .search
            .save(SearchSettings {
                enabled: true,
                mode: SearchMode::Local,
                ..SearchSettings::default()
            })
            .unwrap();
        *gateway.search.fixture.lock().unwrap() = Some(Ok(vec![SearchResult {
            title: "Rust documentation".into(),
            url: SOURCE.into(),
            snippet: SNIPPET.into(),
        }]));
        Self {
            gateway,
            records,
            model,
            paid,
            scratch,
        }
    }

    fn tavily(&self, status: u16, body: &'static str) -> Server {
        let server = Server::new(move |_, _| (status, body.into()));
        self.gateway
            .search
            .save_tavily_key("local-search-fixture")
            .unwrap();
        self.gateway
            .search
            .save(SearchSettings {
                enabled: true,
                mode: SearchMode::Local,
                backend: SearchBackend::Tavily,
                ..SearchSettings::default()
            })
            .unwrap();
        *self.gateway.search.tavily_endpoint.lock().unwrap() = Some(server.endpoint.clone());
        server
    }

    fn request(
        &self,
        anthropic: bool,
        stream: bool,
        modify: impl FnOnce(&mut Value),
    ) -> (u16, String) {
        let mut body = if anthropic {
            json!({"model":"gpt-5.5","max_tokens":200,"messages":[{"role":"user","content":"Find Rust documentation"}],"tools":[{"type":"web_search_20250305","name":"web_search"}],"tool_choice":{"type":"tool","name":"web_search"},"stream":stream})
        } else {
            json!({"model":"gpt-5.5","input":"Find Rust documentation","tools":[{"type":"web_search"}],"tool_choice":{"type":"web_search"},"stream":stream})
        };
        modify(&mut body);
        let mut status = 0;
        let mut output = String::new();
        self.gateway.chat(
            "POST",
            if anthropic {
                "/v1/messages"
            } else {
                "/v1/responses"
            },
            &[],
            body.to_string().as_bytes(),
            &mut |reply| {
                match reply {
                    Reply::BeginJson(reply) => {
                        status = reply.status;
                        output.push_str(&reply.body);
                    }
                    Reply::BeginStream => status = 200,
                    Reply::Chunk(chunk) => output.push_str(&chunk),
                }
                true
            },
        );
        assert_eq!(
            self.paid.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "subscription search must never contact the paid fallback"
        );
        assert!(!output.contains("local-plan-fixture") && !output.contains("local-search-fixture"));
        (status, output)
    }

    fn receipt(&self) -> RequestRecord {
        self.records.0.lock().unwrap().last().unwrap().clone()
    }
}

fn two_rounds(round: usize, _: &Value) -> (u16, String) {
    (
        200,
        if round == 0 {
            searches(round, &[json!({"query":"Rust documentation"})])
        } else {
            final_answer("completed")
        },
    )
}

#[test]
fn plan_search_returns_real_evidence_in_both_protocols_and_stream_modes() {
    for anthropic in [false, true] {
        for stream in [false, true] {
            let harness = Harness::new(two_rounds);
            let (status, output) = harness.request(anthropic, stream, |_| {});
            assert_eq!(status, 200, "{output}");
            assert!(
                output.contains(SNIPPET) && output.contains(SOURCE),
                "{output}"
            );
            assert!(!output.contains(INVENTED) && !output.contains(INTERNAL));
            let requests = harness.model.seen.lock().unwrap();
            assert_eq!(requests.len(), 2);
            let history = requests[1]["input"].as_array().unwrap();
            let call = history
                .iter()
                .find(|v| v["type"] == "function_call")
                .unwrap();
            let result = history
                .iter()
                .find(|v| v["type"] == "function_call_output")
                .unwrap();
            assert_eq!(call["call_id"], result["call_id"]);
            assert!(result["output"].as_str().unwrap().contains(SNIPPET));
            assert_eq!(
                requests[0]["stream"], true,
                "the plan component must consume Responses SSE"
            );
            assert_eq!(requests[0]["parallel_tool_calls"], false);
            let receipt = harness.receipt();
            assert!(receipt.subscription_billing);
            assert_eq!(
                (
                    receipt.cost_kind,
                    receipt.cost_micros,
                    receipt.price_version
                ),
                (CostKind::Unknown, None, None)
            );
            assert_eq!(receipt.attempts, 2);
            assert_eq!(receipt.usage.unwrap().input_tokens, 20);
            assert_eq!(receipt.usage.unwrap().output_tokens, 10);
            let db = harness.scratch.0.join("metrics.sqlite");
            let store = crate::store::SqliteStore::open(&db).unwrap();
            store.record(&receipt);
            let report = crate::stats::collect(&db, None, None).unwrap();
            assert_eq!(report.total.cost_micros, None);
            assert_eq!(report.total.subscription_requests, 1);
        }
    }
}

#[test]
fn plan_search_filters_domains_before_returning_or_replaying_evidence() {
    for anthropic in [false, true] {
        let harness = Harness::new(two_rounds);
        harness
            .gateway
            .search
            .fixture
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .push(SearchResult {
                title: "Excluded source".into(),
                url: "https://example.com/disallowed".into(),
                snippet: "EXCLUDED_FIXTURE".into(),
            });
        let (status, output) = harness.request(anthropic, false, |body| {
            if anthropic {
                body["tools"][0]["allowed_domains"] = json!(["rust-lang.org"]);
            } else {
                body["tools"][0]["filters"] = json!({"allowed_domains":["rust-lang.org"]});
            }
        });
        assert_eq!(status, 200, "{output}");
        assert!(output.contains(SNIPPET));
        assert!(!output.contains("EXCLUDED_FIXTURE"));
        assert!(
            !harness.model.seen.lock().unwrap()[1]["input"]
                .to_string()
                .contains("EXCLUDED_FIXTURE")
        );
    }
}

#[test]
fn plan_search_invalid_arguments_become_explicit_tool_errors() {
    let harness = Harness::new(|round, _| {
        (
            200,
            if round == 0 {
                searches(round, &[json!({"query":42})])
            } else {
                final_answer("completed")
            },
        )
    });
    let (status, output) = harness.request(false, false, |_| {});
    assert_eq!(status, 200, "{output}");
    assert!(!output.contains(SNIPPET) && !output.contains(INVENTED));
    assert!(
        output.contains("invalid_arguments") && output.contains("failed"),
        "{output}"
    );
    assert_eq!(
        harness.receipt().browser_searches[0].outcome,
        BrowserSearchOutcome::InvalidArguments
    );
    assert!(
        harness.model.seen.lock().unwrap()[1]["input"]
            .to_string()
            .contains("invalid_arguments")
    );
}

#[test]
fn plan_search_parallel_calls_stop_at_the_declared_budget() {
    let harness = Harness::new(|round, _| {
        (
            200,
            if round == 0 {
                searches(
                    round,
                    &[json!({"query":"Rust"}), json!({"query":"second query"})],
                )
            } else {
                final_answer("completed")
            },
        )
    });
    let (status, output) = harness.request(false, true, |body| body["max_tool_calls"] = json!(1));
    assert_eq!(status, 200, "{output}");
    assert!(output.contains(SNIPPET) && output.contains("search limit"));
    let receipt = harness.receipt();
    assert_eq!(
        receipt
            .browser_searches
            .iter()
            .map(|s| &s.outcome)
            .collect::<Vec<_>>(),
        vec![
            &BrowserSearchOutcome::Succeeded,
            &BrowserSearchOutcome::LimitExceeded
        ]
    );
    let requests = harness.model.seen.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["tools"].is_null() || requests[1]["tools"] == json!([]));
    assert!(
        !requests[1]["input"].to_string().contains("call_0_"),
        "retiring tools must not leave dangling call pairs"
    );
}

#[test]
fn plan_search_no_results_is_not_a_successful_retrieval() {
    let harness = Harness::new(two_rounds);
    let api = harness.tavily(200, r#"{"results":[],"usage":{"credits":2}}"#);
    let (status, output) = harness.request(false, true, |_| {});
    assert_eq!(status, 200, "{output}");
    assert!(
        output.contains("no_results") && output.contains("failed"),
        "{output}"
    );
    assert!(!output.contains(SNIPPET) && !output.contains(INVENTED));
    assert_eq!(
        harness.receipt().browser_searches[0].outcome,
        BrowserSearchOutcome::NoResults
    );
    assert_eq!(
        harness.receipt().browser_searches[0].backend.as_deref(),
        Some("tavily")
    );
    assert_eq!(harness.receipt().browser_searches[0].credits, Some(2));
    assert_eq!(api.seen.lock().unwrap().len(), 1);
}

#[test]
fn plan_search_rate_limit_is_explicit_and_does_not_switch_backends() {
    let harness = Harness::new(two_rounds);
    let api = harness.tavily(429, r#"{"error":"local-search-fixture"}"#);
    let (status, output) = harness.request(true, false, |_| {});
    assert_eq!(status, 200, "{output}");
    assert!(
        output.contains("rate_limited") && output.contains("web_search_tool_result_error"),
        "{output}"
    );
    assert!(!output.contains(SNIPPET) && !output.contains(INVENTED));
    assert_eq!(
        harness.receipt().browser_searches[0].error_kind.as_deref(),
        Some("rate_limited")
    );
    assert_eq!(api.seen.lock().unwrap().len(), 1);
}

#[test]
fn plan_search_failed_or_truncated_model_round_never_becomes_success() {
    for terminal in ["failed", "incomplete", "truncated", "rate_limited"] {
        for stream in [false, true] {
            let harness = Harness::new(move |round, body| {
                if round == 0 {
                    return two_rounds(round, body);
                }
                if terminal == "rate_limited" {
                    (
                        429,
                        json!({"error":{"message":"Fixture rate limit","type":"rate_limit_error"}})
                            .to_string(),
                    )
                } else {
                    (200, final_answer(terminal))
                }
            });
            let (status, output) = harness.request(false, stream, |_| {});
            assert!(status >= 400, "{terminal}: {status}: {output}");
            assert!(!output.contains(INVENTED));
            assert_eq!(harness.model.seen.lock().unwrap().len(), 2);
            assert_ne!(harness.receipt().status, 200);
            assert_eq!(harness.receipt().cost_micros, None);
        }
    }
}

#[test]
fn plan_search_omitted_required_call_does_not_invent_evidence() {
    let harness = Harness::new(|_, _| (200, final_answer("completed")));
    let (status, output) = harness.request(true, true, |_| {});
    assert_eq!(status, 502, "{output}");
    assert!(!output.contains(INVENTED));
    assert_eq!(harness.model.seen.lock().unwrap().len(), 1);
    assert!(harness.receipt().browser_searches.is_empty());
}

#[test]
fn plan_search_can_read_a_returned_page_without_real_network_access() {
    let harness = Harness::new(|round, _| {
        (
            200,
            match round {
                0 => searches(round, &[json!({"query":"Rust"})]),
                1 => searches(round, &[json!({"url":SOURCE})]),
                _ => final_answer("completed"),
            },
        )
    });
    *harness.gateway.search.page_fixture.lock().unwrap() = Some((
        "<html><body><main>PAGE_FIXTURE: Rust ownership guide.</main></body></html>".into(),
        "text/html".into(),
    ));
    let (status, output) = harness.request(false, false, |_| {});
    assert_eq!(status, 200, "{output}");
    assert!(
        output.contains("PAGE_FIXTURE") && output.contains(SOURCE),
        "{output}"
    );
    assert_eq!(harness.model.seen.lock().unwrap().len(), 3);
    assert_eq!(harness.receipt().browser_searches.len(), 2);
    assert!(
        harness
            .receipt()
            .browser_searches
            .iter()
            .all(|s| s.outcome == BrowserSearchOutcome::Succeeded)
    );
}

#[test]
fn plan_auto_search_uses_tavily_results_and_records_search_credits_separately() {
    let harness = Harness::new(two_rounds);
    let api = harness.tavily(200, r#"{"results":[{"title":"Rust docs","url":"https://www.rust-lang.org/learn","content":"TAVILY_VERIFIED_FIXTURE"},{"title":"Excluded","url":"https://example.com/other","content":"EXCLUDED_FIXTURE"}],"usage":{"credits":1}}"#);
    harness
        .gateway
        .search
        .save(SearchSettings {
            enabled: true,
            mode: SearchMode::Auto,
            backend: SearchBackend::Tavily,
            ..SearchSettings::default()
        })
        .unwrap();
    let (status, output) = harness.request(true, true, |body| {
        body["tools"][0]["allowed_domains"] = json!(["rust-lang.org"]);
    });
    assert_eq!(status, 200, "{output}");
    assert!(output.contains("TAVILY_VERIFIED_FIXTURE") && output.contains(SOURCE));
    assert!(!output.contains("EXCLUDED_FIXTURE") && !output.contains(INVENTED));
    let requests = api.seen.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["query"], "Rust documentation");
    assert_eq!(requests[0]["include_domains"], json!(["rust-lang.org"]));
    assert!(requests[0].get("messages").is_none() && requests[0].get("input").is_none());
    let record = harness.receipt();
    assert_eq!(
        record.browser_searches[0].backend.as_deref(),
        Some("tavily")
    );
    assert_eq!(record.browser_searches[0].credits, Some(1));
    assert_eq!(
        record.browser_searches[0].outcome,
        BrowserSearchOutcome::Succeeded
    );
    assert_eq!(record.cost_micros, None);
    assert!(record.subscription_billing);
}

#[test]
fn plan_search_rejects_tool_constraint_violations_before_retrieval() {
    for anthropic in [false, true] {
        for scenario in ["none", "undeclared", "parallel", "forced_client"] {
            let harness = Harness::new(move |round, _| {
                let mut response = searches(
                    round,
                    &if scenario == "parallel" {
                        vec![json!({"query":"Rust"}), json!({"query":"second"})]
                    } else {
                        vec![json!({"query":"Rust"})]
                    },
                );
                if matches!(scenario, "undeclared" | "forced_client") {
                    response = response.replace(INTERNAL, "edit");
                }
                (200, response)
            });
            let (status, output) = harness.request(anthropic, false, |body| {
                if scenario == "none" {
                    body["tool_choice"] = if anthropic {json!({"type":"none"})} else {json!("none")};
                }
                if scenario == "parallel" {
                    if anthropic {body["tool_choice"]["disable_parallel_tool_use"] = json!(true);}
                    else {body["parallel_tool_calls"] = json!(false);}
                }
                if scenario == "forced_client" {
                    body["tools"].as_array_mut().unwrap().push(if anthropic {
                        json!({"name":"edit","input_schema":{"type":"object","properties":{}}})
                    } else {json!({"type":"function","name":"edit","parameters":{"type":"object","properties":{}}})});
                }
            });
            // The Anthropic adapter rejects `none` before inference. The
            // Responses contract admits it and must reject an upstream violation.
            let rejected_at_ingress = anthropic && scenario == "none";
            assert_eq!(
                status,
                if rejected_at_ingress { 400 } else { 502 },
                "{anthropic}/{scenario}: {output}"
            );
            assert!(
                harness.receipt().browser_searches.is_empty(),
                "No retrieval may occur before all calls are validated"
            );
            assert_eq!(
                harness.model.seen.lock().unwrap().len(),
                usize::from(!rejected_at_ingress)
            );
        }
    }
}
