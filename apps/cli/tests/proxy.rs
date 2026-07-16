//! The C1#1 exit criteria, exercised for real: an OpenAI-dialect client talks
//! to the loopback proxy, the proxy routes through the official WASM plugins,
//! and a mock upstream plays the provider — asserting on exactly what reached
//! it, credential and all.
//!
//! Nothing here mocks the pipeline. The agent plugin, the router, the provider
//! plugin, the exfiltration gate and the credential injection all run as they
//! would in production; only the far end of the wire is scripted.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};
use token_station_cli::config::ClientConfig;
use token_station_cli::gateway::Gateway;
use token_station_cli::server;

// -- plugin packages -------------------------------------------------------------

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("apps/cli sits two levels below the root")
}

/// Builds both official plugins once and assembles a plugins directory.
fn plugins_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("ts-proxy-plugins-{}", std::process::id()));
        for plugin in [
            "agent-openai",
            "agent-openai-responses",
            "agent-anthropic",
            "provider-openai-compatible",
        ] {
            let source = repo_root().join("plugins/official").join(plugin);
            let status = Command::new("cargo")
                .args(["build", "--target", "wasm32-wasip2"])
                .current_dir(&source)
                .status()
                .expect("cargo is on PATH");
            assert!(status.success(), "{plugin} must build");

            let package = dir.join(plugin);
            std::fs::create_dir_all(&package).expect("temp dir writable");
            std::fs::copy(source.join("manifest.json"), package.join("manifest.json"))
                .expect("manifest copies");
            std::fs::copy(
                source
                    .join("target/wasm32-wasip2/debug")
                    .join(format!("{}.wasm", plugin.replace('-', "_"))),
                package.join("adapter.wasm"),
            )
            .expect("wasm copies");
        }
        dir
    })
}

// -- the scripted upstream ---------------------------------------------------------

/// What one upstream exchange looked like from the provider's side.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    authorization: Option<String>,
    body: Value,
}

/// A provider played by a script: every connection gets the next response in
/// the list; every request is recorded for the test to assert on.
struct MockUpstream {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    hits: Arc<AtomicUsize>,
}

impl MockUpstream {
    /// `responses`: raw HTTP/1.1 response bytes, possibly written in several
    /// TCP segments to force awkward split points (`Vec<Vec<u8>>` per
    /// response).
    fn start(responses: Vec<Vec<Vec<u8>>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
        let port = listener.local_addr().expect("bound").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hits = Arc::new(AtomicUsize::new(0));

        let record = Arc::clone(&seen);
        let counter = Arc::clone(&hits);
        std::thread::spawn(move || {
            for (index, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { break };
                let request = read_http_request(&mut stream);
                record.lock().expect("recorder").push(request);
                counter.fetch_add(1, Ordering::SeqCst);

                let script = responses.get(index.min(responses.len().saturating_sub(1)));
                if let Some(segments) = script {
                    for segment in segments {
                        stream.write_all(segment).expect("mock writes");
                        stream.flush().expect("mock flushes");
                        // Let the proxy's reader observe the split.
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            }
        });

        Self { port, seen, hits }
    }

    fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("recorder").clone()
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

fn read_http_request(stream: &mut TcpStream) -> Seen {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let (mut header_end, mut content_length) = (None, 0usize);

    loop {
        if header_end.is_none() {
            if let Some(position) = find(&buffer, b"\r\n\r\n") {
                header_end = Some(position + 4);
                let head = String::from_utf8_lossy(&buffer[..position]).to_string();
                content_length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(str::trim)
                            .map(str::to_owned)
                    })
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
            }
        }
        if let Some(end) = header_end {
            if buffer.len() >= end + content_length {
                break;
            }
        }
        let read = stream.read(&mut chunk).expect("mock reads");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }

    let end = header_end.expect("a whole request arrived");
    let head = String::from_utf8_lossy(&buffer[..end]).to_string();
    let path = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or_default()
        .to_owned();
    let authorization = head.lines().find_map(|line| {
        line.to_ascii_lowercase()
            .starts_with("authorization:")
            .then(|| line.split_once(':').expect("header").1.trim().to_owned())
    });
    let body = serde_json::from_slice(&buffer[end..end + content_length]).unwrap_or(Value::Null);

    Seen {
        path,
        authorization,
        body,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn http_json(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

// -- the proxy under test ----------------------------------------------------------

/// A running proxy under test: where it listens, where it writes, and the
/// virtual key that opens its door.
struct Proxy {
    url: String,
    data_dir: PathBuf,
    virtual_key: String,
}

/// Starts the whole server against `upstream`, auth on — the default posture.
fn start_proxy(upstream: &MockUpstream, key_file: &Path) -> Proxy {
    start_proxy_with(upstream, key_file, true)
}

fn start_proxy_with(upstream: &MockUpstream, key_file: &Path, metrics: bool) -> Proxy {
    start_proxy_with_agent(upstream, key_file, metrics, "agent-openai")
}

fn start_proxy_with_agent(
    upstream: &MockUpstream,
    key_file: &Path,
    metrics: bool,
    agent_plugin: &str,
) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-proxy-data-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": data_dir, "metrics": metrics },
        "plugins": {
            "dir": plugins_dir(),
            "agent": agent_plugin,
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "mock_primary": {
                "provider": "openai-compatible",
                "base_url": upstream.base_url(),
                "auth": { "slot": "provider_api_key", "file": key_file },
                "models": [
                    { "model": "gpt-5.5", "tool": true, "json_schema": true, "context_window": 400_000 }
                ]
            }
        },
        "router": {
            "version": 1,
            "pools": { "main": [ { "upstream": "mock_primary", "model": "gpt-5.5" } ] },
            "hint_routes": [
                { "kind": "step_type", "value": "summarize", "route_to": "main" }
            ],
            "default_pool": "main"
        }
    });
    let config: ClientConfig = serde_json::from_value(config).expect("test config parses");

    let mut sinks: Vec<Box<dyn token_station_metrics::Recorder>> = vec![Box::new(
        token_station_cli::filelog::FileLog::open(&config.data.dir).expect("log opens"),
    )];
    if config.data.metrics {
        sinks.push(Box::new(
            token_station_cli::store::SqliteStore::open(&config.data.dir.join("metrics.sqlite"))
                .expect("store opens"),
        ));
    }
    let recorder = Arc::new(token_station_cli::filelog::Recorders(sinks));
    let gateway = Arc::new(Gateway::new(&config, recorder).expect("gateway assembles"));

    // Auth on, exactly as a real first start would set it up.
    let (virtual_key, created) =
        token_station_cli::virtual_key::load_or_create(&config.data.dir).expect("key creates");
    assert!(created, "each test gets a fresh data dir");
    let state = server::AppState {
        gateway,
        virtual_key: Some(Arc::from(virtual_key.as_str())),
    };

    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
    listener.set_nonblocking(true).expect("nonblocking");
    let address = listener.local_addr().expect("bound");

    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio builds");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("listener converts");
            server::serve(state, listener).await.expect("server runs");
        });
    });

    Proxy {
        url: format!("http://{address}"),
        data_dir,
        virtual_key,
    }
}

