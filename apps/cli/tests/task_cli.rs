//! Public task commands must be real CLI entry points, not library-only fixtures.
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn command_help(command: &str) {
    let log = std::env::temp_dir().join(format!("ts-task-help-{}-{command}", std::process::id()));
    let output = std::fs::File::create(&log).expect("scratch log");
    let mut child = Command::new(env!("CARGO_BIN_EXE_token-station-cli"))
        .args(["task", command, "--help"])
        .stdin(Stdio::null())
        .stdout(Stdio::from(output.try_clone().expect("clone log")))
        .stderr(Stdio::from(output))
        .spawn()
        .expect("real CLI process");
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll child") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill timed out CLI");
            child.wait().expect("reap CLI");
            panic!("CLI help exceeded 10 seconds");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let text = std::fs::read_to_string(&log).expect("read output");
    std::fs::remove_file(&log).expect("remove scratch log");
    assert!(
        status.success(),
        "task {command} must be a public command: {text}"
    );
    assert!(
        text.contains("Usage:"),
        "command must expose structured help: {text}"
    );
}

#[test]
fn task_cli_exposes_submit() {
    command_help("submit");
}
#[test]
fn task_cli_exposes_inspect() {
    command_help("inspect");
}
#[test]
fn task_cli_exposes_observe() {
    command_help("observe");
}
#[test]
fn task_cli_exposes_wait() {
    command_help("wait");
}
#[test]
fn task_cli_exposes_cancel() {
    command_help("cancel");
}
#[test]
fn task_cli_exposes_fetch() {
    command_help("fetch");
}

