//! Full local tool loops through the shipped Agent and `ChatGPT` Responses components.
//! Default tests and installed-CLI tests use fixture credentials and a local upstream.
//! Explicitly ignored `live_*` tests use the selected real subscription account.
//! Every CLI runs with isolated configuration and receives only a local virtual key.

#[path = "common/live_subscription.rs"]
mod live_subscription;

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use token_station_cli::{config::ClientConfig, gateway::Gateway, server};

const MARKERS: [&str; 2] = ["TOKEN_STATION_TOOL_ONE", "TOKEN_STATION_TOOL_TWO"];
const FINAL_TEXT: &str = "Both local tool results were verified.";
const MOCK_CREDENTIAL: &str = "chatgpt-tool-fixture-only";
const VIRTUAL_KEY: &str = "local-agent-fixture-only";

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ts-chatgpt-tools-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(std::fs::canonicalize(path).unwrap())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stage_plugins(destination: &Path) {
    static BUILT: OnceLock<()> = OnceLock::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let packages = [
        ("agent-openai", "adapter.wasm"),
        ("agent-openai-responses", "adapter.wasm"),
        ("agent-gemini", "adapter.wasm"),
        ("agent-anthropic", "adapter.wasm"),
        ("provider-openai-compatible-v2", "component.wasm"),
        ("provider-chatgpt-plan-v2", "component.wasm"),
    ];
    BUILT.get_or_init(|| {
        for (name, _) in packages {
            let source = root.join("plugins/official").join(name);
            assert!(
                Command::new("cargo")
                    .args(["build", "--release", "--target", "wasm32-wasip2"])
                    .current_dir(&source)
                    .status()
                    .unwrap()
                    .success(),
                "build {name}"
            );
        }
    });
    for (name, file) in packages {
        let source = root.join("plugins/official").join(name);
        let dest = destination.join(name);
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::copy(source.join("manifest.json"), dest.join("manifest.json")).unwrap();
        std::fs::copy(
            source
                .join("target/wasm32-wasip2/release")
                .join(format!("{}.wasm", name.replace('-', "_"))),
            dest.join(file),
        )
        .unwrap();
    }
}

fn read_request(stream: &mut TcpStream) -> (String, Value) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .unwrap();
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut chunk = [0; 8192];
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0, "request headers closed early");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break index + 4;
        }
        assert!(bytes.len() < 1024 * 1024);
    };
    let header = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
    let length: usize = header
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse().unwrap())
        })
        .unwrap();
    while bytes.len() - header_end < length {
        let mut chunk = [0; 8192];
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&chunk[..count]);
    }
    (
        header,
        serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap(),
    )
}

fn sse(events: Vec<Value>) -> String {
    use std::fmt::Write as _;
    events.into_iter().fold(String::new(), |mut text, event| {
        write!(
            text,
            "event: {}\ndata: {event}\n\n",
            event["type"].as_str().unwrap()
        )
        .unwrap();
        text
    })
}

fn text_response_with(text: &str, failed: bool) -> String {
    sse(vec![
        json!({"type":"response.created","response":{"id":"resp_final","model":"gpt-5.5","status":"in_progress"}}),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":text}),
        json!({"type":if failed { "response.failed" } else { "response.completed" },"response":{"id":"resp_final","model":"gpt-5.5","status":if failed { "failed" } else { "completed" },"output":[],"usage":{"input_tokens":10,"output_tokens":8},"error":if failed { json!({"code":"server_error","message":"Fixture terminal failure"}) } else { Value::Null }}}),
    ])
}
fn tool_response(name: &str, argument: &str, path: &Path, turn: usize) -> String {
    let arguments = json!({argument:path}).to_string();
    let id = format!("fc_{turn}");
    let call = format!("call_{turn}");
    let item = json!({"type":"function_call","id":id,"call_id":call,"namespace":"tokenstation_tools","name":name,"arguments":"","status":"in_progress"});
    let mut done = item.clone();
    done["arguments"] = json!(arguments);
    done["status"] = json!("completed");
    let split = arguments.len() / 2;
    sse(vec![
        json!({"type":"response.created","response":{"id":format!("resp_{turn}"),"model":"gpt-5.5","status":"in_progress"}}),
        json!({"type":"response.output_item.added","output_index":0,"item":item}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":id,"delta":arguments[..split]}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":id,"delta":arguments[split..]}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":id,"arguments":arguments}),
        json!({"type":"response.output_item.done","output_index":0,"item":done}),
        json!({"type":"response.completed","response":{"id":format!("resp_{turn}"),"model":"gpt-5.5","status":"completed","output":[done],"usage":{"input_tokens":10,"output_tokens":8}}}),
    ])
}

fn declared_read_tool(body: &Value) -> Option<&Value> {
    body["tools"].as_array()?.iter()
        .find(|group| group["type"] == "namespace" && group["name"] == "tokenstation_tools")?["tools"].as_array()?.iter()
        .find(|tool| tool["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case("read")))
}

struct FakeResponses {
    url: String,
    seen: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    status: Arc<AtomicUsize>,
    text: Arc<Mutex<String>>,
    failed_after_text: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl FakeResponses {
    fn start(files: [PathBuf; 2]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let status = Arc::new(AtomicUsize::new(200));
        let response_status = Arc::clone(&status);
        let text = Arc::new(Mutex::new(FINAL_TEXT.to_owned()));
        let response_text = Arc::clone(&text);
        let failed_after_text = Arc::new(AtomicBool::new(false));
        let failed = Arc::clone(&failed_after_text);
        let stopped = Arc::clone(&stop);
        let requests = Arc::clone(&seen);
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                let (headers, body) = read_request(&mut stream);
                assert!(headers.starts_with("POST /v1/responses "), "{headers}");
                assert!(
                    headers
                        .to_ascii_lowercase()
                        .contains(&format!("authorization: bearer {MOCK_CREDENTIAL}"))
                );
                assert_eq!(body["stream"], true);
                assert_eq!(body["store"], false);
                for unsupported in ["temperature", "max_output_tokens", "previous_response_id"] {
                    assert!(body.get(unsupported).is_none());
                }
                let status = response_status.load(Ordering::Relaxed);
                if status != 200 {
                    requests.lock().unwrap().push(body);
                    let error = json!({"error":{"code":if status == 429 { "usage_limit_reached" } else { "server_error" },"message":"Fixture plan request failed"}}).to_string();
                    write!(stream, "HTTP/1.1 {status} Fixture Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{error}", error.len()).unwrap();
                    continue;
                }
                let input = body["input"].as_array().unwrap();
                let outputs: Vec<_> = input
                    .iter()
                    .filter(|item| item["type"] == "function_call_output")
                    .collect();
                for (index, output) in outputs.iter().enumerate() {
                    assert_eq!(output["call_id"], format!("call_{index}"));
                    assert!(
                        output["output"].to_string().contains(MARKERS[index]),
                        "the Agent must execute the local read: {output}"
                    );
                }
                let reader = declared_read_tool(&body);
                let response = if outputs.len() < 2 {
                    if let Some(reader) = reader {
                        let properties = &reader["parameters"]["properties"];
                        let argument = if properties.get("file_path").is_some() {
                            "file_path"
                        } else {
                            "filePath"
                        };
                        tool_response(
                            reader["name"].as_str().unwrap(),
                            argument,
                            &files[outputs.len()],
                            outputs.len(),
                        )
                    } else {
                        text_response_with(
                            &response_text.lock().unwrap(),
                            failed.load(Ordering::Relaxed),
                        )
                    }
                } else {
                    text_response_with(
                        &response_text.lock().unwrap(),
                        failed.load(Ordering::Relaxed),
                    )
                };
                requests.lock().unwrap().push(body);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len()).unwrap();
                // Split frames across transport writes as well as argument events.
                for chunk in response.as_bytes().chunks(37) {
                    stream.write_all(chunk).unwrap();
                }
            }
        });
        Self {
            url,
            seen,
            stop,
            status,
            text,
            failed_after_text,
            worker: Some(worker),
        }
    }
    fn verify(&self) {
        let seen = self.seen.lock().unwrap();
        assert!(
            seen.iter().any(|body| body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["type"] == "function_call_output")
                .count()
                == 2),
            "expected two completed local tool turns, got {} upstream requests",
            seen.len()
        );
    }
}
impl Drop for FakeResponses {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take()
            && let Err(error) = worker.join()
            && !std::thread::panicking()
        {
            std::panic::resume_unwind(error);
        }
    }
}