/// One row out of the metrics store, as (column -> debug-rendered value).
fn last_row(data_dir: &Path) -> std::collections::BTreeMap<String, String> {
    let db = rusqlite::Connection::open(data_dir.join("metrics.sqlite")).expect("db opens");
    db.query_row(
        "SELECT * FROM requests ORDER BY id DESC LIMIT 1",
        [],
        |row| {
            let mut out = std::collections::BTreeMap::new();
            for (index, name) in row.as_ref().column_names().iter().enumerate() {
                let value: rusqlite::types::Value = row.get(index)?;
                out.insert((*name).to_owned(), format!("{value:?}"));
            }
            Ok(out)
        },
    )
    .expect("a row exists")
}

/// Metrics writes race the HTTP response (the record lands after the last
/// byte); give the recorder a moment.
fn settle() {
    std::thread::sleep(std::time::Duration::from_millis(300));
}

fn key_file(name: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ts-proxy-key-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).expect("temp dir writable");
    path
}

fn post_chat(proxy: &Proxy, body: &Value, hint: Option<(&str, &str)>) -> (u16, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let mut request = agent
        .post(format!("{}/v1/chat/completions", proxy.url))
        .header("authorization", &format!("Bearer {}", proxy.virtual_key));
    if let Some((name, value)) = hint {
        request = request.header(name, value);
    }
    let response = request.send(&body.to_string()).expect("the proxy answers");
    let status = response.status().as_u16();
    let body = response.into_body().read_to_string().expect("body reads");
    (status, body)
}

fn send_messages(proxy: &Proxy, body: &Value, token: &str) -> (u16, Option<String>, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let response = agent
        .post(format!("{}/v1/messages?beta=true", proxy.url))
        .header("authorization", &format!("Bearer {token}"))
        .header("anthropic-version", "2023-06-01")
        .header("x-claude-code-session-id", "session-test")
        .send(&body.to_string())
        .expect("the proxy answers");
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.into_body().read_to_string().expect("body reads");
    (status, content_type, body)
}

fn post_messages(proxy: &Proxy, body: &Value, token: &str) -> (u16, String) {
    let (status, _, body) = send_messages(proxy, body, token);
    (status, body)
}

fn send_responses(proxy: &Proxy, body: &Value, token: &str) -> (u16, Option<String>, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let response = agent
        .post(format!("{}/v1/responses?include=usage", proxy.url))
        .header("authorization", &format!("Bearer {token}"))
        .send(&body.to_string())
        .expect("the proxy answers");
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.into_body().read_to_string().expect("body reads");
    (status, content_type, body)
}

fn read_marker_tool() -> Value {
    json!({
        "type": "function",
        "name": "read_marker",
        "description": "Read a marker file",
        "parameters": {
            "type": "object",
            "properties": {"path": {"type": "string"}},
            "required": ["path"]
        }
    })
}

// -- the tests -----------------------------------------------------------------------

