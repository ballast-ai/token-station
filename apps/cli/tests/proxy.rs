//! The C1#1 exit criteria, exercised for real: an OpenAI-dialect client talks
//! to the loopback proxy, the proxy routes through the official WASM plugins,
//! and a mock upstream plays the provider — asserting on exactly what reached
//! it, credential and all.
//!
//! Nothing here mocks the pipeline. The agent plugin, the router, the provider
//! plugin, the exfiltration gate and the credential injection all run as they
//! would in production; only the far end of the wire is scripted.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use token_station_cli::config::ClientConfig;
use token_station_cli::gateway::{FeatureLayer, Gateway, Reply, StageStatus};
use token_station_cli::server;

const TEST_TEXT_PDF_BASE64: &str = "JVBERi0xLjQKJeLjz9MKMSAwIG9iago8PCAvVHlwZSAvQ2F0YWxvZyAvUGFnZXMgMiAwIFIgPj4KZW5kb2JqCjIgMCBvYmoKPDwgL1R5cGUgL1BhZ2VzIC9LaWRzIFszIDAgUl0gL0NvdW50IDEgPj4KZW5kb2JqCjMgMCBvYmoKPDwgL1R5cGUgL1BhZ2UgL1BhcmVudCAyIDAgUiAvTWVkaWFCb3ggWzAgMCA2MTIgNzkyXSAvUmVzb3VyY2VzIDw8IC9Gb250IDw8IC9GMSA0IDAgUiA+PiA+PiAvQ29udGVudHMgNSAwIFIgPj4KZW5kb2JqCjQgMCBvYmoKPDwgL1R5cGUgL0ZvbnQgL1N1YnR5cGUgL1R5cGUxIC9CYXNlRm9udCAvSGVsdmV0aWNhID4+CmVuZG9iago1IDAgb2JqCjw8IC9MZW5ndGggNTMgPj4Kc3RyZWFtCkJUIC9GMSAxMiBUZiA3MiA3MjAgVGQgKFRPS0VOX1NUQVRJT05fUERGX1RFWFQpIFRqIEVUCmVuZHN0cmVhbQplbmRvYmoKeHJlZgowIDYKMDAwMDAwMDAwMCA2NTUzNSBmIAowMDAwMDAwMDE1IDAwMDAwIG4gCjAwMDAwMDAwNjQgMDAwMDAgbiAKMDAwMDAwMDEyMSAwMDAwMCBuIAowMDAwMDAwMjQ3IDAwMDAwIG4gCjAwMDAwMDAzMTcgMDAwMDAgbiAKdHJhaWxlcgo8PCAvU2l6ZSA2IC9Sb290IDEgMCBSID4+CnN0YXJ0eHJlZgo0MjAKJSVFT0YK";

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
            "agent-gemini",
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

        let marker_agent = repo_root().join("apps/cli/tests/guests/marker-agent");
        let status = Command::new("cargo")
            .args(["build", "--target", "wasm32-wasip2"])
            .current_dir(&marker_agent)
            .status()
            .expect("cargo is on PATH");
        assert!(status.success(), "marker-agent must build");
        let package = dir.join("marker-agent");
        std::fs::create_dir_all(&package).expect("temp dir writable");
        std::fs::copy(
            marker_agent.join("manifest.json"),
            package.join("manifest.json"),
        )
        .expect("marker-agent manifest copies");
        std::fs::copy(
            marker_agent.join("target/wasm32-wasip2/debug/marker_agent.wasm"),
            package.join("adapter.wasm"),
        )
        .expect("marker-agent wasm copies");
        dir
    })
}

// -- the scripted upstream ---------------------------------------------------------

/// What one upstream exchange looked like from the provider's side.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    authorization: Option<String>,
    content_type: Option<String>,
    body: Value,
}

/// A provider played by a script: every connection gets the next response in
/// the list; every request is recorded for the test to assert on.
struct MockUpstream {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
    hits: Arc<AtomicUsize>,
    response_started: Arc<AtomicBool>,
    peer_closed: Arc<AtomicBool>,
    hanging_stop: Option<Arc<AtomicBool>>,
    hanging_worker: Option<std::thread::JoinHandle<()>>,
}

/// A bounded local endpoint that distinguishes a direct TLS connection from
/// an environment-proxy CONNECT without allowing either path onto the network.
struct ConnectionTrap {
    port: u16,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl ConnectionTrap {
    fn start(response: Option<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("connection trap binds");
        listener.set_nonblocking(true).expect("nonblocking trap");
        let port = listener.local_addr().expect("bound trap").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_hits = Arc::clone(&hits);
        let worker_stop = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(150);
            while !worker_stop.load(Ordering::SeqCst) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        worker_hits.fetch_add(1, Ordering::SeqCst);
                        stream
                            .set_nonblocking(false)
                            .expect("accepted trap stream becomes blocking");
                        stream
                            .set_read_timeout(Some(Duration::from_millis(250)))
                            .expect("bounded trap read");
                        let mut request = [0_u8; 4096];
                        let read_deadline = Instant::now() + Duration::from_secs(10);
                        loop {
                            match stream.read(&mut request) {
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) && Instant::now() < read_deadline => {}
                                Ok(_) | Err(_) => break,
                            }
                        }
                        if let Some(response) = &response {
                            let _ = stream.write_all(response);
                        }
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("connection trap accept failed: {error}"),
                }
            }
        });
        Self {
            port,
            hits,
            stop,
            worker: Some(worker),
        }
    }

    fn https_url(&self) -> String {
        format!("https://127.0.0.1:{}/v1", self.port)
    }

    fn http_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    fn finish(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("connection trap exits cleanly");
        }
    }
}

impl Drop for ConnectionTrap {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("connection trap exits cleanly");
        }
    }
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
        let response_started = Arc::new(AtomicBool::new(false));
        let peer_closed = Arc::new(AtomicBool::new(false));

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

        Self {
            port,
            seen,
            hits,
            response_started,
            peer_closed,
            hanging_stop: None,
            hanging_worker: None,
        }
    }

    /// Sends only the streaming response headers, then waits for the proxy to
    /// close the upstream socket. This is the real blocked-read case used by
    /// the drain/cancellation acceptance test.
    fn start_hanging() -> Self {
        Self::start_hanging_with_prefix(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
        )
    }

    /// Starts a valid buffered JSON response whose declared body never
    /// completes, so deadline cancellation can be observed after response
    /// headers have definitely been accepted.
    fn start_hanging_buffered() -> Self {
        Self::start_hanging_with_prefix(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1024\r\nconnection: close\r\n\r\n{",
        )
    }

    fn start_hanging_with_prefix(response_prefix: &'static [u8]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
        let port = listener.local_addr().expect("bound").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let hits = Arc::new(AtomicUsize::new(0));
        let response_started = Arc::new(AtomicBool::new(false));
        let peer_closed = Arc::new(AtomicBool::new(false));
        let hanging_stop = Arc::new(AtomicBool::new(false));
        let record = Arc::clone(&seen);
        let counter = Arc::clone(&hits);
        let started = Arc::clone(&response_started);
        let closed = Arc::clone(&peer_closed);
        let stop = Arc::clone(&hanging_stop);
        let hanging_worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("hanging upstream accepts");
            record
                .lock()
                .expect("recorder")
                .push(read_http_request(&mut stream));
            counter.fetch_add(1, Ordering::SeqCst);
            stream
                .write_all(response_prefix)
                .expect("response prefix writes");
            stream.flush().expect("response prefix flushes");
            started.store(true, Ordering::SeqCst);
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(100)))
                .expect("read timeout sets");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut byte = [0_u8; 1];
            while !stop.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                match stream.read(&mut byte) {
                    Ok(0) => {
                        closed.store(true, Ordering::SeqCst);
                        break;
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => {
                        closed.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
            let _ = stream.shutdown(Shutdown::Both);
        });
        Self {
            port,
            seen,
            hits,
            response_started,
            peer_closed,
            hanging_stop: Some(hanging_stop),
            hanging_worker: Some(hanging_worker),
        }
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

    fn response_started(&self) -> bool {
        self.response_started.load(Ordering::SeqCst)
    }

    fn peer_closed(&self) -> bool {
        self.peer_closed.load(Ordering::SeqCst)
    }

    fn finish_hanging(mut self) {
        if let Some(stop) = self.hanging_stop.take() {
            stop.store(true, Ordering::SeqCst);
        }
        if let Some(worker) = self.hanging_worker.take() {
            worker.join().expect("hanging upstream exits cleanly");
        }
    }
}

fn read_http_request(stream: &mut TcpStream) -> Seen {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let (mut header_end, mut content_length) = (None, 0usize);

    loop {
        if header_end.is_none()
            && let Some(position) = find(&buffer, b"\r\n\r\n")
        {
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
        if let Some(end) = header_end
            && buffer.len() >= end + content_length
        {
            break;
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
    let content_type = head.lines().find_map(|line| {
        line.to_ascii_lowercase()
            .starts_with("content-type:")
            .then(|| line.split_once(':').expect("header").1.trim().to_owned())
    });
    let body = serde_json::from_slice(&buffer[end..end + content_length]).unwrap_or(Value::Null);

    Seen {
        path,
        authorization,
        content_type,
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

fn http_json_with_headers(status: u16, body: &str, headers: &[(&str, &str)]) -> Vec<u8> {
    use std::fmt::Write as _;

    let mut extra = String::new();
    for (name, value) in headers {
        write!(&mut extra, "{name}: {value}\r\n").expect("String writes cannot fail");
    }
    format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn http_redirect(location: &str) -> Vec<u8> {
    http_json_with_headers(302, "redirect refused", &[("location", location)])
}

// -- the proxy under test ----------------------------------------------------------

/// A running proxy under test: where it listens, where it writes, and the
/// virtual key that opens its door.
struct Proxy {
    url: String,
    data_dir: PathBuf,
    virtual_key: String,
    control: server::ServerControl,
    gateway: Arc<Gateway>,
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
    start_proxy_with_agents(upstream, key_file, metrics, &[agent_plugin])
}

fn start_proxy_with_agents(
    upstream: &MockUpstream,
    key_file: &Path,
    metrics: bool,
    agent_plugins: &[&str],
) -> Proxy {
    start_proxy_with_agents_and_budgets(upstream, key_file, metrics, agent_plugins, None)
}

fn start_proxy_with_agents_and_budgets(
    upstream: &MockUpstream,
    key_file: &Path,
    metrics: bool,
    agent_plugins: &[&str],
    agent_budgets: Option<Value>,
) -> Proxy {
    start_proxy_with_agents_budgets_and_catalog_price(
        upstream,
        key_file,
        metrics,
        agent_plugins,
        agent_budgets,
        false,
    )
}

fn start_proxy_with_agents_budgets_and_catalog_price(
    upstream: &MockUpstream,
    key_file: &Path,
    metrics: bool,
    agent_plugins: &[&str],
    agent_budgets: Option<Value>,
    include_catalog_price: bool,
) -> Proxy {
    start_proxy_with_agents_budgets_catalog_price_and_parameters(
        upstream,
        key_file,
        metrics,
        agent_plugins,
        agent_budgets,
        include_catalog_price,
        None,
    )
}

fn start_proxy_with_agent_and_supported_parameters(
    upstream: &MockUpstream,
    key_file: &Path,
    metrics: bool,
    agent_plugin: &str,
    supported_parameters: Value,
) -> Proxy {
    start_proxy_with_agents_budgets_catalog_price_and_parameters(
        upstream,
        key_file,
        metrics,
        &[agent_plugin],
        None,
        false,
        Some(supported_parameters),
    )
}

fn start_proxy_with_agents_budgets_catalog_price_and_parameters(
    upstream: &MockUpstream,
    key_file: &Path,
    metrics: bool,
    agent_plugins: &[&str],
    agent_budgets: Option<Value>,
    include_catalog_price: bool,
    supported_parameters: Option<Value>,
) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-proxy-data-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let mut config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": data_dir, "metrics": metrics },
        "plugins": {
            "dir": plugins_dir(),
            "agents": agent_plugins,
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "mock_primary": {
                "provider": "openai-compatible",
                "base_url": upstream.base_url(),
                "auth": { "slot": "provider_api_key", "file": key_file },
                "models": [
                    { "model": "gpt-5.5", "tool": true, "tool_state": "verified", "vision": true, "vision_state": "verified", "json_schema": true, "json_schema_state": "declared", "context_window": 400_000, "max_output_tokens": 32_768 }
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
    if let Some(agent_budgets) = agent_budgets {
        config["agent_budgets"] = agent_budgets;
    }
    if let Some(supported_parameters) = supported_parameters {
        config["upstreams"]["mock_primary"]["models"][0]["supported_parameters"] =
            supported_parameters;
    }
    if include_catalog_price {
        config["pricing"] = json!({
            "version": 7,
            "models": {
                "mock_primary/gpt-5.5": {
                    "input_per_mtok": 5_000_000,
                    "output_per_mtok": 30_000_000,
                    "cache_read_per_mtok": 500_000,
                    "cache_write_per_mtok": 0
                }
            }
        });
    }
    let config: ClientConfig = serde_json::from_value(config).expect("test config parses");
    spawn_proxy(&config)
}

/// The shared server-spawn tail: recorders, gateway, virtual key, and a
/// background server bound to a loopback port.
fn spawn_proxy(config: &ClientConfig) -> Proxy {
    let data_dir = config.data.dir.clone();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio builds");
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
    let gateway = Arc::new(
        Gateway::new_with_provider_runtime(config, recorder, runtime.handle().clone())
            .expect("gateway assembles"),
    );

    // Auth on, exactly as a real first start would set it up.
    let (virtual_key, created) =
        token_station_cli::virtual_key::load_or_create(&config.data.dir).expect("key creates");
    assert!(created, "each test gets a fresh data dir");
    let state = server::AppState::new(
        Arc::clone(&gateway),
        Some(Arc::from(virtual_key.as_str())),
        Arc::new(
            token_station_cli::admin::AdminContext::from_config(config)
                .expect("admin snapshot compiles"),
        ),
    );
    let control = state.control.clone();

    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
    listener.set_nonblocking(true).expect("nonblocking");
    let address = listener.local_addr().expect("bound");

    std::thread::spawn(move || {
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("listener converts");
            server::serve(state, listener).await.expect("server runs");
        });
    });

    Proxy {
        url: format!("http://{address}"),
        data_dir,
        virtual_key,
        control,
        gateway,
    }
}

/// A proxy whose single upstream is marked `api_dialect: anthropic-native`, so an
/// anthropic-messages request is forwarded verbatim to the mock instead of being
/// lowered through the Canonical IR.
fn start_native_anthropic_proxy(upstream: &MockUpstream, key_file: &Path) -> Proxy {
    start_native_anthropic_proxy_with(upstream, key_file, false, "verified")
}

fn start_native_anthropic_proxy_with(
    upstream: &MockUpstream,
    key_file: &Path,
    direct: bool,
    tool_state: &str,
) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-native-proxy-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let mut config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": data_dir, "metrics": true },
        "plugins": {
            "dir": plugins_dir(),
            "agents": ["agent-anthropic"],
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "deepseek_native": {
                "provider": "openai-compatible",
                "api_dialect": "anthropic-native",
                "base_url": upstream.base_url(),
                "auth": { "slot": "provider_api_key", "file": key_file },
                "models": [
                    {
                        "model": "deepseek-chat",
                        "tool": tool_state != "unsupported",
                        "tool_state": tool_state,
                        "context_window": 128_000
                    }
                ]
            }
        },
        "router": {
            "version": 1,
            "pools": { "main": [ { "upstream": "deepseek_native", "model": "deepseek-chat" } ] },
            "default_pool": "main"
        }
    });
    if direct {
        config["routing"] = json!({
            "mode": "direct",
            "direct_target": { "upstream": "deepseek_native", "model": "deepseek-chat" }
        });
    }
    let config: ClientConfig = serde_json::from_value(config).expect("native config parses");
    spawn_proxy(&config)
}

fn start_scoped_proxy(home: &MockUpstream, custom: &MockUpstream, key_file: &Path) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-scoped-proxy-data-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let tiers = |upstream: &str, model: &str| {
        json!({
            "high": { "upstream": upstream, "model": model },
            "mid": { "upstream": upstream, "model": model },
            "low": { "upstream": upstream, "model": model }
        })
    };
    let config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": data_dir, "metrics": true },
        "plugins": {
            "dir": plugins_dir(),
            "agents": ["agent-openai", "agent-anthropic", "agent-openai-responses"],
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "home_upstream": {
                "provider": "openai-compatible",
                "base_url": home.base_url(),
                "auth": { "slot": "provider_api_key", "file": key_file },
                "models": [{ "model": "home-model", "tool": true, "context_window": 128_000 }]
            },
            "agent_upstream": {
                "provider": "openai-compatible",
                "base_url": custom.base_url(),
                "auth": { "slot": "provider_api_key", "file": key_file },
                "models": [{ "model": "agent-model", "tool": true, "context_window": 128_000 }]
            }
        },
        "router": {
            "version": 1,
            "pools": {
                "tier_high": [{ "upstream": "home_upstream", "model": "home-model" }],
                "tier_mid": [{ "upstream": "home_upstream", "model": "home-model" }],
                "tier_low": [{ "upstream": "home_upstream", "model": "home-model" }]
            },
            "default_pool": "tier_low"
        },
        "agent_routes": {
            "codex": {
                "mode": "custom",
                "custom_route": tiers("agent_upstream", "agent-model")
            },
            "opencode": {
                "mode": "inherit",
                "custom_route": tiers("agent_upstream", "agent-model")
            }
        }
    });
    let config: ClientConfig = serde_json::from_value(config).expect("scoped config parses");
    let recorder = Arc::new(token_station_cli::filelog::Recorders(vec![Box::new(
        token_station_cli::filelog::FileLog::open(&config.data.dir).expect("log opens"),
    )]));
    let gateway = Arc::new(Gateway::new(&config, recorder).expect("scoped gateway assembles"));
    let (virtual_key, created) =
        token_station_cli::virtual_key::load_or_create(&config.data.dir).expect("key creates");
    assert!(created);
    let state = server::AppState::new(
        Arc::clone(&gateway),
        Some(Arc::from(virtual_key.as_str())),
        Arc::new(
            token_station_cli::admin::AdminContext::from_config(&config)
                .expect("admin snapshot compiles"),
        ),
    );
    let control = state.control.clone();
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
        control,
        gateway,
    }
}