#[derive(Default)]
struct CapturedRecords(Mutex<Vec<token_station_metrics::RequestRecord>>);
impl token_station_metrics::Recorder for CapturedRecords {
    fn record(&self, record: &token_station_metrics::RequestRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

struct Harness {
    url: String,
    scratch: Scratch,
    fake: FakeResponses,
    paid_listener: TcpListener,
    receipts: Arc<CapturedRecords>,
    runtime: Option<tokio::runtime::Runtime>,
    server: tokio::task::JoinHandle<()>,
}
impl Harness {
    fn new() -> Self {
        let scratch = Scratch::new();
        let files = [scratch.0.join("first.txt"), scratch.0.join("second.txt")];
        for (file, text) in files.iter().zip(MARKERS) {
            std::fs::write(file, text).unwrap();
        }
        let fake = FakeResponses::start(files);
        let plugin_dir = scratch.0.join("plugins");
        stage_plugins(&plugin_dir);
        let data = scratch.0.join("station");
        std::fs::create_dir_all(&data).unwrap();
        token_station_cli::secrets::store_set(
            &data,
            "plan",
            "chatgpt_access_token",
            MOCK_CREDENTIAL,
        )
        .unwrap();
        let paid_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        paid_listener.set_nonblocking(true).unwrap();
        let paid_url = format!("http://{}/v1", paid_listener.local_addr().unwrap());
        let receipts = Arc::new(CapturedRecords::default());
        let config: ClientConfig = serde_json::from_value(json!({
            "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":data,"metrics":false},
            "plugins":{"dir":plugin_dir,"agents":["agent-openai","agent-openai-responses","agent-anthropic","agent-gemini"],"providers":{"chatgpt-plan":"provider-chatgpt-plan-v2","openai-compatible":"provider-openai-compatible-v2"}},
            "upstreams":{"plan":{"provider":"chatgpt-plan","base_url":fake.url,"auth":{"slot":"chatgpt_access_token","store":true},"models":[{"model":"gpt-5.5","tool":true,"tool_state":"verified","context_window":200_000}]},"paid":{"provider":"openai-compatible","base_url":paid_url,"models":[{"model":"gpt-5.5","tool":true,"tool_state":"verified","context_window":200_000}]}},
            "pricing":{"version":1,"models":{"plan/gpt-5.5":{"input_per_mtok":5_000_000,"output_per_mtok":30_000_000,"cache_read_per_mtok":0,"cache_write_per_mtok":0}}},
            "router":{"version":1,"pools":{"main":[{"upstream":"plan","model":"gpt-5.5"},{"upstream":"paid","model":"gpt-5.5"}]},"default_pool":"main"}
        })).unwrap();
        config.validate().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let gateway = Arc::new(
            Gateway::new_with_provider_runtime(&config, receipts.clone(), runtime.handle().clone())
                .unwrap()
                .with_body_log(Arc::new(
                    token_station_cli::bodylog::BodyLog::open(&data).unwrap(),
                )),
        );
        let state = server::AppState::new(
            gateway,
            Some(Arc::from(VIRTUAL_KEY)),
            Arc::new(token_station_cli::admin::AdminContext::from_config(&config).unwrap()),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = runtime.spawn(async move {
            server::serve(state, tokio::net::TcpListener::from_std(listener).unwrap())
                .await
                .unwrap();
        });
        Self {
            url,
            scratch,
            fake,
            paid_listener,
            receipts,
            runtime: Some(runtime),
            server,
        }
    }
    fn request(&self, path: &str, body: &Value) -> Value {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_mins(1)))
            .build()
            .into();
        let mut response = agent
            .post(format!("{}{path}", self.url))
            .header("authorization", &format!("Bearer {VIRTUAL_KEY}"))
            .header("content-type", "application/json")
            .header("anthropic-version", "2023-06-01")
            .send(body.to_string())
            .unwrap();
        serde_json::from_str(&response.body_mut().read_to_string().unwrap()).unwrap()
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

fn classifier_request(stops: &Value, stream: bool) -> Value {
    json!({"model":"gpt-5.5","system":"Synthetic classification. Preserve the supplied answer.","messages":[{"role":"user","content":"Classify this synthetic harmless request."}],"max_tokens":256,"stream":stream,"stop_sequences":stops})
}

#[test]
fn buffered_classifier_stops_preserve_allow_block_unicode_and_usage() {
    let harness = Harness::new();
    for (upstream, stops, expected, matching) in [
        (
            "<severity>allow</severity>ignored",
            json!(["</block>", "</severity>"]),
            "<severity>allow",
            Some("</severity>"),
        ),
        (
            "<block>拒绝执行</block>ignored</severity>",
            json!(["</severity>", "</block>"]),
            "<block>拒绝执行",
            Some("</block>"),
        ),
        (
            "No stop marker is present.",
            json!(["</block>"]),
            "No stop marker is present.",
            None,
        ),
    ] {
        *harness.fake.text.lock().unwrap() = upstream.to_owned();
        let response = harness.request("/v1/messages", &classifier_request(&stops, false));
        let text: String = response["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|block| block["text"].as_str())
            .collect();
        assert_eq!(text, expected);
        assert_eq!(response["stop_sequence"], json!(matching));
        assert_eq!(
            response["stop_reason"],
            if matching.is_some() {
                "stop_sequence"
            } else {
                "end_turn"
            }
        );
        assert_eq!(response["usage"]["input_tokens"], 10);
        assert_eq!(response["usage"]["output_tokens"], 8);
    }
    for body in harness.fake.seen.lock().unwrap().iter() {
        assert!(body.get("stop").is_none() && body.get("stop_sequences").is_none());
    }
}

#[test]
fn classifier_stop_cannot_hide_failed_terminal_or_enable_streaming_stops() {
    let harness = Harness::new();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_mins(1)))
        .build()
        .into();
    let send = |body: Value| {
        agent
            .post(format!("{}/v1/messages", harness.url))
            .header("authorization", &format!("Bearer {VIRTUAL_KEY}"))
            .header("content-type", "application/json")
            .send(body.to_string())
            .unwrap()
    };
    let response = send(classifier_request(&json!(["</block>"]), true));
    assert!(response.status().is_client_error());
    assert!(harness.fake.seen.lock().unwrap().is_empty());
    *harness.fake.text.lock().unwrap() = "allow</block>".to_owned();
    harness
        .fake
        .failed_after_text
        .store(true, Ordering::Relaxed);
    let mut response = send(classifier_request(&json!(["</block>"]), false));
    assert!(response.status().is_server_error());
    let body: Value = serde_json::from_str(&response.body_mut().read_to_string().unwrap()).unwrap();
    assert_ne!(body["stop_reason"], "stop_sequence");
    assert_eq!(harness.fake.seen.lock().unwrap().len(), 1);
}

#[test]
fn subscription_failures_never_contact_a_configured_paid_fallback() {
    for status in [429, 503] {
        let harness = Harness::new();
        harness.fake.status.store(status, Ordering::Relaxed);
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_mins(1)))
            .build()
            .into();
        let response = agent
            .post(format!("{}/v1/chat/completions", harness.url))
            .header("authorization", &format!("Bearer {VIRTUAL_KEY}"))
            .header("content-type", "application/json")
            .send(
                json!({"model":"gpt-5.5","messages":[{"role":"user","content":"hello"}]})
                    .to_string(),
            )
            .unwrap();
        assert!(response.status().is_client_error() || response.status().is_server_error());
        assert_eq!(harness.fake.seen.lock().unwrap().len(), 1);
        assert_eq!(
            harness.paid_listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn subscription_receipt_does_not_estimate_api_cost_even_with_a_catalog_price() {
    let harness = Harness::new();
    let response = harness.request(
        "/v1/chat/completions",
        &json!({"model":"gpt-5.5","messages":[{"role":"user","content":"hello"}]}),
    );
    assert_eq!(response["choices"][0]["message"]["content"], FINAL_TEXT);
    let deadline = Instant::now() + Duration::from_secs(2);
    while harness.receipts.0.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let receipts = harness.receipts.0.lock().unwrap();
    let receipt = receipts.last().expect("one receipt after completion");
    assert!(receipt.subscription_billing);
    assert_eq!(receipt.cost_micros, None);
    assert_eq!(receipt.cost_kind, token_station_metrics::CostKind::Unknown);
    for entry in std::fs::read_dir(harness.scratch.0.join("station/request-bodies")).unwrap() {
        let logged = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!logged.contains(MOCK_CREDENTIAL));
        assert!(!logged.contains(VIRTUAL_KEY));
    }
}

#[test]
fn claude_messages_complete_two_local_tool_turns() {
    let harness = Harness::new();
    let mut messages = vec![json!({"role":"user","content":"Read the two fixture files."})];
    for turn in 0..3 {
        let response = harness.request("/v1/messages", &json!({"model":"gpt-5.5","max_tokens":1024,"stream":false,"messages":messages,"tools":[{"name":"Read","description":"Read a local fixture","input_schema":{"type":"object","properties":{"file_path":{"type":"string"}},"required":["file_path"]}}]}));
        if turn == 2 {
            assert!(
                response["content"].to_string().contains(FINAL_TEXT),
                "{response}"
            );
            break;
        }
        assert_eq!(response["stop_reason"], "tool_use", "{response}");
        let tool = response["content"]
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "tool_use")
            .unwrap();
        let result = std::fs::read_to_string(tool["input"]["file_path"].as_str().unwrap()).unwrap();
        messages.push(json!({"role":"assistant","content":response["content"]}));
        messages.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":tool["id"],"content":result}]}));
    }
    harness.fake.verify();
}