#[test]
fn a_chat_completion_round_trips_with_the_credential_injected() {
    let upstream_answer = json!({
        "id": "chatcmpl-77",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "42." },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 9, "completion_tokens": 3 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("roundtrip", "sk-test-key-abc\n");
    let proxy = start_proxy(&mock, &key);

    let (status, body) = post_chat(
        &proxy,
        &json!({
            "model": "auto",
            "messages": [{ "role": "user", "content": "what is six times seven" }]
        }),
        None,
    );

    assert_eq!(status, 200, "{body}");
    let body: Value = serde_json::from_str(&body).expect("the reply is JSON");
    assert_eq!(body["choices"][0]["message"]["content"], json!("42."));
    assert_eq!(body["object"], json!("chat.completion"));
    assert_eq!(body["usage"]["total_tokens"], json!(12));

    // What the provider actually received: the routed model, the plugin-built
    // path under the configured base_url, and the injected credential.
    let seen = mock.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some("Bearer sk-test-key-abc")
    );
    assert_eq!(
        seen[0].body["model"],
        json!("gpt-5.5"),
        "routing replaced `auto`"
    );

    // And what the exchange left behind: one row, decision and usage included.
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(200)");
    assert_eq!(row["requested_model"], "Text(\"auto\")");
    assert_eq!(row["upstream"], "Text(\"mock_primary\")");
    assert_eq!(row["model"], "Text(\"gpt-5.5\")");
    assert_eq!(row["tier"], "Text(\"default\")");
    assert_eq!(row["attempts"], "Integer(1)");
    assert_eq!(row["input_tokens"], "Integer(9)");
    assert_eq!(row["output_tokens"], "Integer(3)");
    assert_eq!(row["cost_micros"], "Null", "no pricing table until C2#4");
    assert!(
        proxy.data_dir.join("requests.log").exists(),
        "the file log is always written"
    );

    std::fs::remove_file(key).ok();
}

#[test]
fn chat_completions_structured_output_fails_before_the_upstream() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("chat-structured-output", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    let (status, body) = post_chat(
        &proxy,
        &json!({
            "model": "auto",
            "messages": [{"role": "user", "content": "return JSON"}],
            "response_format": {"type": "json_object"}
        }),
        None,
    );

    assert_eq!(status, 400, "{body}");
    let body: Value = serde_json::from_str(&body).expect("the refusal is JSON");
    assert_eq!(body["error"]["code"], json!("capability"));
    assert_eq!(mock.hits(), 0, "the adapter must reject before routing");

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(400)");
    assert_eq!(row["error_code"], "Text(\"capability\")");
    assert_eq!(row["attempts"], "Integer(0)");
    assert_eq!(row["upstream"], "Null");

    std::fs::remove_file(key).ok();
}

#[test]
fn a_responses_request_round_trips_through_the_existing_provider_pipeline() {
    let upstream_answer = json!({
        "id": "chatcmpl-responses-1",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "M4_OK"},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 7,
            "completion_tokens": 2,
            "prompt_tokens_details": {"cached_tokens": 3},
            "completion_tokens_details": {"reasoning_tokens": 1}
        }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("responses-json", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");

    let token = proxy.virtual_key.clone();
    let (status, content_type, body) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "instructions": "Answer with the marker.",
            "input": "Return M4_OK.",
            "stream": false
        }),
        &token,
    );

    assert_eq!(status, 200, "{body}");
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let body: Value = serde_json::from_str(&body).expect("Responses body is JSON");
    assert_eq!(body["object"], json!("response"));
    assert_eq!(body["status"], json!("completed"));
    assert_eq!(body["model"], json!("gpt-5.5"));
    assert_eq!(body["output"][0]["type"], json!("message"));
    assert_eq!(body["output"][0]["content"][0]["text"], json!("M4_OK"));
    assert_eq!(body["usage"]["input_tokens"], json!(7));
    assert_eq!(
        body["usage"]["input_tokens_details"]["cached_tokens"],
        json!(3)
    );

    let seen = mock.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some("Bearer sk-test-key-abc")
    );
    assert_eq!(seen[0].body["model"], json!("gpt-5.5"));
    assert_eq!(seen[0].body["messages"][0]["role"], json!("system"));
    assert_eq!(
        seen[0].body["messages"][1]["content"],
        json!("Return M4_OK.")
    );

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["protocol"], "Text(\"openai-responses\")");
    assert_eq!(row["input_tokens"], "Integer(7)");
    assert_eq!(row["output_tokens"], "Integer(2)");

    std::fs::remove_file(key).ok();
}

#[test]
fn responses_structured_output_fails_before_the_upstream() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("responses-structured-output", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();

    let (status, content_type, body) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "input": "return JSON",
            "text": {
                "format": {
                    "type": "json_schema",
                    "name": "answer",
                    "schema": {"type": "object"},
                    "strict": true
                }
            }
        }),
        &token,
    );

    assert_eq!(status, 400, "{body}");
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let body: Value = serde_json::from_str(&body).expect("the refusal is JSON");
    assert_eq!(body["error"]["code"], json!("unsupported_capability"));
    assert_eq!(mock.hits(), 0, "the adapter must reject before routing");

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(400)");
    assert_eq!(row["error_code"], "Text(\"capability\")");
    assert_eq!(row["attempts"], "Integer(0)");
    assert_eq!(row["upstream"], "Null");

    std::fs::remove_file(key).ok();
}

