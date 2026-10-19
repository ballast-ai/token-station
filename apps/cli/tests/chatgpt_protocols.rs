//! Cross-protocol tool contracts through the installed `ChatGPT` plan component.
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use token_station_cli::{
    config::ClientConfig,
    gateway::{Gateway, Reply},
};
use token_station_metrics::{Recorder, RequestRecord};

const FINAL: &str = "FIXTURE_RESULTS_VERIFIED";
const PATCH: &str = "*** Begin Patch\n*** Add File: fixture.txt\n+fixture\n*** End Patch";

#[derive(Clone, Copy, Debug)]
enum Protocol {
    Anthropic,
    Chat,
    Responses,
    Gemini,
}
impl Protocol {
    fn path(self, stream: bool) -> &'static str {
        match self {
            Self::Anthropic => "/v1/messages",
            Self::Chat => "/v1/chat/completions",
            Self::Responses => "/v1/responses",
            Self::Gemini if stream => "/v1beta/models/gpt-5.5:streamGenerateContent?alt=sse",
            Self::Gemini => "/v1beta/models/gpt-5.5:generateContent",
        }
    }
    fn request(self, stream: bool) -> Value {
        let schema = json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false});
        match self {
            Self::Anthropic => {
                json!({"model":"gpt-5.5","max_tokens":200,"stream":stream,"messages":[{"role":"user","content":"Read fixture files"}],"tools":[{"name":"read","input_schema":schema}]})
            }
            Self::Chat => {
                json!({"model":"gpt-5.5","stream":stream,"messages":[{"role":"user","content":"Read fixture files"}],"tools":[{"type":"function","function":{"name":"read","parameters":schema}}]})
            }
            Self::Responses => {
                json!({"model":"gpt-5.5","stream":stream,"input":"Read fixture files","tools":[{"type":"function","name":"read","parameters":schema}]})
            }
            Self::Gemini => {
                json!({"contents":[{"role":"user","parts":[{"text":"Read fixture files"}]}],"tools":[{"functionDeclarations":[{"name":"read","parametersJsonSchema":schema}]}]})
            }
        }
    }
}

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
            "ts-plan-protocols-{}-{}",
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

fn read_request(socket: &mut TcpStream) -> Value {
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        socket.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 65536);
    }
    let headers = String::from_utf8(bytes).unwrap();
    assert!(headers.starts_with("POST /v1/responses "));
    let len: usize = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, n)| n.trim().parse().unwrap())
        })
        .unwrap();
    assert!(len < 1024 * 1024);
    let mut body = vec![0; len];
    socket.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