#[test]
fn opencode_chat_complete_two_local_tool_turns() {
    let harness = Harness::new();
    let mut messages = vec![json!({"role":"user","content":"Read the two fixture files."})];
    for turn in 0..3 {
        let response = harness.request("/v1/chat/completions", &json!({"model":"gpt-5.5","stream":false,"messages":messages,"tools":[{"type":"function","function":{"name":"read","description":"Read a local fixture","parameters":{"type":"object","properties":{"filePath":{"type":"string"}},"required":["filePath"]}}}]}));
        let choice = &response["choices"][0];
        if turn == 2 {
            assert_eq!(choice["message"]["content"], FINAL_TEXT, "{response}");
            break;
        }
        assert_eq!(choice["finish_reason"], "tool_calls", "{response}");
        let call = &choice["message"]["tool_calls"][0];
        let args: Value =
            serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
        let result = std::fs::read_to_string(args["filePath"].as_str().unwrap()).unwrap();
        messages.push(choice["message"].clone());
        messages.push(json!({"role":"tool","tool_call_id":call["id"],"content":result}));
    }
    harness.fake.verify();
}

fn run_cli(harness: &Harness, name: &str, args: &[&str], extra: &[(&str, String)]) {
    let output = harness.scratch.0.join("cli-output");
    let errors = harness.scratch.0.join("cli-errors");
    let mut command = Command::new(name);
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("LANG", "en_US.UTF-8")
        .current_dir(&harness.scratch.0)
        .args(args)
        .stdin(Stdio::null())
        .stdout(std::fs::File::create(&output).unwrap())
        .stderr(std::fs::File::create(&errors).unwrap());
    for (key, value) in extra {
        command.env(key, value);
    }
    if name == "claude" {
        command
            .arg("--debug-file")
            .arg(harness.scratch.0.join("claude-debug.txt"));
    }
    let mut child = command.spawn().unwrap_or_else(|error| {
        panic!("installed {name} is required for this explicit test: {error}")
    });
    let deadline = Instant::now() + Duration::from_mins(2);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{name} timed out: {}",
                std::fs::read_to_string(&errors).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let text = std::fs::read_to_string(&output).unwrap();
    assert!(
        status.success(),
        "{name}: {text}\n{}",
        std::fs::read_to_string(&errors).unwrap()
    );
    assert!(
        text.contains(FINAL_TEXT),
        "{name} did not show final response: {text}"
    );
    harness.fake.verify();
}

#[test]
#[ignore = "Requires installed Claude Code. Uses only an isolated local mock upstream."]
fn installed_claude_code_executes_two_tools() {
    let harness = Harness::new();
    run_cli(
        &harness,
        "claude",
        &[
            "--bare",
            "--print",
            "--system-prompt",
            "Read the requested fixture files with the Read tool. Report the results.",
            "--model",
            "sonnet",
            "--tools",
            "Read",
            "--allowedTools",
            "Read",
            "--setting-sources",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
            "--output-format",
            "json",
            "Read first.txt then second.txt and report both results.",
        ],
        &[
            ("ANTHROPIC_BASE_URL", harness.url.clone()),
            ("ANTHROPIC_API_KEY", VIRTUAL_KEY.to_owned()),
            ("ANTHROPIC_AUTH_TOKEN", VIRTUAL_KEY.to_owned()),
            (
                "CLAUDE_CONFIG_DIR",
                harness.scratch.0.join("claude").display().to_string(),
            ),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1".to_owned()),
        ],
    );
}

#[test]
#[ignore = "Requires installed OpenCode. Uses only an isolated local mock upstream."]
fn installed_opencode_executes_two_tools() {
    let harness = Harness::new();
    let config = json!({"model":"tokenstation/gpt-5.5","small_model":"tokenstation/gpt-5.5","enabled_providers":["tokenstation"],"permission":{"*":"deny","read":"allow","external_directory":{format!("{}/*",harness.scratch.0.display()):"allow"}},"provider":{"tokenstation":{"npm":"@ai-sdk/openai-compatible","name":"fixture","options":{"baseURL":format!("{}/v1",harness.url),"apiKey":VIRTUAL_KEY},"models":{"gpt-5.5":{"name":"fixture","limit":{"context":200_000,"output":8192}}}}}});
    let mut env = vec![
        ("OPENCODE_CONFIG_CONTENT", config.to_string()),
        ("OPENCODE_AUTH_CONTENT", "{}".to_owned()),
    ];
    for key in [
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_TEST_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        let dir = harness.scratch.0.join(key);
        std::fs::create_dir_all(&dir).unwrap();
        env.push((key, dir.display().to_string()));
    }
    for key in [
        "OPENCODE_DISABLE_PROJECT_CONFIG",
        "OPENCODE_DISABLE_DEFAULT_PLUGINS",
        "OPENCODE_DISABLE_EXTERNAL_SKILLS",
        "OPENCODE_DISABLE_CLAUDE_CODE",
        "OPENCODE_DISABLE_LSP_DOWNLOAD",
        "OPENCODE_DISABLE_MODELS_FETCH",
        "OPENCODE_DISABLE_AUTOUPDATE",
        "OPENCODE_DISABLE_SHARE",
    ] {
        env.push((key, "1".to_owned()));
    }
    run_cli(
        &harness,
        "opencode",
        &[
            "run",
            "--pure",
            "--format",
            "json",
            "--model",
            "tokenstation/gpt-5.5",
            "Read first.txt then second.txt and report both results.",
        ],
        &env,
    );
}

fn live_tool_shape(tool: &Value) -> Value {
    json!({"type":tool["type"],"name":tool["name"],"keys":tool.as_object().map(|object| object.keys().collect::<Vec<_>>()),"parameter_keys":tool["parameters"].as_object().map(|object| object.keys().collect::<Vec<_>>()),"property_keys":tool["parameters"]["properties"].as_object().map(|object| object.keys().collect::<Vec<_>>()),"nested_tools":tool["tools"].as_array().map(|tools| tools.iter().map(live_tool_shape).collect::<Vec<_>>())})
}

