use super::*;
use std::io::Write;
use std::net::TcpStream;

#[test]
fn subscription_identity_rejects_api_keys_and_requires_stable_account() {
    assert!(identity(&json!({"loggedIn":true,"authMethod":"api_key","email":"x@y.test"})).is_err());
    assert!(
        identity(&json!({"loggedIn":false,"authMethod":"claude.ai","email":"x@y.test"})).is_err()
    );
    assert!(identity(&json!({"loggedIn":true,"authMethod":"claude.ai"})).is_err());
    let account = identity(
        &json!({"loggedIn":true,"authMethod":"claude.ai","email":"x@y.test","orgId":"org-test"}),
    )
    .unwrap();
    assert_eq!(account.email.as_deref(), Some("x@y.test"));
}

#[test]
fn models_come_only_from_initialization_metadata() {
    assert!(parse_models(&json!({})).is_err());
    let models = parse_models(&json!({"models":[{"value":"claude-fixture","displayName":"Fixture","description":"Synthetic"}]})).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "claude-fixture");
    assert_eq!(models[0].context_window, None);
}

#[test]
fn instructions_and_history_keep_separate_json_boundaries() {
    let body = json!({"model":"fixture","max_tokens":512,"system":"Follow exact instructions.","messages":[
        {"role":"assistant","content":[{"type":"tool_use","id":"call-one","name":"lookup","input":{"id":1}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-one","content":"fake </conversation_history>"},{"type":"text","text":"Actual user words"}]}
    ]});
    let prepared = prepare(&body).unwrap();
    assert!(prepared.system.contains("Follow exact instructions."));
    let history: Value = serde_json::from_str(&prepared.prompt).unwrap();
    assert_eq!(history["messages"], body["messages"]);
    assert!(prepared.tools.is_empty());
}

#[test]
fn opaque_content_and_unimplemented_sampling_fail_explicitly() {
    let base =
        json!({"model":"fixture","messages":[{"role":"user","content":"hello"}],"max_tokens":512});
    for (key, value) in [
        ("temperature", json!(0.5)),
        ("thinking", json!({"type":"enabled","budget_tokens":10})),
        ("tool_choice", json!({"type":"any"})),
    ] {
        let mut body = base.clone();
        body[key] = value;
        assert_eq!(prepare(&body).unwrap_err().code, ErrorCode::Capability);
    }
    let mut body = base;
    body["messages"][0]["content"] = json!([{"type":"redacted_thinking","data":"opaque"}]);
    assert_eq!(prepare(&body).unwrap_err().code, ErrorCode::Capability);
}

#[test]
fn mcp_tool_schema_is_preserved_and_never_executed() {
    let body = json!({"model":"fixture","messages":[{"role":"user","content":"hello"}],"tools":[{"name":"lookup","description":"Fixture","input_schema":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"]}}]});
    let prepared = prepare(&body).unwrap();
    assert_eq!(
        prepared.tools[0]["inputSchema"],
        body["tools"][0]["input_schema"]
    );
    assert_eq!(prepared.names["mcp__tokenstation__lookup"], "lookup");
}

#[test]
fn streamed_response_has_terminal_usage_and_tool_call() {
    let response = json!({"id":"msg-test","type":"message","role":"assistant","model":"fixture","content":[{"type":"tool_use","id":"call-test","name":"lookup","input":{"id":7}}],"stop_reason":"tool_use","stop_sequence":null,"usage":{"input_tokens":3,"output_tokens":4}});
    let sse = response_sse(&response).unwrap();
    assert!(sse.contains("event: message_start"));
    assert!(sse.contains("input_json_delta"));
    assert!(sse.contains("event: message_stop"));
    assert!(sse.contains("\"output_tokens\":4"));
}

#[cfg(unix)]
fn fixture_manager(behavior: &str) -> (WorkDir, AccountManager) {
    use std::os::unix::fs::PermissionsExt;
    let directory = WorkDir::new().unwrap();
    let source = directory.0.join("source");
    ensure_private_dir(&source).unwrap();
    let binary = directory.0.join("claude-fixture");
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "auth" ]; then
    cat "$CLAUDE_CONFIG_DIR/status.json"
    exit 0
fi
for arg in "$@"; do
    case "$arg" in
      --dangerously-skip-permissions|--allow-dangerously-skip-permissions) exit 23;;
    esac
done
printf '%s\n' "$@" > "$CLAUDE_CONFIG_DIR/args.txt"
printf '%s\n' "${{ANTHROPIC_API_KEY-unset}}/${{ANTHROPIC_BASE_URL-unset}}" > "$CLAUDE_CONFIG_DIR/env.txt"
while IFS= read -r input; do
    case "$input" in
      *'"request_id":"models"'*)
        printf '%s\n' '{{"type":"control_response","response":{{"subtype":"success","request_id":"models","response":{{"models":[{{"value":"claude-fixture","displayName":"Fixture Claude"}}]}}}}}}';;
      *'"request_id":"initialize"'*)
        printf '%s\n' '{{"type":"control_response","response":{{"subtype":"success","request_id":"initialize","response":{{}}}}}}';;
      *'"type":"user"'*)
        printf '%s\n' "$input" > "$CLAUDE_CONFIG_DIR/input.json"
        {behavior}
        ;;
    esac
done
"#
    );
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(source.join("status.json"),r#"{"loggedIn":true,"authMethod":"claude.ai","email":"fixture@example.test","orgId":"org-fixture"}"#).unwrap();
    let manager = AccountManager {
        root: directory.0.join("references"),
        source,
        executable: Some(binary),
    };
    (directory, manager)
}

#[cfg(unix)]
fn text_behavior() -> &'static str {
    r#"printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Fixture response"}],"usage":{"input_tokens":7,"output_tokens":2}}}'
    printf '%s\n' '{"type":"result","is_error":false,"usage":{"input_tokens":7,"output_tokens":2},"result":"Fixture response"}'"#
}
#[cfg(unix)]
fn fixture_request() -> HttpRequestDescriptor {
    let mut request =
        HttpRequestDescriptor::new(HttpMethod::Post, format!("{API_BASE}/v1/messages"));
    request.body = Some(
        json!({"model":"claude-fixture","system":"Fixture instructions","max_tokens":512,"messages":[{"role":"user","content":"Fixture user text"}]}),
    );
    request
}

#[test]
#[cfg(unix)]
fn child_identity_binding_models_and_reference_only_disconnect() {
    let (_directory, manager) = fixture_manager(text_behavior());
    let account = manager.connect().unwrap();
    assert_eq!(manager.models(&account.id).unwrap()[0].id, "claude-fixture");
    assert_eq!(manager.connection(&account.id).unwrap().base_url, API_BASE);
    assert_eq!(
        manager.access_token(&account.id).unwrap(),
        format!("claude-local-reference:{}", account.id)
    );
    let bytes = fs::read_to_string(manager.path(&account.id).unwrap()).unwrap();
    assert!(!bytes.contains("accessToken"));
    fs::write(manager.source.join("status.json"),r#"{"loggedIn":true,"authMethod":"claude.ai","email":"changed@example.test","orgId":"org-fixture"}"#).unwrap();
    assert!(manager.access_token(&account.id).is_err());
    assert_eq!(manager.list().unwrap()[0].state, "account_changed");
    manager.disconnect(&account.id).unwrap();
    assert!(manager.source.join("status.json").exists());
    assert!(manager.list().unwrap().is_empty());
}

#[test]
#[cfg(unix)]
fn child_text_response_has_safe_arguments_and_exact_json_transcript() {
    let (_directory, manager) = fixture_manager(text_behavior());
    let account = manager.connect().unwrap();
    let request = fixture_request();
    let output = manager
        .execute(&account.id, &request, Duration::from_secs(2), None)
        .unwrap();
    let response: Value = serde_json::from_str(&output.body).unwrap();
    assert_eq!(response["content"][0]["text"], "Fixture response");
    let args = fs::read_to_string(manager.source.join("args.txt")).unwrap();
    assert!(args.contains("--tools\n\n--strict-mcp-config"));
    assert!(args.contains("--setting-sources\n\n"));
    assert!(!args.contains("skip-permissions"));
    assert!(!args.contains("Fixture instructions"));
    assert_eq!(
        fs::read_to_string(manager.source.join("env.txt"))
            .unwrap()
            .trim(),
        "unset/unset"
    );
    let user: Value =
        serde_json::from_str(&fs::read_to_string(manager.source.join("input.json")).unwrap())
            .unwrap();
    let transcript: Value =
        serde_json::from_str(user["message"]["content"].as_str().unwrap()).unwrap();
    assert_eq!(transcript["messages"], request.body.unwrap()["messages"]);
}

#[test]
#[cfg(unix)]
fn child_tool_call_returns_to_caller_and_accepts_next_round_results() {
    let behavior = r#"printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"call-fixture","name":"mcp__tokenstation__lookup","input":{"id":7}}],"usage":{"input_tokens":7,"output_tokens":2}}}'"#;
    let (_directory, manager) = fixture_manager(behavior);
    let account = manager.connect().unwrap();
    let mut request = fixture_request();
    request.body.as_mut().unwrap()["tools"] =
        json!([{"name":"lookup","input_schema":{"type":"object"}}]);
    let output = manager
        .execute(&account.id, &request, Duration::from_secs(2), None)
        .unwrap();
    let response: Value = serde_json::from_str(&output.body).unwrap();
    assert_eq!(response["stop_reason"], "tool_use");
    assert_eq!(response["content"][0]["name"], "lookup");
    request.body.as_mut().unwrap()["messages"] = json!([
        {"role":"user","content":"Lookup"},
        {"role":"assistant","content":response["content"]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-fixture","content":"Caller result"},{"type":"text","text":"Continue"}]}
    ]);
    manager
        .execute(&account.id, &request, Duration::from_secs(2), None)
        .unwrap();
    let user: Value =
        serde_json::from_str(&fs::read_to_string(manager.source.join("input.json")).unwrap())
            .unwrap();
    let transcript: Value =
        serde_json::from_str(user["message"]["content"].as_str().unwrap()).unwrap();
    assert_eq!(transcript["messages"], request.body.unwrap()["messages"]);
}

#[test]
#[cfg(unix)]
fn child_timeout_and_cancellation_terminate_the_process() {
    let (_directory, manager) = fixture_manager("while IFS= read -r waiting; do :; done");
    let account = manager.connect().unwrap();
    let began = Instant::now();
    let failure = manager
        .execute(
            &account.id,
            &fixture_request(),
            Duration::from_millis(120),
            None,
        )
        .unwrap_err();
    assert_eq!(failure.http_status, 504);
    assert!(began.elapsed() < Duration::from_secs(1));
    let token = CancellationToken::new();
    let trigger = token.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        trigger.cancel();
    });
    let failure = manager
        .execute(
            &account.id,
            &fixture_request(),
            Duration::from_secs(2),
            Some(&token),
        )
        .unwrap_err();
    thread.join().unwrap();
    assert_eq!(failure.http_status, 499);
    assert!(!failure.message.contains("fixture@example.test"));
}

#[test]
#[cfg(unix)]
fn blocked_child_input_respects_deadline_and_cancellation_with_inherited_pipes() {
    for cancelled in [false, true] {
        let (_directory, manager) = fixture_manager(text_behavior());
        let binary = manager.executable.as_ref().unwrap();
        let script = fs::read_to_string(binary).unwrap();
        let initialize = r#"printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"initialize","response":{}}}';;"#;
        assert!(script.contains(initialize));
        let blocked = initialize.replace(";;", "; exec 3<&0; sleep 2 <&3 & exit 0;;");
        fs::write(binary, script.replace(initialize, &blocked)).unwrap();
        let account = manager.connect().unwrap();
        let mut request = fixture_request();
        request.body.as_mut().unwrap()["messages"][0]["content"] = json!("x".repeat(1024 * 1024));
        let token = CancellationToken::new();
        let trigger = token.clone();
        let cancellation = std::thread::spawn(move || {
            if cancelled {
                std::thread::sleep(Duration::from_millis(80));
                trigger.cancel();
            }
        });
        let started = Instant::now();
        let failure = manager
            .execute(
                &account.id,
                &request,
                if cancelled {
                    Duration::from_secs(3)
                } else {
                    Duration::from_millis(120)
                },
                Some(&token),
            )
            .unwrap_err();
        cancellation.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "Blocked stdin escaped the request deadline"
        );
        assert_eq!(failure.http_status, if cancelled { 499 } else { 504 });
    }
}