#[test]
fn a_responses_stream_is_incremental_and_protocol_shaped() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"M4_\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"STREAM_OK\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let header = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        sse.len()
    );
    let bytes = sse.as_bytes();
    let mock = MockUpstream::start(vec![vec![
        header.into_bytes(),
        bytes[..31].to_vec(),
        bytes[31..].to_vec(),
    ]]);
    let key = key_file("responses-stream", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();

    let (status, content_type, body) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "input": "Return the marker.",
            "stream": true
        }),
        &token,
    );

    assert_eq!(status, 200, "{body}");
    assert_eq!(content_type.as_deref(), Some("text/event-stream"));
    assert!(body.contains("event: response.created"), "{body}");
    assert!(body.contains("event: response.output_item.added"), "{body}");
    assert!(
        body.contains("event: response.content_part.added"),
        "{body}"
    );
    assert!(body.contains("event: response.output_text.delta"), "{body}");
    assert!(body.contains(r#""delta":"M4_""#), "{body}");
    assert!(body.contains(r#""delta":"STREAM_OK""#), "{body}");
    assert!(body.contains("event: response.output_item.done"), "{body}");
    assert!(body.contains("M4_STREAM_OK"), "{body}");
    assert!(body.contains("event: response.completed"), "{body}");

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["protocol"], "Text(\"openai-responses\")");
    assert_eq!(row["stream"], "Integer(1)");
    assert_eq!(row["input_tokens"], "Integer(5)");
    assert_eq!(row["output_tokens"], "Integer(2)");

    std::fs::remove_file(key).ok();
}

#[test]
fn responses_function_call_and_output_complete_a_second_turn() {
    let tool_answer = json!({
        "id": "chatcmpl-tool",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_marker",
                    "type": "function",
                    "function": {
                        "name": "read_marker",
                        "arguments": "{\"path\":\"marker.txt\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 8, "completion_tokens": 3}
    });
    let final_answer = json!({
        "id": "chatcmpl-final",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "TOOL_RESULT_OK"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 12, "completion_tokens": 2}
    });
    let mock = MockUpstream::start(vec![
        vec![http_json(200, &tool_answer.to_string())],
        vec![http_json(200, &final_answer.to_string())],
    ]);
    let key = key_file("responses-tool", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();
    let tool = read_marker_tool();

    let (status, _, first) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "input": "Read marker.txt.",
            "tools": [tool.clone()],
            "stream": false
        }),
        &token,
    );
    assert_eq!(status, 200, "{first}");
    let first: Value = serde_json::from_str(&first).expect("first response is JSON");
    assert_eq!(first["output"][0]["type"], json!("function_call"));
    assert_eq!(first["output"][0]["call_id"], json!("call_marker"));

    let (status, _, second) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "input": [
                {"type": "message", "role": "user", "content": "Read marker.txt."},
                {
                    "type": "function_call",
                    "call_id": "call_marker",
                    "name": "read_marker",
                    "arguments": "{\"path\":\"marker.txt\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_marker",
                    "output": "TOOL_RESULT_OK"
                }
            ],
            "tools": [tool],
            "stream": false
        }),
        &token,
    );
    assert_eq!(status, 200, "{second}");
    let second: Value = serde_json::from_str(&second).expect("second response is JSON");
    assert_eq!(
        second["output"][0]["content"][0]["text"],
        json!("TOOL_RESULT_OK")
    );

    let seen = mock.seen();
    assert_eq!(seen.len(), 2);
    assert_eq!(
        seen[1].body["messages"][1]["tool_calls"][0]["id"],
        json!("call_marker")
    );
    assert_eq!(seen[1].body["messages"][2]["role"], json!("tool"));
    assert_eq!(
        seen[1].body["messages"][2]["tool_call_id"],
        json!("call_marker")
    );
    assert_eq!(
        seen[1].body["messages"][2]["content"],
        json!("TOOL_RESULT_OK")
    );

    std::fs::remove_file(key).ok();
}

#[test]
fn responses_local_auth_and_protocol_mismatch_fail_before_upstream() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("responses-local-auth", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let body = json!({"model": "auto", "input": "hi", "stream": false});

    let (status, _, refusal) = send_responses(&proxy, &body, "wrong-local-key");
    assert_eq!(status, 401, "{refusal}");
    let refusal: Value = serde_json::from_str(&refusal).expect("auth error is JSON");
    assert_eq!(refusal["error"]["code"], json!("authentication_error"));

    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let mismatch = agent
        .post(format!("{}/v1/chat/completions", proxy.url))
        .header("authorization", &format!("Bearer {}", proxy.virtual_key))
        .send(
            &json!({
                "model": "auto",
                "messages": [{"role": "user", "content": "hi"}]
            })
            .to_string(),
        )
        .expect("the proxy answers");
    assert_eq!(mismatch.status().as_u16(), 404);
    let mismatch = mismatch
        .into_body()
        .read_to_string()
        .expect("mismatch body reads");
    let mismatch: Value = serde_json::from_str(&mismatch).expect("mismatch is JSON");
    assert_eq!(mismatch["error"]["code"], json!("invalid_request"));
    assert_eq!(mock.hits(), 0, "both failures stop before the upstream");

    std::fs::remove_file(key).ok();
}

#[test]
fn an_upstream_auth_failure_is_rendered_as_responses() {
    let refusal = json!({
        "error": {
            "message": "Incorrect API key provided",
            "type": "invalid_request_error"
        }
    });
    let mock = MockUpstream::start(vec![vec![http_json(401, &refusal.to_string())]]);
    let key = key_file("responses-upstream-auth", "sk-live-topsecret");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();

    let (status, _, body) = send_responses(
        &proxy,
        &json!({"model": "auto", "input": "hi", "stream": false}),
        &token,
    );

    assert_eq!(status, 401, "{body}");
    let parsed: Value = serde_json::from_str(&body).expect("error is JSON");
    assert_eq!(parsed["error"]["code"], json!("authentication_error"));
    assert!(!body.contains("topsecret"));
    assert_eq!(mock.hits(), 1);

    std::fs::remove_file(key).ok();
}