fn live_failure_metadata(harness: &LiveHarness) {
    let Ok(entries) = std::fs::read_dir(harness.scratch.0.join("request-bodies")) else {
        return;
    };
    for entry in entries {
        let exchange: token_station_cli::bodylog::PlaintextExchange =
            serde_json::from_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
        if let Some(request) = &exchange.http_trace.agent_request {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            let fields: Vec<_> = body
                .as_object()
                .into_iter()
                .flatten()
                .map(|(key, _)| key)
                .collect();
            let tool_types: Vec<_> = body["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|tool| tool["type"].as_str().unwrap_or("function"))
                .collect();
            eprintln!("live failed request: fields={fields:?} tool_types={tool_types:?}");
            let input_items: Vec<_> = body["input"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|item| {
                    (
                        item["type"].as_str(),
                        item["role"].as_str(),
                        item.as_object()
                            .map(|object| object.keys().collect::<Vec<_>>()),
                    )
                })
                .collect();
            eprintln!("live failed input structure: {input_items:?}");
            for (index, item) in body["input"].as_array().into_iter().flatten().enumerate() {
                if item["type"] == "additional_tools" {
                    let tools: Vec<_> = item["tools"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(live_tool_shape)
                        .collect();
                    eprintln!(
                        "live embedded tool structure: input_index={index} tools={}",
                        json!(tools)
                    );
                }
            }
        }
        if let Some(response) = exchange
            .http_trace
            .agent_response
            .filter(|response| response.status >= 400)
        {
            let body: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
            let error = body["error"]["message"]
                .as_str()
                .or_else(|| body["detail"].as_str())
                .unwrap_or("no_json_error");
            if !["Bearer", "sk-", "eyJ", "access_token", "refresh_token"]
                .iter()
                .any(|marker| error.contains(marker))
            {
                eprintln!(
                    "live failed response: status={} message={}",
                    response.status,
                    error
                        .replace(&harness.virtual_key, "[redacted]")
                        .chars()
                        .take(600)
                        .collect::<String>()
                );
            }
        }
    }
}

/// Explicit live acceptance. The account reference reaches only the Host.
/// The CLI receives a new local virtual key, never an OAuth credential.
struct LiveHarness {
    scratch: Scratch,
    url: String,
    virtual_key: String,
    model: String,
    marker: String,
    receipts: Arc<CapturedRecords>,
    runtime: Option<tokio::runtime::Runtime>,
    server: tokio::task::JoinHandle<()>,
}
impl LiveHarness {
    fn new() -> Self {
        Self::with_request_trace(false)
    }
    fn with_request_trace(trace: bool) -> Self {
        use base64::Engine as _;
        let required = |name| {
            std::env::var(name)
                .unwrap_or_else(|_| panic!("Explicit live acceptance requires {name}"))
        };
        let data = PathBuf::from(required("TOKEN_STATION_CHATGPT_LIVE_DATA_DIR"));
        let account = if live_subscription::use_codex() {
            String::new()
        } else {
            required("TOKEN_STATION_CHATGPT_LIVE_ACCOUNT_ID")
        };
        let model = required("TOKEN_STATION_CHATGPT_LIVE_MODEL");
        assert!(
            data.is_dir(),
            "The explicitly selected live data directory must exist"
        );
        let scratch = Scratch::new();
        let mut random = [0_u8; 32];
        getrandom::fill(&mut random).unwrap();
        let virtual_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random);
        getrandom::fill(&mut random).unwrap();
        let marker = format!(
            "TOKEN_STATION_LIVE_{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random)
        );
        std::fs::write(scratch.0.join("marker.txt"), &marker).unwrap();
        let plugin_dir = scratch.0.join("plugins");
        stage_plugins(&plugin_dir);
        let config: ClientConfig = serde_json::from_value(live_subscription::configure(json!({
            "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":data,"metrics":false},
            "plugins":{"dir":plugin_dir,"agents":["agent-openai","agent-openai-responses","agent-anthropic","agent-gemini"],"providers":{"chatgpt-plan":"provider-chatgpt-plan-v2"}},
            "upstreams":{"live_plan":{"provider":"chatgpt-plan","base_url":"https://api.openai.com/v1","auth":{"slot":"chatgpt_access_token","chatgpt_account":account},"models":[{"model":model,"tool":true,"tool_state":"declared","context_window":200_000}]}},
            "router":{"version":1,"pools":{"main":[{"upstream":"live_plan","model":model}]},"default_pool":"main"}
        }), "live_plan", Path::new(&data))).unwrap();
        config.validate().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let receipts = Arc::new(CapturedRecords::default());
        let mut gateway =
            Gateway::new_with_provider_runtime(&config, receipts.clone(), runtime.handle().clone())
                .unwrap();
        if trace {
            gateway = gateway.with_body_log(Arc::new(
                token_station_cli::bodylog::BodyLog::open(&scratch.0).unwrap(),
            ));
        }
        let gateway = Arc::new(gateway);
        let state = server::AppState::new(
            gateway,
            Some(Arc::from(virtual_key.as_str())),
            Arc::new(token_station_cli::admin::AdminContext::from_config(&config).unwrap()),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = runtime.spawn(async move {
            server::serve(state, tokio::net::TcpListener::from_std(listener).unwrap())
                .await
                .unwrap();
        });
        Self {
            scratch,
            url,
            virtual_key,
            model,
            marker,
            receipts,
            runtime: Some(runtime),
            server,
        }
    }
    fn run(&self, name: &str, args: &[&str], env: &[(&str, String)]) -> Vec<Value> {
        let stdout = self.scratch.0.join("live-output.jsonl");
        let stderr = self.scratch.0.join("live-errors.txt");
        let mut command = Command::new(name);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("LANG", "en_US.UTF-8")
            .current_dir(&self.scratch.0)
            .args(args)
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&stdout).unwrap())
            .stderr(std::fs::File::create(&stderr).unwrap());
        for (key, value) in env {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("The installed CLI must start");
        let deadline = Instant::now() + Duration::from_mins(3);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("The live CLI exceeded the bounded acceptance deadline");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if self
            .receipts
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|receipt| receipt.status >= 400)
        {
            live_failure_metadata(self);
        }
        // Failure diagnostics are closed metadata. Do not print upstream bodies
        // or a child environment during a live account test.
        let receipts = self.receipts.0.lock().unwrap();
        for receipt in receipts.iter() {
            eprintln!(
                "live receipt: status={} error={:?} subscription={} cost_kind={:?}",
                receipt.status, receipt.error_code, receipt.subscription_billing, receipt.cost_kind
            );
            assert!(receipt.subscription_billing);
            assert_eq!(receipt.cost_micros, None);
        }
        assert!(
            status.success(),
            "The live CLI failed. Inspect the receipt status and error code"
        );
        assert!(
            !receipts.is_empty(),
            "The live CLI must reach the Host gateway"
        );
        let text = std::fs::read_to_string(stdout).unwrap();
        assert!(
            !text.contains(&self.virtual_key),
            "The CLI must not expose its local key"
        );
        text.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .collect()
    }
}
impl Drop for LiveHarness {
    fn drop(&mut self) {
        self.server.abort();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_timeout(Duration::from_secs(2));
        }
    }
}

#[test]
#[ignore = "Consumes the explicitly selected real ChatGPT plan. Requires three TOKEN_STATION_CHATGPT_LIVE_* variables."]
fn live_claude_code_reads_a_random_marker() {
    let harness = LiveHarness::new();
    let events = harness.run("claude",&[
        "--bare","--print","--system-prompt","Use only Read on marker.txt in the current directory. Read it exactly once. Return LIVE_TOOL_OK and its exact content. Do not inspect any other path.",
        "--model","sonnet","--tools","Read","--allowedTools","Read(./marker.txt)","--setting-sources","","--strict-mcp-config","--no-session-persistence","--output-format","stream-json","--verbose",
        "Read marker.txt exactly once. Return LIVE_TOOL_OK followed by the exact file content.",
    ],&[
        ("ANTHROPIC_BASE_URL",harness.url.clone()),("ANTHROPIC_API_KEY",harness.virtual_key.clone()),("ANTHROPIC_AUTH_TOKEN",harness.virtual_key.clone()),
        ("CLAUDE_CONFIG_DIR",harness.scratch.0.join("claude").display().to_string()),("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC","1".to_owned()),
    ]);
    let calls: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "assistant")
        .filter_map(|event| event["message"]["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_use")
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "The live Claude run must perform exactly one tool call"
    );
    assert_eq!(calls[0]["name"], "Read");
    let requested = calls[0]["input"]["file_path"].as_str().unwrap();
    let requested = if Path::new(requested).is_absolute() {
        PathBuf::from(requested)
    } else {
        harness.scratch.0.join(requested)
    };
    assert_eq!(
        std::fs::canonicalize(requested).unwrap(),
        harness.scratch.0.join("marker.txt")
    );
    let result = events
        .iter()
        .filter(|event| event["type"] == "user")
        .filter_map(|event| event["message"]["content"].as_array())
        .flatten()
        .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == calls[0]["id"])
        .expect("A real tool result must match the emitted call ID");
    assert_ne!(result["is_error"], true);
    assert!(
        result["content"].to_string().contains(&harness.marker),
        "The tool result must contain the random file content"
    );
    let final_result = events
        .iter()
        .find(|event| event["type"] == "result")
        .expect("Claude must emit its final structured result");
    assert_ne!(final_result["is_error"], true);
    assert!(
        final_result["result"]
            .as_str()
            .unwrap_or_default()
            .contains("LIVE_TOOL_OK")
    );
    assert!(
        final_result["result"]
            .as_str()
            .unwrap_or_default()
            .contains(&harness.marker)
    );
}