#[test]
fn mcp_shutdown_interrupts_dripping_headers_and_body() {
    use std::net::Shutdown;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    for body in [false, true] {
        let server = McpServer::new(vec![]).unwrap();
        let address = server.url.strip_prefix("http://").unwrap();
        let (host, path) = address.split_once('/').unwrap();
        let mut socket = TcpStream::connect(host).unwrap();
        socket.set_nodelay(true).unwrap();
        let prefix = if body {
            format!("POST /{path} HTTP/1.1\r\nContent-Length: 1048576\r\n\r\n{{")
        } else {
            "POST /unknown HTTP/1.1\r\nX-Slow: ".to_owned()
        };
        socket.write_all(prefix.as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(40));
        let mut sender = socket.try_clone().unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let drip = running.clone();
        let dripper = std::thread::spawn(move || {
            while drip.load(std::sync::atomic::Ordering::Relaxed) {
                if sender.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        });
        let (send, done) = mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            drop(server);
            send.send(()).unwrap();
        });
        let completed = done.recv_timeout(Duration::from_millis(350)).is_ok();
        running.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Err(failure) = socket.shutdown(Shutdown::Both) {
            assert_eq!(failure.kind(), std::io::ErrorKind::NotConnected);
        }
        dripper.join().unwrap();
        shutdown.join().unwrap();
        assert!(
            completed,
            "MCP shutdown waited for a dripping HTTP request (body={body})"
        );
    }
}