#[test]
fn an_anthropic_message_round_trips_through_the_same_provider_pipeline() {
    let upstream_answer = json!({
        "id": "chatcmpl-88",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "42."},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 11, "completion_tokens": 3}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("anthropic-roundtrip", "sk-test-key-abc\n");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");

    let (status, content_type, body) = send_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 128,
            "system": "You are concise.",
            "messages": [{"role": "user", "content": "what is six times seven"}]
        }),
        &proxy.virtual_key,
    );

    assert_eq!(status, 200, "{body}");
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let body: Value = serde_json::from_str(&body).expect("the reply is JSON");
    assert_eq!(body["type"], json!("message"));
    assert_eq!(body["role"], json!("assistant"));
    assert_eq!(body["content"][0]["type"], json!("text"));
    assert_eq!(body["content"][0]["text"], json!("42."));
    assert_eq!(body["stop_reason"], json!("end_turn"));
    assert_eq!(body["usage"]["input_tokens"], json!(11));

    let seen = mock.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(seen[0].body["model"], json!("gpt-5.5"));
    assert_eq!(seen[0].body["messages"][0]["role"], json!("system"));
    assert_eq!(seen[0].body["messages"][1]["role"], json!("user"));

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["protocol"], "Text(\"anthropic-messages\")");
    assert_eq!(row["status"], "Integer(200)");

    std::fs::remove_file(key).ok();
}

#[test]
fn anthropic_local_auth_and_protocol_mismatch_fail_before_upstream() {
    let mock = MockUpstream::start(Vec::new());
    let key = key_file("anthropic-local-auth", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");
    let request = json!({
        "model": "auto",
        "max_tokens": 64,
        "messages": [{"role": "user", "content": "hi"}]
    });

    let (status, body) = post_messages(&proxy, &request, "wrong-local-key");
    assert_eq!(status, 401, "{body}");
    let error: Value = serde_json::from_str(&body).expect("Anthropic error JSON");
    assert_eq!(error["type"], json!("error"));
    assert_eq!(error["error"]["type"], json!("authentication_error"));

    let (status, body) = post_chat(
        &proxy,
        &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
        None,
    );
    assert_eq!(status, 404, "{body}");
    let error: Value = serde_json::from_str(&body).expect("Anthropic error JSON");
    assert_eq!(error["error"]["type"], json!("invalid_request_error"));
    assert_eq!(mock.hits(), 0, "both refusals happen before routing");

    std::fs::remove_file(key).ok();
}

#[test]
fn an_anthropic_stream_is_incremental_and_protocol_shaped() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        sse.len()
    );
    let raw = [head.as_bytes(), sse.as_bytes()].concat();
    let segments = vec![
        raw[..17].to_vec(),
        raw[17..89].to_vec(),
        raw[89..151].to_vec(),
        raw[151..].to_vec(),
    ];
    let mock = MockUpstream::start(vec![segments]);
    let key = key_file("anthropic-stream", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");

    let (status, content_type, body) = send_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 128,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }),
        &proxy.virtual_key,
    );

    assert_eq!(status, 200, "{body}");
    assert_eq!(content_type.as_deref(), Some("text/event-stream"));
    assert_eq!(body.matches("event: message_start").count(), 1, "{body}");
    assert!(
        body.contains(r#""text":"Hel","type":"text_delta""#),
        "{body}"
    );
    assert!(
        body.contains(r#""text":"lo","type":"text_delta""#),
        "{body}"
    );
    assert!(body.contains("event: message_stop"), "{body}");

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["protocol"], "Text(\"anthropic-messages\")");
    assert_eq!(row["stream"], "Integer(1)");
    assert_eq!(row["input_tokens"], "Integer(7)");
    assert_eq!(row["output_tokens"], "Integer(2)");

    std::fs::remove_file(key).ok();
}

#[test]
fn an_upstream_auth_failure_is_rendered_as_anthropic() {
    let refusal = json!({"error": {"message": "Incorrect API key provided"}}).to_string();
    let mock = MockUpstream::start(vec![vec![http_json(401, &refusal)]]);
    let key = key_file("anthropic-upstream-auth", "sk-live-topsecret");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");

    let (status, body) = post_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "hi"}]
        }),
        &proxy.virtual_key,
    );

    assert_eq!(status, 401, "{body}");
    let error: Value = serde_json::from_str(&body).expect("Anthropic error JSON");
    assert_eq!(error["type"], json!("error"));
    assert_eq!(error["error"]["type"], json!("authentication_error"));
    assert!(!body.contains("topsecret"));
    assert_eq!(mock.hits(), 1);

    std::fs::remove_file(key).ok();
}