#[test]
#[ignore = "Consumes the explicitly selected real ChatGPT plan. Requires three TOKEN_STATION_CHATGPT_LIVE_* variables."]
fn live_opencode_reads_a_random_marker() {
    let harness = LiveHarness::new();
    let model_id = format!("tokenstation/{}", harness.model);
    let marker_path = harness.scratch.0.join("marker.txt").display().to_string();
    let config = json!({"model":model_id,"small_model":model_id,"enabled_providers":["tokenstation"],"permission":{"*":"deny","read":{"*":"deny","marker.txt":"allow","./marker.txt":"allow",marker_path.trim_start_matches('/') :"allow",marker_path:"allow"},"external_directory":{format!("{}/*",harness.scratch.0.display()):"allow"}},"agent":{"title":{"disable":true}},"provider":{"tokenstation":{"npm":"@ai-sdk/openai-compatible","name":"live plan acceptance","options":{"baseURL":format!("{}/v1",harness.url),"apiKey":harness.virtual_key},"models":{harness.model.clone():{"name":harness.model,"limit":{"context":200_000,"output":2048}}}}}});
    let mut env = vec![
        ("OPENCODE_CONFIG_CONTENT", config.to_string()),
        ("OPENCODE_AUTH_CONTENT", "{}".to_owned()),
    ];
    for key in [
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_TEST_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        let dir = harness.scratch.0.join(key);
        std::fs::create_dir_all(&dir).unwrap();
        env.push((key, dir.display().to_string()));
    }
    for key in [
        "OPENCODE_DISABLE_PROJECT_CONFIG",
        "OPENCODE_DISABLE_DEFAULT_PLUGINS",
        "OPENCODE_DISABLE_EXTERNAL_SKILLS",
        "OPENCODE_DISABLE_CLAUDE_CODE",
        "OPENCODE_DISABLE_LSP_DOWNLOAD",
        "OPENCODE_DISABLE_MODELS_FETCH",
        "OPENCODE_DISABLE_AUTOUPDATE",
        "OPENCODE_DISABLE_SHARE",
    ] {
        env.push((key, "1".to_owned()));
    }
    let events=harness.run("opencode",&["run","--pure","--format","json","--model",&model_id,"Use read to read marker.txt in the current directory exactly once. Return LIVE_TOOL_OK followed by the exact file content. Do not inspect other paths."],&env);
    let calls: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "tool_use")
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "The live OpenCode run must perform exactly one tool call"
    );
    let call = &calls[0]["part"];
    assert_eq!(call["tool"], "read");
    if call["state"]["status"] != "completed" {
        let error = call["state"]["error"].as_str().unwrap_or_default();
        eprintln!(
            "live tool failure: permission_denied={} file_missing={} invalid_arguments={}",
            error.contains("rule") || error.contains("permission"),
            error.contains("not found") || error.contains("ENOENT"),
            error.contains("argument") || error.contains("validation")
        );
    }
    assert_eq!(call["state"]["status"], "completed");
    assert!(
        call["state"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains(&harness.marker),
        "The completed tool event must contain the random file content"
    );
    let text: String = events
        .iter()
        .filter(|event| event["type"] == "text")
        .filter_map(|event| event["part"]["text"].as_str())
        .collect();
    assert!(
        text.contains("LIVE_TOOL_OK") && text.contains(&harness.marker),
        "The final assistant text must contain the marker read by the tool"
    );
}

#[test]
#[ignore = "Consumes the selected real ChatGPT plan and exercises Claude auto-mode classification without allow rules."]
fn live_claude_auto_mode_classifies_a_temporary_file_write() {
    let harness = LiveHarness::with_request_trace(true);
    let debug = harness.scratch.0.join("auto-mode-debug.txt");
    let debug_path = debug.to_str().unwrap();
    let events = harness.run("claude", &[
        "--bare", "--print", "--permission-mode", "auto", "--permission-prompts", "none",
        "--system-prompt", "Only operate on marker.txt and copied-marker.txt in the current working directory. Use Bash once to run the requested Python file-copy command. Return LIVE_TOOL_OK and the copied file content. Do not inspect other paths.",
        "--model", "sonnet", "--tools", "Bash", "--setting-sources", "", "--strict-mcp-config",
        "--no-session-persistence", "--output-format", "stream-json", "--verbose", "--debug-file", debug_path,
        "Use Bash once to run: python3 -c \"from pathlib import Path; text = Path('marker.txt').read_text(); Path('copied-marker.txt').write_text(text); print(text)\". This copies only the temporary fixture file in the current directory. Return LIVE_TOOL_OK followed by the exact copied content.",
    ], &[
        ("ANTHROPIC_BASE_URL",harness.url.clone()),("ANTHROPIC_API_KEY",harness.virtual_key.clone()),("ANTHROPIC_AUTH_TOKEN",harness.virtual_key.clone()),
        ("CLAUDE_CONFIG_DIR",harness.scratch.0.join("claude").display().to_string()),("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC","1".to_owned()),
    ]);
    let debug_text =
        std::fs::read_to_string(debug).expect("The isolated auto-mode debug log exists");
    assert!(
        !debug_text.contains("classifier_api_error"),
        "Auto-mode classification must not fail its model request"
    );
    let receipts = harness.receipts.0.lock().unwrap();
    let mut classifier_requests = 0;
    for entry in std::fs::read_dir(harness.scratch.0.join("request-bodies")).unwrap() {
        let exchange: token_station_cli::bodylog::PlaintextExchange =
            serde_json::from_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
        let Some(request) = exchange.http_trace.agent_request else {
            continue;
        };
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let classifier_stop = body["stop_sequences"].as_array().is_some_and(|stops| {
            stops
                .iter()
                .any(|stop| stop == "</block>" || stop == "</severity>")
        });
        if classifier_stop
            && body["stream"] != true
            && body["tools"].as_array().is_none_or(Vec::is_empty)
        {
            let receipt = receipts
                .iter()
                .find(|receipt| receipt.request_id == exchange.request_id)
                .expect("The classifier has a matching receipt");
            assert_eq!(
                receipt.status, 200,
                "The real classifier request must finish successfully"
            );
            classifier_requests += 1;
        }
    }
    eprintln!("successful live classifier requests: {classifier_requests}");
    assert!(
        classifier_requests > 0,
        "Auto-mode acceptance requires a real buffered stop-sequence classifier request, not a tool exemption"
    );
    drop(receipts);
    let calls: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "assistant")
        .filter_map(|event| event["message"]["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_use")
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "Exactly one harmless fixture-copy tool call is required"
    );
    assert_eq!(calls[0]["name"], "Bash");
    let result = events
        .iter()
        .filter(|event| event["type"] == "user")
        .filter_map(|event| event["message"]["content"].as_array())
        .flatten()
        .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == calls[0]["id"])
        .expect("A real Bash result must match the emitted tool ID");
    assert_ne!(
        result["is_error"], true,
        "The auto classifier must permit this harmless fixture operation"
    );
    assert!(result["content"].to_string().contains(&harness.marker));
    assert_eq!(
        std::fs::read_to_string(harness.scratch.0.join("copied-marker.txt")).unwrap(),
        harness.marker
    );
    let final_result = events
        .iter()
        .find(|event| event["type"] == "result")
        .expect("Claude emits a final structured result");
    assert_ne!(final_result["is_error"], true);
    assert!(
        final_result["result"]
            .as_str()
            .unwrap_or_default()
            .contains("LIVE_TOOL_OK")
    );
    assert!(
        final_result["result"]
            .as_str()
            .unwrap_or_default()
            .contains(&harness.marker)
    );
}

fn live_web_upstream_error(trace: &token_station_cli::bodylog::HttpTraceSnapshot) {
    for exchange in &trace.upstream_exchanges {
        let Some(response) = exchange
            .response
            .as_ref()
            .filter(|response| response.status >= 400)
        else {
            continue;
        };
        let body: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
        let request: Value = serde_json::from_str(&exchange.request.body).unwrap_or(Value::Null);
        let fields: Vec<_> = request
            .as_object()
            .into_iter()
            .flatten()
            .map(|(key, _)| key)
            .collect();
        let error = &body["error"];
        let raw = error["message"].as_str().unwrap_or("no_json_error_message");
        let safe = if ["Bearer", "sk-", "eyJ", "access_token", "refresh_token"]
            .iter()
            .any(|marker| raw.contains(marker))
            || raw.split_whitespace().any(|word| word.len() > 80)
        {
            "redacted_error_message".to_owned()
        } else {
            raw.chars().take(400).collect()
        };
        eprintln!(
            "web upstream error: status={} code={} type={} param={} message={safe} fields={fields:?} reasoning={} tool_choice={}",
            response.status,
            error["code"],
            error["type"],
            error["param"],
            request["reasoning"],
            request["tool_choice"]
        );
    }
}