fn start_proxy_with_missing_store_secret(upstream: &MockUpstream) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-missing-store-secret-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": data_dir, "metrics": false },
        "plugins": {
            "dir": plugins_dir(),
            "agents": ["agent-openai-responses"],
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "deepseek_release": {
                "provider": "openai-compatible",
                "base_url": upstream.base_url(),
                "auth": { "slot": "provider_api_key", "store": true },
                "models": [
                    { "model": "deepseek-v4-pro", "tool": true, "tool_state": "verified", "context_window": 128_000 }
                ]
            }
        },
        "router": {
            "version": 1,
            "pools": { "main": [ { "upstream": "deepseek_release", "model": "deepseek-v4-pro" } ] },
            "default_pool": "main"
        }
    });
    let config: ClientConfig = serde_json::from_value(config).expect("missing store config parses");
    spawn_proxy(&config)
}

/// One row out of the metrics store, as (column -> debug-rendered value).
fn last_row(data_dir: &Path) -> std::collections::BTreeMap<String, String> {
    let db = rusqlite::Connection::open(data_dir.join("metrics.sqlite")).expect("db opens");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match db.query_row(
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
        ) {
            Ok(row) => return row,
            Err(rusqlite::Error::QueryReturnedNoRows) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(error) => panic!("a row exists: {error}"),
        }
    }
}

fn request_count(data_dir: &Path) -> i64 {
    let db = rusqlite::Connection::open(data_dir.join("metrics.sqlite")).expect("db opens");
    db.query_row("SELECT COUNT(*) FROM requests", [], |row| row.get(0))
        .expect("request count reads")
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

fn post_chat_stream(proxy: &Proxy) -> (u16, String) {
    post_chat(
        proxy,
        &json!({
            "model": "auto",
            "stream": true,
            "messages": [{"role": "user", "content": "stream"}]
        }),
        None,
    )
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

fn post_gemini(proxy: &Proxy, body: &Value, token: &str) -> (u16, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let response = agent
        .post(format!(
            "{}/agents/gemini-cli/v1beta/models/gemini-2.5-pro:generateContent",
            proxy.url
        ))
        .header("authorization", &format!("Bearer {token}"))
        .send(&body.to_string())
        .expect("the proxy answers");
    let status = response.status().as_u16();
    let body = response.into_body().read_to_string().expect("body reads");
    (status, body)
}

fn post_scoped(
    proxy: &Proxy,
    path: &str,
    body: &Value,
    token: &str,
    anthropic: bool,
) -> (u16, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let mut request = agent
        .post(format!("{}{path}", proxy.url))
        .header("authorization", &format!("Bearer {token}"));
    if anthropic {
        request = request.header("anthropic-version", "2023-06-01");
    }
    let response = request
        .send(&body.to_string())
        .expect("the scoped proxy answers");
    let status = response.status().as_u16();
    let body = response.into_body().read_to_string().expect("body reads");
    (status, body)
}

fn sse_events(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter_map(|frame| {
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .map(|data| serde_json::from_str(data).expect("SSE data is JSON"))
        })
        .collect()
}

fn assert_responses_terminal_is_unique_and_last(events: &[Value], body: &str) {
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "response.created")
            .count(),
        1,
        "one logical stream has one response.created: {body}"
    );
    assert_eq!(
        events.last().map(|event| &event["type"]),
        Some(&json!("response.completed")),
        "response.completed is terminal: {body}"
    );
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

fn namespace_read_marker_tool() -> Value {
    json!({
        "type": "namespace",
        "name": "workspace_v1",
        "description": "Read workspace files.",
        "tools": [{
            "type": "function",
            "name": "read_marker",
            "description": "Read one marker.",
            "strict": true,
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            }
        }]
    })
}

fn assert_namespace_stream_events(events: &[Value]) {
    let added: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.output_item.added")
        .map(|event| {
            (
                event["output_index"].as_u64().unwrap(),
                event["item"]["type"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(added, vec![(0, "message"), (1, "function_call")]);
    let added_call = events
        .iter()
        .find(|event| {
            event["type"] == "response.output_item.added"
                && event["item"]["type"] == "function_call"
        })
        .expect("streamed function call is added");
    assert_eq!(added_call["item"]["namespace"], json!("workspace_v1"));
    assert_eq!(added_call["item"]["name"], json!("read_marker"));
    let function_delta_indices: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.function_call_arguments.delta")
        .map(|event| event["output_index"].as_u64().unwrap())
        .collect();
    assert_eq!(function_delta_indices, vec![1, 1]);
    let text_delta_indices: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.output_text.delta")
        .map(|event| event["output_index"].as_u64().unwrap())
        .collect();
    assert_eq!(text_delta_indices, vec![0, 0]);
    let done_indices: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "response.output_item.done")
        .map(|event| event["output_index"].as_u64().unwrap())
        .collect();
    assert_eq!(done_indices, vec![0, 1]);
    let completed = events
        .iter()
        .find(|event| event["type"] == "response.completed")
        .expect("completed event");
    assert_eq!(completed["response"]["output"][0]["type"], json!("message"));
    assert_eq!(
        completed["response"]["output"][1]["type"],
        json!("function_call")
    );
    assert_eq!(
        completed["response"]["output"][1]["namespace"],
        json!("workspace_v1")
    );
    assert_eq!(
        completed["response"]["output"][1]["name"],
        json!("read_marker")
    );
}

const EGRESS_CHILD_MODE: &str = "TOKEN_STATION_EGRESS_CHILD_MODE";
const EGRESS_TARGET_URL: &str = "TOKEN_STATION_EGRESS_TARGET_URL";
const EGRESS_PROXY_URL: &str = "TOKEN_STATION_EGRESS_PROXY_URL";
const EGRESS_KEY_FILE: &str = "TOKEN_STATION_EGRESS_KEY_FILE";
const EGRESS_PLUGIN_DIR: &str = "TOKEN_STATION_EGRESS_PLUGIN_DIR";
const EGRESS_CHILD_TIMEOUT: Duration = Duration::from_mins(2);
const PROXY_ENV_VARS: [&str; 8] = [
    "ALL_PROXY",
    "all_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "NO_PROXY",
    "no_proxy",
];

fn run_https_proxy_child(
    mode: &str,
    target_url: &str,
    proxy_url: &str,
    key_file: &Path,
    plugin_dir: &Path,
) -> (Vec<u8>, Vec<u8>) {
    let mut command = Command::new(std::env::current_exe().expect("test executable is known"));
    command
        .args([
            "--exact",
            "egress_https_proxy_child",
            "--ignored",
            "--nocapture",
        ])
        .env(EGRESS_CHILD_MODE, mode)
        .env(EGRESS_TARGET_URL, target_url)
        .env(EGRESS_PROXY_URL, proxy_url)
        .env(EGRESS_KEY_FILE, key_file)
        .env(EGRESS_PLUGIN_DIR, plugin_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in PROXY_ENV_VARS {
        command.env_remove(name);
    }
    for name in &PROXY_ENV_VARS[..6] {
        command.env(name, proxy_url);
    }

    let mut child = command.spawn().expect("isolated proxy test child starts");
    let mut stdout = child.stdout.take().expect("child stdout is piped");
    let mut stderr = child.stderr.take().expect("child stderr is piped");
    let stdout_worker = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).expect("child stdout reads");
        bytes
    });
    let stderr_worker = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).expect("child stderr reads");
        bytes
    });

    let deadline = Instant::now() + EGRESS_CHILD_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("child status polls") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("timed out child is killed");
            let _ = child.wait();
            let stdout = stdout_worker.join().expect("stdout reader exits");
            let stderr = stderr_worker.join().expect("stderr reader exits");
            panic!(
                "isolated {mode} proxy child timed out\nstdout={}\nstderr={}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let stdout = stdout_worker.join().expect("stdout reader exits");
    let stderr = stderr_worker.join().expect("stderr reader exits");
    assert!(
        status.success(),
        "isolated {mode} proxy child failed\nstdout={}\nstderr={}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    (stdout, stderr)
}

#[test]
fn production_and_probe_ignore_all_proxy_environment_variants() {
    // Build plugins before injecting proxy variables into the child, so Cargo
    // itself never participates in this network-path assertion.
    let plugin_dir = plugins_dir().to_path_buf();

    for mode in ["production", "probe"] {
        let target = ConnectionTrap::start(None);
        let proxy = ConnectionTrap::start(Some(
            b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
        ));
        let target_url = target.https_url();
        let proxy_url = proxy.http_url();
        let key_value = format!("sk-https-proxy-{mode}");
        let key = key_file(&format!("https-proxy-{mode}"), &key_value);

        let (stdout, stderr) =
            run_https_proxy_child(mode, &target_url, &proxy_url, &key, &plugin_dir);

        assert_eq!(target.hits(), 1, "{mode} connects directly to its target");
        assert_eq!(proxy.hits(), 0, "{mode} must ignore HTTPS_PROXY");
        for output in [&stdout, &stderr] {
            assert!(
                !String::from_utf8_lossy(output).contains(&key_value),
                "{mode} child output must not disclose its credential"
            );
        }

        target.finish();
        proxy.finish();
        std::fs::remove_file(key).ok();
    }
}

#[test]
#[ignore = "spawned by its parent to isolate proxy environment"]
fn egress_https_proxy_child() {
    let mode = std::env::var(EGRESS_CHILD_MODE).expect("child mode is provided");
    let target_url = std::env::var(EGRESS_TARGET_URL).expect("child target is provided");
    let expected_proxy = std::env::var(EGRESS_PROXY_URL).expect("child proxy is provided");
    let key_file = PathBuf::from(std::env::var(EGRESS_KEY_FILE).expect("child key is provided"));
    let plugin_dir =
        PathBuf::from(std::env::var(EGRESS_PLUGIN_DIR).expect("child plugins are provided"));
    for name in &PROXY_ENV_VARS[..6] {
        assert_eq!(
            std::env::var(name).as_deref(),
            Ok(expected_proxy.as_str()),
            "the isolated child must inherit hostile {name}"
        );
    }

    let gateway = gateway_for_base_url(&target_url, &key_file, &plugin_dir);
    match mode.as_str() {
        "production" => {
            let body = json!({
                "model": "auto",
                "messages": [{"role": "user", "content": "hi"}]
            })
            .to_string();
            let mut result = None;
            gateway.chat(
                "POST",
                "/v1/chat/completions",
                &[],
                body.as_bytes(),
                &mut |reply| {
                    if let Reply::BeginJson(reply) = reply {
                        result = Some((reply.status, reply.body));
                    }
                    true
                },
            );
            let (status, response_body) = result.expect("production emits a JSON refusal");
            assert_eq!(status, 502, "{response_body}");
        }
        "probe" => {
            let outcomes = gateway
                .probe("mock_primary", None)
                .expect("probe reaches the transport boundary");
            let error = outcomes[0]
                .latency_ms
                .as_ref()
                .expect_err("the local TLS trap cannot be a healthy provider");
            assert!(error.contains("upstream"), "{error}");
        }
        other => panic!("unknown isolated child mode `{other}`"),
    }
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
            "messages": [{ "role": "user", "content": "what is six times seven" }],
            "tools": [{
                "type": "function",
                "function": {"name": "calculator", "parameters": {"type": "object"}}
            }],
            "metadata": {"type": "audio", "modalities": ["audio"]}
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
    assert_eq!(seen[0].body["tools"][0]["function"]["name"], "calculator");

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
fn redirects_to_other_hosts_loopback_and_metadata_never_receive_a_second_hop() {
    let canary = MockUpstream::start(Vec::new());
    // The metadata lure keeps the canonical path and IP literal in the URL,
    // but resolves to our local canary. A redirect regression therefore fails
    // deterministically without ever opening a socket to real link-local
    // metadata infrastructure.
    let locations = [
        (
            "other-host",
            format!("http://localhost:{}/credential-canary", canary.port),
        ),
        (
            "loopback",
            format!("http://127.0.0.1:{}/credential-canary", canary.port),
        ),
        (
            "metadata",
            format!(
                "http://127.0.0.1:{}/latest/meta-data/iam/security-credentials/?original-host=169.254.169.254",
                canary.port
            ),
        ),
    ];

    for (label, location) in locations {
        let source = MockUpstream::start(vec![vec![http_redirect(&location)]]);
        let key_value = format!("sk-redirect-{label}");
        let key = key_file(&format!("redirect-{label}"), &key_value);
        let proxy = start_proxy(&source, &key);

        let (status, body) = post_chat(
            &proxy,
            &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
            None,
        );

        assert_eq!(status, 502, "redirect must fail for {label}: {body}");
        assert!(body.contains("upstream_unavailable"), "{body}");
        assert_eq!(source.hits(), 1, "the authorized first hop is called once");
        assert_eq!(canary.hits(), 0, "{label} target must not be contacted");
        let _receipt = last_row(&proxy.data_dir);
        let db = rusqlite::Connection::open(proxy.data_dir.join("metrics.sqlite"))
            .expect("receipt db opens");
        let http_status: Option<u16> = db
            .query_row("SELECT http_status FROM attempts", [], |row| row.get(0))
            .expect("redirect attempt exists");
        assert_eq!(
            http_status,
            Some(302),
            "the refused redirect keeps its raw upstream status"
        );
        assert_eq!(
            source.seen()[0].authorization.as_deref(),
            Some(format!("Bearer {key_value}").as_str())
        );
        assert!(!body.contains(&key_value));
        assert!(!body.contains(&location));

        std::fs::remove_file(key).ok();
    }
}

#[test]
fn a_hung_upstream_is_force_cancelled_after_the_five_second_grace_and_returns_503() {
    let mock = MockUpstream::start_hanging();
    let key = key_file("hung-drain", "sk-test-key-abc\n");
    let proxy = start_proxy(&mock, &key);
    let url = proxy.url.clone();
    let virtual_key = proxy.virtual_key.clone();
    let client = std::thread::spawn(move || {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(15)))
                .http_status_as_error(false)
                .build(),
        );
        let response = agent
            .post(format!("{url}/v1/chat/completions"))
            .header("authorization", &format!("Bearer {virtual_key}"))
            .send(
                &json!({
                    "model": "auto",
                    "stream": true,
                    "messages": [{"role": "user", "content": "hang"}]
                })
                .to_string(),
            )
            .expect("proxy sends stream headers");
        let status = response.status().as_u16();
        let _ = response.into_body().read_to_string();
        status
    });

    let arrival_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while (mock.hits() == 0 || proxy.control.in_flight() == 0)
        && std::time::Instant::now() < arrival_deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(mock.hits(), 1, "the hanging exchange reached its upstream");
    assert_eq!(proxy.control.in_flight(), 1, "one request is in flight");

    proxy.control.stop_accepting();
    let grace_started = std::time::Instant::now();
    std::thread::sleep(std::time::Duration::from_secs(5));
    assert_eq!(
        proxy.control.in_flight(),
        1,
        "the blocked request remains alive throughout the approved grace"
    );
    proxy.control.cancel_in_flight();

    let cleanup_deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while (proxy.control.in_flight() != 0 || !mock.peer_closed())
        && std::time::Instant::now() < cleanup_deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(grace_started.elapsed() >= std::time::Duration::from_secs(5));
    assert_eq!(
        proxy.control.in_flight(),
        0,
        "the gateway worker exits after cancel"
    );
    assert!(
        mock.peer_closed(),
        "the cancelled worker closes the upstream socket"
    );
    assert_eq!(
        client.join().expect("client joins"),
        503,
        "an uncommitted stream cancelled by server drain is explicitly retryable"
    );

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(503)");
    assert_eq!(row["error_code"], "Text(\"upstream_unavailable\")");
    std::fs::remove_file(key).ok();
}

#[test]
fn a_server_drained_non_stream_body_returns_503_without_hanging() {
    let mock = MockUpstream::start_hanging();
    let key = key_file("hung-json-cancel", "sk-test-key-abc\n");
    let proxy = start_proxy(&mock, &key);
    let url = proxy.url.clone();
    let virtual_key = proxy.virtual_key.clone();
    let client = std::thread::spawn(move || {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(std::time::Duration::from_secs(5)))
                .http_status_as_error(false)
                .build(),
        );
        agent
            .post(format!("{url}/v1/chat/completions"))
            .header("authorization", &format!("Bearer {virtual_key}"))
            .send(
                &json!({
                    "model": "auto",
                    "messages": [{"role": "user", "content": "hang"}]
                })
                .to_string(),
            )
            .expect("proxy answers the cancelled request")
            .status()
            .as_u16()
    });

    let arrival_deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while (mock.hits() == 0 || proxy.control.in_flight() == 0)
        && std::time::Instant::now() < arrival_deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(mock.hits(), 1);
    proxy.control.cancel_in_flight();
    assert_eq!(client.join().expect("client joins"), 503);
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(503)");
    assert_eq!(row["error_code"], "Text(\"upstream_unavailable\")");
    std::fs::remove_file(key).ok();
}

#[test]
fn all_four_inbound_protocols_coexist_in_declared_match_order() {
    let upstream_answer = json!({
        "id": "chatcmpl-shared",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "OK"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("four-inbound-agents", "sk-test-key-abc");
    let proxy = start_proxy_with_agents(
        &mock,
        &key,
        true,
        &[
            "agent-openai",
            "agent-anthropic",
            "agent-openai-responses",
            "agent-gemini",
        ],
    );

    let (chat_status, _) = post_chat(
        &proxy,
        &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
        None,
    );
    let token = proxy.virtual_key.clone();
    let (responses_status, _, _) =
        send_responses(&proxy, &json!({"model": "auto", "input": "hi"}), &token);
    let (messages_status, _) = post_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hi"}]
        }),
        &token,
    );
    let (gemini_status, gemini_body) = post_gemini(
        &proxy,
        &json!({"contents": [{"role": "user", "parts": [{"text": "hi"}]}]}),
        &token,
    );

    assert_eq!(
        (
            chat_status,
            responses_status,
            messages_status,
            gemini_status
        ),
        (200, 200, 200, 200)
    );
    assert_eq!(
        serde_json::from_str::<Value>(&gemini_body).unwrap()["candidates"][0]["content"]["parts"]
            [0]["text"],
        "OK"
    );
    assert_eq!(mock.hits(), 4);
    std::fs::remove_file(key).ok();
}