#[test]
fn a_streaming_completion_survives_awkward_tcp_split_points() {
    // SSE frames deliberately split mid-frame and mid-multibyte-boundary: the
    // proxy's reader must hand the pieces to the stream parser as they come,
    // and the parser holds the tail — that is what conformance drilled.
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    );
    let header = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        sse.len()
    );
    // Split the body at points that cut SSE frames in half.
    let bytes = sse.as_bytes();
    let segments = vec![
        header.into_bytes(),
        bytes[..17].to_vec(),
        bytes[17..70].to_vec(),
        bytes[70..].to_vec(),
    ];
    let mock = MockUpstream::start(vec![segments]);
    let key = key_file("stream", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let response = agent
        .post(format!("{}/v1/chat/completions", proxy.url))
        .header("authorization", &format!("Bearer {}", proxy.virtual_key))
        .send(
            &json!({
                "model": "auto",
                "stream": true,
                "messages": [{ "role": "user", "content": "hi" }]
            })
            .to_string(),
        )
        .expect("the proxy answers");

    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("")),
        Some("text/event-stream")
    );
    let body = response.into_body().read_to_string().expect("stream reads");

    // The rendered stream carries the reassembled deltas and terminates.
    assert!(body.contains(r#""content":"Hel""#), "{body}");
    assert!(body.contains(r#""content":"lo""#), "{body}");
    assert!(body.contains("data: [DONE]"), "{body}");

    // The usage event inside the stream reached the metrics row.
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["stream"], "Integer(1)");
    assert_eq!(row["input_tokens"], "Integer(7)");
    assert_eq!(row["output_tokens"], "Integer(2)");

    std::fs::remove_file(key).ok();
}

#[test]
fn the_models_catalog_aggregates_configured_upstreams() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("models", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    let response = ureq::get(format!("{}/v1/models", proxy.url))
        .header("authorization", &format!("Bearer {}", proxy.virtual_key))
        .call()
        .expect("the proxy answers");
    let body: Value =
        serde_json::from_str(&response.into_body().read_to_string().expect("body reads"))
            .expect("valid JSON");

    assert_eq!(body["object"], json!("list"));
    assert_eq!(body["data"][0]["id"], json!("gpt-5.5"));
    assert_eq!(body["data"][0]["owned_by"], json!("mock_primary"));
    assert_eq!(
        mock.hits(),
        0,
        "the catalog is served from config, not upstream"
    );

    std::fs::remove_file(key).ok();
}

#[test]
fn an_upstream_401_is_mapped_not_retried_and_never_leaks_the_key() {
    let refusal = json!({ "error": { "message": "Incorrect API key provided", "type": "invalid_request_error" } });
    let mock = MockUpstream::start(vec![vec![http_json(401, &refusal.to_string())]]);
    let key = key_file("refused", "sk-live-topsecret");
    let proxy = start_proxy(&mock, &key);

    let (status, body) = post_chat(
        &proxy,
        &json!({ "model": "auto", "messages": [{ "role": "user", "content": "hi" }] }),
        None,
    );

    assert_eq!(status, 401);
    let parsed: Value = serde_json::from_str(&body).expect("error is JSON");
    assert_eq!(parsed["error"]["type"], json!("auth"));
    assert!(
        !body.contains("topsecret"),
        "a client-visible error must never carry the credential"
    );
    assert_eq!(mock.hits(), 1, "an auth failure is not retried anywhere");

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(401)");
    assert_eq!(row["error_code"], "Text(\"auth\")");
    assert_eq!(row["attempts"], "Integer(1)");

    std::fs::remove_file(key).ok();
}

#[test]
fn a_request_the_pool_cannot_serve_is_refused_with_the_reason() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("capability", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    // The only model has no vision; an image request has nowhere to go.
    let (status, body) = post_chat(
        &proxy,
        &json!({
            "model": "auto",
            "messages": [{ "role": "user", "content": [
                { "type": "image_url", "image_url": { "url": "https://example/cat.png" } }
            ]}]
        }),
        None,
    );

    assert_eq!(status, 503, "{body}");
    assert!(body.contains("vision"), "{body}");
    assert_eq!(mock.hits(), 0, "nothing capable, so nothing was sent");

    std::fs::remove_file(key).ok();
}

#[test]
fn nothing_the_caller_wrote_reaches_disk() {
    // The canary rides in message content, a tool definition and a hint header
    // — everything except the model name, which is classified metadata. After
    // the exchange, the *raw bytes* of both sinks must not contain it.
    const CANARY: &str = "DATA-LAYER-CONTENT-CANARY-9f3a";

    let answer = json!({
        "id": "chatcmpl-1", "model": "gpt-5.5",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("canary", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    let (status, _) = post_chat(
        &proxy,
        &json!({
            "model": "auto",
            "messages": [{ "role": "user", "content": format!("please keep {CANARY} secret") }],
            "tools": [{ "type": "function", "function": {
                "name": "get_weather",
                "description": CANARY,
                "parameters": { "secret": CANARY }
            }}]
        }),
        Some(("x-agent-step", CANARY)),
    );
    assert_eq!(status, 200);

    settle();
    for artifact in ["metrics.sqlite", "requests.log"] {
        let bytes = std::fs::read(proxy.data_dir.join(artifact)).expect("artifact exists");
        let haystack = String::from_utf8_lossy(&bytes);
        assert!(
            !haystack.contains(CANARY),
            "`{artifact}` holds caller content"
        );
    }

    std::fs::remove_file(key).ok();
}

#[test]
fn switching_metrics_off_leaves_the_file_log_only() {
    let answer = json!({
        "id": "chatcmpl-1", "model": "gpt-5.5",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("no-metrics", "sk-test-key-abc");
    let proxy = start_proxy_with(&mock, &key, false);

    let (status, _) = post_chat(
        &proxy,
        &json!({ "model": "auto", "messages": [{ "role": "user", "content": "hi" }] }),
        None,
    );
    assert_eq!(status, 200);

    settle();
    assert!(
        !proxy.data_dir.join("metrics.sqlite").exists(),
        "metrics off means no store is even created"
    );
    let log = std::fs::read_to_string(proxy.data_dir.join("requests.log")).expect("log exists");
    assert!(
        log.contains("\"status\":200"),
        "the file log is always written: {log}"
    );
}

/// A second proxy builder: two upstreams in one pool, aggressive cooldown.
fn start_proxy_two(primary: &MockUpstream, fallback: &MockUpstream, key_file: &Path) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-proxy-two-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let upstream = |mock: &MockUpstream| {
        json!({
            "provider": "openai-compatible",
            "base_url": mock.base_url(),
            "auth": { "slot": "provider_api_key", "file": key_file },
            "models": [ { "model": "gpt-5.5", "tool": true, "context_window": 400_000 } ]
        })
    };
    let config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": data_dir, "metrics": true },
        "health": { "eject_after": 3, "cooldown_ms": 500 },
        "plugins": {
            "dir": plugins_dir(),
            "agent": "agent-openai",
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "mock_primary": upstream(primary),
            "mock_fallback": upstream(fallback)
        },
        "router": {
            "version": 1,
            "pools": { "main": [
                { "upstream": "mock_primary", "model": "gpt-5.5" },
                { "upstream": "mock_fallback", "model": "gpt-5.5" }
            ]},
            "default_pool": "main"
        }
    });
    let config: ClientConfig = serde_json::from_value(config).expect("test config parses");
    let recorder = Arc::new(token_station_cli::filelog::Recorders(vec![Box::new(
        token_station_cli::filelog::FileLog::open(&config.data.dir).expect("log opens"),
    )]));
    let gateway = Arc::new(Gateway::new(&config, recorder).expect("gateway assembles"));
    let (virtual_key, _) =
        token_station_cli::virtual_key::load_or_create(&config.data.dir).expect("key creates");
    let state = server::AppState {
        gateway,
        virtual_key: Some(Arc::from(virtual_key.as_str())),
    };

    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
    listener.set_nonblocking(true).expect("nonblocking");
    let address = listener.local_addr().expect("bound");
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio builds");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("listener converts");
            server::serve(state, listener).await.expect("server runs");
        });
    });
    Proxy {
        url: format!("http://{address}"),
        data_dir,
        virtual_key,
    }
}