mod lifecycle {
    use super::*;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::{Path, PathBuf};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        Normal,
        SlowQuery,
        LostReply,
        Cancelled,
        BarrierQuery,
    }

    struct Fixture {
        root: PathBuf,
        stop: Arc<AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
        requests: Arc<Mutex<Vec<String>>>,
        trace: Arc<Mutex<Vec<String>>>,
        deadline: Instant,
        hold_query: Arc<AtomicBool>,
        query_seen: Arc<AtomicBool>,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(worker) = self.worker.take() {
                let result = worker.join();
                if !std::thread::panicking() {
                    result.expect("mock joins");
                }
            }
            if std::thread::panicking() {
                eprintln!("task fixture trace: {:#?}", self.trace.lock().unwrap());
            }
            std::fs::remove_dir_all(&self.root).expect("remove owned scratch fixture");
        }
    }
    fn hex(bytes: &[u8]) -> String {
        {
            use std::fmt::Write;
            bytes.iter().fold(String::new(), |mut out, byte| {
                write!(out, "{byte:02x}").unwrap();
                out
            })
        }
    }
    impl Fixture {
        fn new() -> Self {
            Self::with_mode(Mode::Normal)
        }
        fn with_slow_query(_: bool) -> Self {
            Self::with_mode(Mode::SlowQuery)
        }
        // Keep the controlled HTTP behavior and its isolated package/config setup together.
        #[allow(clippy::too_many_lines)]
        fn with_mode(mode: Mode) -> Self {
            let root = std::env::temp_dir().join(format!(
                "ts-task-lifecycle-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&root).expect("unique fixture directory");
            let listener = TcpListener::bind("127.0.0.1:0").expect("mock binds");
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let trace = Arc::new(Mutex::new(Vec::new()));
            let http_trace = Arc::clone(&trace);
            let hold_query = Arc::new(AtomicBool::new(mode == Mode::BarrierQuery));
            let query_seen = Arc::new(AtomicBool::new(false));
            let thread_hold = Arc::clone(&hold_query);
            let thread_seen = Arc::clone(&query_seen);
            let thread_stop = Arc::clone(&stop);
            let thread_requests = Arc::clone(&requests);
            let media_url = format!("{base}/artifact.mp4?signature=private");
            let worker = std::thread::spawn(move || {
                'accept: while !thread_stop.load(Ordering::SeqCst) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(socket) => socket,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(e) => panic!("mock accept: {e}"),
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(1)))
                        .unwrap();
                    let mut bytes = Vec::new();
                    let mut buffer = [0; 4096];
                    loop {
                        let n = match stream.read(&mut buffer) {
                            Ok(n) => n,
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::TimedOut
                                        | std::io::ErrorKind::ConnectionReset
                                ) =>
                            {
                                continue 'accept;
                            }
                            Err(error) => panic!("mock read: {error}"),
                        };
                        if n == 0 {
                            break;
                        }
                        bytes.extend_from_slice(&buffer[..n]);
                        if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                            let length = String::from_utf8_lossy(&bytes[..end])
                                .lines()
                                .find_map(|line| {
                                    line.split_once(':').filter(|(name, _)| {
                                        name.eq_ignore_ascii_case("content-length")
                                    })
                                })
                                .map_or(0, |(_, v)| v.trim().parse::<usize>().unwrap());
                            if bytes.len() >= end + 4 + length {
                                break;
                            }
                        }
                        assert!(bytes.len() < 65536, "bounded fixture request");
                    }
                    let request = String::from_utf8(bytes).unwrap();
                    let first = request.lines().next().unwrap_or_default();
                    let safe_line = first
                        .split_whitespace()
                        .take(2)
                        .map(|v| v.split('?').next().unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join(" ");
                    http_trace.lock().unwrap().push(format!("HTTP {safe_line}"));
                    thread_requests.lock().unwrap().push(request.clone());
                    if mode == Mode::LostReply && first.starts_with("POST ") {
                        continue;
                    }
                    let (status, body) = if first
                        .starts_with("POST /api/v1/services/aigc/video-generation/video-synthesis ")
                    {
                        (
                            200,
                            json!({"output":{"task_id":"original-001","task_status":"PENDING"}})
                                .to_string(),
                        )
                    } else if first.starts_with("GET /api/v1/tasks/original-001 ") {
                        thread_seen.store(true, Ordering::SeqCst);
                        while thread_hold.load(Ordering::SeqCst)
                            && !thread_stop.load(Ordering::SeqCst)
                        {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        if mode == Mode::SlowQuery {
                            std::thread::sleep(Duration::from_secs(4));
                        }
                        if mode == Mode::Cancelled {
                            let body = json!({"output":{"task_status":"CANCELED"}}).to_string();
                            let _ = write!(
                                stream,
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            continue;
                        }
                        (200, json!({"output":{"task_status":"SUCCEEDED","video_url":media_url},"usage":{"duration":4}}).to_string())
                    } else if first.starts_with("GET /artifact.mp4?signature=private ") {
                        (200, "SHARED-TASK-MEDIA".into())
                    } else {
                        (404, "unexpected fixture route".into())
                    };
                    write!(stream, "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).ok();
                }
            });
            let fixture = Self {
                root,
                stop,
                worker: Some(worker),
                requests,
                trace,
                hold_query,
                query_seen,
                deadline: Instant::now() + Duration::from_secs(55),
            };
            let source = PathBuf::from(
                std::env::var("TOKEN_STATION_TASK_BAILIAN_PACKAGE_DIR")
                    .expect("official task-v2 package is required"),
            );
            let manifest_bytes = std::fs::read(source.join("manifest.json")).unwrap();
            let manifest: Value = serde_json::from_slice(&manifest_bytes).unwrap();
            let wasm = std::fs::read(source.join("component.wasm")).unwrap();
            assert_eq!(manifest["name"], "task-bailian-v2");
            assert_eq!(manifest["api_version"], "task-adapter-v2");
            let package = fixture.root.join("components/task-bailian-v2");
            std::fs::create_dir_all(&package).unwrap();
            std::fs::write(package.join("manifest.json"), &manifest_bytes).unwrap();
            std::fs::write(package.join("component.wasm"), &wasm).unwrap();
            let data = fixture.root.join("data");
            std::fs::create_dir(&data).unwrap();
            token_station_cli::secrets::store_set(
                &data,
                "bailian-task",
                "provider_api_key",
                "test-task-secret",
            )
            .unwrap();
            let mut config: Value =
                serde_json::from_str(token_station_cli::EXAMPLE_CONFIG).unwrap();
            config["data"] = json!({"dir":data,"metrics":false});
            std::fs::write(fixture.root.join("config.json"), config.to_string()).unwrap();
            std::fs::write(fixture.root.join("tasks.json"),json!({
                "state_dir":fixture.root.join("tasks"),"components_dir":fixture.root.join("components"),
                "providers":{"bailian":{"endpoint":base,"model":"happyhorse-1.0","dialect":"bailian_video",
                    "pin":{"world":"task-adapter-v2","name":"task-bailian-v2","version":manifest["version"],
                        "manifest_sha256":hex(&Sha256::digest(&manifest_bytes)),"wasm_sha256":hex(&Sha256::digest(&wasm))},
                    "credential":{"upstream":"bailian-task","slot":"provider_api_key"}}}
            }).to_string()).unwrap();
            std::fs::write(
                fixture.root.join("request.json"),
                json!({"prompt":"moving cloud","duration":5,"resolution":"720P"}).to_string(),
            )
            .unwrap();
            fixture
        }
        fn run(&self, args: &[&str]) -> Value {
            let (success, output) = self.run_result(args);
            assert!(success, "real task CLI failed: {output}");
            serde_json::from_str(&output).expect("one JSON result")
        }
        fn run_result(&self, args: &[&str]) -> (bool, String) {
            let started = Instant::now();
            let log = self.root.join("command.log");
            let file = std::fs::File::create(&log).unwrap();
            let mut child = Command::new(env!("CARGO_BIN_EXE_token-station-cli"))
                .args([
                    "--config",
                    "config.json",
                    "task",
                    "--task-config",
                    "tasks.json",
                ])
                .args(args)
                .current_dir(&self.root)
                .stdin(Stdio::null())
                .stdout(Stdio::from(file.try_clone().unwrap()))
                .stderr(Stdio::from(file))
                .spawn()
                .unwrap();
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if Instant::now() >= self.deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("task scenario exceeded 55s");
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            let output = std::fs::read_to_string(log).unwrap();
            let value = serde_json::from_str::<Value>(&output).ok();
            self.trace.lock().unwrap().push(format!(
                "CLI {} success={} elapsed={:?} submission={} execution={} version={}",
                args[0],
                status.success(),
                started.elapsed(),
                value
                    .as_ref()
                    .map_or(&Value::Null, |v| &v["submission_state"]),
                value
                    .as_ref()
                    .map_or(&Value::Null, |v| &v["execution_state"]),
                value.as_ref().map_or(&Value::Null, |v| &v["version"])
            ));
            assert!(!output.contains("test-task-secret"));
            (status.success(), output)
        }
    }

    #[test]
    #[ignore = "requires the official Bailian task-v2 package; explicit lifecycle gate"]
    fn task_cli_real_submit_restart_observe_fetch_uses_shared_component() {
        let fixture = Fixture::new();
        let submit = [
            "submit",
            "--provider",
            "bailian",
            "--request",
            "request.json",
            "--idempotency-key",
            "same-request",
        ];
        let created = fixture.run(&submit);
        let id = created["id"].as_str().expect("host task id");
        assert_eq!(created["billing_effect"], "not_applicable");
        assert_eq!(fixture.run(&submit)["id"], id);
        let inspected = fixture.run(&["inspect", id]);
        assert_eq!(inspected["id"], id);
        let observed = fixture.run(&["observe", id]);
        assert_eq!(observed["execution_state"], "succeeded");
        assert_eq!(observed["billing_effect"], "not_applicable");
        fixture.run(&["fetch", id, "--output", "video.mp4"]);
        assert_eq!(
            std::fs::read(fixture.root.join("video.mp4")).unwrap(),
            b"SHARED-TASK-MEDIA"
        );
        assert!(Path::new(&fixture.root.join("tasks/tasks.sqlite")).exists());
        let requests = fixture.requests.lock().unwrap();
        assert_eq!(
            requests.iter().filter(|r| r.starts_with("POST ")).count(),
            1,
            "restart and replay must not resubmit"
        );
        let create = requests
            .iter()
            .find(|r| r.starts_with("POST "))
            .unwrap()
            .to_ascii_lowercase();
        assert!(create.contains("x-dashscope-async: enable"));
        for request in requests.iter().filter(|r| r.starts_with("GET /artifact")) {
            assert!(!request.to_ascii_lowercase().contains("authorization:"));
        }
        assert!(
            requests.iter().any(|r| r.starts_with("GET /artifact")),
            "fetch must deliver real bytes"
        );
    }
    const SUBMIT: &[&str] = &[
        "submit",
        "--provider",
        "bailian",
        "--request",
        "request.json",
        "--idempotency-key",
        "same-request",
    ];

    #[test]
    #[ignore = "requires the official Bailian task-v2 package; explicit lifecycle gate"]
    fn task_cli_wait_deadline_cancels_real_slow_http() {
        let fixture = Fixture::with_slow_query(true);
        let created = fixture.run(SUBMIT);
        let id = created["id"].as_str().unwrap();
        let started = Instant::now();
        let outcome = fixture.run(&["wait", id, "--timeout-seconds", "1"]);
        assert_eq!(outcome["wait_outcome"], "timed_out");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "one second wait must cancel the four second HTTP response"
        );
        let inspected = fixture.run(&["inspect", id]);
        assert_eq!(inspected["submission_state"], "accepted");
        assert!(!["failed", "cancelled"].contains(&inspected["execution_state"].as_str().unwrap()));
    }

    #[test]
    #[ignore = "requires the official Bailian task-v2 package; explicit lifecycle gate"]
    fn task_cli_missing_credential_guard_refuses_to_replace_original_key() {
        let fixture = Fixture::new();
        let created = fixture.run(SUBMIT);
        std::fs::remove_file(fixture.root.join("tasks/credential-hmac.key")).unwrap();
        let id = created["id"].as_str().unwrap();
        assert_eq!(fixture.run(&["inspect", id])["id"], id);
        assert_eq!(fixture.run(SUBMIT)["id"], id);
        assert_eq!(
            fixture.run(&["observe", id])["execution_state"],
            "status_unknown"
        );
        assert!(
            !fixture.root.join("tasks/credential-hmac.key").exists(),
            "never replace guard for existing tasks"
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }

    #[test]
    #[ignore = "requires the official Bailian task-v2 package; explicit lifecycle gate"]
    fn task_cli_lost_submit_reply_never_resends() {
        let fixture = Fixture::with_mode(Mode::LostReply);
        let created = fixture.run(SUBMIT);
        assert_eq!(created["submission_state"], "unknown");
        assert_eq!(created["execution_state"], "status_unknown");
        let id = created["id"].as_str().unwrap();
        assert_eq!(fixture.run(SUBMIT)["id"], id);
        assert_eq!(
            fixture.run(&["observe", id])["execution_state"],
            "status_unknown"
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }

    #[test]
    #[ignore = "requires the official Bailian task-v2 package; explicit lifecycle gate"]
    fn task_cli_missing_exact_package_preserves_history_without_native_fallback() {
        let fixture = Fixture::new();
        let created = fixture.run(SUBMIT);
        let id = created["id"].as_str().unwrap();
        std::fs::rename(
            fixture.root.join("components/task-bailian-v2"),
            fixture.root.join("retained-outside-active-root"),
        )
        .unwrap();
        assert_eq!(fixture.run(SUBMIT)["id"], id);
        assert_eq!(
            fixture.run(&["inspect", id])["submission_state"],
            "accepted"
        );
        assert_eq!(
            fixture.run(&["observe", id])["execution_state"],
            "status_unknown"
        );
        let (ok, _) = fixture.run_result(&["fetch", id, "--output", "refused.mp4"]);
        assert!(!ok);
        assert!(!fixture.root.join("refused.mp4").exists());
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }

    #[test]
    #[ignore = "requires the official Bailian task-v2 package; explicit lifecycle gate"]
    fn task_cli_rotated_original_credential_never_queries_with_new_secret() {
        let fixture = Fixture::new();
        let created = fixture.run(SUBMIT);
        let id = created["id"].as_str().unwrap();
        token_station_cli::secrets::store_set(
            &fixture.root.join("data"),
            "bailian-task",
            "provider_api_key",
            "replacement-secret",
        )
        .unwrap();
        assert_eq!(fixture.run(SUBMIT)["id"], id);
        assert_eq!(
            fixture.run(&["observe", id])["execution_state"],
            "status_unknown"
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        let saved = std::fs::read(fixture.root.join("tasks/tasks.sqlite")).unwrap();
        for secret in [
            b"test-task-secret".as_slice(),
            b"replacement-secret".as_slice(),
            b"moving cloud".as_slice(),
        ] {
            assert!(!saved.windows(secret.len()).any(|v| v == secret));
        }
    }

    #[test]
    #[ignore = "requires the official Bailian task-v2 package; explicit lifecycle gate"]
    fn task_cli_cancel_intent_is_distinct_from_observed_cancellation_without_funds() {
        let fixture = Fixture::with_mode(Mode::Cancelled);
        let created = fixture.run(SUBMIT);
        let id = created["id"].as_str().unwrap();
        let intent = fixture.run(&["cancel", id]);
        assert_eq!(intent["cancel_requested"], true);
        assert_eq!(intent["execution_state"], "queued");
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
        let terminal = fixture.run(&["observe", id]);
        assert_eq!(terminal["execution_state"], "cancelled");
        assert_eq!(terminal["billing_effect"], "not_applicable");
        assert_eq!(
            fixture.run(&["wait", id, "--timeout-seconds", "1"])["execution_state"],
            "cancelled"
        );
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    }
    #[test]
    #[ignore = "requires the official Bailian task-v2 package; explicit lifecycle gate"]
    fn task_cli_cas_loser_cannot_deliver_its_stale_success_artifact() {
        let fixture = Fixture::with_mode(Mode::BarrierQuery);
        let created = fixture.run(SUBMIT);
        let id = created["id"].as_str().unwrap();
        std::thread::scope(|scope| {
            let pending =
                scope.spawn(|| fixture.run_result(&["fetch", id, "--output", "loser.mp4"]));
            while !fixture.query_seen.load(Ordering::SeqCst) {
                assert!(
                    Instant::now() < fixture.deadline,
                    "query must reach HTTP barrier"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            let mut competing =
                token_station_cli::tasks::TaskStore::open(&fixture.root.join("tasks")).unwrap();
            let winner = competing.get(id).unwrap();
            assert!(competing.apply(&winner, "succeeded").unwrap());
            fixture.hold_query.store(false, Ordering::SeqCst);
            let (ok, output) = pending.join().unwrap();
            assert!(
                !ok,
                "losing response must not become a deliverable winner: {output}"
            );
        });
        assert!(!fixture.root.join("loser.mp4").exists());
        assert_eq!(
            fixture.run(&["inspect", id])["execution_state"],
            "succeeded"
        );
        assert!(
            !fixture
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|request| request.starts_with("GET /artifact"))
        );
    }
}