#[test]
fn exceeded_agent_budget_is_observe_only_and_never_blocks_routing() {
    let answer = json!({
        "id": "chatcmpl-budget-observe-only",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "still routed" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("budget-observe-only", "sk-test-key-abc");
    let proxy = start_proxy_with_agents_and_budgets(
        &mock,
        &key,
        true,
        &["agent-openai-responses"],
        Some(json!({
            "codex": {
                "limit_micros": 1,
                "warning_percent": 1
            }
        })),
    );

    let token = proxy.virtual_key.clone();
    let (status, body) = post_scoped(
        &proxy,
        "/agents/codex/v1/responses",
        &json!({ "model": "auto", "input": "hi", "stream": false }),
        &token,
        false,
    );

    assert_eq!(
        status, 200,
        "a display-only budget cannot reject requests: {body}"
    );
    assert_eq!(mock.hits(), 1, "the request must still reach the provider");
    std::fs::remove_file(key).ok();
}

#[test]
fn agent_namespaces_select_custom_or_inherited_routers_and_strip_paths() {
    let answer = |id: &str, model: &str| {
        json!({
            "id": id,
            "model": model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "OK" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
        })
    };
    let home = MockUpstream::start(vec![vec![http_json(
        200,
        &answer("chatcmpl-home", "home-model").to_string(),
    )]]);
    let custom = MockUpstream::start(vec![vec![http_json(
        200,
        &answer("chatcmpl-agent", "agent-model").to_string(),
    )]]);
    let key = key_file("scoped-routing", "sk-test-key-abc");
    let proxy = start_scoped_proxy(&home, &custom, &key);
    let token = proxy.virtual_key.clone();

    let responses = json!({ "model": "auto", "input": "hi", "stream": false });
    let (status, _) = post_scoped(&proxy, "/v1/responses", &responses, &token, false);
    assert_eq!(status, 200);
    let (status, _) = post_scoped(
        &proxy,
        "/agents/codex/v1/responses",
        &responses,
        &token,
        false,
    );
    assert_eq!(status, 200);

    let chat = json!({
        "model": "auto",
        "messages": [{ "role": "user", "content": "hi" }]
    });
    for agent_id in [
        "opencode",
        "openclaw",
        "nous-hermes-agent",
        "workbuddy",
        "cursor",
        "grok-build",
        "kimi-code",
        "deepseek-harness",
    ] {
        let (status, _) = post_scoped(
            &proxy,
            &format!("/agents/{agent_id}/v1/chat/completions"),
            &chat,
            &token,
            false,
        );
        assert_eq!(status, 200, "{agent_id}");
    }
    let messages = json!({
        "model": "auto",
        "max_tokens": 16,
        "messages": [{ "role": "user", "content": "hi" }]
    });
    let (status, _) = post_scoped(
        &proxy,
        "/agents/claude-code/v1/messages",
        &messages,
        &token,
        true,
    );
    assert_eq!(status, 200);
    let mut claude_desktop_messages = messages.clone();
    claude_desktop_messages["model"] = json!("claude-sonnet-4-6");
    let (status, _) = post_scoped(
        &proxy,
        "/agents/claude-desktop/v1/messages",
        &claude_desktop_messages,
        &token,
        true,
    );
    assert_eq!(status, 200);

    assert_eq!(custom.hits(), 1, "only Codex uses its custom route");
    assert_eq!(home.hits(), 11, "home plus ten inherited Agent requests");
    assert!(
        custom
            .seen()
            .iter()
            .all(|seen| seen.body["model"] == "agent-model")
    );
    assert!(
        home.seen()
            .iter()
            .all(|seen| seen.body["model"] == "home-model")
    );

    std::fs::remove_file(key).ok();
}

#[test]
fn claude_desktop_discovers_only_the_token_station_compatibility_alias() {
    let answer = json!({
        "id": "chatcmpl-unused",
        "model": "home-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "unused" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let home = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let custom = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("claude-desktop-models", "sk-test-key-abc");
    let proxy = start_scoped_proxy(&home, &custom, &key);
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );

    let response = agent
        .get(format!(
            "{}/agents/claude-desktop/v1/models?limit=1000",
            proxy.url
        ))
        .header("authorization", &format!("Bearer {}", proxy.virtual_key))
        .call()
        .expect("Claude Desktop model discovery answers");
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = serde_json::from_str(
        &response
            .into_body()
            .read_to_string()
            .expect("model catalog body reads"),
    )
    .expect("model catalog is JSON");

    assert_eq!(body["object"], json!("list"));
    assert_eq!(body["data"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["data"][0]["id"], json!("claude-sonnet-4-6"));
    assert_eq!(body["data"][0]["display_name"], json!("Token Station Auto"));
    assert_eq!(body["data"][0]["anthropic_family_tier"], json!("sonnet"));
    assert_eq!(body["data"][0]["is_family_default"], json!(true));
    assert_eq!(home.hits(), 0, "model discovery stays local");
    assert_eq!(custom.hits(), 0, "model discovery stays local");

    std::fs::remove_file(key).ok();
}

#[test]
fn scoped_models_auth_and_unknown_namespaces_fail_closed() {
    let answer = json!({
        "id": "chatcmpl-unused",
        "model": "home-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "unused" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let home = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let custom = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("scoped-boundaries", "sk-test-key-abc");
    let proxy = start_scoped_proxy(&home, &custom, &key);
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );

    let model_ids = |path: &str| {
        let models = agent
            .get(format!("{}{}", proxy.url, path))
            .header("authorization", &format!("Bearer {}", proxy.virtual_key))
            .call()
            .expect("models endpoint answers");
        assert_eq!(models.status().as_u16(), 200);
        let models: Value = serde_json::from_str(
            &models
                .into_body()
                .read_to_string()
                .expect("models body reads"),
        )
        .expect("models body is JSON");
        models["data"]
            .as_array()
            .expect("models data is an array")
            .iter()
            .map(|model| model["id"].as_str().expect("model id").to_owned())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        model_ids("/agents/opencode/v1/models"),
        std::collections::BTreeSet::from(["home-model".to_owned()])
    );
    assert_eq!(
        model_ids("/agents/codex/v1/models"),
        std::collections::BTreeSet::from(["agent-model".to_owned()])
    );
    assert_eq!(
        model_ids("/v1/models"),
        std::collections::BTreeSet::from(["home-model".to_owned()])
    );

    let wrong_key = agent
        .get(format!("{}/agents/claude-desktop/v1/models", proxy.url))
        .header("authorization", "Bearer wrong-local-key")
        .call()
        .expect("wrong-key model discovery answers");
    assert_eq!(wrong_key.status().as_u16(), 401);

    let unknown_models = agent
        .get(format!("{}/agents/future/v1/models", proxy.url))
        .header("authorization", &format!("Bearer {}", proxy.virtual_key))
        .call()
        .expect("unknown model namespace answers");
    assert_eq!(unknown_models.status().as_u16(), 404);

    let responses = json!({ "model": "auto", "input": "hi", "stream": false });
    let (status, body) = post_scoped(
        &proxy,
        "/agents/codex/v1/responses",
        &responses,
        "wrong-local-key",
        false,
    );
    assert_eq!(status, 401, "{body}");
    let body: Value = serde_json::from_str(&body).expect("Responses auth error is JSON");
    assert_eq!(body["error"]["code"], "authentication_error");

    let (status, body) = post_scoped(
        &proxy,
        "/agents/future/v1/responses",
        &responses,
        &proxy.virtual_key,
        false,
    );
    assert_eq!(status, 404, "{body}");
    assert_eq!(home.hits(), 0);
    assert_eq!(custom.hits(), 0);

    std::fs::remove_file(key).ok();
}

#[test]
fn scoped_models_switch_on_the_next_request_after_router_reload() {
    let answer = json!({
        "id": "chatcmpl-unused",
        "model": "home-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "unused" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let home = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let custom = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("scoped-model-reload", "sk-test-key-abc");
    let proxy = start_scoped_proxy(&home, &custom, &key);
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let model_ids = || {
        let response = agent
            .get(format!("{}/agents/opencode/v1/models", proxy.url))
            .header("authorization", &format!("Bearer {}", proxy.virtual_key))
            .call()
            .expect("scoped model discovery answers");
        assert_eq!(response.status().as_u16(), 200);
        let document: Value = serde_json::from_str(
            &response
                .into_body()
                .read_to_string()
                .expect("model discovery body reads"),
        )
        .expect("model discovery body is JSON");
        document["data"]
            .as_array()
            .expect("model data is an array")
            .iter()
            .map(|model| model["id"].as_str().expect("model id").to_owned())
            .collect::<std::collections::BTreeSet<_>>()
    };

    assert_eq!(
        model_ids(),
        std::collections::BTreeSet::from(["home-model".to_owned()])
    );
    let replacement = serde_json::from_value(json!({
        "version": 1,
        "pools": {
            "tier_high": [{"upstream": "agent_upstream", "model": "agent-model"}],
            "tier_mid": [{"upstream": "agent_upstream", "model": "agent-model"}],
            "tier_low": [{"upstream": "agent_upstream", "model": "agent-model"}]
        },
        "default_pool": "tier_low"
    }))
    .expect("replacement Agent router parses");
    proxy
        .gateway
        .reload_agent_router("opencode", Some(replacement))
        .expect("Agent router reloads");
    assert_eq!(
        model_ids(),
        std::collections::BTreeSet::from(["agent-model".to_owned()])
    );

    std::fs::remove_file(key).ok();
}

#[test]
fn codex_missing_provider_key_names_the_selected_upstream_without_leaking_a_value() {
    let mock = MockUpstream::start(Vec::new());
    let proxy = start_proxy_with_missing_store_secret(&mock);
    let (status, body) = post_scoped(
        &proxy,
        "/agents/codex/v1/responses",
        &json!({ "model": "auto", "input": "hi", "stream": false }),
        &proxy.virtual_key,
        false,
    );

    assert_eq!(status, 401, "{body}");
    assert!(body.contains("deepseek_release"), "{body}");
    assert!(body.contains("provider_api_key"), "{body}");
    assert!(body.contains("re-enter the key"), "{body}");
    assert!(!body.contains("Bearer"), "{body}");
    assert!(!body.contains("sk-"), "{body}");
    assert_eq!(mock.hits(), 0, "missing auth must stop before the provider");
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
            "stream": false,
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "previous_response_id": null,
            "reasoning": {}
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
fn responses_structured_output_round_trips_through_the_provider() {
    let answer = json!({
        "id": "chatcmpl-structured",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "{\"answer\":true}"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 4, "completion_tokens": 3}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
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

    assert_eq!(status, 200, "{body}");
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let body: Value = serde_json::from_str(&body).expect("the response is JSON");
    assert_eq!(body["status"], json!("completed"));
    assert_eq!(
        body["output"][0]["content"][0]["text"],
        json!("{\"answer\":true}")
    );
    let seen = mock.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].body["response_format"]["type"],
        json!("json_schema")
    );
    assert_eq!(
        seen[0].body["response_format"]["json_schema"]["name"],
        json!("answer")
    );
    assert_eq!(
        seen[0].body["response_format"]["json_schema"]["strict"],
        json!(true)
    );

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(200)");
    assert_eq!(row["error_code"], "Null");
    assert_eq!(row["attempts"], "Integer(1)");
    assert_eq!(row["upstream"], "Text(\"mock_primary\")");

    std::fs::remove_file(key).ok();
}

#[test]
fn an_unknown_responses_continuation_fails_explicitly_before_the_upstream() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("responses-semantic-options", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();

    let request = json!({
        "model": "auto",
        "input": "hi",
        "previous_response_id": "resp_previous"
    });
    let (status, content_type, body) = send_responses(&proxy, &request, &token);

    assert_eq!(status, 400, "request={request} body={body}");
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let body: Value = serde_json::from_str(&body).expect("the refusal is JSON");
    assert_eq!(body["error"]["code"], json!("invalid_request"));
    assert!(
        body["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("continuation_expired")),
        "{body}"
    );
    assert_eq!(mock.hits(), 0, "expired continuations stop before routing");
    std::fs::remove_file(key).ok();
}

#[test]
fn responses_previous_response_id_replays_history_through_the_provider_pipeline() {
    let first_answer = json!({
        "id": "chatcmpl-continuation-1",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "first answer"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 2}
    });
    let second_answer = json!({
        "id": "chatcmpl-continuation-2",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "second answer"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 8, "completion_tokens": 2}
    });
    let mock = MockUpstream::start(vec![
        vec![http_json(200, &first_answer.to_string())],
        vec![http_json(200, &second_answer.to_string())],
    ]);
    let key = key_file("responses-continuation", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();

    let (first_status, _, first_body) = send_responses(
        &proxy,
        &json!({"model": "auto", "input": "first turn", "store": false}),
        &token,
    );
    assert_eq!(first_status, 200, "{first_body}");
    let first_body: Value = serde_json::from_str(&first_body).expect("first response is JSON");
    assert_eq!(first_body["id"], json!("chatcmpl-continuation-1"));

    let (second_status, _, second_body) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "input": "second turn",
            "previous_response_id": "chatcmpl-continuation-1",
            "store": false
        }),
        &token,
    );
    assert_eq!(second_status, 200, "{second_body}");
    let seen = mock.seen();
    assert_eq!(seen.len(), 2);
    let messages = seen[1].body["messages"]
        .as_array()
        .expect("continued provider request has messages");
    assert_eq!(messages.len(), 3, "{}", seen[1].body);
    assert_eq!(messages[0]["content"], json!("first turn"));
    assert_eq!(messages[1]["content"], json!("first answer"));
    assert_eq!(messages[2]["content"], json!("second turn"));

    std::fs::remove_file(key).ok();
}

#[test]
fn responses_parallel_tool_calls_false_is_preserved_to_the_upstream() {
    // Codex sends `parallel_tool_calls: false` on every request. The Canonical
    // IR carries it through the extensions passthrough and the OpenAI-compatible
    // provider renders it alongside `tools`, so the constraint survives to the
    // upstream instead of the request being refused before routing.
    let upstream_answer = json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "M4_OK"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("responses-parallel-false", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();

    let (status, _content_type, body) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "input": "Read the marker.",
            "stream": false,
            "parallel_tool_calls": false,
            "tools": [{
                "type": "function",
                "name": "read_marker",
                "description": "Read one marker",
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"]
                }
            }]
        }),
        &token,
    );

    assert_eq!(status, 200, "{body}");
    let seen = mock.seen();
    assert_eq!(seen.len(), 1, "the request reaches the upstream");
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].body["parallel_tool_calls"],
        json!(false),
        "parallel_tool_calls=false is preserved to the provider: {}",
        seen[0].body
    );
    assert_eq!(
        seen[0].body["tools"][0]["function"]["name"],
        json!("read_marker")
    );
    std::fs::remove_file(key).ok();
}

#[test]
fn responses_reasoning_effort_maps_to_the_upstream_parameter() {
    // Codex sends `reasoning: { effort: "high" }`. The effort maps onto the
    // OpenAI-compatible `reasoning_effort` parameter and is rendered to the
    // upstream (the demo model declares no parameter set, so it is optimistic);
    // `summary` has no chat equivalent and is dropped without refusing.
    let upstream_answer = json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "M4_OK"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("responses-reasoning-effort", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();

    let (status, _content_type, body) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "input": "Read the marker.",
            "stream": false,
            "reasoning": {"effort": "high", "summary": "auto"}
        }),
        &token,
    );

    assert_eq!(status, 200, "{body}");
    let seen = mock.seen();
    assert_eq!(seen.len(), 1, "the request reaches the upstream");
    assert_eq!(
        seen[0].body["reasoning_effort"],
        json!("high"),
        "reasoning.effort maps to reasoning_effort: {}",
        seen[0].body
    );
    std::fs::remove_file(key).ok();
}

#[test]
fn a_responses_namespace_stream_is_incremental_and_protocol_shaped() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"M4_\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_stream\",\"function\":{\"name\":\"workspace_v1__read_marker\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"STREAM_OK\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"marker.txt\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n",
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
            "stream": true,
            "tools": [namespace_read_marker_tool()]
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

    let events = sse_events(&body);
    assert_responses_terminal_is_unique_and_last(&events, &body);
    assert_namespace_stream_events(&events);
    let seen = mock.seen();
    assert_eq!(
        seen[0].body["tools"][0]["function"]["name"],
        json!("workspace_v1__read_marker")
    );

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["protocol"], "Text(\"openai-responses\")");
    assert_eq!(row["stream"], "Integer(1)");
    assert_eq!(row["input_tokens"], "Integer(5)");
    assert_eq!(row["output_tokens"], "Integer(2)");

    std::fs::remove_file(key).ok();
}

fn assert_responses_tool_follow_up(body: &Value) {
    assert_eq!(
        body["messages"][1]["tool_calls"][0]["id"],
        json!("call_marker")
    );
    assert_eq!(body["messages"][1]["content"], json!("Reading the marker."));
    assert_eq!(body["messages"][2]["role"], json!("tool"));
    assert_eq!(body["messages"][2]["tool_call_id"], json!("call_marker"));
    assert_eq!(body["messages"][2]["content"], json!("TOOL_RESULT_OK"));
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
                    "type": "message",
                    "role": "assistant",
                    "content": "Reading the marker."
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
    assert_responses_tool_follow_up(&seen[1].body);

    std::fs::remove_file(key).ok();
}

#[test]
fn responses_namespace_tool_is_flattened_for_the_provider_and_restored_for_codex() {
    let tool_answer = json!({
        "id": "chatcmpl-namespace-tool",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_spawn_1",
                    "type": "function",
                    "function": {
                        "name": "multi_agent_v1__spawn_agent",
                        "arguments": "{\"task\":\"inspect\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": {"prompt_tokens": 8, "completion_tokens": 3}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &tool_answer.to_string())]]);
    let key = key_file("responses-namespace-tool", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");
    let token = proxy.virtual_key.clone();

    let (status, _, response) = send_responses(
        &proxy,
        &json!({
            "model": "auto",
            "input": "Spawn one worker.",
            "stream": false,
            "tools": [{
                "type": "namespace",
                "name": "multi_agent_v1",
                "description": "Manage isolated workers.",
                "tools": [{
                    "type": "function",
                    "name": "spawn_agent",
                    "description": "Spawn one worker.",
                    "strict": true,
                    "parameters": {
                        "type": "object",
                        "properties": {"task": {"type": "string"}},
                        "required": ["task"],
                        "additionalProperties": false
                    }
                }]
            }]
        }),
        &token,
    );

    assert_eq!(status, 200, "{response}");
    let response: Value = serde_json::from_str(&response).expect("Responses body is JSON");
    assert_eq!(response["output"][0]["type"], json!("function_call"));
    assert_eq!(response["output"][0]["namespace"], json!("multi_agent_v1"));
    assert_eq!(response["output"][0]["name"], json!("spawn_agent"));
    assert_eq!(response["output"][0]["call_id"], json!("call_spawn_1"));

    let seen = mock.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0].body["tools"][0]["function"]["name"],
        json!("multi_agent_v1__spawn_agent")
    );
    assert_eq!(seen[0].body["tools"][0]["function"]["strict"], json!(true));
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
            "thinking": {"type": "disabled"},
            "tool_choice": {"type": "auto"},
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
    assert!(seen[0].body.get("anthropic_thinking").is_none());

    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["protocol"], "Text(\"anthropic-messages\")");
    assert_eq!(row["status"], "Integer(200)");

    std::fs::remove_file(key).ok();
}