fn live_web_trace_summary(harness: &LiveHarness) -> (usize, usize) {
    let receipts = harness.receipts.0.lock().unwrap();
    let mut classifiers = 0;
    let mut auxiliary = 0;
    for entry in std::fs::read_dir(harness.scratch.0.join("request-bodies")).unwrap() {
        let exchange: token_station_cli::bodylog::PlaintextExchange =
            serde_json::from_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
        live_web_upstream_error(&exchange.http_trace);
        let Some(request) = exchange.http_trace.agent_request else {
            continue;
        };
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let stops = body["stop_sequences"].as_array();
        let classifier = stops.is_some_and(|stops| {
            stops
                .iter()
                .any(|stop| stop == "</block>" || stop == "</severity>")
        });
        let receipt = receipts
            .iter()
            .find(|receipt| receipt.request_id == exchange.request_id)
            .unwrap();
        if receipt.status >= 400 {
            let fields: Vec<_> = body.as_object().unwrap().keys().collect();
            let tool_fields: Vec<_> = body["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .map(|tool| tool.keys().collect::<Vec<_>>())
                .collect();
            let conversion: Vec<_> = receipt
                .conversion_reports
                .iter()
                .map(|report| (report.stage, report.outcome, report.error_code))
                .collect();
            eprintln!(
                "web failure: fields={fields:?} tool_fields={tool_fields:?} attempts={} conversions={conversion:?}",
                receipt.attempts
            );
        }
        if classifier {
            classifiers += 1;
        }
        if !classifier && body["tools"].as_array().is_none_or(Vec::is_empty) {
            auxiliary += 1;
        }
        let types: Vec<_> = body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tool| tool["type"].as_str())
            .collect();
        // Print structure and status only. The temporary trace holds no real
        // user conversation, and neither its prompt nor credentials are printed.
        eprintln!(
            "web request: stream={} classifier={} stop_count={} tools={} tool_types={types:?} status={} error={:?}",
            body["stream"] == true,
            classifier,
            stops.map_or(0, Vec::len),
            body["tools"].as_array().map_or(0, Vec::len),
            receipt.status,
            receipt.error_code
        );
    }
    let searches: Vec<_> = receipts
        .iter()
        .flat_map(|receipt| receipt.browser_searches.iter())
        .map(|search| (&search.backend, search.outcome))
        .collect();
    eprintln!(
        "web acceptance: classifier_requests={classifiers} auxiliary_requests={auxiliary} searches={searches:?}"
    );
    (classifiers, auxiliary)
}

fn run_live_claude_web(harness: &LiveHarness, tool: &str, prompt: &str) -> Vec<Value> {
    harness.run("claude", &[
        "--safe-mode","--print","--permission-mode","auto","--permission-prompts","none",
        "--system-prompt","Use the requested web tool exactly once. Ground the final answer in its actual result. Cite the retrieved public source URL. Do not inspect local files or perform unrelated searches.",
        "--model","sonnet","--tools",tool,"--setting-sources","","--strict-mcp-config","--no-session-persistence","--output-format","stream-json","--verbose",prompt,
    ], &[
        ("ANTHROPIC_BASE_URL",harness.url.clone()),("ANTHROPIC_API_KEY",harness.virtual_key.clone()),("ANTHROPIC_AUTH_TOKEN",harness.virtual_key.clone()),
        ("HOME",harness.scratch.0.display().to_string()),("CLAUDE_CONFIG_DIR",harness.scratch.0.join("claude").display().to_string()),("DISABLE_AUTOUPDATER","1".to_owned()),("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC","1".to_owned()),
    ])
}

fn live_web_tool_result<'a>(events: &'a [Value], name: &str) -> &'a Value {
    let calls: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "assistant")
        .filter_map(|event| event["message"]["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_use")
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "The live web task must invoke exactly one requested tool"
    );
    assert_eq!(calls[0]["name"], name);
    let result = events
        .iter()
        .filter(|event| event["type"] == "user")
        .filter_map(|event| event["message"]["content"].as_array())
        .flatten()
        .find(|block| block["type"] == "tool_result" && block["tool_use_id"] == calls[0]["id"])
        .expect("A real web tool result must match the emitted call ID");
    if result["is_error"] == true {
        let error_text = result["content"].to_string();
        if !["Bearer", "sk-", "eyJ", "access_token", "refresh_token"]
            .iter()
            .any(|marker| error_text.contains(marker))
        {
            eprintln!(
                "web tool error detail: {}",
                error_text.chars().take(600).collect::<String>()
            );
        }
        let rendered = error_text.to_ascii_lowercase();
        eprintln!(
            "web tool error: http400={} http403={} classifier={} capability={} network={}",
            rendered.contains("400"),
            rendered.contains("403"),
            rendered.contains("classif"),
            rendered.contains("capability"),
            rendered.contains("fetch") || rendered.contains("network")
        );
    }
    result
}

fn live_web_final(events: &[Value]) -> &str {
    let result = events
        .iter()
        .find(|event| event["type"] == "result")
        .expect("Claude emits a final structured result");
    assert_ne!(result["is_error"], true);
    result["result"].as_str().expect("Final answer is text")
}

#[test]
#[ignore = "Consumes the selected real ChatGPT plan and fetches example.com through Claude auto mode without allow rules."]
fn live_claude_webfetch_completes_page_summary() {
    let harness = LiveHarness::with_request_trace(true);
    let events = run_live_claude_web(
        &harness,
        "WebFetch",
        "Use WebFetch exactly once on https://example.com with the prompt: identify the page title and explain in one sentence what this domain is for. Return the title, that sentence, and the source URL. Do not use prior knowledge instead of the fetched result.",
    );
    let (classifiers, auxiliary) = live_web_trace_summary(&harness);
    assert!(
        classifiers > 0,
        "Claude auto mode must perform a real safety-classifier request"
    );
    let result = live_web_tool_result(&events, "WebFetch");
    assert_ne!(result["is_error"], true, "The actual web tool must succeed");
    assert!(
        result["content"]
            .to_string()
            .to_ascii_lowercase()
            .contains("example"),
        "The actual fetched page result must concern the example domain"
    );
    assert!(
        auxiliary > 0,
        "WebFetch must exercise its page-summary model request"
    );
    let final_answer = live_web_final(&events).to_ascii_lowercase();
    assert!(final_answer.contains("example domain"));
    assert!(final_answer.contains("https://example.com"));
}

#[test]
#[ignore = "Consumes the selected real ChatGPT plan and a real search query through Claude auto mode without allow rules."]
fn live_claude_websearch_returns_retrieved_citations() {
    let harness = LiveHarness::with_request_trace(true);
    let events = run_live_claude_web(
        &harness,
        "WebSearch",
        "Use WebSearch exactly once for: IANA reserved example domains example.com example.org documentation. Explain what these domains are reserved for and cite the actual IANA source URL returned by the search. Do not answer from prior knowledge or fetch another page.",
    );
    let (classifiers, _) = live_web_trace_summary(&harness);
    assert!(
        classifiers > 0,
        "Claude auto mode must perform a real safety-classifier request"
    );
    let result = live_web_tool_result(&events, "WebSearch");
    assert_ne!(result["is_error"], true, "The actual web tool must succeed");
    let result_text = result["content"].to_string().to_ascii_lowercase();
    assert!(
        result_text.contains("iana.org") && result_text.contains("https://"),
        "The actual search tool result must contain a retrieved IANA source URL"
    );
    let receipts = harness.receipts.0.lock().unwrap();
    assert!(
        receipts
            .iter()
            .flat_map(|receipt| &receipt.browser_searches)
            .any(|search| search.outcome == token_station_metrics::BrowserSearchOutcome::Succeeded),
        "The Host must record actual successful retrieval"
    );
    drop(receipts);
    let final_answer = live_web_final(&events).to_ascii_lowercase();
    assert!(final_answer.contains("iana.org") && final_answer.contains("https://"));
    let grounded_citation = final_answer
        .split([' ', '\n', '(', ')', '[', ']', '"'])
        .map(|word| word.trim_end_matches(['.', ',', ';']))
        .find(|url| {
            (url.starts_with("https://www.iana.org/") || url.starts_with("https://iana.org/"))
                && result_text.contains(url)
        });
    assert!(
        grounded_citation.is_some(),
        "At least one final IANA citation must match an actual retrieved source URL"
    );
    assert!(
        final_answer.contains("documentation")
            || final_answer.contains("illustrat")
            || final_answer.contains("文档")
    );
}

#[test]
#[ignore = "Consumes the selected real ChatGPT plan and one public HTTPS missing-page fetch without allow rules."]
fn live_claude_webfetch_reports_a_missing_page() {
    let harness = LiveHarness::with_request_trace(true);
    let url = format!(
        "https://example.com/token-station-missing-page-{}",
        std::process::id()
    );
    let prompt = format!(
        "Use WebFetch exactly once on {url} with the prompt: identify the page title. If fetching fails, report that failure and the returned HTTP status. Do not retry, use another tool, or pretend that page content was retrieved."
    );
    let events = run_live_claude_web(&harness, "WebFetch", &prompt);
    let (classifiers, _) = live_web_trace_summary(&harness);
    assert!(
        classifiers > 0,
        "Claude auto mode must perform a real safety-classifier request"
    );
    let result = live_web_tool_result(&events, "WebFetch");
    let result_text = result["content"].to_string().to_ascii_lowercase();
    eprintln!(
        "missing-page result: error_flag={} http404={} failed={} denied={}",
        result["is_error"] == true,
        result_text.contains("404"),
        result_text.contains("fail"),
        result_text.contains("denied")
    );
    assert!(
        result["content"].to_string().contains("404"),
        "The actual tool result must confirm HTTP 404"
    );
    let final_answer = live_web_final(&events).to_ascii_lowercase();
    assert!(final_answer.contains("404"));
    assert!(
        final_answer.contains("fail")
            || final_answer.contains("not found")
            || final_answer.contains("失败")
    );
}

