//! The official Anthropic component must not silently discard structured output.
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use token_station_cli::{
    config::ClientConfig,
    gateway::{Gateway, Reply},
};
use token_station_metrics::{Recorder, RequestRecord};

struct Records;
impl Recorder for Records {
    fn record(&self, _: &RequestRecord) {}
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn reply(mut socket: TcpStream) {
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut part = [0; 8192];
    loop {
        let count = socket.read(&mut part).unwrap();
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&part[..count]);
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]);
            let size = head
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= end + 4 + size {
                break;
            }
        }
    }
    let body=json!({"id":"msg_fixture","type":"message","role":"assistant","model":"claude-sonnet-4","content":[{"type":"text","text":"fixture"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}).to_string();
    let _ = write!(
        socket,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
fn request(gateway: &Gateway, body: &Value) -> (u16, String) {
    let mut status = 0;
    let mut output = String::new();
    gateway.chat(
        "POST",
        "/v1/responses",
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
    (status, output)
}
#[test]
fn official_anthropic_wasm_rejects_structured_formats_before_http() {
    let scratch =
        Scratch(std::env::temp_dir().join(format!("ts-anthropic-format-{}", std::process::id())));
    std::fs::create_dir_all(&scratch.0).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let seen = Arc::new(AtomicUsize::new(0));
    let worker_stop = stop.clone();
    let worker_seen = seen.clone();
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((socket, _)) => {
                    worker_seen.fetch_add(1, Ordering::SeqCst);
                    reply(socket);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) => panic!("fixture listener: {e}"),
            }
        }
    });
    token_station_cli::secrets::store_set(
        &scratch.0,
        "anthropic",
        "provider_api_key",
        "anthropic-fixture-only",
    )
    .unwrap();
    let config:ClientConfig=serde_json::from_value(json!({
        "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":scratch.0,"metrics":false},
        "plugins":{"dir":Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins-dist"),"allow_unsigned":true,"agents":["agent-openai-responses"],"providers":{"anthropic":"provider-anthropic-v2"}},
        "upstreams":{"anthropic":{"provider":"anthropic","base_url":endpoint,"auth":{"slot":"provider_api_key","store":true},"models":[{"model":"claude-sonnet-4","tool":true,"tool_state":"verified","json_schema":true,"json_schema_state":"verified","context_window":200_000}]}},
        "router":{"version":1,"pools":{"main":[{"upstream":"anthropic","model":"claude-sonnet-4"}]},"default_pool":"main"}
    })).unwrap();
    let gateway = Gateway::new(&config, Arc::new(Records)).unwrap();
    let mut results = Vec::new();
    for stream in [false, true] {
        for format in [
            json!({"type":"json_object"}),
            json!({"type":"json_schema","name":"result","schema":{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false},"strict":true}),
        ] {
            let body = json!({"model":"auto","input":"Return a result.","stream":stream,"text":{"format":format}});
            results.push(request(&gateway, &body));
        }
    }
    let structured_http_count = seen.load(Ordering::SeqCst);
    let mut text_results = Vec::new();
    for format in [None, Some(json!({"type":"text"}))] {
        let mut body = json!({"model":"auto","input":"Return text.","stream":false});
        if let Some(format) = format {
            body["text"] = json!({"format":format});
        }
        let mut status = 0;
        gateway.chat(
            "POST",
            "/v1/responses",
            &[],
            body.to_string().as_bytes(),
            &mut |reply| {
                if let Reply::BeginJson(reply) = reply {
                    status = reply.status;
                }
                true
            },
        );
        text_results.push(status);
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    assert_eq!(
        structured_http_count, 0,
        "structured output must fail before any HTTP request"
    );
    assert_eq!(text_results, [200, 200]);
    assert_eq!(seen.load(Ordering::SeqCst), 2);
    for (status, body) in results {
        assert_eq!(status, 400, "{body}");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["error"]["code"], "unsupported_capability", "{body}");
    }
}