#[test]
fn translated_anthropic_server_tool_fails_before_upstream_with_receipt_reason() {
    let mock = MockUpstream::start(Vec::new());
    let key = key_file("anthropic-server-tool-capability", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");

    let (status, body) = post_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": "find current information"}],
            "tools": [{"type": "web_search_20250305", "name": "web_search", "max_uses": 3}]
        }),
        &proxy.virtual_key,
    );

    assert_eq!(status, 400, "body={body}");
    let error: Value = serde_json::from_str(&body).expect("Anthropic error JSON");
    assert_eq!(error["error"]["type"], json!("invalid_request_error"));
    assert!(body.contains("anthropic-native"), "body={body}");
    assert_eq!(
        mock.hits(),
        0,
        "unsupported server tools stop before upstream"
    );

    settle();
    let (status, _, body) = admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    assert_eq!(status, 200, "body={body}");
    let receipts: Value = serde_json::from_str(&body).expect("receipts are JSON");
    assert_eq!(
        receipts[0]["attempt_records"].as_array().map(Vec::len),
        Some(0)
    );
    let conversion = receipts[0]["conversion_reports"]
        .as_array()
        .and_then(|reports| reports.first())
        .expect("failed inbound conversion is recorded");
    assert_eq!(conversion["stage"], json!("inbound_normalize"));
    assert_eq!(
        conversion["reason_code"],
        json!("provider_tool_unsupported")
    );
    assert_eq!(conversion["reason_detail"], json!("web_search"));

    std::fs::remove_file(key).ok();
}

#[test]
fn translated_responses_hosted_tool_fails_before_upstream_with_receipt_reason() {
    let mock = MockUpstream::start(Vec::new());
    let key = key_file("responses-hosted-tool-capability", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-openai-responses");

    for (tool, expected_detail) in [
        ("web_search", "web_search"),
        ("file_search", "other_tool_type"),
    ] {
        let (status, body) = post_scoped(
            &proxy,
            "/agents/codex/v1/responses",
            &json!({
                "model": "auto",
                "input": "use a provider-hosted tool",
                "tools": [{"type": tool}]
            }),
            &proxy.virtual_key,
            false,
        );

        assert_eq!(status, 400, "body={body}");
        assert!(body.contains(tool), "body={body}");
        assert_eq!(
            mock.hits(),
            0,
            "unsupported hosted tools stop before upstream"
        );

        settle();
        let (status, _, body) =
            admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
        assert_eq!(status, 200, "body={body}");
        let receipts: Value = serde_json::from_str(&body).expect("receipts are JSON");
        assert_eq!(
            receipts[0]["attempt_records"].as_array().map(Vec::len),
            Some(0)
        );
        let conversion = receipts[0]["conversion_reports"]
            .as_array()
            .and_then(|reports| reports.first())
            .expect("failed inbound conversion is recorded");
        assert_eq!(conversion["stage"], json!("inbound_normalize"));
        assert_eq!(
            conversion["reason_code"],
            json!("provider_tool_unsupported")
        );
        assert_eq!(conversion["reason_detail"], json!(expected_detail));
    }

    std::fs::remove_file(key).ok();
}

#[test]
fn claude_desktop_adaptive_thinking_reaches_the_translated_provider() {
    let upstream_answer = json!({
        "id": "chatcmpl-adaptive",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "ready"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 1}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("claude-desktop-adaptive-thinking", "sk-test-key-abc");
    let proxy = start_proxy_with_agent_and_supported_parameters(
        &mock,
        &key,
        true,
        "agent-anthropic",
        json!(["reasoning_effort"]),
    );

    let (status, body) = post_scoped(
        &proxy,
        "/agents/claude-desktop/v1/messages",
        &json!({
            "model": "claude-sonnet-4-6",
            "max_tokens": 128,
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "high"},
            "messages": [{"role": "user", "content": "check the route"}]
        }),
        &proxy.virtual_key,
        true,
    );

    assert_eq!(status, 200, "{body}");
    let seen = mock.seen();
    assert_eq!(
        seen.len(),
        1,
        "adaptive thinking must reach the upstream once"
    );
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(seen[0].body["reasoning_effort"], json!("high"));
    assert!(seen[0].body.get("anthropic_thinking").is_none());

    std::fs::remove_file(key).ok();
}

#[test]
fn anthropic_adaptive_thinking_degrades_safely_for_an_undeclared_chat_provider() {
    let upstream_answer = json!({
        "id": "chatcmpl-adaptive",
        "model": "deepseek-v4-flash",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "hello"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 1}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("anthropic-adaptive-thinking", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");

    let (status, content_type, body) = send_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 128,
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "medium"},
            "messages": [{"role": "user", "content": "think before answering"}]
        }),
        &proxy.virtual_key,
    );

    assert_eq!(status, 200, "{body}");
    assert_eq!(content_type.as_deref(), Some("application/json"));
    let seen = mock.seen();
    assert_eq!(seen.len(), 1, "adaptive thinking reaches the upstream");
    assert!(seen[0].body.get("thinking").is_none());
    assert!(seen[0].body.get("anthropic_thinking").is_none());
    assert!(
        seen[0].body.get("reasoning_effort").is_none(),
        "an undeclared OpenAI-compatible model must not receive reasoning_effort: {}",
        seen[0].body
    );

    std::fs::remove_file(key).ok();
}

#[test]
fn anthropic_forced_tool_choice_is_translated_and_reaches_the_upstream() {
    // A forced tool_choice used to 400 before routing with a message that wrongly
    // blamed the Canonical IR. It now translates on this Chat-translate path
    // ("any" -> Required, "tool" -> Auto) and the request reaches the upstream;
    // the exact per-shape mapping is covered by the plugin's wasm unit test. (The
    // native-Anthropic passthrough path preserves {type:tool} verbatim instead.)
    let upstream_answer = json!({
        "id": "chatcmpl-tc",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "done"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 5, "completion_tokens": 1}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("anthropic-tool-choice", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");

    let (status, body) = post_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "read the marker"}],
            "tools": [{
                "name": "read_marker",
                "description": "read a marker",
                "input_schema": {"type": "object"}
            }],
            "tool_choice": {"type": "tool", "name": "read_marker"}
        }),
        &proxy.virtual_key,
    );

    assert_eq!(
        status, 200,
        "a translated forced tool_choice reaches the upstream, body={body}"
    );
    assert!(
        !body.contains("Canonical IR"),
        "the old IR-blaming refusal must be gone, body={body}"
    );
    assert_eq!(
        mock.hits(),
        1,
        "the forced tool_choice reached the upstream"
    );
    std::fs::remove_file(key).ok();
}

#[test]
fn anthropic_pdf_tool_result_is_extracted_before_chat_translation() {
    const TWO_PAGE_PDF_BASE64: &str = TEST_TEXT_PDF_BASE64;
    let upstream_answer = json!({
        "id": "chatcmpl-pdf",
        "model": "home-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "PDF_OK"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 40, "completion_tokens": 2}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("anthropic-pdf-tool-result", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");

    let (status, body) = post_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 256,
            "messages": [
                {"role": "user", "content": "请检查这个 PDF"},
                {"role": "assistant", "content": [{
                    "type": "tool_use",
                    "id": "toolu_read_pdf",
                    "name": "Read",
                    "input": {"file_path": "/private/report.pdf"}
                }]},
                {"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_read_pdf",
                    "content": [{
                        "type": "document",
                        "source": {
                            "type": "base64",
                            "media_type": "application/pdf",
                            "data": TWO_PAGE_PDF_BASE64
                        },
                        "title": "report.pdf"
                    }]
                }]}
            ]
        }),
        &proxy.virtual_key,
    );

    assert_eq!(status, 200, "PDF request should reach the upstream: {body}");
    assert_eq!(mock.hits(), 1);
    let seen = mock.seen();
    assert!(
        seen[0].body["messages"][1]
            .as_object()
            .is_some_and(|message| message.contains_key("content")),
        "strict OpenAI-compatible providers require content:null beside assistant tool_calls"
    );
    assert_eq!(seen[0].body["messages"][1]["content"], Value::Null);
    let forwarded = seen[0].body.to_string();
    assert!(forwarded.contains("Token Station 已把 PDF 附件转换为文字"));
    assert!(forwarded.contains("report.pdf"));
    assert!(forwarded.contains("TOKEN_STATION_PDF_TEXT"));
    assert!(
        !forwarded.contains(TWO_PAGE_PDF_BASE64),
        "the PDF base64 must never be flattened into upstream text"
    );
    std::fs::remove_file(key).ok();
}

#[test]
fn anthropic_unreadable_documents_become_honest_localized_text() {
    let upstream_answer = json!({
        "id": "chatcmpl-unreadable-document",
        "model": "home-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "I did not read the attachments."},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 40, "completion_tokens": 8}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("anthropic-unreadable-documents", "sk-test-key-abc");
    let proxy = start_proxy_with_agent(&mock, &key, true, "agent-anthropic");

    let (status, body) = post_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 128,
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "Read these attachments."},
                {
                    "type": "document",
                    "source": {
                        "type": "base64",
                        "media_type": "application/pdf",
                        "data": "bm90LWEtcGRm"
                    },
                    "title": "broken.pdf"
                },
                {
                    "type": "document",
                    "source": {"type": "url", "url": "https://private.invalid/file.pdf"},
                    "title": "remote.pdf"
                }
            ]}]
        }),
        &proxy.virtual_key,
    );

    assert_eq!(
        status, 200,
        "unreadable documents should not abort chat: {body}"
    );
    assert_eq!(mock.hits(), 1);
    let forwarded = mock.seen()[0].body.to_string();
    assert!(forwarded.contains("Attachment not read"));
    assert!(forwarded.contains("broken.pdf"));
    assert!(forwarded.contains("remote.pdf"));
    assert!(forwarded.contains("edit the earlier message, remove the attachment, and resend"));
    assert!(!forwarded.contains("bm90LWEtcGRm"));
    assert!(!forwarded.contains("private.invalid"));
    std::fs::remove_file(key).ok();
}

#[test]
fn anthropic_native_direct_refuses_tools_before_hitting_an_unsupported_target() {
    let upstream_answer = json!({
        "id": "msg_native_tools_unsupported",
        "type": "message",
        "role": "assistant",
        "model": "deepseek-chat",
        "content": [{"type": "text", "text": "should not be reached"}],
        "usage": {"input_tokens": 4, "output_tokens": 2}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("direct-native-tools-unsupported", "sk-upstream-secret\n");
    let proxy = start_native_anthropic_proxy_with(&mock, &key, true, "unsupported");

    let (status, body) = post_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": "Read the marker"}],
            "tools": [{
                "name": "read_marker",
                "description": "Read a marker",
                "input_schema": {"type": "object", "properties": {}}
            }]
        }),
        &proxy.virtual_key,
    );

    assert_eq!(
        (status, mock.hits()),
        (400, 0),
        "tool-incapable Direct target must fail before dispatch: {body}"
    );
    std::fs::remove_file(key).ok();
}

#[test]
fn anthropic_native_passthrough_only_replaces_images_for_a_text_only_model() {
    // Native passthrough keeps server tools and forced tool choice verbatim, but
    // its confirmed text-only target receives localized image and PDF fallback.
    let upstream_answer = json!({
        "id": "msg_native_1",
        "type": "message",
        "role": "assistant",
        "model": "deepseek-chat",
        "content": [{"type": "text", "text": "searched"}],
        "usage": {"input_tokens": 4, "output_tokens": 2}
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("passthrough-native", "sk-upstream-secret\n");
    let proxy = start_native_anthropic_proxy(&mock, &key);

    let (status, body) = post_messages(
        &proxy,
        &json!({
            "model": "auto",
            "max_tokens": 64,
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "abc"}},
                {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": TEST_TEXT_PDF_BASE64}, "title": "native.pdf"},
                {"type": "text", "text": "搜索这张图相关的资料"}
            ]}],
            "tools": [{"type": "web_search_20250305", "name": "web_search", "max_uses": 5}],
            "tool_choice": {"type": "tool", "name": "web_search"}
        }),
        &proxy.virtual_key,
    );

    // The client receives the upstream's response verbatim.
    assert_eq!(status, 200, "body={body}");
    assert!(
        body.contains("msg_native_1"),
        "upstream body relayed verbatim, body={body}"
    );

    let seen = mock.seen();
    assert_eq!(seen.len(), 1, "exactly one upstream hit");
    let forwarded = &seen[0];
    // base_url is origin-only, so it resolves to /v1/messages.
    assert_eq!(forwarded.path, "/v1/messages");
    // The server tool and forced tool_choice survived verbatim (NOT translated to
    // a function or refused).
    assert_eq!(
        forwarded.body["tools"][0]["type"],
        json!("web_search_20250305")
    );
    assert_eq!(forwarded.body["tool_choice"]["type"], json!("tool"));
    assert_eq!(
        forwarded.body["messages"][0]["content"][0],
        json!({"type": "text", "text": "[图片已省略：当前模型不支持视觉输入。]"})
    );
    assert_eq!(
        forwarded.body["messages"][0]["content"][1],
        json!({
            "type": "text",
            "text": "[Token Station 已把 PDF 附件转换为文字，因为当前路由不能直接传递 document 内容块。]\n文件名：native.pdf\n\nTOKEN_STATION_PDF_TEXT"
        })
    );
    assert_eq!(
        forwarded.body["messages"][0]["content"][2],
        json!({"type": "text", "text": "搜索这张图相关的资料"})
    );
    // Only the model was remapped to the routed upstream model.
    assert_eq!(forwarded.body["model"], json!("deepseek-chat"));
    // The upstream saw the injected upstream key; the client's own virtual key
    // never reached it.
    assert_eq!(
        forwarded.authorization.as_deref(),
        Some("Bearer sk-upstream-secret")
    );
    let client_token = format!("Bearer {}", proxy.virtual_key);
    assert_ne!(
        forwarded.authorization.as_deref(),
        Some(client_token.as_str()),
        "the client's own token must never reach the upstream"
    );
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
    settle();
    assert_eq!(request_count(&proxy.data_dir), 0, "the 401 is not recorded");

    let (status, body) = post_chat(
        &proxy,
        &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
        None,
    );
    assert_eq!(status, 404, "{body}");
    let error: Value = serde_json::from_str(&body).expect("Anthropic error JSON");
    assert_eq!(error["error"]["type"], json!("invalid_request"));
    assert_eq!(error["error"]["code"], json!("invalid_request"));
    assert_eq!(mock.hits(), 0, "both refusals happen before routing");

    std::fs::remove_file(key).ok();
}

#[test]
fn an_anthropic_stream_is_incremental_and_protocol_shaped() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":3,\"cache_write_tokens\":2},\"completion_tokens_details\":{\"reasoning_tokens\":1}}}\n\n",
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
    let events = sse_events(&body);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "message_stop")
            .count(),
        1,
        "message_stop appears exactly once: {body}"
    );
    assert_eq!(
        events.last().map(|event| &event["type"]),
        Some(&json!("message_stop")),
        "message_stop is terminal: {body}"
    );
    let usage = events
        .into_iter()
        .filter(|event| event["type"] == "message_delta")
        .find_map(|event| event.get("usage").cloned())
        .expect("a message_delta carries cumulative usage");
    assert_eq!(usage["input_tokens"], json!(7));
    assert_eq!(usage["output_tokens"], json!(2));
    assert_eq!(usage["cache_read_input_tokens"], json!(3));
    assert_eq!(usage["cache_creation_input_tokens"], json!(2));

    let seen = mock.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].body["stream_options"]["include_usage"], json!(true));

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
    assert_eq!(body.matches("data: [DONE]").count(), 1, "{body}");
    assert!(body.trim_end().ends_with("data: [DONE]"), "{body}");

    // The usage event inside the stream reached the metrics row.
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["stream"], "Integer(1)");
    assert_eq!(row["input_tokens"], "Integer(7)");
    assert_eq!(row["output_tokens"], "Integer(2)");

    std::fs::remove_file(key).ok();
}

#[test]
fn sse_crlf_no_space_multiline_data_and_split_unicode_are_decoded_once() {
    let sse = concat!(
        "data:{\"choices\":[\r\n",
        "data: {\"index\":0,\"delta\":{\"content\":\"你好😀\"}}]}\r\n\r\n",
        "data:{\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\r\n\r\n",
        "data:[DONE]\r\n\r\n"
    );
    let header = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        sse.len()
    );
    let bytes = sse.as_bytes();
    let unicode = sse.find('你').expect("Chinese payload exists");
    let emoji = sse.find('😀').expect("emoji payload exists");
    let chinese_split = unicode + 1;
    let emoji_split = emoji + 2;
    let segments = vec![
        header.into_bytes(),
        bytes[..chinese_split].to_vec(),
        bytes[chinese_split..emoji_split].to_vec(),
        bytes[emoji_split..].to_vec(),
    ];
    let mock = MockUpstream::start(vec![segments]);
    let key = key_file("sse-rfc-boundaries", "sk-test-key");
    let proxy = start_proxy(&mock, &key);
    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("你好😀"), "{body}");
    settle();
    assert_eq!(last_row(&proxy.data_dir)["status"], "Integer(200)");
    std::fs::remove_file(key).ok();
}

#[test]
fn an_sse_error_before_output_stays_uncommitted_and_typed() {
    let sse = "event:error\n\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    )
    .into_bytes();
    let mock = MockUpstream::start(vec![vec![response]]);
    let key = key_file("sse-event-error", "sk-test-key");
    let proxy = start_proxy(&mock, &key);
    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(status, 502, "{body}");
    assert!(
        !body.contains("data:"),
        "the stream was never committed: {body}"
    );
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["error_code"], "Text(\"provider_protocol_error\")");
    assert_eq!(row["status"], "Integer(502)");
    std::fs::remove_file(key).ok();
}

#[test]
fn an_empty_stream_choices_frame_cannot_become_a_successful_done() {
    let sse = "data: {\"choices\":[]}\n\ndata: [DONE]\n\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    )
    .into_bytes();
    let mock = MockUpstream::start(vec![vec![response]]);
    let key = key_file("sse-empty-choices", "sk-test-key");
    let proxy = start_proxy(&mock, &key);
    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(status, 502, "{body}");
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(502)");
    assert_eq!(row["error_code"], "Text(\"provider_protocol_error\")");
    std::fs::remove_file(key).ok();
}

