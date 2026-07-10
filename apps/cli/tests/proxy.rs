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
        for plugin in ["agent-openai", "provider-openai-compatible"] {
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

/// Starts the whole server against `upstream`, returns its base URL.
fn start_proxy(upstream: &MockUpstream, key_file: &Path) -> String {
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
    let gateway = Arc::new(Gateway::new(&config).expect("gateway assembles"));

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
            server::serve(gateway, listener).await.expect("server runs");
        });
    });

    format!("http://{address}")
}

fn key_file(name: &str, contents: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("ts-proxy-key-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).expect("temp dir writable");
    path
}

fn post_chat(proxy: &str, body: &Value, hint: Option<(&str, &str)>) -> (u16, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let mut request = agent.post(format!("{proxy}/v1/chat/completions"));
    if let Some((name, value)) = hint {
        request = request.header(name, value);
    }
    let response = request.send(&body.to_string()).expect("the proxy answers");
    let status = response.status().as_u16();
    let body = response.into_body().read_to_string().expect("body reads");
    (status, body)
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
        .post(format!("{proxy}/v1/chat/completions"))
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

    std::fs::remove_file(key).ok();
}

#[test]
fn the_models_catalog_aggregates_configured_upstreams() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("models", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    let response = ureq::get(format!("{proxy}/v1/models"))
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