struct Harness {
    gateway: Gateway,
    seen: Arc<Mutex<Vec<Value>>>,
    records: Arc<Records>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    paid: TcpListener,
    _scratch: Scratch,
}
impl Harness {
    fn new(handler: impl Fn(usize, &Value) -> (u16, String) + Send + 'static) -> Self {
        Self::for_provider(false, handler)
    }
    fn for_provider(
        codex: bool,
        handler: impl Fn(usize, &Value) -> (u16, String) + Send + 'static,
    ) -> Self {
        let scratch = Scratch::new();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let capture = Arc::clone(&seen);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let worker = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("fixture: {error}"),
                };
                let request = read_request(&mut socket);
                let mut rows = capture.lock().unwrap();
                let index = rows.len();
                rows.push(request.clone());
                drop(rows);
                assert!(index < 8, "test must not create an unbounded loop");
                let (status, body) = handler(index, &request);
                let mime = if body.starts_with("event:") {
                    "text/event-stream"
                } else {
                    "application/json"
                };
                let _ = write!(
                    socket,
                    "HTTP/1.1 {status} Fixture\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let paid = TcpListener::bind("127.0.0.1:0").unwrap();
        paid.set_nonblocking(true).unwrap();
        token_station_cli::secrets::store_set(
            &scratch.0,
            "plan",
            "chatgpt_access_token",
            "protocol-fixture-only",
        )
        .unwrap();
        let mut config:ClientConfig=serde_json::from_value(json!({
            "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":scratch.0,"metrics":false},
            "plugins":{"dir":Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins-dist"),"allow_unsigned":true,"agents":["agent-anthropic","agent-openai","agent-openai-responses","agent-gemini"],"providers":{"chatgpt-plan":"provider-chatgpt-plan-v2","openai-compatible":"provider-openai-compatible-v2"}},
            "upstreams":{"plan":{"provider":"chatgpt-plan","base_url":endpoint,"auth":{"slot":"chatgpt_access_token","store":true},"models":[{"model":"gpt-5.5","tool":true,"tool_state":"verified","json_schema":true,"json_schema_state":"declared","context_window":200_000}]},"paid":{"provider":"openai-compatible","base_url":format!("http://{}/v1",paid.local_addr().unwrap()),"models":[{"model":"gpt-5.5","tool":true,"tool_state":"verified","json_schema":true,"json_schema_state":"declared","context_window":200_000}]}},
            "router":{"version":1,"pools":{"main":[{"upstream":"plan","model":"gpt-5.5"},{"upstream":"paid","model":"gpt-5.5"}]},"default_pool":"main"}
        })).unwrap();
        if codex {
            config
                .plugins
                .providers
                .insert("codex-plan".into(), "provider-chatgpt-plan-v2".into());
            let plan = config.upstreams.get_mut("plan").unwrap();
            plan.provider = "codex-plan".into();
            plan.auth.as_mut().unwrap().slot = "codex_access_token".into();
            plan.codex_identity = Some(token_station_cli::config::CodexIdentity {
                account_id: "fixture-account".into(),
                client_version: "0.162.0".into(),
            });
            token_station_cli::secrets::store_set(
                &scratch.0,
                "plan",
                "codex_access_token",
                "protocol-fixture-only",
            )
            .unwrap();
        }
        config.validate().unwrap();
        let records = Arc::new(Records::default());
        let gateway = Gateway::new(&config, records.clone()).unwrap();
        Self {
            gateway,
            seen,
            records,
            stop,
            worker: Some(worker),
            paid,
            _scratch: scratch,
        }
    }
    fn request(&self, protocol: Protocol, stream: bool, body: &Value) -> (u16, String) {
        let mut status = 0;
        let mut output = String::new();
        self.gateway.chat(
            "POST",
            protocol.path(stream),
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
            std::io::ErrorKind::WouldBlock
        );
        assert!(!output.contains("protocol-fixture-only"));
        (status, output)
    }
}
impl Drop for Harness {
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
fn sse(events: Vec<Value>) -> String {
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
fn tools_response(calls: Vec<(String, Value)>) -> String {
    let mut frames = vec![
        json!({"type":"response.created","response":{"id":"resp_tools","model":"gpt-5.5","status":"in_progress"}}),
    ];
    let mut items = Vec::new();
    for (index, (name, args)) in calls.into_iter().enumerate() {
        let item = json!({"type":"function_call","namespace":"tokenstation_tools","id":format!("fc_{index}"),"call_id":format!("call_{index}"),"name":name,"arguments":"","status":"in_progress"});
        let arguments = args.to_string();
        let mut done = item.clone();
        done["arguments"] = json!(arguments);
        done["status"] = json!("completed");
        frames.push(json!({"type":"response.output_item.added","output_index":index,"item":item}));
        items.push((done, arguments));
    }
    // Interleave fragments across calls to exercise IDs and independent buffers.
    for half in 0..2 {
        for (index, (_, arguments)) in items.iter().enumerate().rev() {
            let split = arguments.len() / 2;
            let delta = if half == 0 {
                &arguments[..split]
            } else {
                &arguments[split..]
            };
            frames.push(json!({"type":"response.function_call_arguments.delta","output_index":index,"item_id":format!("fc_{index}"),"delta":delta}));
        }
    }
    for (index, (done, arguments)) in items.iter().enumerate() {
        frames.push(json!({"type":"response.function_call_arguments.done","output_index":index,"item_id":format!("fc_{index}"),"arguments":arguments}));
        frames.push(json!({"type":"response.output_item.done","output_index":index,"item":done}));
    }
    let output: Vec<_> = items.into_iter().map(|(item, _)| item).collect();
    frames.push(json!({"type":"response.completed","response":{"id":"resp_tools","model":"gpt-5.5","status":"completed","output":output,"usage":{"input_tokens":10,"output_tokens":5}}}));
    sse(frames)
}
fn final_response(terminal: &str) -> String {
    let mut events = vec![
        json!({"type":"response.created","response":{"id":"resp_final","model":"gpt-5.5","status":"in_progress"}}),
        json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":FINAL}),
    ];
    if terminal != "truncated" {
        events.push(json!({"type":format!("response.{terminal}"),"response":{"id":"resp_final","model":"gpt-5.5","status":terminal,"output":[],"usage":{"input_tokens":10,"output_tokens":5},"error":if terminal=="completed"{Value::Null}else{json!({"code":"server_error","message":"Fixture failure"})}}}));
    }
    sse(events)
}

#[test]
fn both_plan_dialects_handle_metadata_without_changing_protocol_outcomes() {
    for codex in [false, true] {
        for protocol in [
            Protocol::Anthropic,
            Protocol::Chat,
            Protocol::Responses,
            Protocol::Gemini,
        ] {
            for stream in [false, true] {
                for terminal in ["completed", "failed", "truncated"] {
                    let harness = Harness::for_provider(codex, move |_, _| {
                        let mut events = frames(&final_response(terminal));
                        events.insert(2, json!({"type":"response.metadata","metadata":{"private":"SYNTHETIC_METADATA_ONLY","status":"completed"}}));
                        (200, sse(events))
                    });
                    let (status, output) =
                        harness.request(protocol, stream, &protocol.request(stream));
                    assert!(!output.contains("SYNTHETIC_METADATA_ONLY"));
                    let records = harness.records.0.lock().unwrap();
                    let record = records.last().unwrap();
                    if terminal == "completed" {
                        assert_eq!(status, 200, "{codex} {protocol:?} {stream}: {output}");
                        assert!(output.contains(FINAL), "{output}");
                        assert_eq!(record.status, 200);
                        let usage = record.usage.as_ref().unwrap();
                        assert_eq!(usage.input_tokens, 10);
                        assert_eq!(usage.output_tokens, 5);
                    } else {
                        assert_ne!(
                            record.status, 200,
                            "{codex} {protocol:?} {stream}: {output}"
                        );
                        if stream {
                            assert!(output.contains("error"), "{output}");
                        } else {
                            assert!(status >= 400, "{output}");
                        }
                    }
                    assert!(record.subscription_billing);
                    assert_eq!(record.cost_micros, None);
                    assert_eq!(record.attempts, 1);
                }
            }
        }
    }
}

fn frames(output: &str) -> Vec<Value> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|line| *line != "[DONE]")
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
fn response_document(output: &str, stream: bool) -> Value {
    if !stream {
        return serde_json::from_str(output).unwrap();
    }
    frames(output)
        .into_iter()
        .find(|v| v["type"] == "response.completed")
        .expect("successful Responses terminal")["response"]
        .clone()
}
fn declared_names(request: &Value) -> Vec<String> {
    request["tools"][0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}
fn assert_history(request: &Value, count: usize) {
    let input = request["input"].as_array().unwrap();
    for index in 0..count {
        let id = format!("call_{index}");
        assert!(
            input
                .iter()
                .any(|item| item["type"] == "function_call" && item["call_id"] == id),
            "{input:?}"
        );
        let result = input
            .iter()
            .find(|item| item["type"] == "function_call_output" && item["call_id"] == id)
            .expect("matched result");
        assert!(
            result["output"]
                .as_str()
                .unwrap()
                .contains(&format!("RESULT_{index}")),
            "{result}"
        );
    }
}

#[test]
fn responses_custom_patch_and_local_shell_round_trip_in_json_and_sse() {
    for stream in [false, true] {
        let harness = Harness::new(|turn, request| {
            if turn == 0 {
                let calls=declared_names(request).into_iter().map(|name|{let args=if name=="apply_patch"{json!({"input":PATCH})}else{json!({"action":{"type":"exec","command":["echo","fixture"],"timeout_ms":1000}})};(name,args)}).collect();
                (200, tools_response(calls))
            } else {
                (200, final_response("completed"))
            }
        });
        let mut body = Protocol::Responses.request(stream);
        body["tools"] = json!([{"type":"custom","name":"apply_patch","description":"Apply a patch","format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}},{"type":"local_shell"}]);
        let (status, output) = harness.request(Protocol::Responses, stream, &body);
        assert_eq!(status, 200, "{output}");
        let response = response_document(&output, stream);
        let calls: Vec<_> = response["output"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| {
                matches!(
                    item["type"].as_str(),
                    Some("custom_tool_call" | "local_shell_call")
                )
            })
            .collect();
        assert_eq!(calls.len(), 2, "{response}");
        assert_eq!(calls[0]["type"], "custom_tool_call");
        assert_eq!(calls[0]["input"], PATCH);
        assert_eq!(calls[0]["call_id"], "call_0");
        assert_eq!(calls[1]["type"], "local_shell_call");
        assert_eq!(calls[1]["action"]["command"], json!(["echo", "fixture"]));
        body["input"] = json!([{"role":"user","content":"Apply the fixture patch and run echo"},calls[0],calls[1],{"type":"custom_tool_call_output","call_id":"call_0","output":"RESULT_0"},{"type":"local_shell_call_output","id":"call_1","output":"RESULT_1"}]);
        let (status, output) = harness.request(Protocol::Responses, stream, &body);
        assert_eq!(status, 200, "{output}");
        assert!(output.contains(FINAL));
        assert_history(&harness.seen.lock().unwrap()[1], 2);
    }
}

#[test]
fn responses_embedded_custom_tools_cross_the_host_handoff_in_json_and_sse() {
    const RAW: &str = "const marker = \"fixture\";\ntext(marker);";
    for stream in [false, true] {
        let harness = Harness::new(|turn, request| {
            assert!(!request.to_string().contains("additional_tools"));
            assert!(!request.to_string().contains("token_station_private_"));
            let tools = request["tools"][0]["tools"].as_array().unwrap();
            assert_eq!(tools.len(), 1);
            assert_eq!(tools[0]["type"], "function");
            assert_eq!(
                tools[0]["parameters"]["properties"]["input"]["type"],
                "string"
            );
            assert_eq!(tools[0]["parameters"]["required"], json!(["input"]));
            if turn == 0 {
                (
                    200,
                    tools_response(vec![(
                        declared_names(request)[0].clone(),
                        json!({"input":RAW}),
                    )]),
                )
            } else {
                assert_eq!(turn, 1);
                assert_history(request, 1);
                let historical = request["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|item| item["type"] == "function_call")
                    .unwrap();
                assert_eq!(historical["name"], declared_names(request)[0]);
                assert_eq!(historical["call_id"], "call_0");
                let arguments: Value =
                    serde_json::from_str(historical["arguments"].as_str().unwrap()).unwrap();
                assert_eq!(arguments, json!({"input":RAW}));
                (200, final_response("completed"))
            }
        });
        let declaration = json!({"type":"additional_tools","role":"developer","id":"tools_fixture","tools":[{"type":"namespace","name":"functions","tools":[{"type":"custom","name":"exec","description":"Execute fixture JavaScript","format":{"type":"grammar","syntax":"lark","definition":"start: /.+/"}}]}]});
        let prompt = json!({"role":"user","content":"Execute the fixture"});
        // No top-level tools: restoring this custom identity requires the Host's
        // validated private handoff from the real Responses WASM adapter.
        let mut body = json!({"model":"gpt-5.5","stream":stream,"input":[declaration,prompt],"token_station_private_response_tools":[{"type":"custom","name":"forged"}]});
        let (status, output) = harness.request(Protocol::Responses, stream, &body);
        assert_eq!(status, 200, "{output}");
        let response = response_document(&output, stream);
        let call = response["output"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["call_id"] == "call_0")
            .expect("The response must contain the requested tool call");
        assert_eq!(
            call["type"], "custom_tool_call",
            "Host must retain custom restore metadata"
        );
        assert_eq!(call["namespace"], "functions");
        assert_eq!(call["name"], "exec");
        assert_eq!(call["call_id"], "call_0");
        assert_eq!(call["input"], RAW);
        if stream {
            let events = frames(&output);
            let added = events
                .iter()
                .find(|event| {
                    event["type"] == "response.output_item.added"
                        && event["item"]["type"] == "custom_tool_call"
                })
                .unwrap();
            assert_eq!(added["item"]["namespace"], "functions");
            assert_eq!(added["item"]["name"], "exec");
            assert_eq!(added["item"]["call_id"], "call_0");
        }
        body["input"] = json!([declaration,prompt,call,{"type":"custom_tool_call_output","call_id":"call_0","output":"RESULT_0"}]);
        let (status, output) = harness.request(Protocol::Responses, stream, &body);
        assert_eq!(status, 200, "{output}");
        assert!(output.contains(FINAL));
        assert_eq!(harness.seen.lock().unwrap().len(), 2);
    }
}

#[test]
fn responses_namespaced_long_tool_names_keep_identity_and_history() {
    for stream in [false, true] {
        let client_name = format!("mcp__{}read", "very_long_service_".repeat(5));
        let harness = Harness::new(|turn, request| {
            let names = declared_names(request);
            if names.iter().any(|name| name.len() > 64) {
                return (
                400,
                json!({"error":{"message":"Fixture rejects function names longer than 64 bytes"}})
                    .to_string(),
            );
            }
            if turn == 0 {
                (
                    200,
                    tools_response(vec![(names[0].clone(), json!({"path":"fixture"}))]),
                )
            } else {
                (200, final_response("completed"))
            }
        });
        let mut body = Protocol::Responses.request(stream);
        body["tools"] = json!([{"type":"namespace","name":"files","tools":[{"type":"function","name":client_name,"parameters":{"type":"object"}}]}]);
        let (status, output) = harness.request(Protocol::Responses, stream, &body);
        assert_eq!(status, 200, "{output}");
        let response = response_document(&output, stream);
        let call = response["output"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call")
            .unwrap();
        assert_eq!(call["namespace"], "files");
        assert_eq!(call["name"], client_name);
        body["input"] = json!([{"role":"user","content":"Read fixture"},call,{"type":"function_call_output","call_id":call["call_id"],"output":"RESULT_0"}]);
        let (status, output) = harness.request(Protocol::Responses, stream, &body);
        assert_eq!(status, 200, "{output}");
        assert_history(&harness.seen.lock().unwrap()[1], 1);
    }
}

#[test]
fn responses_forced_long_tool_name_uses_its_alias_and_restores_stream_identity() {
    for stream in [false, true] {
        let original = format!("client_{}read", "long_tool_".repeat(8));
        let harness = Harness::new(|turn, request| {
            let names = declared_names(request);
            if names.iter().any(|name| name.len() > 64) {
                return (
                    400,
                    json!({"error":{"message":"Fixture rejects long function names"}}).to_string(),
                );
            }
            if turn == 0 {
                assert_eq!(request["tool_choice"], "required");
                assert_eq!(
                    names.len(),
                    1,
                    "forced choice must not expose the other tool"
                );
                (
                    200,
                    tools_response(vec![(names[0].clone(), json!({"path":"fixture"}))]),
                )
            } else {
                (200, final_response("completed"))
            }
        });
        let mut body = Protocol::Responses.request(stream);
        body["tools"] = json!([
            {"type":"function","name":original,"parameters":{"type":"object"}},
            {"type":"function","name":"unselected","parameters":{"type":"object"}}
        ]);
        body["tool_choice"] = json!({"type":"function","name":original});
        let (status, output) = harness.request(Protocol::Responses, stream, &body);
        assert_eq!(status, 200, "{output}");
        let response = response_document(&output, stream);
        let call = response["output"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call")
            .unwrap();
        assert_eq!(call["name"], original);
        body["input"] = json!([{"role":"user","content":"Read fixture"},call,{"type":"function_call_output","call_id":call["call_id"],"output":"RESULT_0"}]);
        body["tool_choice"] = json!("auto");
        let (status, output) = harness.request(Protocol::Responses, stream, &body);
        assert_eq!(status, 200, "{output}");
        let rows = harness.seen.lock().unwrap();
        assert_history(&rows[1], 1);
        let historical = rows[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "function_call")
            .unwrap();
        assert_eq!(historical["name"], declared_names(&rows[0])[0]);
    }
}

#[test]
fn gemini_json_schema_and_parallel_results_retain_ids_in_json_and_sse() {
    for stream in [false, true] {
        let harness = Harness::new(|turn, _| {
            if turn == 0 {
                (
                    200,
                    tools_response(vec![
                        ("read".into(), json!({"path":"one"})),
                        ("read".into(), json!({"path":"two"})),
                    ]),
                )
            } else {
                (200, final_response("completed"))
            }
        });
        let mut body = Protocol::Gemini.request(stream);
        let (status, output) = harness.request(Protocol::Gemini, stream, &body);
        assert_eq!(status, 200, "{output}");
        let document = if stream {
            frames(&output)
                .into_iter()
                .find(|v| {
                    v["candidates"][0]["content"]["parts"]
                        .as_array()
                        .is_some_and(|p| p.iter().any(|p| p.get("functionCall").is_some()))
                })
                .unwrap()
        } else {
            serde_json::from_str(&output).unwrap()
        };
        let content = &document["candidates"][0]["content"];
        let parts: Vec<_> = content["parts"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|part| part.get("functionCall").is_some())
            .collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(
            parts[0]["functionCall"]["id"], "call_0",
            "Gemini needs IDs for out-of-order same-name tool results: {document}"
        );
        assert_eq!(parts[1]["functionCall"]["id"], "call_1");
        body["contents"]
            .as_array_mut()
            .unwrap()
            .push(content.clone());
        body["contents"].as_array_mut().unwrap().push(json!({"role":"user","parts":[{"functionResponse":{"id":"call_1","name":"read","response":{"text":"RESULT_1"}}},{"functionResponse":{"id":"call_0","name":"read","response":{"text":"RESULT_0"}}}]}));
        let (status, output) = harness.request(Protocol::Gemini, stream, &body);
        assert_eq!(status, 200, "{output}");
        assert!(output.contains(FINAL));
        let rows = harness.seen.lock().unwrap();
        assert_history(&rows[1], 2);
        assert_eq!(
            rows[0]["tools"][0]["tools"][0]["parameters"],
            body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"]
        );
    }
}

#[test]
fn gemini_legacy_schema_type_enums_become_valid_json_schema() {
    let harness = Harness::new(|_, _| (200, final_response("completed")));
    let mut body = Protocol::Gemini.request(false);
    body["tools"] = json!([{"functionDeclarations":[{"name":"read","parameters":{"type":"OBJECT","properties":{"paths":{"type":"ARRAY","items":{"type":"STRING"}},"mode":{"type":"STRING","enum":["FAST","SAFE"]}},"required":["paths"]}}]}]);
    let (status, output) = harness.request(Protocol::Gemini, false, &body);
    assert_eq!(status, 200, "{output}");
    let rows = harness.seen.lock().unwrap();
    let schema = &rows[0]["tools"][0]["tools"][0]["parameters"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["paths"]["type"], "array");
    assert_eq!(schema["properties"]["paths"]["items"]["type"], "string");
    assert_eq!(
        schema["properties"]["mode"]["enum"],
        json!(["FAST", "SAFE"])
    );
}

#[test]
fn gemini_tool_config_constraints_are_enforced_or_explicitly_rejected() {
    for (mode, names) in [
        ("NONE", None),
        ("AUTO", None),
        ("ANY", Some(json!(["read"]))),
        ("ANY", Some(json!(["read", "write"]))),
    ] {
        let harness = Harness::new(|_, _| (200, final_response("completed")));
        let mut body = Protocol::Gemini.request(false);
        body["toolConfig"] = json!({"functionCallingConfig":{"mode":mode}});
        if let Some(names) = &names {
            body["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"] = names.clone();
        }
        body["tools"][0]["functionDeclarations"]
            .as_array_mut()
            .unwrap()
            .extend([
                json!({"name":"write", "parametersJsonSchema":{"type":"object"}}),
                json!({"name":"delete", "parametersJsonSchema":{"type":"object"}}),
            ]);
        let (status, output) = harness.request(Protocol::Gemini, false, &body);
        assert_eq!(
            status, 200,
            "supported Gemini function modes must work: {output}"
        );
        let rows = harness.seen.lock().unwrap();
        assert_eq!(
            rows[0]["tool_choice"],
            match mode {
                "NONE" => json!("none"),
                "AUTO" => json!("auto"),
                _ => json!("required"),
            },
            "Gemini toolConfig must not be silently erased"
        );
        if let Some(names) = names {
            assert_eq!(
                json!(declared_names(&rows[0])),
                names,
                "allowedFunctionNames must restrict declarations"
            );
        }
    }
}

#[test]
fn anthropic_and_chat_stream_parallel_calls_keep_separate_arguments() {
    for protocol in [Protocol::Anthropic, Protocol::Chat] {
        let harness = Harness::new(|_, _| {
            (
                200,
                tools_response(vec![
                    ("read".into(), json!({"path":"one"})),
                    ("read".into(), json!({"path":"two"})),
                ]),
            )
        });
        let (status, output) = harness.request(protocol, true, &protocol.request(true));
        assert_eq!(status, 200, "{output}");
        let events = frames(&output);
        let mut ids = Vec::new();
        let mut arguments = std::collections::BTreeMap::<u64, String>::new();
        for event in events {
            match protocol {
                Protocol::Anthropic => {
                    if event["type"] == "content_block_start"
                        && event["content_block"]["type"] == "tool_use"
                    {
                        ids.push(event["content_block"]["id"].as_str().unwrap().to_owned());
                    }
                    if let Some(text) = event["delta"]["partial_json"].as_str() {
                        arguments
                            .entry(event["index"].as_u64().unwrap())
                            .or_default()
                            .push_str(text);
                    }
                }
                Protocol::Chat => {
                    for call in event["choices"][0]["delta"]["tool_calls"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        if let Some(id) = call["id"].as_str() {
                            ids.push(id.into());
                        }
                        if let Some(text) = call["function"]["arguments"].as_str() {
                            arguments
                                .entry(call["index"].as_u64().unwrap())
                                .or_default()
                                .push_str(text);
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(ids, vec!["call_0", "call_1"], "{output}");
        let parsed: Vec<Value> = arguments
            .values()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        assert_eq!(parsed, vec![json!({"path":"one"}), json!({"path":"two"})]);
    }
}

#[test]
fn all_four_protocols_reject_failed_and_truncated_plan_responses() {
    for protocol in [
        Protocol::Anthropic,
        Protocol::Chat,
        Protocol::Responses,
        Protocol::Gemini,
    ] {
        for stream in [false, true] {
            for terminal in ["failed", "truncated"] {
                let harness = Harness::new(move |_, _| (200, final_response(terminal)));
                let (status, output) = harness.request(protocol, stream, &protocol.request(stream));
                if stream {
                    assert!(
                        output.contains("error"),
                        "{protocol:?} {terminal}: {status}: {output}"
                    );
                    assert!(
                        frames(&output).iter().all(|event| {
                            event["type"] != "response.completed"
                                && event["choices"][0]["finish_reason"] != "stop"
                                && event["candidates"][0]["finishReason"] != "STOP"
                        }),
                        "{protocol:?} {terminal}: {status}: {output}"
                    );
                } else {
                    assert!(status >= 400, "{protocol:?} {terminal}: {status}: {output}");
                    assert!(!output.contains(FINAL));
                }
                let records = harness.records.0.lock().unwrap();
                assert_ne!(records.last().unwrap().status, 200);
                assert_eq!(records.last().unwrap().cost_micros, None);
            }
        }
    }
}

#[test]
fn gemini_hosted_tools_and_ambiguous_schemas_fail_before_model_dispatch() {
    for tools in [
        json!([{"googleSearch":{}}]),
        json!([{"functionDeclarations":[{"name":"read","parameters":{"type":"object"},"parametersJsonSchema":{"type":"object"}}]}]),
    ] {
        let harness = Harness::new(|_, _| panic!("unsupported request reached plan upstream"));
        let mut body = Protocol::Gemini.request(false);
        body["tools"] = tools;
        let (status, output) = harness.request(Protocol::Gemini, false, &body);
        assert!(status >= 400, "{output}");
        assert!(harness.seen.lock().unwrap().is_empty());
    }
}

#[test]
fn chat_response_formats_reach_plan_text_format_in_json_and_sse() {
    let schema = json!({"name":"session_title","description":"Session title","strict":true,"schema":{"type":"object","properties":{"title":{"type":"string","maxLength":80}},"required":["title"],"additionalProperties":false}});
    for stream in [false, true] {
        for format in [
            json!({"type":"text"}),
            json!({"type":"json_object"}),
            json!({"type":"json_schema","json_schema":schema}),
        ] {
            let expected = if format["type"] == "json_schema" {
                let mut expected = schema.clone();
                expected["type"] = json!("json_schema");
                expected
            } else {
                format.clone()
            };
            let harness = Harness::new(move |_, request| {
                assert_eq!(request["text"]["format"], expected);
                assert!(request.get("response_format").is_none());
                (200, final_response("completed"))
            });
            let body = json!({"model":"gpt-5.5","stream":stream,"messages":[{"role":"user","content":"Return a JSON session title."}],"response_format":format});
            let (status, output) = harness.request(Protocol::Chat, stream, &body);
            assert_eq!(status, 200, "{output}");
            assert!(output.contains(FINAL));
            assert_eq!(harness.seen.lock().unwrap().len(), 1);
        }
    }
}

#[test]
fn malformed_chat_response_formats_fail_before_plan_dispatch() {
    for codex in [false, true] {
        for stream in [false, true] {
            let harness = Harness::for_provider(codex, |_, _| {
                panic!("Invalid structured output must not reach any provider")
            });
            for format in [
                json!({"type":"text","unknown":true}),
                json!({"type":"json_schema","json_schema":{"name":"answer","schema":{},"strict":"true"}}),
                json!({"type":"json_schema","json_schema":{"name":"answer","schema":{},"unknown":true}}),
            ] {
                let body = json!({"model":"gpt-5.5","stream":stream,"messages":[{"role":"user","content":"Return a title"}],"response_format":format});
                let (status, output) = harness.request(Protocol::Chat, stream, &body);
                assert_eq!(status, 400, "{output}");
                assert!(harness.seen.lock().unwrap().is_empty());
            }
        }
    }
}

#[test]
fn codex_subscription_keeps_four_protocol_tool_history_and_accounting() {
    for protocol in [
        Protocol::Anthropic,
        Protocol::Chat,
        Protocol::Responses,
        Protocol::Gemini,
    ] {
        for stream in [false, true] {
            let harness = Harness::for_provider(true, |turn, request| {
                assert!(request["instructions"].is_string());
                assert_eq!(request["stream"], true);
                assert_eq!(request["store"], false);
                if turn == 0 {
                    (
                        200,
                        tools_response(vec![(
                            declared_names(request)[0].clone(),
                            json!({"path":"fixture.txt"}),
                        )]),
                    )
                } else {
                    assert_history(request, 1);
                    (200, final_response("completed"))
                }
            });
            let mut body = protocol.request(stream);
            let (status, output) = harness.request(protocol, stream, &body);
            assert_eq!(status, 200, "{protocol:?}: {output}");
            assert!(
                output.contains("call_0") && output.contains("read"),
                "{output}"
            );
            match protocol {
                Protocol::Anthropic => {
                    body["messages"] = json!([
                        {"role":"user","content":"Read fixture files"},
                        {"role":"assistant","content":[{"type":"tool_use","id":"call_0","name":"read","input":{"path":"fixture.txt"}}]},
                        {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_0","content":"RESULT_0"}]}
                    ]);
                }
                Protocol::Chat => {
                    body["messages"] = json!([
                        {"role":"user","content":"Read fixture files"},
                        {"role":"assistant","tool_calls":[{"id":"call_0","type":"function","function":{"name":"read","arguments":"{\"path\":\"fixture.txt\"}"}}]},
                        {"role":"tool","tool_call_id":"call_0","content":"RESULT_0"}
                    ]);
                }
                Protocol::Responses => {
                    body["input"] = json!([
                        {"role":"user","content":"Read fixture files"},
                        {"type":"function_call","call_id":"call_0","name":"read","arguments":"{\"path\":\"fixture.txt\"}"},
                        {"type":"function_call_output","call_id":"call_0","output":"RESULT_0"}
                    ]);
                }
                Protocol::Gemini => {
                    body["contents"] = json!([
                        {"role":"user","parts":[{"text":"Read fixture files"}]},
                        {"role":"model","parts":[{"functionCall":{"id":"call_0","name":"read","args":{"path":"fixture.txt"}}}]},
                        {"role":"user","parts":[{"functionResponse":{"id":"call_0","name":"read","response":{"result":"RESULT_0"}}}]}
                    ]);
                }
            }
            let (status, output) = harness.request(protocol, stream, &body);
            assert_eq!(status, 200, "{output}");
            assert!(output.contains(FINAL));
            let records = harness.records.0.lock().unwrap();
            assert_eq!(records.len(), 2);
            for record in records.iter() {
                assert!(record.subscription_billing);
                assert_eq!(record.cost_micros, None);
                assert_eq!(record.attempts, 1);
            }
        }
    }
}

#[test]
fn codex_subscription_errors_never_fall_back_to_paid_models() {
    for stream in [false, true] {
        let harness = Harness::for_provider(true, |_, _| {
            (
                429,
                json!({"error":{"code":"usage_limit_reached"}}).to_string(),
            )
        });
        let (status, body) =
            harness.request(Protocol::Chat, stream, &Protocol::Chat.request(stream));
        assert!(status >= 400, "{body}");
        assert_eq!(harness.seen.lock().unwrap().len(), 1);
        let records = harness.records.0.lock().unwrap();
        assert!(records[0].subscription_billing);
        assert_eq!(records[0].cost_micros, None);
        assert_eq!(records[0].attempts, 1);
    }
}

#[test]
fn plan_sse_reasoning_rejection_preserves_client_recovery_and_schema() {
    let schema = json!({"type":"object","properties":{"title":{"type":"string","maxLength":80}},"required":["title"],"additionalProperties":false});
    for codex in [false, true] {
        for stream in [false, true] {
            let expected = schema.clone();
            let harness = Harness::for_provider(codex, move |_, request| {
                assert_eq!(request["text"]["format"]["schema"], expected);
                assert_eq!(request["reasoning"]["effort"], "none");
                (
                    200,
                    format!(
                        "event: error\ndata: {}\n\n",
                        json!({"type":"error","error":{"code":"unsupported_value","param":"reasoning.effort","message":"secret-model-token"}})
                    ),
                )
            });
            let body = json!({"model":"gpt-5.5","stream":stream,"messages":[{"role":"user","content":"Return title"}],"reasoning_effort":"none","response_format":{"type":"json_schema","json_schema":{"name":"title","strict":true,"schema":schema}}});
            let (status, output) = harness.request(Protocol::Chat, stream, &body);
            assert_eq!(status, 400, "{output}");
            assert!(
                output.contains("Unsupported value for reasoning_effort"),
                "{output}"
            );
            assert!(!output.contains("secret-model-token"));
            assert_eq!(
                harness.seen.lock().unwrap().len(),
                1,
                "Host must not silently retry or strip reasoning"
            );
        }
    }
}

#[test]
fn both_plan_dialects_preserve_supported_schema_constraints() {
    let schema = json!({"type":"object","properties":{"title":{"type":"string","maxLength":80,"pattern":"^[A-Z]"}},"required":["title"],"additionalProperties":false});
    for codex in [false, true] {
        for protocol in [Protocol::Chat, Protocol::Responses] {
            for stream in [false, true] {
                let format =
                    json!({"type":"json_schema","name":"answer","strict":true,"schema":schema});
                let expected = format.clone();
                let harness = Harness::for_provider(codex, move |_, request| {
                    assert_eq!(request["text"]["format"], expected);
                    (200, final_response("completed"))
                });
                let mut body = protocol.request(stream);
                body.as_object_mut().unwrap().remove("tools");
                match protocol {
                    Protocol::Chat => {
                        body["response_format"] = json!({"type":"json_schema","json_schema":{"name":"answer","strict":true,"schema":schema}});
                    }
                    Protocol::Responses => body["text"] = json!({"format":format}),
                    _ => unreachable!(),
                }
                let (status, output) = harness.request(protocol, stream, &body);
                assert_eq!(status, 200, "{protocol:?}: {output}");
                assert!(output.contains(FINAL));
                assert_eq!(harness.seen.lock().unwrap().len(), 1);
            }
        }
    }
}

#[test]
fn both_plan_dialects_reject_unsupported_schema_constraints_before_dispatch() {
    for codex in [false, true] {
        for protocol in [Protocol::Anthropic, Protocol::Gemini] {
            for stream in [false, true] {
                let harness =
                    Harness::for_provider(codex, |_, _| (200, final_response("completed")));
                let mut body = protocol.request(stream);
                body.as_object_mut().unwrap().remove("tools");
                let schema = json!({"type":"object","properties":{"title":{"type":"string","maxLength":80}},"required":["title"],"additionalProperties":false});
                match protocol {
                    Protocol::Anthropic => {
                        body["output_config"] =
                            json!({"format":{"type":"json_schema","schema":schema}});
                    }
                    Protocol::Gemini => {
                        body["generationConfig"] = json!({"responseMimeType":"application/json","responseJsonSchema":schema});
                    }
                    _ => unreachable!(),
                }
                let (status, output) = harness.request(protocol, stream, &body);
                assert_eq!(
                    status, 400,
                    "{protocol:?} must reject unsupported schema rather than erase constraints: {output}"
                );
                assert!(harness.seen.lock().unwrap().is_empty());
            }
        }
    }
}

#[test]
fn both_plan_dialects_apply_buffered_stops_and_reject_streamed_stops() {
    for codex in [false, true] {
        for protocol in [
            Protocol::Anthropic,
            Protocol::Chat,
            Protocol::Responses,
            Protocol::Gemini,
        ] {
            for stream in [false, true] {
                let harness = Harness::for_provider(codex, |_, request| {
                    assert!(request.get("stop").is_none());
                    (
                        200,
                        final_response("completed").replace(FINAL, "VISIBLE_STOP_HIDDEN"),
                    )
                });
                let mut body = protocol.request(stream);
                body.as_object_mut().unwrap().remove("tools");
                match protocol {
                    Protocol::Anthropic => body["stop_sequences"] = json!(["_STOP_"]),
                    Protocol::Chat | Protocol::Responses => body["stop"] = json!(["_STOP_"]),
                    Protocol::Gemini => {
                        body["generationConfig"] = json!({"stopSequences":["_STOP_"]});
                    }
                }
                let (status, output) = harness.request(protocol, stream, &body);
                assert_eq!(
                    status,
                    if stream { 400 } else { 200 },
                    "{protocol:?}: {output}"
                );
                assert_eq!(harness.seen.lock().unwrap().len(), usize::from(!stream));
                if !stream {
                    assert!(output.contains("VISIBLE"), "{output}");
                    assert!(!output.contains("HIDDEN"), "{output}");
                    if matches!(protocol, Protocol::Anthropic) {
                        let response: Value = serde_json::from_str(&output).unwrap();
                        assert_eq!(response["stop_reason"], "stop_sequence");
                        assert_eq!(response["stop_sequence"], "_STOP_");
                    }
                }
            }
        }
    }
}