#[test]
fn finish_reason_then_clean_eof_is_a_complete_stream() {
    let sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"complete\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    )
    .into_bytes();
    let mock = MockUpstream::start(vec![vec![response]]);
    let key = key_file("sse-finish-eof", "sk-test-key");
    let proxy = start_proxy(&mock, &key);
    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("complete"), "{body}");
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(200)");
    assert_eq!(row["error_code"], "Null");
    std::fs::remove_file(key).ok();
}

#[test]
fn a_delta_followed_by_eof_is_partial_failure_not_success() {
    let sse = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"half\"}}]}\n\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    )
    .into_bytes();
    let mock = MockUpstream::start(vec![vec![response]]);
    let key = key_file("sse-partial-eof", "sk-test-key");
    let proxy = start_proxy(&mock, &key);
    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(status, 200, "HTTP was already committed: {body}");
    assert!(body.contains("half"), "{body}");
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(502)");
    assert_eq!(row["error_code"], "Text(\"transport_truncated\")");
    std::fs::remove_file(key).ok();
}

#[test]
fn invalid_first_sse_event_falls_back_before_stream_commit() {
    let broken_sse = "data: not-json\n\n";
    let broken = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{broken_sse}",
        broken_sse.len()
    )
    .into_bytes();
    let good_sse = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"fallback-stream\"}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let good = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{good_sse}",
        good_sse.len()
    )
    .into_bytes();
    let primary = MockUpstream::start(vec![vec![broken]]);
    let fallback = MockUpstream::start(vec![vec![good]]);
    let key = key_file("sse-precommit-fallback", "sk-test-key");
    let proxy = start_proxy_two(&primary, &fallback, &key);
    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("fallback-stream"), "{body}");
    assert_eq!((primary.hits(), fallback.hits()), (1, 1));
    settle();
    assert_eq!(last_row(&proxy.data_dir)["attempts"], "Integer(2)");
    std::fs::remove_file(key).ok();
}

#[test]
fn the_models_catalog_aggregates_configured_upstreams() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("models", "sk-test-key-abc");
    let proxy = start_proxy_with_agents_budgets_and_catalog_price(
        &mock,
        &key,
        true,
        &["agent-openai"],
        None,
        true,
    );

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
    assert_eq!(body["data"][0]["context_window"], json!(400_000));
    assert_eq!(body["data"][0]["max_output_tokens"], json!(32_768));
    assert_eq!(body["data"][0]["limit"]["context"], json!(400_000));
    assert_eq!(body["data"][0]["limit"]["output"], json!(32_768));
    assert_eq!(body["data"][0]["cost"]["input"], json!(5.0));
    assert_eq!(body["data"][0]["cost"]["output"], json!(30.0));
    assert_eq!(body["data"][0]["cost"]["cache_read"], json!(0.5));
    assert_eq!(
        body["data"][0]["modalities"]["input"],
        json!(["text", "image"])
    );
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
fn opencode_and_hermes_images_reach_a_vision_capable_upstream() {
    let answer = json!({
        "id": "chatcmpl-vision",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "VISION_OK" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 12, "completion_tokens": 2 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("vision-agents", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);
    let token = proxy.virtual_key.clone();
    let image_url = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB";
    let request = json!({
        "model": "auto",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": "Read this image." },
                { "type": "image_url", "image_url": { "url": image_url, "detail": "low" } }
            ]
        }]
    });

    for agent_id in ["opencode", "nous-hermes-agent"] {
        let (status, body) = post_scoped(
            &proxy,
            &format!("/agents/{agent_id}/v1/chat/completions"),
            &request,
            &token,
            false,
        );
        assert_eq!(status, 200, "{agent_id}: {body}");
    }

    let seen = mock.seen();
    assert_eq!(seen.len(), 2);
    for request in seen {
        assert_eq!(request.path, "/v1/chat/completions");
        assert_eq!(
            request.body["messages"][0]["content"][1],
            json!({
                "type": "image_url",
                "image_url": { "url": image_url, "detail": "low" }
            })
        );
    }

    std::fs::remove_file(key).ok();
}

#[test]
fn every_openai_chat_agent_degrades_images_before_a_non_vision_upstream() {
    let answer = json!({
        "id": "chatcmpl-media-fallback",
        "model": "home-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "I can continue with the text." },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 12, "completion_tokens": 6 }
    });
    let home = MockUpstream::start(vec![
        vec![http_json(200, &answer.to_string())],
        vec![http_json(200, &answer.to_string())],
        vec![http_json(200, &answer.to_string())],
    ]);
    let custom = MockUpstream::start(Vec::new());
    let key = key_file("vision-agents-unsupported", "sk-test-key-abc");
    let proxy = start_scoped_proxy(&home, &custom, &key);
    let token = proxy.virtual_key.clone();
    let request = json!({
        "model": "auto",
        "messages": [{
            "role": "user",
            "content": [{
                "type": "image_url",
                "image_url": { "url": "https://example.test/cat.png" }
            }]
        }]
    });

    for agent_id in ["workbuddy", "opencode", "nous-hermes-agent"] {
        let (status, body) = post_scoped(
            &proxy,
            &format!("/agents/{agent_id}/v1/chat/completions"),
            &request,
            &token,
            false,
        );
        assert_eq!(status, 200, "{agent_id}: {body}");
        assert!(
            body.contains("I can continue with the text."),
            "{agent_id}: {body}"
        );
    }

    assert_eq!(home.hits(), 3);
    assert_eq!(custom.hits(), 0);
    for request in home.seen() {
        assert_eq!(
            request.body["messages"][0]["content"],
            json!("[Image omitted: the current model does not support visual input.]")
        );
    }
    std::fs::remove_file(key).ok();
}

#[test]
fn media_fallback_localizes_from_the_latest_user_text_and_records_a_real_attempt() {
    let answer = json!({
        "id": "chatcmpl-localized-fallback",
        "model": "home-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "我会根据剩余文字继续。" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 18, "completion_tokens": 8 }
    });
    let home = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let custom = MockUpstream::start(Vec::new());
    let key = key_file("workbuddy-vision-unsupported", "sk-test-key-abc");
    let proxy = start_scoped_proxy(&home, &custom, &key);
    let token = proxy.virtual_key.clone();
    let request = json!({
        "model": "auto",
        "messages": [
          {
            "role": "user",
            "content": [
                { "type": "image_url", "image_url": { "url": "https://example.test/cat.png" } }
            ]
          },
          {
            "role": "user",
            "content": [{ "type": "text", "text": "请继续查看这份报告。" }]
          }
        ]
    });

    let (status, body) = post_scoped(
        &proxy,
        "/agents/workbuddy/v1/chat/completions",
        &request,
        &token,
        false,
    );
    assert_eq!(status, 200, "{body}");
    let response: Value = serde_json::from_str(&body).expect("model response is JSON");
    assert_eq!(response["object"], json!("chat.completion"));
    assert_eq!(
        response["choices"][0]["message"]["role"],
        json!("assistant")
    );
    assert_eq!(
        response["choices"][0]["message"]["content"],
        json!("我会根据剩余文字继续。")
    );
    assert_eq!(home.hits(), 1);
    assert_eq!(custom.hits(), 0);
    let seen = home.seen();
    assert_eq!(
        seen[0].body["messages"][0]["content"],
        json!("[图片已省略：当前模型不支持视觉输入。]")
    );
    assert_eq!(
        seen[0].body["messages"][1]["content"],
        json!("请继续查看这份报告。")
    );
    settle();
    let log = std::fs::read_to_string(proxy.data_dir.join("requests.log")).expect("log exists");
    let receipt: Value = serde_json::from_str(log.lines().last().expect("a receipt exists"))
        .expect("receipt is JSON");
    assert_eq!(receipt["status"], json!(200));
    assert_eq!(receipt["error_code"], Value::Null);
    assert_eq!(receipt["attempts"], json!(1));
    assert_eq!(receipt["agent_id"], json!("workbuddy"));
    std::fs::remove_file(key).ok();
}

#[test]
fn codex_responses_input_images_use_the_same_text_only_fallback() {
    let answer = json!({
        "id": "chatcmpl-responses-media-fallback",
        "model": "agent-model",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Continued without the image." },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
    });
    let home = MockUpstream::start(Vec::new());
    let custom = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("responses-media-fallback", "sk-test-key-abc");
    let proxy = start_scoped_proxy(&home, &custom, &key);
    let token = proxy.virtual_key.clone();

    let (status, body) = post_scoped(
        &proxy,
        "/agents/codex/v1/responses",
        &json!({
            "model": "auto",
            "input": [{
                "type": "message",
                "role": "user",
                "content": [
                    { "type": "input_text", "text": "Continue from the text." },
                    { "type": "input_image", "image_url": "https://example.test/cat.png" }
                ]
            }],
            "stream": false
        }),
        &token,
        false,
    );

    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Continued without the image."), "{body}");
    assert_eq!(home.hits(), 0);
    assert_eq!(custom.hits(), 1);
    let seen = custom.seen();
    assert_eq!(
        seen[0].body["messages"][0]["content"],
        json!(
            "Continue from the text.[Image omitted: the current model does not support visual input.]"
        )
    );

    std::fs::remove_file(key).ok();
}

#[test]
fn an_upstream_media_refusal_retries_once_with_localized_markers() {
    let refusal = json!({
        "error": { "message": "This model does not support image attachments." }
    });
    let answer = json!({
        "id": "chatcmpl-reactive-media-fallback",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "Continued after fallback." },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 4 }
    });
    let mock = MockUpstream::start(vec![
        vec![http_json(400, &refusal.to_string())],
        vec![http_json(200, &answer.to_string())],
    ]);
    let key = key_file("reactive-media-fallback", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    let (status, body) = post_chat(
        &proxy,
        &json!({
            "model": "auto",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "text", "text": "Continue without the attachment." },
                    { "type": "image_url", "image_url": { "url": "https://example.test/cat.png" } }
                ]
            }]
        }),
        None,
    );

    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Continued after fallback."), "{body}");
    assert_eq!(mock.hits(), 2);
    let seen = mock.seen();
    assert_eq!(
        seen[0].body["messages"][0]["content"][1]["type"],
        "image_url"
    );
    assert_eq!(
        seen[1].body["messages"][0]["content"],
        json!(
            "Continue without the attachment.[Image omitted: the current model does not support visual input.]"
        )
    );
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(200)");
    assert_eq!(row["attempts"], "Integer(2)");

    std::fs::remove_file(key).ok();
}

#[test]
fn audio_and_embeddings_are_refused_before_the_upstream() {
    let mock = MockUpstream::start(Vec::new());
    let key = key_file("capability", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);

    let request = json!({
        "model": "auto",
        "messages": [{ "role": "user", "content": [
            { "type": "input_audio", "input_audio": { "data": "AA==", "format": "wav" } }
        ]}]
    });
    let (status, body) = post_chat(&proxy, &request, None);
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("audio and embeddings are unsupported"),
        "{body}"
    );
    assert!(!body.contains("image"), "{body}");
    assert!(body.contains("capability"), "{body}");
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["error_code"], "Text(\"capability\")");
    assert_eq!(row["attempts"], "Integer(0)");

    let response = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    )
    .post(format!("{}/v1/embeddings", proxy.url))
    .header("authorization", &format!("Bearer {}", proxy.virtual_key))
    .send(&json!({"model": "auto", "input": "hello"}).to_string())
    .expect("the proxy returns a typed refusal");
    assert_eq!(response.status().as_u16(), 400);
    let body = response.into_body().read_to_string().expect("body reads");
    assert!(body.contains("capability"), "{body}");
    assert!(
        body.contains("audio and embeddings are unsupported"),
        "{body}"
    );
    assert!(!body.contains("image"), "{body}");
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["error_code"], "Text(\"capability\")");
    assert_eq!(row["attempts"], "Integer(0)");
    assert_eq!(mock.hits(), 0, "unsupported media is refused pre-upstream");

    std::fs::remove_file(key).ok();
}

#[test]
fn a_broken_agent_adapter_is_skipped_and_the_proxy_still_serves() {
    let upstream_answer = json!({
        "id": "chatcmpl-ok",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok." },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 4, "completion_tokens": 1 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &upstream_answer.to_string())]]);
    let key = key_file("resilient-agents", "sk-test-key-abc");
    let proxy = start_proxy_with_agents(&mock, &key, false, &["agent-openai", "does-not-exist"]);

    let (status, body) = post_chat(
        &proxy,
        &json!({
            "model": "auto",
            "messages": [{ "role": "user", "content": "hi" }]
        }),
        None,
    );

    assert_eq!(status, 200, "the surviving adapter still serves: {body}");
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
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": CANARY }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("canary", &format!("sk-test-{CANARY}"));
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
    let recorder = Arc::new(token_station_cli::filelog::Recorders(vec![
        Box::new(token_station_cli::filelog::FileLog::open(&config.data.dir).expect("log opens")),
        Box::new(
            token_station_cli::store::SqliteStore::open(&config.data.dir.join("metrics.sqlite"))
                .expect("store opens"),
        ),
    ]));
    let gateway = Arc::new(Gateway::new(&config, recorder).expect("gateway assembles"));
    let (virtual_key, _) =
        token_station_cli::virtual_key::load_or_create(&config.data.dir).expect("key creates");
    let state = server::AppState::new(
        Arc::clone(&gateway),
        Some(Arc::from(virtual_key.as_str())),
        Arc::new(
            token_station_cli::admin::AdminContext::from_config(&config)
                .expect("admin snapshot compiles"),
        ),
    );
    let control = state.control.clone();

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
        control,
        gateway,
    }
}

fn start_direct_proxy_two(first: &MockUpstream, applied: &MockUpstream, key_file: &Path) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-proxy-direct-{}-{}",
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
        "plugins": {
            "dir": plugins_dir(),
            "agent": "agent-openai",
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "a_first": upstream(first),
            "z_applied": upstream(applied)
        },
        "router": {
            "version": 1,
            "pools": { "main": [
                { "upstream": "a_first", "model": "gpt-5.5" },
                { "upstream": "z_applied", "model": "gpt-5.5" }
            ]},
            "default_pool": "main"
        },
        "routing": {
            "mode": "direct",
            "direct_target": { "upstream": "z_applied", "model": "gpt-5.5" }
        }
    });
    let config: ClientConfig = serde_json::from_value(config).expect("direct config parses");
    spawn_proxy(&config)
}

#[test]
fn direct_gateway_dispatches_only_to_the_applied_target() {
    let answer = json!({
        "id": "cmpl-direct",
        "object": "chat.completion",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let first = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let applied = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("direct-dispatch", "sk-test-key-abc\n");
    let proxy = start_direct_proxy_two(&first, &applied, &key);

    let (status, body) = post_chat(
        &proxy,
        &json!({ "model": "auto", "messages": [{ "role": "user", "content": "hello" }] }),
        None,
    );

    assert_eq!(
        (status, first.hits(), applied.hits()),
        (200, 0, 1),
        "only the applied target may receive the request: {body}"
    );

    let (admin_status, _, admin_body) = admin_get(
        &proxy,
        "/admin/router-table",
        Some(&proxy.virtual_key),
        None,
    );
    assert_eq!(admin_status, 200, "admin body={admin_body}");
    let table: Value = serde_json::from_str(&admin_body).expect("router table is JSON");
    assert_eq!(table["default_pool"], "direct");
    assert_eq!(table["pools"].as_array().map(Vec::len), Some(1));
    assert_eq!(table["pools"][0]["pool"], "direct");
    assert_eq!(table["pools"][0]["upstream"], "z_applied");
    assert_eq!(table["pools"][0]["model"], "gpt-5.5");
    std::fs::remove_file(key).ok();
}

fn start_quota_proxy_two(
    primary: &MockUpstream,
    fallback: &MockUpstream,
    key_file: &Path,
) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-proxy-quota-{}-{}",
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
    // Quota-first mode with an explicit account order: mock_primary is the
    // preferred account (index 0). `eject_after: 3` keeps a single 429 from
    // health-ejecting the primary, so the only thing that can drop it from
    // rotation on the next request is the quota cooldown under test.
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
            "routing_mode": "quota_first",
            "quota_accounts": [
                { "upstream": "mock_primary", "model": "gpt-5.5" },
                { "upstream": "mock_fallback", "model": "gpt-5.5" }
            ]
        }
    });
    let config: ClientConfig = serde_json::from_value(config).expect("test config parses");
    let recorder = Arc::new(token_station_cli::filelog::Recorders(vec![
        Box::new(token_station_cli::filelog::FileLog::open(&config.data.dir).expect("log opens")),
        Box::new(
            token_station_cli::store::SqliteStore::open(&config.data.dir.join("metrics.sqlite"))
                .expect("store opens"),
        ),
    ]));
    let gateway = Arc::new(Gateway::new(&config, recorder).expect("gateway assembles"));
    let (virtual_key, _) =
        token_station_cli::virtual_key::load_or_create(&config.data.dir).expect("key creates");
    let state = server::AppState::new(
        Arc::clone(&gateway),
        Some(Arc::from(virtual_key.as_str())),
        Arc::new(
            token_station_cli::admin::AdminContext::from_config(&config)
                .expect("admin snapshot compiles"),
        ),
    );
    let control = state.control.clone();
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
        control,
        gateway,
    }
}

#[test]
fn quota_first_cools_an_account_the_upstream_rate_limited() {
    // The preferred account (mock_primary) hits a 429; the exchange fails over
    // to mock_fallback and succeeds. The 429 is ground truth that primary's
    // allowance is spent, so the L1 feedback loop cools it — and a *later,
    // unrelated* request must skip primary entirely rather than re-provoking the
    // rate limit.
    let ok = json!({
        "id": "cmpl-1",
        "object": "chat.completion",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 5, "completion_tokens": 1 }
    });
    // No `Retry-After` on the 429, so the cooldown uses the default and no
    // inter-attempt wait slows the test. The mock repeats its last scripted
    // response, so a 2nd hit on primary (i.e. a cooldown failure) would still
    // 429 — but the assertion below catches it by hit count.
    let primary = MockUpstream::start(vec![vec![http_json(
        429,
        &json!({ "error": { "message": "slow down" } }).to_string(),
    )]]);
    let fallback = MockUpstream::start(vec![vec![http_json(200, &ok.to_string())]]);
    let key = key_file("quota-cooldown", "sk-test-key-abc\n");
    let proxy = start_quota_proxy_two(&primary, &fallback, &key);

    // Request 1: quota prefers primary (listed first) → 429 → retries fallback → 200.
    let (s1, b1) = post_chat(
        &proxy,
        &json!({ "model": "auto", "messages": [{ "role": "user", "content": "first question" }] }),
        None,
    );
    assert_eq!(s1, 200, "{b1}");
    assert_eq!(primary.hits(), 1, "request 1 tries the preferred account");
    assert_eq!(fallback.hits(), 1, "and fails over to the healthy account");

    // Request 2 is a *different* conversation (distinct messages ⇒ no session
    // affinity to the fallback), so without the cooldown quota routing would
    // again prefer primary and re-provoke the 429. It must instead skip the
    // cooling account and go straight to the fallback.
    let (s2, b2) = post_chat(
        &proxy,
        &json!({ "model": "auto", "messages": [{ "role": "user", "content": "an entirely separate ask" }] }),
        None,
    );
    assert_eq!(s2, 200, "{b2}");
    assert_eq!(
        primary.hits(),
        1,
        "the cooled account is not retried by the next request"
    );
    assert_eq!(fallback.hits(), 2, "the healthy account serves it directly");
}