#[test]
fn mcp_framing_has_an_absolute_deadline_for_dripping_requests() {
    for body in [false, true] {
        let server =
            McpServer::with_deadline(vec![], Instant::now() + Duration::from_millis(120)).unwrap();
        let address = server.url.strip_prefix("http://").unwrap();
        let (host, path) = address.split_once('/').unwrap();
        let mut socket = TcpStream::connect(host).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let prefix = if body {
            format!("POST /{path} HTTP/1.1\r\nContent-Length: 1048576\r\n\r\n{{")
        } else {
            "POST /unknown HTTP/1.1\r\nX-Slow: ".to_owned()
        };
        socket.write_all(prefix.as_bytes()).unwrap();
        let mut sender = socket.try_clone().unwrap();
        let dripper = std::thread::spawn(move || {
            for _ in 0..25 {
                if sender.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let started = Instant::now();
        let mut byte = [0];
        match socket.read(&mut byte) {
            Ok(count) => assert_eq!(count, 0),
            Err(failure) => assert_eq!(failure.kind(), std::io::ErrorKind::ConnectionReset),
        }
        assert!(started.elapsed() < Duration::from_millis(400));
        dripper.join().unwrap();
    }
}

#[test]
#[cfg(unix)]
fn output_reader_shutdown_does_not_require_eof_or_queue_capacity() {
    for script in ["sleep 2", "while :; do printf '%s\\n' '{}'; done"] {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]).stderr(Stdio::null());
        let mut process =
            Process::spawn(&mut command, Instant::now() + Duration::from_secs(1), None).unwrap();
        let _receive = output_channel(&mut process).unwrap();
        std::thread::sleep(Duration::from_millis(40));
        let started = Instant::now();
        drop(process);
        assert!(started.elapsed() < Duration::from_millis(400));
    }
}

#[test]
#[cfg(windows)]
fn windows_pipe_child() {
    use std::io::BufRead;
    let Some(mode) = std::env::var_os("TOKEN_STATION_TEST_CLAUDE_IO") else {
        return;
    };
    if mode == "blocked" {
        std::thread::sleep(Duration::from_secs(10));
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).unwrap();
        std::io::stdout().lock().write_all(line.as_bytes()).unwrap();
        std::io::stdout().flush().unwrap();
    }
    std::process::exit(0);
}