#[test]
fn a_failing_upstream_is_ejected_bypassed_probed_and_restored() {
    let ok = json!({
        "id": "chatcmpl-1", "model": "gpt-5.5",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "ok" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    })
    .to_string();
    let boom =
        json!({ "error": { "message": "upstream exploded", "type": "server_error" } }).to_string();

    // Primary: three failures, then healthy again (what the probe will find).
    let primary = MockUpstream::start(vec![
        vec![http_json(503, &boom)],
        vec![http_json(503, &boom)],
        vec![http_json(503, &boom)],
        vec![http_json(200, &ok)],
    ]);
    // Fallback: serves, until its fifth answer fails once — which is what
    // forces the router down to the degraded primary.
    let fallback = MockUpstream::start(vec![
        vec![http_json(200, &ok)],
        vec![http_json(200, &ok)],
        vec![http_json(200, &ok)],
        vec![http_json(200, &ok)],
        vec![http_json(503, &boom)],
        vec![http_json(200, &ok)],
    ]);
    let key = key_file("health", "sk-test-key-abc");
    let proxy = start_proxy_two(&primary, &fallback, &key);
    let ask = || {
        post_chat(
            &proxy,
            &json!({ "model": "auto", "messages": [{ "role": "user", "content": "hi" }] }),
            None,
        )
    };

    // Three requests: primary fails each time, fallback answers each time.
    for round in 1..=3 {
        let (status, body) = ask();
        assert_eq!(status, 200, "round {round}: fallback serves: {body}");
    }
    assert_eq!(primary.hits(), 3);
    assert_eq!(fallback.hits(), 3);

    // Ejected: the fourth request must not even knock on the primary.
    let (status, _) = ask();
    assert_eq!(status, 200);
    assert_eq!(primary.hits(), 3, "an ejected upstream receives no traffic");
    assert_eq!(fallback.hits(), 4);

    // Past the cooldown the primary is on probation: preferred candidates
    // first, so it is probed exactly when the fallback lets us down.
    std::thread::sleep(std::time::Duration::from_millis(700));
    let (status, body) = ask();
    assert_eq!(status, 200, "the probe answered: {body}");
    assert_eq!(
        fallback.hits(),
        5,
        "the healthy fallback was still tried first"
    );
    assert_eq!(primary.hits(), 4, "the degraded primary took the probe");

    // The probe succeeded, so the primary is fully back: config order wins.
    let (status, _) = ask();
    assert_eq!(status, 200);
    assert_eq!(
        primary.hits(),
        5,
        "a recovered upstream leads its pool again"
    );
    assert_eq!(fallback.hits(), 5);

    std::fs::remove_file(key).ok();
}