#[test]
fn quota_snapshot_reports_authoritative_windows_from_response_headers() {
    // A successful response carries the provider's own remaining/limit/reset
    // headers; the gateway harvests them (L2) and the /admin/quota snapshot
    // reports that account's window as authoritative, no local estimate needed.
    let ok = json!({
        "id": "cmpl-1",
        "object": "chat.completion",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "ok" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 5, "completion_tokens": 1 }
    });
    let primary = MockUpstream::start(vec![vec![http_json_with_headers(
        200,
        &ok.to_string(),
        &[
            ("x-ratelimit-limit-tokens", "1000"),
            ("x-ratelimit-remaining-tokens", "250"),
            ("x-ratelimit-reset-tokens", "300"),
        ],
    )]]);
    let fallback = MockUpstream::start(vec![vec![http_json(200, &ok.to_string())]]);
    let key = key_file("quota-snapshot", "sk-test-key-abc\n");
    let proxy = start_quota_proxy_two(&primary, &fallback, &key);

    let (status, body) = post_chat(
        &proxy,
        &json!({ "model": "auto", "messages": [{ "role": "user", "content": "hi" }] }),
        None,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(primary.hits(), 1, "routed to the preferred account");

    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let snapshot = agent
        .get(format!("{}/admin/quota", proxy.url))
        .header("authorization", &format!("Bearer {}", proxy.virtual_key))
        .call()
        .expect("admin/quota answers");
    assert_eq!(snapshot.status().as_u16(), 200);
    let snapshot: Value =
        serde_json::from_str(&snapshot.into_body().read_to_string().expect("body reads"))
            .expect("snapshot is JSON");

    let accounts = snapshot["accounts"].as_array().expect("accounts array");
    let primary_account = accounts
        .iter()
        .find(|account| account["upstream"] == "mock_primary")
        .expect("mock_primary present");
    assert_eq!(
        primary_account["source"], "authoritative",
        "figures came from the provider's headers, not a local estimate"
    );
    let window = &primary_account["windows"][0];
    assert_eq!(window["limit"], 1000);
    assert_eq!(window["remaining_permille"], 250);
    assert_eq!(window["ms_until_reset"], 300_000);
}

fn start_proxy_many(upstream: &MockUpstream, count: usize, key_file: &Path) -> Proxy {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-proxy-many-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    let mut upstreams = serde_json::Map::new();
    let mut members = Vec::new();
    for index in 0..count {
        let name = format!("candidate_{index:02}");
        upstreams.insert(
            name.clone(),
            json!({
                "provider": "openai-compatible",
                "base_url": upstream.base_url(),
                "auth": {"slot": "provider_api_key", "file": key_file},
                "models": [{"model": "gpt-5.5", "tool": true, "context_window": 400_000}]
            }),
        );
        members.push(json!({"upstream": name, "model": "gpt-5.5"}));
    }
    let config: ClientConfig = serde_json::from_value(json!({
        "version": 1,
        "server": {"listen": "127.0.0.1:0"},
        "data": {"dir": data_dir, "metrics": true},
        "plugins": {
            "dir": plugins_dir(),
            "agent": "agent-openai",
            "providers": {"openai-compatible": "provider-openai-compatible"}
        },
        "upstreams": upstreams,
        "router": {
            "version": 1,
            "pools": {"main": members},
            "default_pool": "main"
        }
    }))
    .expect("many-upstream config parses");
    let recorder = Arc::new(token_station_cli::filelog::Recorders(vec![
        Box::new(token_station_cli::filelog::FileLog::open(&config.data.dir).expect("log opens")),
        Box::new(
            token_station_cli::store::SqliteStore::open(&config.data.dir.join("metrics.sqlite"))
                .expect("store opens"),
        ),
    ]));
    let gateway = Arc::new(Gateway::new(&config, recorder).expect("gateway assembles"));
    let (virtual_key, _) =
        token_station_cli::virtual_key::load_or_create(&config.data.dir).expect("key creates");
    let state = server::AppState::new(
        Arc::clone(&gateway),
        Some(Arc::from(virtual_key.as_str())),
        Arc::new(
            token_station_cli::admin::AdminContext::from_config(&config)
                .expect("admin snapshot compiles"),
        ),
    );
    let control = state.control.clone();
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
        control,
        gateway,
    }
}

#[test]
fn malformed_2xx_and_truncated_bodies_fallback_without_recording_empty_success() {
    let valid = json!({
        "id": "chatcmpl-fallback",
        "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "fallback"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    })
    .to_string();
    let truncated = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 999\r\nconnection: close\r\n\r\n{\"choices\":[".to_vec();
    let cases = [
        http_json(200, r#"{"error":{"message":"smuggled"}}"#),
        http_json(200, r#"{"choices":[]}"#),
        http_json(200, "not-json"),
        truncated,
    ];

    for (index, primary_response) in cases.into_iter().enumerate() {
        let primary = MockUpstream::start(vec![vec![primary_response]]);
        let fallback = MockUpstream::start(vec![vec![http_json(200, &valid)]]);
        let key = key_file(&format!("protocol-fallback-{index}"), "sk-test-key");
        let proxy = start_proxy_two(&primary, &fallback, &key);

        let (status, body) = post_chat(
            &proxy,
            &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
            None,
        );
        assert_eq!(status, 200, "case {index}: {body}");
        assert!(body.contains("fallback"), "case {index}: {body}");
        assert_eq!((primary.hits(), fallback.hits()), (1, 1));
        settle();
        let row = last_row(&proxy.data_dir);
        assert_eq!(row["attempts"], "Integer(2)", "case {index}");
        assert_eq!(row["error_code"], "Null", "case {index}");
        assert_eq!(
            row["upstream"], "Text(\"mock_fallback\")",
            "legacy routing keeps the actual server"
        );

        let db = rusqlite::Connection::open(proxy.data_dir.join("metrics.sqlite"))
            .expect("receipt db opens");
        let original: String = db
            .query_row("SELECT upstream FROM decisions", [], |row| row.get(0))
            .expect("original decision exists");
        assert_eq!(original, "mock_primary", "fallback cannot rewrite decision");
        let mut statement = db
            .prepare(
                "SELECT ordinal, upstream, http_status, error_code, stream_outcome, fallback_allowed
                   FROM attempts ORDER BY ordinal",
            )
            .expect("attempt query prepares");
        let attempts = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<u16>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, bool>(5)?,
                ))
            })
            .expect("attempts query")
            .collect::<Result<Vec<_>, _>>()
            .expect("attempt rows decode");
        assert_eq!(attempts.len(), 2);
        assert_eq!((attempts[0].0, attempts[0].1.as_str()), (1, "mock_primary"));
        assert_eq!(
            attempts[0].2,
            Some(200),
            "attempt keeps the upstream HTTP status when its body is invalid"
        );
        assert!(attempts[0].3.is_some(), "first attempt keeps its error");
        assert_eq!(attempts[0].4.as_deref(), Some("failed_before_output"));
        assert!(attempts[0].5, "classified failure permits fallback");
        assert_eq!(
            (attempts[1].0, attempts[1].1.as_str()),
            (2, "mock_fallback")
        );
        assert_eq!(attempts[1].2, Some(200));
        assert_eq!(attempts[1].3, None);
        assert_eq!(attempts[1].4.as_deref(), Some("complete"));
        assert!(!attempts[1].5);
        let conversions: i64 = db
            .query_row("SELECT COUNT(*) FROM conversion_reports", [], |row| {
                row.get(0)
            })
            .expect("conversion count");
        assert!(conversions >= 6, "both attempts carry stage receipts");
        std::fs::remove_file(key).ok();
    }
}

#[test]
fn malformed_2xx_and_truncated_bodies_have_stable_terminal_receipts() {
    let truncated = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 999\r\nconnection: close\r\n\r\n{\"choices\":[".to_vec();
    let cases = [
        (
            http_json(200, r#"{"error":{"message":"smuggled"}}"#),
            "provider_protocol_error",
        ),
        (
            http_json(200, r#"{"choices":[]}"#),
            "provider_protocol_error",
        ),
        (http_json(200, "not-json"), "provider_protocol_error"),
        (truncated, "transport_truncated"),
    ];

    for (index, (response, expected)) in cases.into_iter().enumerate() {
        let mock = MockUpstream::start(vec![vec![response]]);
        let key = key_file(&format!("protocol-terminal-{index}"), "sk-test-key");
        let proxy = start_proxy(&mock, &key);
        let (status, _) = post_chat(
            &proxy,
            &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
            None,
        );
        assert_eq!(status, 502, "case {index}");
        settle();
        let row = last_row(&proxy.data_dir);
        assert_eq!(row["status"], "Integer(502)", "case {index}");
        assert_eq!(
            row["error_code"],
            format!("Text(\"{expected}\")"),
            "case {index}"
        );
        std::fs::remove_file(key).ok();
    }
}

#[test]
fn http_402_404_and_529_map_to_the_stable_error_catalog() {
    let cases = [
        (402, "", "payment_required"),
        (404, "", "invalid_request"),
        (529, "", "capacity"),
        (400, "context_length_exceeded", "context_length"),
    ];
    for (status, provider_code, expected) in cases {
        let mock = MockUpstream::start(vec![vec![http_json(
            status,
            &json!({"error": {"message": "fixture", "code": provider_code}}).to_string(),
        )]]);
        let key = key_file(&format!("status-{status}"), "sk-test-key");
        let proxy = start_proxy(&mock, &key);
        let (actual, _) = post_chat(
            &proxy,
            &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
            None,
        );
        assert_eq!(actual, status);
        settle();
        assert_eq!(
            last_row(&proxy.data_dir)["error_code"],
            format!("Text(\"{expected}\")")
        );
        std::fs::remove_file(key).ok();
    }
}

#[test]
fn retry_after_waits_within_the_budget_before_fallback() {
    let primary = MockUpstream::start(vec![vec![http_json_with_headers(
        429,
        r#"{"error":{"message":"slow down"}}"#,
        &[("retry-after", "1")],
    )]]);
    let answer = json!({
        "id": "chatcmpl-ok", "model": "gpt-5.5",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    });
    let fallback = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key = key_file("retry-after-budget", "sk-test-key");
    let proxy = start_proxy_two(&primary, &fallback, &key);
    let started = std::time::Instant::now();
    let (status, _) = post_chat(
        &proxy,
        &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
        None,
    );
    assert_eq!(status, 200);
    assert!(started.elapsed() >= std::time::Duration::from_millis(900));
    assert_eq!((primary.hits(), fallback.hits()), (1, 1));
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["status"], "Integer(200)");
    assert_eq!(row["error_code"], "Null");
    assert_eq!(row["attempts"], "Integer(2)");
    std::fs::remove_file(key).ok();
}

#[test]
fn twenty_failing_upstreams_never_exceed_the_attempt_budget() {
    let failure = r#"{"error":{"message":"unavailable"}}"#;
    let mock = MockUpstream::start(vec![vec![http_json(503, failure)]]);
    let key = key_file("twenty-upstreams", "sk-test-key");
    let proxy = start_proxy_many(&mock, 20, &key);
    let (status, _) = post_chat(
        &proxy,
        &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
        None,
    );
    assert_eq!(status, 503);
    assert_eq!(mock.hits(), 6);
    settle();
    let row = last_row(&proxy.data_dir);
    assert_eq!(row["attempts"], "Integer(6)");
    assert_eq!(row["status"], "Integer(503)");
    assert_eq!(row["error_code"], "Text(\"upstream_unavailable\")");
    std::fs::remove_file(key).ok();
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
    gateway_for_base_url(&upstream.base_url(), key_file, plugins_dir())
}

fn south_probe_gateway(upstream: &MockUpstream, secret: &str) -> (Gateway, PathBuf) {
    let (config_path, data_dir) = write_south_probe_config(upstream, secret, true);
    let config = ClientConfig::load(&config_path).expect("approved South probe config loads");
    let gateway = Gateway::new(&config, Arc::new(token_station_metrics::NoopRecorder))
        .expect("South probe gateway assembles");
    (gateway, data_dir)
}

fn start_south_production_proxy(upstream: &MockUpstream, secret: &str) -> Proxy {
    let (config_path, _) = write_south_probe_config(upstream, secret, true);
    let mut config: Value = serde_json::from_slice(
        &std::fs::read(&config_path).expect("South production config reads"),
    )
    .expect("South production config is JSON");
    config["data"]["metrics"] = json!(true);
    config["upstreams"]["mock_primary"]["provider_call"] = json!("south_v1_buffered");
    let config: ClientConfig =
        serde_json::from_value(config).expect("South production config parses");
    spawn_proxy(&config)
}

fn start_south_streaming_production_proxy(upstream: &MockUpstream, secret: &str) -> Proxy {
    let (config_path, _) = write_south_probe_config(upstream, secret, true);
    let mut config: Value = serde_json::from_slice(
        &std::fs::read(&config_path).expect("South production config reads"),
    )
    .expect("South production config is JSON");
    config["data"]["metrics"] = json!(true);
    config["upstreams"]["mock_primary"]["provider_call"] = json!("south_v1_buffered_streaming");
    let config: ClientConfig =
        serde_json::from_value(config).expect("South streaming production config parses");
    spawn_proxy(&config)
}

#[test]
fn production_buffered_opt_in_executes_south_once_and_records_the_actual_engine() {
    let answer = json!({
        "id": "chatcmpl-production-south", "model": "gpt-5.5",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "south" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 2, "completion_tokens": 1 }
    });
    let mock = MockUpstream::start(vec![vec![http_json_with_headers(
        200,
        &answer.to_string(),
        &[
            ("x-ratelimit-limit-tokens", "1000"),
            ("x-ratelimit-remaining-tokens", "900"),
            ("x-ratelimit-reset-tokens", "1s"),
        ],
    )]]);
    let proxy = start_south_production_proxy(&mock, "sk-south-production");

    let (status, body) = post_chat(
        &proxy,
        &json!({
            "model": "auto",
            "messages": [{ "role": "user", "content": "hi" }]
        }),
        None,
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(mock.hits(), 1, "one attempt must issue one request");
    let seen = mock.seen();
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some("Bearer sk-south-production")
    );

    settle();
    let (admin_status, _, receipts_body) =
        admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    assert_eq!(admin_status, 200, "{receipts_body}");
    let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
    assert_eq!(
        receipts[0]["attempt_records"][0]["provider_call_engine"],
        json!("south_v1_buffered"),
        "the receipt records the engine that actually performed the attempt: {receipts}"
    );

    let (_, _, quota_body) = admin_get(&proxy, "/admin/quota", Some(&proxy.virtual_key), None);
    let quota: Value = serde_json::from_str(&quota_body).expect("quota snapshot is JSON");
    let account = quota["accounts"]
        .as_array()
        .and_then(|accounts| {
            accounts
                .iter()
                .find(|account| account["upstream"] == "mock_primary")
        })
        .expect("South quota account is projected");
    assert_eq!(account["source"], json!("authoritative"));
    assert_eq!(account["windows"][0]["limit"], json!(1000));
    assert_eq!(account["windows"][0]["remaining_permille"], json!(900));
}

#[test]
fn production_south_failures_are_fail_closed_and_never_replayed_by_legacy() {
    let provider_error = |status| {
        http_json(
            status,
            &json!({"error": {"message": "fixture", "code": "fixture"}}).to_string(),
        )
    };
    let cases = [
        ("provider-401", provider_error(401), 401),
        ("provider-500", provider_error(500), 500),
        (
            "redirect",
            b"HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1/second-hop\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
            502,
        ),
        (
            "invalid-utf8",
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1\r\nconnection: close\r\n\r\n\xff".to_vec(),
            502,
        ),
        (
            "oversized",
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 33554433\r\nconnection: close\r\n\r\n".to_vec(),
            502,
        ),
    ];

    for (name, response, expected_status) in cases {
        let mock = MockUpstream::start(vec![vec![response]]);
        let proxy = start_south_production_proxy(&mock, &format!("sk-south-{name}"));
        let (status, body) = post_chat(
            &proxy,
            &json!({
                "model": "auto",
                "messages": [{ "role": "user", "content": "fail closed" }]
            }),
            None,
        );

        assert_eq!(status, expected_status, "{name}: {body}");
        assert_eq!(mock.hits(), 1, "{name}: South failure cannot replay");
        settle();
        let (_, _, receipts_body) =
            admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
        let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
        assert_eq!(
            receipts[0]["attempt_records"][0]["provider_call_engine"],
            json!("south_v1_buffered"),
            "{name}: the failed attempt still records its actual engine"
        );
    }
}

#[test]
fn production_south_opt_in_keeps_streaming_on_legacy_and_records_the_fallback() {
    let sse = concat!(
        "data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-5.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    )
    .into_bytes();
    let mock = MockUpstream::start(vec![vec![response]]);
    let proxy = start_south_production_proxy(&mock, "sk-south-stream-fallback");

    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(status, 200, "{body}");
    assert_eq!(mock.hits(), 1, "stream fallback cannot double-send");
    assert!(body.contains("data: [DONE]"), "{body}");

    settle();
    let (_, _, receipts_body) =
        admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
    assert_eq!(
        receipts[0]["attempt_records"][0]["provider_call_engine"],
        json!("legacy")
    );
}

#[test]
fn explicit_streaming_opt_in_executes_south_once_and_records_the_actual_engine() {
    let sse = concat!(
        "data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-5.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"南向\"}}]}\n\n",
        "data: [DONE]\n\n"
    );
    let head = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nx-ratelimit-limit-tokens: 1000\r\nx-ratelimit-remaining-tokens: 900\r\nx-ratelimit-reset-tokens: 1s\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        sse.len()
    )
    .into_bytes();
    let split = sse
        .as_bytes()
        .windows("南".len())
        .position(|bytes| bytes == "南".as_bytes())
        .expect("fixture contains the multibyte token")
        + 1;
    let mock = MockUpstream::start(vec![vec![
        head,
        sse.as_bytes()[..split].to_vec(),
        sse.as_bytes()[split..].to_vec(),
    ]]);
    let proxy = start_south_streaming_production_proxy(&mock, "sk-south-streaming-production");

    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("南向"), "{body}");
    assert!(body.contains("data: [DONE]"), "{body}");
    assert_eq!(mock.hits(), 1, "one streaming attempt issues one request");

    settle();
    let (_, _, receipts_body) =
        admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
    assert_eq!(
        receipts[0]["attempt_records"][0]["provider_call_engine"],
        json!("south_v1_streaming")
    );
    let (_, _, quota_body) = admin_get(&proxy, "/admin/quota", Some(&proxy.virtual_key), None);
    let quota: Value = serde_json::from_str(&quota_body).expect("quota snapshot is JSON");
    let account = quota["accounts"]
        .as_array()
        .and_then(|accounts| {
            accounts
                .iter()
                .find(|account| account["upstream"] == "mock_primary")
        })
        .expect("South streaming quota account is projected");
    assert_eq!(account["source"], json!("authoritative"));
}

#[test]
fn south_streaming_open_failures_are_never_replayed_by_legacy() {
    let cases = [
        (
            "provider-429",
            http_json(
                429,
                &json!({"error": {"message": "fixture", "code": "fixture"}}).to_string(),
            ),
            429,
        ),
        (
            "redirect",
            b"HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1/second-hop\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
            502,
        ),
    ];

    for (name, response, expected_status) in cases {
        let mock = MockUpstream::start(vec![vec![response]]);
        let proxy = start_south_streaming_production_proxy(&mock, &format!("sk-stream-{name}"));
        let (status, body) = post_chat_stream(&proxy);
        assert_eq!(status, expected_status, "{name}: {body}");
        assert_eq!(mock.hits(), 1, "{name}: South failure cannot replay");

        settle();
        let (_, _, receipts_body) =
            admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
        let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
        assert_eq!(
            receipts[0]["attempt_records"][0]["provider_call_engine"],
            json!("south_v1_streaming"),
            "{name}: failed open records the engine that performed the attempt"
        );
    }
}

#[test]
fn south_midstream_truncation_is_postcommit_and_never_replayed() {
    let prefix = concat!(
        "HTTP/1.1 200 OK\r\n",
        "content-type: text/event-stream\r\n",
        "connection: close\r\n",
        "\r\n",
        "data: {\"id\":\"chatcmpl-cut\",\"model\":\"gpt-5.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial\"}}]}\n\n"
    )
    .as_bytes()
    .to_vec();
    let mock = MockUpstream::start(vec![vec![prefix]]);
    let proxy = start_south_streaming_production_proxy(&mock, "sk-stream-truncated");

    let (status, body) = post_chat_stream(&proxy);
    assert_eq!(
        status, 200,
        "the valid first event commits the stream: {body}"
    );
    assert!(body.contains("partial"), "{body}");
    assert_eq!(mock.hits(), 1, "a committed stream can never replay");

    settle();
    let (_, _, receipts_body) =
        admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
    let attempt = &receipts[0]["attempt_records"][0];
    assert_eq!(attempt["provider_call_engine"], json!("south_v1_streaming"));
    assert_eq!(attempt["stream_outcome"], json!("failed_after_partial"));
    assert_eq!(attempt["error_code"], json!("transport_truncated"));
}

#[test]
fn production_south_server_drain_cancels_buffered_io_without_legacy_replay() {
    let mock = MockUpstream::start_hanging();
    let peer_closed = Arc::clone(&mock.peer_closed);
    let proxy = start_south_production_proxy(&mock, "sk-south-drain");
    let url = proxy.url.clone();
    let virtual_key = proxy.virtual_key.clone();
    let client = std::thread::spawn(move || {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .http_status_as_error(false)
                .build(),
        );
        agent
            .post(format!("{url}/v1/chat/completions"))
            .header("authorization", &format!("Bearer {virtual_key}"))
            .send(
                &json!({
                    "model": "auto",
                    "messages": [{ "role": "user", "content": "hang" }]
                })
                .to_string(),
            )
            .expect("proxy answers the drained South request")
            .status()
            .as_u16()
    });

    let arrival_deadline = Instant::now() + Duration::from_secs(3);
    while (mock.hits() == 0 || proxy.control.in_flight() == 0) && Instant::now() < arrival_deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(mock.hits(), 1, "South request reached the upstream once");
    proxy.control.cancel_in_flight();
    assert_eq!(client.join().expect("client joins"), 503);

    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while (proxy.control.in_flight() != 0 || !peer_closed.load(Ordering::SeqCst))
        && Instant::now() < cleanup_deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(
        proxy.control.in_flight(),
        0,
        "the blocking worker is accounted"
    );
    assert!(
        peer_closed.load(Ordering::SeqCst),
        "cancel drops reqwest I/O"
    );
    assert_eq!(mock.hits(), 1, "drain cannot replay through legacy");
    mock.finish_hanging();

    settle();
    let (_, _, receipts_body) =
        admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
    assert_eq!(receipts[0]["status"], json!(503));
    assert_eq!(
        receipts[0]["attempt_records"][0]["provider_call_engine"],
        json!("south_v1_buffered")
    );
}