#[test]
#[ignore = "Consumes the selected real ChatGPT plan and one public HTTPS fetch with isolated OpenCode defaults."]
fn live_opencode_webfetch_returns_page_content() {
    let harness = LiveHarness::with_request_trace(true);
    let model_id = format!("tokenstation/{}", harness.model);
    let denied = [
        "bash",
        "edit",
        "write",
        "read",
        "glob",
        "grep",
        "list",
        "task",
        "question",
        "todoread",
        "todowrite",
        "websearch",
        "codesearch",
        "skill",
        "apply_patch",
        "lsp",
    ];
    let permission: serde_json::Map<String, Value> = denied
        .into_iter()
        .map(|tool| (tool.to_owned(), json!("deny")))
        .collect();
    let config = json!({"model":model_id,"small_model":model_id,"enabled_providers":["tokenstation"],"permission":permission,"agent":{"title":{"disable":true}},"provider":{"tokenstation":{"npm":"@ai-sdk/openai-compatible","name":"live plan acceptance","options":{"baseURL":format!("{}/v1",harness.url),"apiKey":harness.virtual_key},"models":{harness.model.clone():{"name":harness.model,"limit":{"context":200_000,"output":2048}}}}}});
    let mut env = vec![
        ("OPENCODE_CONFIG_CONTENT", config.to_string()),
        ("OPENCODE_AUTH_CONTENT", "{}".to_owned()),
    ];
    for key in [
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_TEST_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        let dir = harness.scratch.0.join(key);
        std::fs::create_dir_all(&dir).unwrap();
        env.push((key, dir.display().to_string()));
    }
    for key in [
        "OPENCODE_DISABLE_PROJECT_CONFIG",
        "OPENCODE_DISABLE_DEFAULT_PLUGINS",
        "OPENCODE_DISABLE_EXTERNAL_SKILLS",
        "OPENCODE_DISABLE_CLAUDE_CODE",
        "OPENCODE_DISABLE_LSP_DOWNLOAD",
        "OPENCODE_DISABLE_MODELS_FETCH",
        "OPENCODE_DISABLE_AUTOUPDATE",
        "OPENCODE_DISABLE_SHARE",
    ] {
        env.push((key, "1".to_owned()));
    }
    let events = harness.run("opencode", &["run", "--pure", "--format", "json", "--model", &model_id, "Use webfetch exactly once on https://example.com in text format. Report the actual page title and its purpose, and cite the source URL. Do not use any other tool or answer from prior knowledge."], &env);
    let calls: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "tool_use")
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "OpenCode must execute exactly one actual web tool"
    );
    let call = &calls[0]["part"];
    assert_eq!(call["tool"], "webfetch");
    assert_eq!(call["state"]["status"], "completed");
    assert!(
        call["state"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("Example Domain")
    );
    let final_text = events
        .iter()
        .filter(|event| event["type"] == "text")
        .filter_map(|event| event["part"]["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    assert!(final_text.contains("example domain") && final_text.contains("https://example.com"));
    assert!(final_text.contains("documentation") || final_text.contains("illustrat"));
}
#[test]
#[ignore = "Consumes the selected real ChatGPT plan and one isolated OpenCode Exa opt-in search without external credentials."]
fn live_opencode_websearch_exa_opt_in_returns_sources() {
    let harness = LiveHarness::with_request_trace(true);
    let model_id = format!("tokenstation/{}", harness.model);
    let denied = [
        "bash",
        "edit",
        "write",
        "read",
        "glob",
        "grep",
        "list",
        "task",
        "question",
        "todoread",
        "todowrite",
        "webfetch",
        "codesearch",
        "skill",
        "apply_patch",
        "lsp",
    ];
    let permission: serde_json::Map<String, Value> = denied
        .into_iter()
        .map(|tool| (tool.to_owned(), json!("deny")))
        .collect();
    let config = json!({"model":model_id,"small_model":model_id,"enabled_providers":["tokenstation"],"permission":permission,"agent":{"title":{"disable":true}},"provider":{"tokenstation":{"npm":"@ai-sdk/openai-compatible","name":"live plan acceptance","options":{"baseURL":format!("{}/v1",harness.url),"apiKey":harness.virtual_key},"models":{harness.model.clone():{"name":harness.model,"limit":{"context":200_000,"output":2048}}}}}});
    let mut env = vec![
        ("OPENCODE_CONFIG_CONTENT", config.to_string()),
        ("OPENCODE_AUTH_CONTENT", "{}".to_owned()),
        ("OPENCODE_ENABLE_EXA", "1".to_owned()),
    ];
    for key in [
        "OPENCODE_CONFIG_DIR",
        "OPENCODE_TEST_HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ] {
        let dir = harness.scratch.0.join(key);
        std::fs::create_dir_all(&dir).unwrap();
        env.push((key, dir.display().to_string()));
    }
    for key in [
        "OPENCODE_DISABLE_PROJECT_CONFIG",
        "OPENCODE_DISABLE_DEFAULT_PLUGINS",
        "OPENCODE_DISABLE_EXTERNAL_SKILLS",
        "OPENCODE_DISABLE_CLAUDE_CODE",
        "OPENCODE_DISABLE_LSP_DOWNLOAD",
        "OPENCODE_DISABLE_MODELS_FETCH",
        "OPENCODE_DISABLE_AUTOUPDATE",
        "OPENCODE_DISABLE_SHARE",
    ] {
        env.push((key, "1".to_owned()));
    }
    let events = harness.run("opencode", &["run", "--pure", "--format", "json", "--model", &model_id, "Use websearch exactly once for: IANA reserved example domains example.com example.org documentation. Use numResults 3 and type fast. Explain their purpose and cite the actual IANA source URL returned by search. Do not use other tools or answer from prior knowledge."], &env);
    let calls: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "tool_use")
        .collect();
    assert_eq!(
        calls.len(),
        1,
        "OpenCode must execute exactly one actual web tool"
    );
    let call = &calls[0]["part"];
    assert_eq!(call["tool"], "websearch");
    assert_eq!(call["state"]["status"], "completed");
    assert_eq!(call["state"]["metadata"]["provider"], "exa");
    let actual_result = call["state"]["output"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(actual_result.contains("iana.org") && actual_result.contains("https://"));
    let final_text = events
        .iter()
        .filter(|event| event["type"] == "text")
        .filter_map(|event| event["part"]["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    assert!(
        final_text
            .split([' ', '\n', '(', ')', '[', ']', '"'])
            .map(|word| word.trim_end_matches(['.', ',', ';']))
            .any(|url| (url.starts_with("https://www.iana.org/")
                || url.starts_with("https://iana.org/"))
                && actual_result.contains(url)),
        "A final IANA citation must match the actual Exa tool result"
    );
    assert!(
        harness
            .receipts
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|receipt| receipt.browser_searches.is_empty()),
        "OpenCode Exa retrieval is independent of Host hosted search"
    );
    assert!(final_text.contains("documentation") || final_text.contains("illustrat"));
}

#[test]
#[ignore = "Consumes the selected real ChatGPT plan with isolated installed Codex and a workspace-write sandbox."]
fn live_codex_reads_edits_and_executes_in_a_scratch_workspace() {
    let harness = LiveHarness::with_request_trace(true);
    let home = harness.scratch.0.join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(harness.scratch.0.join("edited.txt"), "UNINITIALIZED\n").unwrap();
    let provider = format!(
        "model_providers.tokenstation={{name=\"Token Station acceptance\",base_url=\"{}/agents/codex/v1\",env_key=\"TOKEN_STATION_LOCAL_KEY\",wire_api=\"responses\",requires_openai_auth=false,supports_websockets=false}}",
        harness.url
    );
    let events = harness.run("codex", &[
        "--no-daemon", "-a", "on-request", "exec", "--ignore-user-config", "--ephemeral", "--skip-git-repo-check", "--json", "-s", "workspace-write", "-C", harness.scratch.0.to_str().unwrap(),
        "-m", &harness.model, "-c", "model_provider=\"tokenstation\"", "-c", &provider, "-c", "web_search=\"disabled\"", "-c", "model_reasoning_effort=\"low\"",
        "Perform these three tool steps in order and separately. First use a shell command to read marker.txt. Second use apply_patch to update edited.txt, which already contains UNINITIALIZED followed by a newline. Use one Update File operation with a replacement hunk. Do not combine Delete File and Add File for that path. Write the exact marker you read followed by ::EDITED and a newline. Third use a shell command to read edited.txt and confirm its actual content. If a local tool reports an error, correct it and retry before verification. Do not inspect any other user file, use the network, delegate, spawn agents, or ask for user input. Use only the functions exec tool and wait if needed. Finish with LIVE_CODEX_OK and the exact edited content. The random marker is available only in marker.txt.",
    ], &[
        ("HOME",home.display().to_string()), ("CODEX_HOME",home.display().to_string()), ("TOKEN_STATION_LOCAL_KEY",harness.virtual_key.clone()),
    ]);
    let expected = format!("{}::EDITED\n", harness.marker);
    if std::fs::read_to_string(harness.scratch.0.join("edited.txt")).unwrap() != expected {
        diagnose_codex_local_results(&harness, &events);
    }
    assert_eq!(
        std::fs::read_to_string(harness.scratch.0.join("edited.txt")).unwrap(),
        expected
    );
    assert!(events.iter().any(|event| {
        let item = &event["item"];
        event["type"] == "item.completed"
            && item["type"] == "agent_message"
            && item["text"].as_str().is_some_and(|text| {
                text.contains("LIVE_CODEX_OK") && text.contains(&harness.marker)
            })
    }));
    assert!(harness.receipts.0.lock().unwrap().len() >= 3);
    let mut result_turns = 0;
    let mut calls = std::collections::BTreeMap::new();
    let mut outputs = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(harness.scratch.0.join("request-bodies")).unwrap() {
        let exchange: token_station_cli::bodylog::PlaintextExchange =
            serde_json::from_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
        let Some(request) = exchange.http_trace.agent_request else {
            continue;
        };
        let body: Value = serde_json::from_str(&request.body).unwrap();
        for item in body["input"].as_array().into_iter().flatten() {
            let Some(id) = item["call_id"].as_str() else {
                continue;
            };
            match item["type"].as_str() {
                Some("custom_tool_call" | "function_call") => {
                    assert_eq!(
                        item["namespace"], "functions",
                        "Only the authorized local executor may run"
                    );
                    assert!(matches!(item["name"].as_str(), Some("exec" | "wait")));
                    calls.insert(id.to_owned(), item.clone());
                }
                Some("custom_tool_call_output" | "function_call_output") => {
                    outputs.insert(id.to_owned(), item["output"].to_string());
                }
                _ => {}
            }
        }
        if body["input"].as_array().is_some_and(|input| {
            input.iter().any(|item| {
                matches!(
                    item["type"].as_str(),
                    Some("function_call_output" | "custom_tool_call_output")
                ) && item["output"].to_string().contains(&harness.marker)
            })
        }) {
            result_turns += 1;
        }
    }
    assert!(
        calls.len() >= 3,
        "Codex must execute distinct read, patch, and verification calls"
    );
    assert!(
        calls.keys().all(|id| outputs.contains_key(id)),
        "Each executed tool must return an output with its original call ID"
    );
    assert!(
        outputs
            .values()
            .filter(|output| output.contains(&harness.marker))
            .count()
            >= 2,
        "Distinct actual read results must carry the unpredictable marker"
    );
    assert!(
        calls.values().any(|call| call["input"]
            .as_str()
            .is_some_and(|input| input.contains("apply_patch"))),
        "The actual executor call must invoke the patch tool"
    );
    assert!(
        result_turns >= 2,
        "Actual tool output must return in subsequent model requests"
    );
}

fn diagnose_codex_local_results(harness: &LiveHarness, events: &[Value]) {
    for event in events {
        eprintln!(
            "Codex event type={} item_type={} status={}",
            event["type"], event["item"]["type"], event["item"]["status"]
        );
    }
    for entry in std::fs::read_dir(harness.scratch.0.join("request-bodies")).unwrap() {
        let exchange: token_station_cli::bodylog::PlaintextExchange =
            serde_json::from_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap()).unwrap();
        if let Some(request) = exchange.http_trace.agent_request {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            for item in body["input"].as_array().into_iter().flatten() {
                if matches!(
                    item["type"].as_str(),
                    Some("custom_tool_call_output" | "function_call_output")
                ) {
                    let output = item["output"].to_string();
                    if !output.contains(&harness.virtual_key) && !output.contains("access_token") {
                        eprintln!(
                            "Codex local tool output: {}",
                            output.chars().take(1000).collect::<String>()
                        );
                    }
                }
            }
        }
    }
}

fn live_gemini_post(harness: &LiveHarness, body: &Value) -> Value {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_mins(1)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(format!(
            "{}/agents/gemini-cli/v1beta/models/{}:generateContent",
            harness.url, harness.model
        ))
        .header("x-goog-api-key", &harness.virtual_key)
        .header("content-type", "application/json")
        .send(body.to_string())
        .unwrap();
    let status = response.status().as_u16();
    if status >= 400 {
        live_failure_metadata(harness);
    }
    assert_eq!(status, 200, "The real Gemini-protocol request must succeed");
    serde_json::from_str(&response.body_mut().read_to_string().unwrap()).unwrap()
}

#[test]
#[ignore = "Consumes the selected real ChatGPT plan through Gemini HTTP function calls. This is protocol acceptance, not Gemini CLI acceptance."]
fn live_gemini_protocol_reads_writes_and_verifies_tool_results() {
    let harness = LiveHarness::with_request_trace(true);
    let mut contents = vec![
        json!({"role":"user","parts":[{"text":"Complete the provided local tool steps one at a time. Start by calling read_marker. After receiving the marker, call write_result with exactly the marker followed by ::GEMINI_EDITED. Then call verify_result. Finally return LIVE_GEMINI_OK and the verified text. The random marker is only available from read_marker."}]}),
    ];
    let mut ids = std::collections::BTreeSet::new();
    for name in ["read_marker", "write_result", "verify_result"] {
        let parameters = if name == "write_result" {
            json!({"type":"object","properties":{"content":{"type":"string"}},"required":["content"],"additionalProperties":false})
        } else {
            json!({"type":"object","properties":{},"additionalProperties":false})
        };
        let answer = live_gemini_post(
            &harness,
            &json!({"contents":contents,"tools":[{"functionDeclarations":[{"name":name,"description":"Execute the current local scratch-workspace step.","parameters":parameters}]}]}),
        );
        let content = &answer["candidates"][0]["content"];
        let calls: Vec<_> = content["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|part| part.get("functionCall"))
            .collect();
        assert_eq!(
            calls.len(),
            1,
            "Each model turn must request one actual function"
        );
        let call = calls[0];
        assert_eq!(call["name"], name);
        let id = call["id"].as_str();
        if let Some(id) = id {
            assert!(
                ids.insert(id.to_owned()),
                "Returned tool IDs must stay unique across turns"
            );
        }
        let actual = match name {
            "read_marker" => std::fs::read_to_string(harness.scratch.0.join("marker.txt")).unwrap(),
            "write_result" => {
                let content = call["args"]["content"].as_str().unwrap();
                assert_eq!(content, format!("{}::GEMINI_EDITED", harness.marker));
                std::fs::write(harness.scratch.0.join("gemini-result.txt"), content).unwrap();
                "written".to_owned()
            }
            "verify_result" => {
                let output = Command::new("/bin/cat")
                    .env_clear()
                    .arg(harness.scratch.0.join("gemini-result.txt"))
                    .output()
                    .unwrap();
                assert!(output.status.success());
                String::from_utf8(output.stdout).unwrap()
            }
            _ => unreachable!(),
        };
        contents.push(content.clone());
        contents.push(json!({"role":"user","parts":[{"functionResponse":{"id":id,"name":name,"response":{"result":actual}}}]}));
    }
    let final_answer = live_gemini_post(&harness, &json!({"contents":contents}));
    let text: String = final_answer["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|part| part["text"].as_str())
        .collect();
    assert!(
        text.contains("LIVE_GEMINI_OK")
            && text.contains(&format!("{}::GEMINI_EDITED", harness.marker))
    );
    assert_eq!(
        std::fs::read_to_string(harness.scratch.0.join("gemini-result.txt")).unwrap(),
        format!("{}::GEMINI_EDITED", harness.marker)
    );
    // HTTP delivery can complete just before the asynchronous recorder commit.
    let deadline = Instant::now() + Duration::from_secs(2);
    while harness.receipts.0.lock().unwrap().len() < 4 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let receipts = harness.receipts.0.lock().unwrap().clone();
    assert_eq!(receipts.len(), 4);
    assert!(receipts.iter().all(|receipt| receipt.status == 200
        && receipt.subscription_billing
        && receipt.cost_micros.is_none()));
}