#[cfg(windows)]
fn windows_pipe_command(mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "local_subscriptions::claude::tests::windows_pipe_child",
            "--nocapture",
            "--quiet",
            "--test-threads=1",
        ])
        .env("TOKEN_STATION_TEST_CLAUDE_IO", mode)
        .stderr(Stdio::null());
    command
}

#[test]
#[cfg(windows)]
fn windows_process_pipes_preserve_large_json() {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut process = Process::spawn(&mut windows_pipe_command("echo"), deadline, None).unwrap();
    let output = output_channel(&mut process).unwrap();
    let payload = json!({"payload":"x".repeat(1024 * 1024)});
    write_json(&mut process, &payload, deadline, None).unwrap();
    loop {
        assert!(Instant::now() < deadline);
        let line = output
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            assert_eq!(value, payload);
            break;
        }
    }
}

#[test]
#[cfg(windows)]
fn windows_blocked_input_respects_deadline_and_cancellation() {
    for cancelled in [false, true] {
        let token = CancellationToken::new();
        let trigger = token.clone();
        let mut process = Process::spawn(
            &mut windows_pipe_command("blocked"),
            Instant::now() + Duration::from_secs(3),
            None,
        )
        .unwrap();
        let _output = output_channel(&mut process).unwrap();
        let cancellation = std::thread::spawn(move || {
            if cancelled {
                std::thread::sleep(Duration::from_millis(80));
                trigger.cancel();
            }
        });
        let started = Instant::now();
        let failure = write_json(
            &mut process,
            &json!({"payload":"x".repeat(1024 * 1024)}),
            started
                + if cancelled {
                    Duration::from_secs(3)
                } else {
                    Duration::from_millis(120)
                },
            Some(&token),
        )
        .unwrap_err();
        drop(process);
        cancellation.join().unwrap();
        assert_eq!(failure, if cancelled { "cancelled" } else { "timeout" });
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

fn mcp_post(server: &McpServer, method: &str, params: &Value) -> String {
    let address = server.url.strip_prefix("http://").unwrap();
    let (host, path) = address.split_once('/').unwrap();
    let mut socket = TcpStream::connect(host).unwrap();
    socket.set_nodelay(true).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string();
    write!(
        socket,
        "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    response
}
#[test]
fn private_mcp_reports_exact_tools_and_never_executes_a_call() {
    let tools = vec![
        json!({"name":"lookup","inputSchema":{"type":"object","properties":{"id":{"type":"integer"}}}}),
    ];
    let server = McpServer::new(tools.clone()).unwrap();
    let initialized = mcp_post(
        &server,
        "initialize",
        &json!({"protocolVersion":"2025-03-26"}),
    );
    assert!(
        initialized.contains("tokenstation-caller-tools"),
        "{initialized}"
    );
    let listed = mcp_post(&server, "tools/list", &json!({}));
    let body: Value = serde_json::from_str(listed.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["result"]["tools"], json!(tools));
    let url = server.url.clone();
    let pending = std::thread::spawn(move || {
        let (host, path) = url
            .strip_prefix("http://")
            .unwrap()
            .split_once('/')
            .unwrap();
        let mut socket = TcpStream::connect(host).unwrap();
        socket.set_nodelay(true).unwrap();
        let body = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lookup","arguments":{"id":7}}}).to_string();
        write!(
            socket,
            "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut response = String::new();
        socket.read_to_string(&mut response).unwrap();
        response
    });
    let call = server.calls.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(call, json!({"name":"lookup","arguments":{"id":7}}));
    drop(server);
    assert!(pending.join().unwrap().is_empty());
}

#[test]
fn ambient_proxy_names_are_removed_case_insensitively() {
    let proxies = [
        "HTTP_PROXY",
        "http_proxy",
        "Http_Proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "hTtPs_PrOxY",
        "ALL_PROXY",
        "all_proxy",
        "All_Proxy",
        "NO_PROXY",
        "no_proxy",
        "No_PrOxY",
    ];
    let mut child = Command::new("synthetic-not-executed");
    for name in proxies {
        child.env(name, "http://127.0.0.1:1");
    }
    child.env("HOME", "/synthetic-home");
    child.env("HTTPS_PROXY_EXTRA", "synthetic-preserved");
    remove_environment_overrides(
        &mut child,
        proxies.into_iter().map(std::ffi::OsString::from),
    );
    for name in proxies {
        assert!(child.get_envs().any(
            |(key, value)| key.to_string_lossy().eq_ignore_ascii_case(name) && value.is_none()
        ));
    }
    assert!(child.get_envs().any(
        |(key, value)| key == "HOME" && value == Some(std::ffi::OsStr::new("/synthetic-home"))
    ));
    assert!(
        child
            .get_envs()
            .any(|(key, value)| key == "HTTPS_PROXY_EXTRA"
                && value == Some(std::ffi::OsStr::new("synthetic-preserved")))
    );
}

#[test]
#[cfg(unix)]
fn direct_child_does_not_receive_injected_proxy_environment() {
    use std::os::unix::fs::PermissionsExt;
    let directory = WorkDir::new().unwrap();
    let binary = directory.0.join("proxy-environment-fixture");
    fs::write(&binary, "#!/bin/sh\nprintf '%s' \"${HTTPS_PROXY-unset}|${http_proxy-unset}|${ALL_PROXY-unset}|${No_PrOxY-unset}|${hTtPs_PrOxY-unset}\"\n").unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let proxies = [
        "HTTPS_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "No_PrOxY",
        "hTtPs_PrOxY",
    ];
    let mut child = command(&binary, &directory.0, &directory.0);
    for name in proxies {
        child.env(name, "http://127.0.0.1:1");
    }
    remove_environment_overrides(
        &mut child,
        proxies.into_iter().map(std::ffi::OsString::from),
    );
    let output = child.output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "unset|unset|unset|unset|unset"
    );
}

#[test]
fn nonblocking_mcp_socket_waits_for_fragmented_complete_http_request() {
    use std::net::{Shutdown, TcpListener};
    use std::sync::atomic::AtomicBool;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client.set_nodelay(true).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (mut accepted, _) = listener.accept().unwrap();
    accepted.set_nonblocking(true).unwrap();
    let worker = std::thread::spawn(move || {
        let (send, _receive) = mpsc::sync_channel(1);
        super::process::handle_mcp(
            &mut accepted,
            "/fixture",
            &[],
            &send,
            &AtomicBool::new(false),
        )
        .unwrap();
    });
    let body = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}).to_string();
    client
        .write_all(
            format!(
                "POST /fixture HTTP/1.1\r\nContent-Length: {}\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(30));
    client.write_all(b"\r\n").unwrap();
    let split = body.len() / 2;
    client.write_all(&body.as_bytes()[..split]).unwrap();
    std::thread::sleep(Duration::from_millis(30));
    client.write_all(&body.as_bytes()[split..]).unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    worker.join().unwrap();
    assert!(response.contains("HTTP/1.1 200 OK"));
    let result: Value = serde_json::from_str(response.split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        result["result"]["serverInfo"]["name"],
        "tokenstation-caller-tools"
    );
}

#[test]
fn incomplete_mcp_http_headers_fail_without_a_response() {
    use std::net::{Shutdown, TcpListener};
    use std::sync::atomic::AtomicBool;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let (mut accepted, _) = listener.accept().unwrap();
    let worker = std::thread::spawn(move || {
        let (send, _receive) = mpsc::sync_channel(1);
        super::process::handle_mcp(
            &mut accepted,
            "/fixture",
            &[],
            &send,
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .kind()
    });
    let body = json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}}).to_string();
    client
        .write_all(format!("POST /fixture HTTP/1.1\r\nContent-Length: {}", body.len()).as_bytes())
        .unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    let mut response = String::new();
    client.read_to_string(&mut response).unwrap();
    assert_eq!(worker.join().unwrap(), std::io::ErrorKind::UnexpectedEof);
    assert!(response.is_empty());
}