#[test]
fn production_south_server_drain_cancels_streaming_pull_without_legacy_replay() {
    let mock = MockUpstream::start_hanging();
    let peer_closed = Arc::clone(&mock.peer_closed);
    let proxy = start_south_streaming_production_proxy(&mock, "sk-south-stream-drain");
    let url = proxy.url.clone();
    let virtual_key = proxy.virtual_key.clone();
    let client = std::thread::spawn(move || {
        let agent = ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(15)))
                .http_status_as_error(false)
                .build(),
        );
        agent
            .post(format!("{url}/v1/chat/completions"))
            .header("authorization", &format!("Bearer {virtual_key}"))
            .send(
                &json!({
                    "model": "auto",
                    "stream": true,
                    "messages": [{ "role": "user", "content": "hang" }]
                })
                .to_string(),
            )
            .expect("proxy answers the drained South stream")
            .status()
            .as_u16()
    });

    let arrival_deadline = Instant::now() + Duration::from_secs(3);
    while (mock.hits() == 0 || proxy.control.in_flight() == 0) && Instant::now() < arrival_deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(mock.hits(), 1, "South stream reached the upstream once");
    proxy.control.cancel_in_flight();
    assert_eq!(client.join().expect("client joins"), 503);

    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while (proxy.control.in_flight() != 0 || !peer_closed.load(Ordering::SeqCst))
        && Instant::now() < cleanup_deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(proxy.control.in_flight(), 0, "stream worker is accounted");
    assert!(
        peer_closed.load(Ordering::SeqCst),
        "cancel drops stream I/O"
    );
    assert_eq!(mock.hits(), 1, "drain cannot replay through legacy");
    mock.finish_hanging();

    settle();
    let (_, _, receipts_body) =
        admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
    assert_eq!(receipts[0]["status"], json!(503));
    assert_eq!(
        receipts[0]["attempt_records"][0]["provider_call_engine"],
        json!("south_v1_streaming")
    );
}

#[test]
fn production_south_client_disconnect_cancels_streaming_pull_without_replay() {
    let mock = MockUpstream::start_hanging();
    let peer_closed = Arc::clone(&mock.peer_closed);
    let proxy = start_south_streaming_production_proxy(&mock, "sk-south-stream-disconnect");
    let host = proxy.url.strip_prefix("http://").expect("loopback URL");
    let body = json!({
        "model": "auto",
        "stream": true,
        "messages": [{ "role": "user", "content": "disconnect" }]
    })
    .to_string();
    let mut client = TcpStream::connect(host).expect("client connects");
    write!(
        client,
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        proxy.virtual_key,
        body.len()
    )
    .expect("request writes");
    client.flush().expect("request flushes");

    let arrival_deadline = Instant::now() + Duration::from_secs(3);
    while (mock.hits() == 0 || proxy.control.in_flight() == 0) && Instant::now() < arrival_deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(mock.hits(), 1, "South stream reached the upstream once");
    client.shutdown(Shutdown::Both).expect("client disconnects");
    drop(client);

    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while (proxy.control.in_flight() != 0 || !peer_closed.load(Ordering::SeqCst))
        && Instant::now() < cleanup_deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(proxy.control.in_flight(), 0, "stream worker is accounted");
    assert!(
        peer_closed.load(Ordering::SeqCst),
        "disconnect drops streaming reqwest I/O"
    );
    assert_eq!(mock.hits(), 1, "disconnect cannot replay the stream");
    mock.finish_hanging();

    settle();
    let (_, _, receipts_body) =
        admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
    assert_eq!(receipts[0]["status"], json!(499));
    assert_eq!(
        receipts[0]["attempt_records"][0]["provider_call_engine"],
        json!("south_v1_streaming")
    );
}

#[test]
fn production_south_client_disconnect_cancels_buffered_io_without_legacy_replay() {
    let mock = MockUpstream::start_hanging();
    let peer_closed = Arc::clone(&mock.peer_closed);
    let proxy = start_south_production_proxy(&mock, "sk-south-disconnect");
    let host = proxy.url.strip_prefix("http://").expect("loopback URL");
    let body = json!({
        "model": "auto",
        "messages": [{ "role": "user", "content": "disconnect" }]
    })
    .to_string();
    let mut client = TcpStream::connect(host).expect("client connects");
    write!(
        client,
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        proxy.virtual_key,
        body.len()
    )
    .expect("request writes");
    client.flush().expect("request flushes");

    let arrival_deadline = Instant::now() + Duration::from_secs(3);
    while (mock.hits() == 0 || proxy.control.in_flight() == 0) && Instant::now() < arrival_deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(mock.hits(), 1, "South request reached the upstream once");
    client.shutdown(Shutdown::Both).expect("client disconnects");
    drop(client);

    let cleanup_deadline = Instant::now() + Duration::from_secs(3);
    while (proxy.control.in_flight() != 0 || !peer_closed.load(Ordering::SeqCst))
        && Instant::now() < cleanup_deadline
    {
        std::thread::yield_now();
    }
    assert_eq!(
        proxy.control.in_flight(),
        0,
        "disconnect releases the worker"
    );
    assert!(
        peer_closed.load(Ordering::SeqCst),
        "disconnect drops reqwest I/O"
    );
    assert_eq!(mock.hits(), 1, "disconnect cannot replay through legacy");
    mock.finish_hanging();

    settle();
    let (_, _, receipts_body) =
        admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    let receipts: Value = serde_json::from_str(&receipts_body).expect("receipts are JSON");
    assert_eq!(receipts[0]["status"], json!(499));
    assert_eq!(
        receipts[0]["attempt_records"][0]["provider_call_engine"],
        json!("south_v1_buffered")
    );
}

#[test]
fn production_south_attempt_deadline_returns_504_without_wall_sleep_or_legacy_replay() {
    let mock = MockUpstream::start_hanging_buffered();
    let (config_path, data_dir) = write_south_probe_config(&mock, "sk-south-deadline", true);
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(&config_path).expect("South deadline config reads"))
            .expect("South deadline config is JSON");
    config["upstreams"]["mock_primary"]["provider_call"] = json!("south_v1_buffered");
    let config: ClientConfig =
        serde_json::from_value(config).expect("South deadline config parses");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .expect("paused runtime builds");
    let gateway = Arc::new(
        Gateway::new_with_provider_runtime(
            &config,
            Arc::new(token_station_metrics::NoopRecorder),
            runtime.handle().clone(),
        )
        .expect("South deadline gateway assembles"),
    );
    let peer_closed = Arc::clone(&mock.peer_closed);
    let (replies, peer_closed) = runtime.block_on(async {
        let worker_gateway = Arc::clone(&gateway);
        let worker = tokio::task::spawn_blocking(move || {
            let ctx = token_station_cli::request_context::RequestContext::detached(
                Duration::from_secs(5),
                Duration::from_secs(5),
            );
            let mut replies = Vec::new();
            worker_gateway.chat_scoped(
                &ctx,
                None,
                None,
                "POST",
                "/v1/chat/completions",
                &[],
                json!({
                    "model": "auto",
                    "messages": [{ "role": "user", "content": "deadline" }]
                })
                .to_string()
                .as_bytes(),
                &mut |reply| {
                    replies.push(reply);
                    true
                },
            );
            replies
        });
        let driver = async {
            for _ in 0..100_000 {
                if mock.response_started() {
                    // The explicit server-side handshake proves the response
                    // head was flushed. Give the current-thread I/O driver a
                    // bounded virtual-time slice and polls to enter the
                    // blocked body read before crossing the attempt deadline.
                    tokio::time::advance(Duration::from_secs(1)).await;
                    for _ in 0..32 {
                        tokio::task::yield_now().await;
                    }
                    tokio::time::advance(Duration::from_secs(5)).await;
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("South deadline response never reached the loopback socket");
        };
        let (replies, ()) = tokio::join!(worker, driver);
        let replies = replies.expect("blocking worker joins");
        // A production server keeps polling its multi-thread runtime after the
        // blocking gateway worker returns. Mirror that lifecycle here so
        // runtime-owned cleanup is not starved by this current-thread fixture.
        tokio::time::advance(Duration::from_secs(1)).await;
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        let peer_closed = tokio::task::spawn_blocking(move || {
            let cleanup_deadline = Instant::now() + Duration::from_secs(1);
            while !peer_closed.load(Ordering::SeqCst) && Instant::now() < cleanup_deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            peer_closed.load(Ordering::SeqCst)
        })
        .await
        .expect("deadline cleanup observer joins");
        (replies, peer_closed)
    });

    assert_eq!(mock.hits(), 1, "deadline cannot replay through legacy");
    assert!(matches!(
        replies.first(),
        Some(Reply::BeginJson(reply)) if reply.status == 504
    ));
    assert!(peer_closed, "deadline closes the upstream socket promptly");
    drop(gateway);
    drop(runtime);
    mock.finish_hanging();
    std::fs::remove_dir_all(data_dir).ok();
}

#[test]
fn south_stream_deadline_hides_host_private_policy_from_the_agent_renderer() {
    let mock = MockUpstream::start_hanging();
    let (config_path, data_dir) = write_south_probe_config(&mock, "sk-south-private-marker", true);
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(&config_path).expect("South marker config reads"))
            .expect("South marker config is JSON");
    config["upstreams"]["mock_primary"]["provider_call"] = json!("south_v1_buffered_streaming");
    config["plugins"]["agents"] = json!(["marker-agent"]);
    config["plugins"]["allow_unsigned"] = json!(true);
    let config: ClientConfig = serde_json::from_value(config).expect("South marker config parses");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime builds");
    let gateway = Arc::new(
        Gateway::new_with_provider_runtime(
            &config,
            Arc::new(token_station_metrics::NoopRecorder),
            runtime.handle().clone(),
        )
        .expect("South marker gateway assembles"),
    );
    let replies = runtime.block_on(async {
        let worker_gateway = Arc::clone(&gateway);
        let worker = tokio::task::spawn_blocking(move || {
            let ctx = token_station_cli::request_context::RequestContext::detached(
                Duration::from_millis(1_200),
                Duration::from_secs(1),
            );
            let mut replies = Vec::new();
            worker_gateway.chat_scoped(
                &ctx,
                None,
                None,
                "POST",
                "/v1/chat/completions",
                &[],
                json!({
                    "model": "auto",
                    "stream": true,
                    "messages": [{ "role": "user", "content": "deadline" }]
                })
                .to_string()
                .as_bytes(),
                &mut |reply| {
                    replies.push(reply);
                    true
                },
            );
            replies
        });
        worker.await.expect("blocking worker joins")
    });

    let reply = match replies.first() {
        Some(Reply::BeginJson(reply)) => reply,
        Some(Reply::BeginStream) => panic!("expected JSON, stream began"),
        Some(Reply::Chunk(chunk)) => panic!("expected JSON, got chunk: {chunk}"),
        None => panic!("expected a rendered 504, got no reply"),
    };
    assert_eq!(
        reply.status, 504,
        "unexpected rendered error: {}",
        reply.body
    );
    let body = &reply.body;
    let rendered: Value = serde_json::from_str(body).expect("marker renderer returns JSON");
    assert_eq!(
        rendered["saw_private_marker"],
        json!(false),
        "host-private fallback policy crossed the Agent WASM boundary: {body}"
    );
    assert_eq!(mock.hits(), 1, "deadline cannot replay through legacy");
    drop(gateway);
    drop(runtime);
    mock.finish_hanging();
    std::fs::remove_dir_all(data_dir).ok();
}

fn write_south_probe_config(
    upstream: &MockUpstream,
    secret: &str,
    conformance_approved: bool,
) -> (PathBuf, PathBuf) {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let data_dir = std::env::temp_dir().join(format!(
        "ts-south-cli-data-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&data_dir).expect("South CLI data dir is writable");
    if conformance_approved {
        let provider_dir = plugins_dir().join("provider-openai-compatible");
        let package_digest = token_station_release::plugin_package_digest(&provider_dir)
            .expect("assembled official provider package has a stable digest");
        let receipts = json!({
            "provider-openai-compatible": {
                "package_digest": package_digest,
                "suite": "provider-protocol-v1",
                "publisher_signature_verified": false
            }
        });
        std::fs::write(
            data_dir.join("plugin-receipts.json"),
            serde_json::to_vec_pretty(&receipts).expect("receipt JSON serializes"),
        )
        .expect("receipt file is writable");
    }
    token_station_cli::secrets::store_set(&data_dir, "mock_primary", "provider_api_key", secret)
        .expect("test secret is stored");
    let config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "data": { "dir": data_dir, "metrics": false },
        "plugins": {
            "dir": plugins_dir(),
            "agent": "agent-openai",
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "mock_primary": {
                "provider": "openai-compatible",
                "base_url": upstream.base_url(),
                "auth": { "slot": "provider_api_key", "store": true },
                "models": [ { "model": "gpt-5.5", "tool": true, "context_window": 400_000 } ]
            }
        },
        "router": {
            "version": 1,
            "pools": { "main": [ { "upstream": "mock_primary", "model": "gpt-5.5" } ] },
            "default_pool": "main"
        }
    });
    let config_path = data_dir.join("token-station.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config).expect("config JSON serializes"),
    )
    .expect("config file is writable");
    (config_path, data_dir)
}

