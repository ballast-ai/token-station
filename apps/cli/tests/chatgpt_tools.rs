//! Full local tool loops through the shipped Agent and `ChatGPT` Responses components.
//! The upstream and credentials are fixtures. Ignored tests run installed Agent CLIs
//! with isolated configuration and never use a real subscription or API key.

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

fn text_response() -> String {
    sse(vec![
        json!({"type":"response.created","response":{"id":"resp_final","model":"gpt-5.5","status":"in_progress"}}),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":FINAL_TEXT}),
        json!({"type":"response.completed","response":{"id":"resp_final","model":"gpt-5.5","status":"completed","output":[],"usage":{"input_tokens":10,"output_tokens":8}}}),
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

struct FakeResponses {
    url: String,
    seen: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    status: Arc<AtomicUsize>,
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
                let functions = body["tools"]
                    .as_array()
                    .and_then(|groups| {
                        groups.iter().find(|group| {
                            group["type"] == "namespace" && group["name"] == "tokenstation_tools"
                        })
                    })
                    .and_then(|group| group["tools"].as_array());
                let reader = functions.and_then(|tools| {
                    tools.iter().find(|tool| {
                        tool["name"]
                            .as_str()
                            .is_some_and(|name| name.eq_ignore_ascii_case("read"))
                    })
                });
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
                        text_response()
                    }
                } else {
                    text_response()
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
            "plugins":{"dir":plugin_dir,"agents":["agent-openai","agent-anthropic"],"providers":{"chatgpt-plan":"provider-chatgpt-plan-v2","openai-compatible":"provider-openai-compatible-v2"}},
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
        use base64::Engine as _;
        let required = |name| {
            std::env::var(name)
                .unwrap_or_else(|_| panic!("Explicit live acceptance requires {name}"))
        };
        let data = PathBuf::from(required("TOKEN_STATION_CHATGPT_LIVE_DATA_DIR"));
        let account = required("TOKEN_STATION_CHATGPT_LIVE_ACCOUNT_ID");
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
        let config: ClientConfig = serde_json::from_value(json!({
            "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":data,"metrics":false},
            "plugins":{"dir":plugin_dir,"agents":["agent-openai","agent-anthropic"],"providers":{"chatgpt-plan":"provider-chatgpt-plan-v2"}},
            "upstreams":{"live_plan":{"provider":"chatgpt-plan","base_url":"https://api.openai.com/v1","auth":{"slot":"chatgpt_access_token","chatgpt_account":account},"models":[{"model":model,"tool":true,"tool_state":"declared","context_window":200_000}]}},
            "router":{"version":1,"pools":{"main":[{"upstream":"live_plan","model":model}]},"default_pool":"main"}
        })).unwrap();
        config.validate().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let receipts = Arc::new(CapturedRecords::default());
        let gateway = Arc::new(
            Gateway::new_with_provider_runtime(&config, receipts.clone(), runtime.handle().clone())
                .unwrap(),
        );
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