#[test]
fn auth_failures_never_eject_an_upstream() {
    let refusal = json!({ "error": { "message": "Incorrect API key provided" } }).to_string();
    let mock = MockUpstream::start(vec![vec![http_json(401, &refusal)]]);
    let key = key_file("auth-noeject", "sk-wrong-key");
    let proxy = start_proxy(&mock, &key);

    for _ in 0..5 {
        let (status, _) = post_chat(
            &proxy,
            &json!({ "model": "auto", "messages": [{ "role": "user", "content": "hi" }] }),
            None,
        );
        assert_eq!(status, 401);
    }

    assert_eq!(
        mock.hits(),
        5,
        "a misconfigured key must keep surfacing as 401, not become a fake outage"
    );

    std::fs::remove_file(key).ok();
}

// -- `upstream test`, the probe ------------------------------------------------------

/// A gateway with no server around it: `upstream test` runs exactly this.
fn probe_gateway(upstream: &MockUpstream, key_file: &Path) -> Gateway {
    let config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "plugins": {
            "dir": plugins_dir(),
            "agent": "agent-openai",
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "mock_primary": {
                "provider": "openai-compatible",
                "base_url": upstream.base_url(),
                "auth": { "slot": "provider_api_key", "file": key_file },
                "models": [ { "model": "gpt-5.5", "tool": true, "context_window": 400_000 } ]
            }
        },
        "router": {
            "version": 1,
            "pools": { "main": [ { "upstream": "mock_primary", "model": "gpt-5.5" } ] },
            "default_pool": "main"
        }
    });
    let config: ClientConfig = serde_json::from_value(config).expect("test config parses");
    Gateway::new(&config, Arc::new(token_station_metrics::NoopRecorder)).expect("gateway assembles")
}

#[test]
fn an_upstream_probe_runs_the_real_southbound_path() {
    let answer = json!({
        "id": "chatcmpl-1", "model": "gpt-5.5",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "p" }, "finish_reason": "length" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("probe-ok", "sk-test-key-abc\n");
    let gateway = probe_gateway(&mock, &key);

    let outcomes = gateway.probe("mock_primary", None).expect("probes");
    assert_eq!(outcomes.len(), 1, "one probe per declared model");
    assert_eq!(outcomes[0].model, "gpt-5.5");
    assert!(
        outcomes[0].latency_ms.is_ok(),
        "{:?}",
        outcomes[0].latency_ms
    );

    // The probe went over the real wire: plugin-built path, injected
    // credential, and a one-token budget so aliveness costs almost nothing.
    let seen = mock.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some("Bearer sk-test-key-abc")
    );
    assert_eq!(seen[0].body["model"], json!("gpt-5.5"));
    assert_eq!(seen[0].body["max_tokens"], json!(1));

    std::fs::remove_file(key).ok();
}

#[test]
fn a_failed_probe_names_the_refusal_and_never_the_key() {
    let refusal = json!({ "error": { "message": "Incorrect API key provided", "type": "invalid_request_error" } });
    let mock = MockUpstream::start(vec![vec![http_json(401, &refusal.to_string())]]);
    let key = key_file("probe-refused", "sk-live-topsecret");
    let gateway = probe_gateway(&mock, &key);

    let outcomes = gateway
        .probe("mock_primary", None)
        .expect("a refusal is an outcome, not a probe error");
    let reason = outcomes[0].latency_ms.as_ref().expect_err("401 must fail");
    assert!(reason.contains("401"), "{reason}");
    assert!(!reason.contains("topsecret"), "value-free errors: {reason}");

    // The refusals that are probe errors: nothing was asked over the wire.
    let error = gateway
        .probe("nowhere", None)
        .expect_err("unknown upstream");
    assert!(error.contains("mock_primary"), "{error}");
    let error = gateway
        .probe("mock_primary", Some("gpt-9"))
        .expect_err("undeclared model");
    assert!(error.contains("gpt-5.5"), "{error}");
    assert_eq!(mock.hits(), 1, "only the real probe reached the upstream");

    std::fs::remove_file(key).ok();
}

#[test]
fn without_the_virtual_key_the_door_stays_shut() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("door", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let body = json!({ "model": "auto", "messages": [{ "role": "user", "content": "hi" }] });

    // No key at all.
    let bare = agent
        .post(format!("{}/v1/chat/completions", proxy.url))
        .send(&body.to_string())
        .expect("the proxy answers");
    assert_eq!(bare.status().as_u16(), 401);

    // A wrong key, same length as the real one.
    let wrong: String = proxy.virtual_key.chars().rev().collect();
    let guessed = agent
        .post(format!("{}/v1/chat/completions", proxy.url))
        .header("authorization", &format!("Bearer {wrong}"))
        .send(&body.to_string())
        .expect("the proxy answers");
    assert_eq!(guessed.status().as_u16(), 401);

    // The catalog is behind the same door.
    let models = agent
        .get(format!("{}/v1/models", proxy.url))
        .call()
        .expect("the proxy answers");
    assert_eq!(models.status().as_u16(), 401);

    assert_eq!(
        mock.hits(),
        0,
        "a rejected caller must cause zero upstream traffic"
    );

    std::fs::remove_file(key).ok();
}