fn gateway_for_base_url(base_url: &str, key_file: &Path, plugin_dir: &Path) -> Gateway {
    let config = json!({
        "version": 1,
        "server": { "listen": "127.0.0.1:0" },
        "plugins": {
            "dir": plugin_dir,
            "agent": "agent-openai",
            "providers": { "openai-compatible": "provider-openai-compatible" }
        },
        "upstreams": {
            "mock_primary": {
                "provider": "openai-compatible",
                "base_url": base_url,
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
fn south_v1_probe_runs_the_official_plugin_and_real_reqwest_transport_once() {
    let answer = json!({
        "id": "chatcmpl-south", "model": "gpt-5.5",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "p" }, "finish_reason": "length" }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1 }
    });
    let mock = MockUpstream::start(vec![vec![http_json(201, &answer.to_string())]]);
    let (gateway, data_dir) = south_probe_gateway(&mock, "sk-south-loopback");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread diagnostic runtime builds");

    let outcomes = runtime
        .block_on(gateway.probe_south_v1("mock_primary", Some("gpt-5.5")))
        .expect("South diagnostic probe runs");

    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].latency_ms.is_ok(), "{outcomes:?}");
    let seen = mock.seen();
    assert_eq!(
        seen.len(),
        1,
        "South failure must never replay through legacy"
    );
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some("Bearer sk-south-loopback")
    );
    assert_eq!(seen[0].content_type.as_deref(), Some("application/json"));
    assert_eq!(seen[0].body["model"], json!("gpt-5.5"));
    assert_eq!(seen[0].body["max_tokens"], json!(1));

    std::fs::remove_dir_all(data_dir).ok();
}

#[test]
fn south_v1_probe_preserves_provider_errors_and_transport_contract_failures() {
    let provider_cases = [
        (400, "InvalidRequest"),
        (429, "RateLimit"),
        (500, "UpstreamUnavailable"),
    ];
    for (status, expected_code) in provider_cases {
        let body = json!({ "error": { "message": "bounded fixture" } }).to_string();
        let mock = MockUpstream::start(vec![vec![http_json(status, &body)]]);
        let (gateway, data_dir) = south_probe_gateway(&mock, "sk-south-provider-error");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread diagnostic runtime builds");

        let outcomes = runtime
            .block_on(gateway.probe_south_v1("mock_primary", Some("gpt-5.5")))
            .expect("provider HTTP status is an assembled outcome");
        let error = outcomes[0]
            .latency_ms
            .as_ref()
            .expect_err("provider error status cannot be a successful probe");
        assert!(error.contains(expected_code), "{status}: {error}");
        assert_eq!(mock.hits(), 1, "{status} cannot trigger a legacy replay");
        std::fs::remove_dir_all(data_dir).ok();
    }

    let transport_cases = [
        (
            b"HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1/second-hop\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_vec(),
            "UpstreamUnavailable",
        ),
        (
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1\r\nconnection: close\r\n\r\n\xff".to_vec(),
            "ProviderProtocolError",
        ),
        (
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 33554433\r\nconnection: close\r\n\r\n".to_vec(),
            "ProviderProtocolError",
        ),
    ];
    for (response, expected_code) in transport_cases {
        let mock = MockUpstream::start(vec![vec![response]]);
        let (gateway, data_dir) = south_probe_gateway(&mock, "sk-south-transport-error");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread diagnostic runtime builds");

        let outcomes = runtime
            .block_on(gateway.probe_south_v1("mock_primary", Some("gpt-5.5")))
            .expect("transport contract failure is a per-model outcome");
        let error = outcomes[0]
            .latency_ms
            .as_ref()
            .expect_err("invalid transport response cannot be successful");
        assert!(error.contains(expected_code), "{error}");
        assert_eq!(
            mock.hits(),
            1,
            "South failure cannot trigger a legacy replay"
        );
        std::fs::remove_dir_all(data_dir).ok();
    }
}

#[test]
fn south_v1_probe_timeout_uses_structured_paused_time_and_closes_the_socket() {
    let mock = MockUpstream::start_hanging();
    let (gateway, data_dir) = south_probe_gateway(&mock, "sk-south-timeout");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        .build()
        .expect("paused current-thread diagnostic runtime builds");

    let outcomes = runtime.block_on(async {
        let driver = async {
            let mut started = false;
            for _ in 0..100_000 {
                if mock.hits() == 1 {
                    started = true;
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(started, "the real South request must reach loopback");
            for _ in 0..32 {
                tokio::task::yield_now().await;
            }
            tokio::time::advance(Duration::from_secs(16)).await;
        };
        let (outcomes, ()) = tokio::join!(
            gateway.probe_south_v1("mock_primary", Some("gpt-5.5")),
            driver
        );
        outcomes.expect("timeout is a per-model diagnostic outcome")
    });

    let error = outcomes[0]
        .latency_ms
        .as_ref()
        .expect_err("a hanging response must time out");
    assert!(error.contains("Timeout"), "{error}");
    assert_eq!(mock.hits(), 1, "timeout cannot replay through legacy");
    drop(runtime);
    let peer_closed = Arc::clone(&mock.peer_closed);
    mock.finish_hanging();
    assert!(
        peer_closed.load(Ordering::SeqCst),
        "dropping the South future must close I/O"
    );
    std::fs::remove_dir_all(data_dir).ok();
}

#[test]
fn cli_south_transport_reports_the_existing_success_shape() {
    let answer = json!({
        "id": "chatcmpl-cli-south", "model": "gpt-5.5",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "p" }, "finish_reason": "length" }]
    });
    let mock = MockUpstream::start(vec![vec![http_json(201, &answer.to_string())]]);
    let (config_path, data_dir) = write_south_probe_config(&mock, "sk-south-cli-success", true);

    let output = Command::new(env!("CARGO_BIN_EXE_token-station-cli"))
        .args([
            "--config",
            config_path.to_str().expect("test path is UTF-8"),
            "upstream",
            "test",
            "mock_primary",
            "--model",
            "gpt-5.5",
            "--transport",
            "south-v1",
        ])
        .output()
        .expect("CLI process runs");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("gpt-5.5: ok ("), "{stdout}");
    assert!(stdout.ends_with(" ms)\n"), "{stdout}");
    assert_eq!(mock.hits(), 1);
    std::fs::remove_dir_all(data_dir).ok();
}

#[test]
fn cli_south_transport_refuses_operator_vouched_package_before_network() {
    let answer = json!({
        "id": "chatcmpl-legacy-would-succeed", "model": "gpt-5.5",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "p" }, "finish_reason": "length" }]
    });
    let mock = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let secret = "sk-south-cli-must-not-leak";
    let (config_path, data_dir) = write_south_probe_config(&mock, secret, false);

    let output = Command::new(env!("CARGO_BIN_EXE_token-station-cli"))
        .args([
            "--config",
            config_path.to_str().expect("test path is UTF-8"),
            "upstream",
            "test",
            "mock_primary",
            "--model",
            "gpt-5.5",
            "--transport",
            "south-v1",
        ])
        .output()
        .expect("CLI process runs");

    assert!(
        !output.status.success(),
        "unsigned package must fail closed"
    );
    assert_eq!(
        mock.hits(),
        0,
        "ineligible South probe cannot replay via legacy"
    );
    let rendered = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(rendered.contains("ProviderPackageUnapproved"), "{rendered}");
    assert!(!rendered.contains(secret), "credential leaked: {rendered}");

    std::fs::remove_dir_all(data_dir).ok();
}

#[test]
fn provider_feature_probe_executes_stream_tool_and_json_requests() {
    let sse = concat!(
        "data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-5.5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"OK\"},\"finish_reason\":null}]}\n\n",
        "data: {\"id\":\"chatcmpl-stream\",\"model\":\"gpt-5.5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let stream = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{sse}",
        sse.len()
    )
    .into_bytes();
    let tool = json!({
        "id": "chatcmpl-tool", "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant", "content": null,
                "tool_calls": [{
                    "id": "call_health", "type": "function",
                    "function": {"name": "provider_health_check", "arguments": "{}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let structured = json!({
        "id": "chatcmpl-json", "model": "gpt-5.5",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "{\"ok\":true}"},
            "finish_reason": "stop"
        }]
    });
    let mock = MockUpstream::start(vec![
        vec![stream],
        vec![http_json(200, &tool.to_string())],
        vec![http_json(200, &structured.to_string())],
    ]);
    let key = key_file("probe-features", "sk-test-key-abc");
    let gateway = probe_gateway(&mock, &key);

    let result = gateway
        .probe_features("mock_primary", "gpt-5.5")
        .expect("feature probes run");
    assert_eq!(
        result
            .stages
            .iter()
            .map(|stage| (stage.layer, stage.status))
            .collect::<Vec<_>>(),
        vec![
            (FeatureLayer::Stream, StageStatus::Pass),
            (FeatureLayer::Tool, StageStatus::Pass),
            (FeatureLayer::Json, StageStatus::Pass),
        ]
    );
    let serialized = serde_json::to_value(&result).expect("feature probe is serializable");
    assert!(
        serialized["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["duration_ms"].is_u64())
    );
    let seen = mock.seen();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[0].body["stream"], json!(true));
    assert_eq!(
        seen[1].body["tools"][0]["function"]["name"],
        json!("provider_health_check")
    );
    assert_eq!(
        seen[2].body["response_format"]["type"],
        json!("json_schema")
    );
    assert_eq!(
        seen[2].body["response_format"]["json_schema"]["name"],
        json!("provider_health_check")
    );

    std::fs::remove_file(key).ok();
}

#[test]
fn an_upstream_probe_refuses_redirects_without_a_second_hop() {
    let canary = MockUpstream::start(Vec::new());
    let location = format!("http://127.0.0.1:{}/probe-canary", canary.port);
    let source = MockUpstream::start(vec![vec![http_redirect(&location)]]);
    let key_value = "sk-probe-redirect-secret";
    let key = key_file("probe-redirect", key_value);
    let gateway = probe_gateway(&source, &key);

    let outcomes = gateway.probe("mock_primary", None).expect("probe runs");
    let error = outcomes[0]
        .latency_ms
        .as_ref()
        .expect_err("redirect cannot be a successful probe");

    assert!(error.contains("redirect refused"), "{error}");
    assert_eq!(source.hits(), 1);
    assert_eq!(canary.hits(), 0, "probe target stays untouched");
    assert_eq!(
        source.seen()[0].authorization.as_deref(),
        Some(format!("Bearer {key_value}").as_str())
    );
    assert!(!error.contains(key_value));
    assert!(!error.contains(&location));

    let layered = gateway
        .probe_layered("mock_primary", None)
        .expect("layered probe runs");
    assert_eq!(layered[0].stages[0].status, StageStatus::Fail);
    assert!(
        layered[0].stages[0]
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("redirect refused"))
    );
    assert_eq!(
        source.hits(),
        2,
        "both probe surfaces hit only the first hop"
    );
    assert_eq!(canary.hits(), 0, "layered probe target stays untouched");

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

#[test]
fn unauthorized_requests_are_rejected_before_the_server_reads_the_body() {
    let mock = MockUpstream::start(vec![vec![http_json(200, "{}")]]);
    let key = key_file("auth-before-body", "sk-test-key-abc");
    let proxy = start_proxy(&mock, &key);
    let host = proxy.url.strip_prefix("http://").expect("loopback URL");
    let mut stream = TcpStream::connect(host).expect("loopback connects");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("bounded read");
    write!(
        stream,
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {host}\r\n\
         Content-Type: application/json\r\nContent-Length: 104857600\r\n\
         Connection: close\r\n\r\n"
    )
    .expect("request headers write");
    stream.flush().expect("request headers flush");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("authentication refusal arrives without waiting for the declared body");
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    assert_eq!(mock.hits(), 0, "unauthorized bytes never reach the gateway");

    std::fs::remove_file(key).ok();
}

// ---------------------------------------------------------------------------
// `/admin/*` — the read-only data plane behind the same virtual-key gate.

fn admin_get(
    proxy: &Proxy,
    path: &str,
    token: Option<&str>,
    origin: Option<&str>,
) -> (u16, Option<String>, String) {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build(),
    );
    let mut request = agent.get(format!("{}{path}", proxy.url));
    if let Some(token) = token {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    if let Some(origin) = origin {
        request = request.header("origin", origin);
    }
    let response = request.call().expect("the proxy answers");
    let status = response.status().as_u16();
    let allow_origin = response
        .headers()
        .get("access-control-allow-origin")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = response.into_body().read_to_string().expect("body reads");
    (status, allow_origin, body)
}

#[test]
fn admin_data_plane_sits_behind_the_virtual_key() {
    let upstream = MockUpstream::start(vec![]);
    let key_file = key_file("admin-auth", "sk-admin-auth");
    let proxy = start_proxy(&upstream, &key_file);

    let (status, _, _) = admin_get(&proxy, "/admin/router-table", None, None);
    assert_eq!(status, 401, "no key, no data");
    let (status, _, _) = admin_get(&proxy, "/admin/stats?since=all", Some("wrong-key"), None);
    assert_eq!(status, 401, "a wrong key is a missing key");
    let (status, _, _) = admin_get(&proxy, "/admin/receipts", None, None);
    assert_eq!(status, 401, "receipts use the same admin gate");
    assert_eq!(
        upstream.hits(),
        0,
        "admin traffic never reaches an upstream"
    );
}

#[test]
fn admin_data_plane_serves_the_running_views() {
    let answer = json!({
        "id": "chatcmpl-admin", "model": "gpt-5.5",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1}
    });
    let upstream = MockUpstream::start(vec![vec![http_json(200, &answer.to_string())]]);
    let key_file = key_file("admin-views", "sk-admin-views");
    let proxy = start_proxy(&upstream, &key_file);
    let (request_status, _) = post_chat(
        &proxy,
        &json!({"model": "auto", "messages": [{"role": "user", "content": "hi"}]}),
        None,
    );
    assert_eq!(request_status, 200);
    settle();

    let (status, _, body) = admin_get(
        &proxy,
        "/admin/router-table",
        Some(&proxy.virtual_key),
        None,
    );
    assert_eq!(status, 200);
    let table: Value = serde_json::from_str(&body).expect("router table is JSON");
    assert_eq!(table["default_pool"], "main");
    assert_eq!(table["pools"][0]["upstream"], "mock_primary");
    assert_eq!(table["pools"][0]["model"], "gpt-5.5");

    let (status, _, body) = admin_get(
        &proxy,
        "/admin/stats?since=all",
        Some(&proxy.virtual_key),
        None,
    );
    assert_eq!(status, 200);
    let stats_view: Value = serde_json::from_str(&body).expect("stats are JSON");
    assert!(
        stats_view["total"]["requests"].is_u64(),
        "stats carry totals: {stats_view}"
    );

    let (_, _, body) = admin_get(
        &proxy,
        "/admin/stats?since=all&source=openai-chat-completions",
        Some(&proxy.virtual_key),
        None,
    );
    let source_filtered: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(source_filtered["total"]["requests"], 1);
    let (_, _, body) = admin_get(
        &proxy,
        "/admin/stats?since=all&agent=codex&source=openai-chat-completions",
        Some(&proxy.virtual_key),
        None,
    );
    let agent_filtered: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        agent_filtered["total"]["requests"], 0,
        "the unnamespaced fixture must not be misattributed to Codex"
    );

    let (status, _, body) = admin_get(&proxy, "/admin/receipts", Some(&proxy.virtual_key), None);
    assert_eq!(status, 200);
    let receipts: Value = serde_json::from_str(&body).expect("receipts are JSON");
    assert_eq!(receipts.as_array().map(Vec::len), Some(1));
    assert_eq!(
        receipts[0]["attempt_records"].as_array().map(Vec::len),
        Some(1)
    );
    assert!(
        receipts[0]["conversion_reports"]
            .as_array()
            .is_some_and(|reports| !reports.is_empty()),
        "admin returns the normalized non-empty timeline: {receipts}"
    );

    let (status, _, body) = admin_get(
        &proxy,
        "/admin/stats?since=nonsense",
        Some(&proxy.virtual_key),
        None,
    );
    assert_eq!(status, 400, "a bad window is the caller's error: {body}");

    let (status, _, body) = admin_get(&proxy, "/admin/plugins", Some(&proxy.virtual_key), None);
    assert_eq!(status, 200);
    let plugins: Value = serde_json::from_str(&body).expect("plugins view is JSON");
    assert!(
        plugins["listing"].is_string(),
        "plugins carry the listing: {plugins}"
    );

    let (status, _, body) = admin_get(&proxy, "/admin/egress", Some(&proxy.virtual_key), None);
    assert_eq!(status, 200);
    let egress: Value = serde_json::from_str(&body).expect("egress view is JSON");
    assert_eq!(egress["mode"], "direct");
    assert_eq!(egress["fixed_direct_classes"][0], "update_check");
    assert!(
        egress["routes"]
            .as_array()
            .is_some_and(|routes| routes.iter().any(|route| {
                route["request_class"] == "provider_request" && route["route"] == "direct"
            })),
        "egress exposes the actual provider route: {egress}"
    );
}

#[test]
fn admin_cors_echoes_loopback_origins_only() {
    let upstream = MockUpstream::start(vec![]);
    let key_file = key_file("admin-cors", "sk-admin-cors");
    let proxy = start_proxy(&upstream, &key_file);

    let (status, allow, _) = admin_get(
        &proxy,
        "/admin/router-table",
        Some(&proxy.virtual_key),
        Some("http://localhost:5173"),
    );
    assert_eq!(status, 200);
    assert_eq!(
        allow.as_deref(),
        Some("http://localhost:5173"),
        "dev origin is echoed"
    );

    let (status, allow, _) = admin_get(
        &proxy,
        "/admin/router-table",
        Some(&proxy.virtual_key),
        Some("https://evil.example"),
    );
    assert_eq!(status, 200, "CORS is a browser contract, not auth");
    assert_eq!(allow, None, "a web origin gets no CORS allowance");
}

#[test]
fn admin_preflight_answers_without_auth_but_stays_loopback() {
    let upstream = MockUpstream::start(vec![]);
    let key_file = key_file("admin-preflight", "sk-admin-preflight");
    let proxy = start_proxy(&upstream, &key_file);

    // Browsers send preflights without Authorization; raw HTTP because ureq
    // has no OPTIONS verb.
    let host = proxy.url.strip_prefix("http://").expect("http url");
    let mut stream = TcpStream::connect(host).expect("loopback connects");
    write!(
        stream,
        "OPTIONS /admin/stats HTTP/1.1\r\nHost: {host}\r\nOrigin: http://127.0.0.1:5173\r\nConnection: close\r\n\r\n"
    )
    .expect("request writes");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("response reads");
    assert!(
        response.starts_with("HTTP/1.1 204"),
        "preflight is 204: {response}"
    );
    assert!(
        response
            .to_ascii_lowercase()
            .contains("access-control-allow-origin: http://127.0.0.1:5173"),
        "preflight carries the loopback allowance: {response}"
    );
}
