//! Search policy tests through the public gateway boundary and real adapters.
#[allow(clippy::wildcard_imports)]
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
struct Records(Mutex<Vec<RequestRecord>>);
impl Recorder for Records {
    fn record(&self, record: &RequestRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

#[test]
fn explicit_rejection_falls_back_and_cache_resets_on_verification() {
    for anthropic in [false, true] {
        exercise(
            "auto",
            400,
            "Tool type 'web_search' is not supported",
            anthropic,
            3,
            false,
        );
    }
}

#[test]
fn function_only_contract_refusal_allows_local_supplement() {
    exercise(
        "auto",
        400,
        "Anthropic text billing allows only client functions with name and object input_schema",
        false,
        1,
        false,
    );
}

#[test]
fn native_only_does_not_start_browser_on_rejection() {
    exercise(
        "native",
        400,
        "Tool type 'web_search' is not supported",
        false,
        1,
        false,
    );
}

#[test]
fn local_only_skips_native_provider_search() {
    exercise("local", 200, "", false, 1, false);
}

#[test]
fn account_transport_and_parameter_errors_do_not_trigger_browser() {
    for (status, error) in [
        (401, "web_search not supported"),
        (402, "Insufficient balance"),
        (429, "web_search not supported"),
        (500, "web_search not supported"),
        (400, "Unsupported web_search allowed_domains"),
        (422, "Invalid request"),
    ] {
        exercise("auto", status, error, false, 1, false);
    }
}

#[test]
fn fallback_does_not_drop_native_domain_constraints() {
    exercise(
        "auto",
        400,
        "Tool type 'web_search' is not supported",
        false,
        1,
        true,
    );
}

#[test]
fn native_search_succeeds_without_browser_execution() {
    for anthropic in [false, true] {
        exercise("auto", 200, "", anthropic, 1, false);
    }
}

#[test]
fn both_native_search_bridges_preserve_stream_completion_and_never_replay_failures() {
    for (anthropic, native_anthropic) in [(true, false), (false, true)] {
        for (truncated, cancel) in [(false, false), (true, false), (false, true)] {
            exercise_transport(
                "native",
                200,
                "",
                anthropic,
                1,
                false,
                native_anthropic,
                true,
                truncated,
                cancel,
            );
        }
        exercise_transport(
            "native",
            200,
            "",
            anthropic,
            1,
            false,
            native_anthropic,
            false,
            false,
            false,
        );
    }
}

#[test]
fn legacy_search_bridge_streams_without_managed_search_settings() {
    for (truncated, cancel) in [(false, false), (true, false), (false, true)] {
        exercise_transport(
            "disabled", 200, "", true, 1, false, false, true, truncated, cancel,
        );
    }
}

fn exercise(
    mode: &str,
    status: u16,
    error: &str,
    anthropic: bool,
    requests: usize,
    constrained: bool,
) {
    exercise_transport(
        mode,
        status,
        error,
        anthropic,
        requests,
        constrained,
        false,
        false,
        false,
        false,
    );
}

#[test]
fn native_stream_completion_truncation_and_cancellation_never_replay() {
    for (truncated, cancel) in [(false, false), (true, false), (false, true)] {
        exercise_transport(
            "auto", 200, "", false, 1, false, false, true, truncated, cancel,
        );
    }
}

#[test]
fn anthropic_native_search_and_rejection_use_the_same_offering() {
    exercise_transport("auto", 200, "", true, 1, false, true, false, false, false);
    exercise_transport(
        "auto",
        400,
        "Tool type 'web_search' is not supported",
        true,
        3,
        false,
        true,
        false,
        false,
        false,
    );
}

#[test]
fn separate_native_search_profile_preserves_regular_transport() {
    for native_anthropic in [false, true] {
        exercise_profile(
            "auto",
            200,
            "",
            true,
            1,
            false,
            native_anthropic,
            false,
            false,
            false,
            true,
        );
        exercise_profile(
            "auto",
            400,
            "Tool type 'web_search' is not supported",
            true,
            1,
            false,
            native_anthropic,
            false,
            false,
            false,
            true,
        );
    }
}

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
fn exercise_transport(
    mode: &str,
    status: u16,
    error: &str,
    anthropic: bool,
    requests: usize,
    constrained: bool,
    native_anthropic: bool,
    stream: bool,
    truncated: bool,
    cancel: bool,
) {
    exercise_profile(
        mode,
        status,
        error,
        anthropic,
        requests,
        constrained,
        native_anthropic,
        stream,
        truncated,
        cancel,
        false,
    );
}

#[allow(
    clippy::too_many_lines,
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools
)]
fn exercise_profile(
    mode: &str,
    status: u16,
    error: &str,
    anthropic: bool,
    requests: usize,
    constrained: bool,
    native_anthropic: bool,
    stream: bool,
    truncated: bool,
    cancel: bool,
    profile: bool,
) {
    static SEQUENCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("ts-policy-{}-{sequence}-{mode}-{status}-{anthropic}-{requests}-{constrained}-{native_anthropic}-{stream}-{truncated}-{cancel}-{profile}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let key = root.join("key");
    std::fs::write(&key, "test-key").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stop);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&seen);
    let early_text = Arc::new(AtomicBool::new(false));
    let observed_text = Arc::clone(&early_text);
    let response_error = error.to_owned();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !stopped.load(Ordering::Relaxed) && Instant::now() < deadline {
            let Ok((mut connection, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            };
            connection.set_nonblocking(false).unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut byte = [0; 1];
            while !bytes.ends_with(b"\r\n\r\n") {
                connection.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            let head = String::from_utf8(bytes).unwrap();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|n| n.trim().parse().unwrap())
                })
                .unwrap();
            let mut bytes = vec![0; length];
            connection.read_exact(&mut bytes).unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            let native = body["tools"].as_array().is_some_and(|tools| {
                tools.iter().any(|t| {
                    t["type"]
                        .as_str()
                        .is_some_and(|kind| kind.starts_with("web_search"))
                })
            });
            captured
                .lock()
                .unwrap()
                .push((native, body.clone(), head.clone()));
            let (code, mut reply) = if native && status != 200 {
                (
                    status,
                    json!({"error":{"type":"invalid_request_error","message":response_error}}),
                )
            } else if native {
                (
                    200,
                    json!({"id":"resp_native","model":"test-model","status":"completed","output":[{"type":"web_search_call","id":"ws_native","status":"completed","action":{"type":"search","query":"Rust","sources":[{"type":"url","url":"https://www.rust-lang.org/","title":"Rust"}]}},{"type":"message","id":"msg_native","role":"assistant","content":[{"type":"output_text","text":"Native evidence","annotations":[]}]}],"usage":{"input_tokens":10,"output_tokens":5}}),
                )
            } else {
                let tool_result = body["messages"].as_array().unwrap().iter().any(|m| {
                    m["role"] == "tool"
                        || m["content"].as_array().is_some_and(|items| {
                            items.iter().any(|item| item["type"] == "tool_result")
                        })
                });
                let message = if tool_result {
                    json!({"role":"assistant","content":"Local evidence"})
                } else {
                    json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_search","type":"function","function":{"name":"token_station_browser_search","arguments":"{\"query\":\"Rust\"}"}}]})
                };
                (
                    200,
                    json!({"id":"chat_local","model":"test-model","choices":[{"index":0,"message":message,"finish_reason":if tool_result {"stop"} else {"tool_calls"}}],"usage":{"prompt_tokens":10,"completion_tokens":5}}),
                )
            };
            if native_anthropic && code == 200 && (native || !profile) {
                let content = if native {
                    json!([{"type":"server_tool_use","id":"srv_native","name":"web_search","input":{"query":"Rust"}},{"type":"web_search_tool_result","tool_use_id":"srv_native","content":[{"type":"web_search_result","url":"https://www.rust-lang.org/","title":"Rust","encrypted_content":"opaque"}]},{"type":"text","text":"Native evidence"}])
                } else if let Some(calls) = reply["choices"][0]["message"]["tool_calls"].as_array()
                {
                    json!([{"type":"tool_use","id":calls[0]["id"],"name":"token_station_browser_search","input":{"query":"Rust"}}])
                } else {
                    json!([{"type":"text","text":"Local evidence"}])
                };
                reply = json!({"id":"msg_a","type":"message","role":"assistant","model":"test-model","stop_reason":if content[0]["type"] == "tool_use" {"tool_use"} else {"end_turn"},"content":content,"usage":{"input_tokens":10,"output_tokens":5}});
            }
            if native && body["stream"] == true {
                let await_early_text = || {
                    if !cancel && !truncated {
                        let start = Instant::now();
                        while !observed_text.load(Ordering::Acquire)
                            && start.elapsed() < Duration::from_secs(3)
                        {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        assert!(
                            observed_text.load(Ordering::Acquire),
                            "The client must receive text before the upstream terminal event"
                        );
                    }
                };
                if native_anthropic {
                    let _ = write!(
                        connection,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                    );
                    if truncated {
                        let _ = write!(
                            connection,
                            "event: message_start\ndata: {}\n\n",
                            json!({"type":"message_start","message":{"id":"msg_a","type":"message","role":"assistant","model":"test-model","content":[],"usage":{"input_tokens":10,"output_tokens":0}}})
                        );
                    } else {
                        super::web_search::emit_message(&reply, true, &mut |reply| {
                            if let Reply::Chunk(chunk) = reply {
                                let accepted = connection.write_all(chunk.as_bytes()).is_ok();
                                if accepted && chunk.contains("Native evidence") {
                                    await_early_text();
                                }
                                return accepted;
                            }
                            true
                        });
                    }
                    continue;
                }

                let _ = write!(
                    connection,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: response.created\ndata: {}\n\n",
                    json!({"type":"response.created","response":{"id":"resp_native","status":"in_progress"}})
                );
                if !truncated {
                    let _ = write!(
                        connection,
                        "event: response.output_text.delta\ndata: {}\n\n",
                        json!({"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"Native evidence"})
                    );
                    await_early_text();
                    let _ = write!(
                        connection,
                        "event: response.completed\ndata: {}\n\n",
                        json!({"type":"response.completed","response":reply})
                    );
                }
                continue;
            }
            let reply = reply.to_string();
            write!(connection,"HTTP/1.1 {code} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",reply.len()).unwrap();
        }
    });
    let mut config_value = json!({
        "version":1,"server":{"listen":"127.0.0.1:0"},"data":{"dir":root},
        "plugins":{"dir":Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins-dist"),"allow_unsigned":true,"agents":[if anthropic {"agent-anthropic"} else {"agent-openai-responses"}],"providers":{"openai-compatible":"provider-openai-compatible-v2","anthropic":"provider-anthropic-v2"}},
        "upstreams":{"mock":{"provider":if native_anthropic {"anthropic"} else {"openai-compatible"},"api_dialect":if native_anthropic {"anthropic-native"} else {"responses-native"},"base_url":endpoint,"auth":{"slot":"provider_api_key","file":key},"models":[{"model":"test-model","tool":true,"tool_state":"verified","context_window":100_000}]}},
        "router":{"version":1,"pools":{"main":[{"upstream":"mock","model":"test-model"}]},"default_pool":"main"}
    });
    if profile {
        config_value["upstreams"]["mock"]["provider"] = json!("openai-compatible");
        config_value["upstreams"]["mock"]["api_dialect"] = json!("translated");
        config_value["upstreams"]["mock"]["native_search"] = json!({"api_dialect":if native_anthropic {"anthropic-native"} else {"responses-native"},"base_url":format!("{endpoint}/search"),"auth":"bearer"});
    }
    let config: ClientConfig = serde_json::from_value(config_value).unwrap();
    let recorder = Arc::new(Records::default());
    let gateway = Gateway::new(&config, recorder.clone()).unwrap();
    let settings =
        serde_json::from_value(json!({"enabled":mode != "disabled","engine":"bing","mode":if mode == "disabled" {"native"} else {mode}})).unwrap();
    gateway.search.save(settings).unwrap();
    *gateway.search.fixture.lock().unwrap() = Some(Ok(vec![crate::search::SearchResult {
        title: "Rust".into(),
        url: "https://www.rust-lang.org/".into(),
        snippet: "Browser evidence".into(),
    }]));
    let mut body = if anthropic {
        json!({"model":"auto","max_tokens":100,"messages":[{"role":"user","content":"Find Rust"}],"tools":[{"type":"web_search_20250305","name":"web_search"}],"tool_choice":{"type":"tool","name":"web_search"}})
    } else {
        json!({"model":"auto","input":"Find Rust","tools":[{"type":"web_search"}],"tool_choice":"required"})
    };
    body["stream"] = json!(stream);
    if constrained {
        body["tools"][0]["filters"] = json!({"allowed_domains":["rust-lang.org"]});
    }
    let mut answers = Vec::new();
    let mut chunks = String::new();
    for index in 0..requests {
        if index == 2 {
            gateway.search.save(gateway.search.settings()).unwrap();
        }
        gateway.chat(
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
                    Reply::BeginJson(reply) => answers.push(reply),
                    Reply::Chunk(chunk) => {
                        if chunk.contains("Native evidence") {
                            early_text.store(true, Ordering::Release);
                        }
                        chunks.push_str(&chunk);
                    }
                    Reply::BeginStream if cancel => return false,
                    Reply::BeginStream => {}
                }
                true
            },
        );
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
    let local = mode == "local"
        || mode == "auto"
            && status == 400
            && (error == "Tool type 'web_search' is not supported"
                || error.contains("only client functions"));
    let seen = seen.lock().unwrap();
    if profile {
        for (native, _, head) in seen.iter() {
            let expected = if *native {
                if native_anthropic {
                    "/v1/search/messages"
                } else {
                    "/v1/search/responses"
                }
            } else {
                "/v1/chat/completions"
            };
            assert!(head.starts_with(&format!("POST {expected} ")), "{head}");
            assert!(
                head.to_ascii_lowercase()
                    .contains("authorization: bearer test-key")
            );
        }
    }
    let records = recorder.0.lock().unwrap();
    assert_eq!(records.len(), requests);
    if stream {
        assert!(records[0].browser_searches.is_empty());
        assert_eq!(records[0].attempts, 1);
        if !truncated && !cancel {
            assert!(
                chunks.contains(if anthropic {
                    "message_stop"
                } else {
                    "response.completed"
                }),
                "{chunks}"
            );
        }
        if cancel {
            assert_eq!(records[0].status, 499);
        }
        if truncated {
            assert!(records[0].error_code.is_some());
            assert!(!chunks.contains("event: response.completed"));
            assert!(!chunks.contains("event: message_stop"));
        }
    } else {
        assert_eq!(answers.len(), requests);
    }
    for (index, answer) in answers.iter().enumerate() {
        if stream {
            continue;
        }
        if local {
            assert_eq!(answer.status, 200, "{}", answer.body);
            assert!(answer.body.contains("Browser evidence"), "{}", answer.body);
            assert_eq!(records[index].browser_searches.len(), 1);
            assert_eq!(records[index].usage.unwrap().input_tokens, 20);
            assert_eq!(records[index].usage.unwrap().output_tokens, 10);
            let execution = if mode == "local" {
                token_station_metrics::SearchExecution::Local
            } else if index == 1 {
                token_station_metrics::SearchExecution::LocalCachedFallback
            } else {
                token_station_metrics::SearchExecution::LocalRejectionFallback
            };
            assert_eq!(records[index].search_execution, Some(execution));
            assert_eq!(
                records[index].attempts,
                if mode == "local" || index == 1 { 2 } else { 3 }
            );
            assert!(
                records[index]
                    .attempt_records
                    .iter()
                    .enumerate()
                    .all(|(i, attempt)| attempt.ordinal == u32::try_from(i + 1).unwrap())
            );
        } else {
            assert_eq!(answer.status, status, "{}", answer.body);
            assert!(records[index].browser_searches.is_empty());
            if status == 200 {
                assert!(answer.body.contains("Native evidence"));
            }
        }
    }
    let native_count = seen.iter().filter(|(native, _, _)| *native).count();
    assert_eq!(
        native_count,
        if mode == "local" {
            0
        } else if requests == 3 {
            2
        } else {
            requests
        }
    );
    if !local {
        assert_eq!(seen.len(), requests);
    }
    assert!(
        seen.iter()
            .all(|(_, body, _)| body["model"] == "test-model")
    );
    drop(seen);
    drop(records);
    std::fs::remove_dir_all(root).unwrap();
}

/// Exercise an existing credential without changing persistent routes or search settings.
#[test]
#[ignore = "Requires an explicitly selected live configuration, upstream, and model"]
#[allow(clippy::too_many_lines)] // Keep the opt-in live setup and evidence checks in one scenario.
fn live_configured_native_search_bridge() {
    let path = std::env::var("TOKEN_STATION_SEARCH_LIVE_CONFIG").unwrap();
    let upstream = std::env::var("TOKEN_STATION_SEARCH_LIVE_UPSTREAM").unwrap();
    let model = std::env::var("TOKEN_STATION_SEARCH_LIVE_MODEL").unwrap();
    let anthropic = std::env::var("TOKEN_STATION_SEARCH_LIVE_CLIENT").unwrap() == "anthropic";
    let mut config = ClientConfig::load(Path::new(&path)).unwrap();
    config.router = serde_json::from_value(json!({"version":1,"pools":{"live":[{"upstream":upstream,"model":model}]},"default_pool":"live"})).unwrap();
    config.routing = None;
    config.agent_routes.clear();
    let recorder = Arc::new(Records::default());
    let mut gateway = Gateway::new(&config, recorder.clone()).unwrap();
    let root = std::env::temp_dir().join(format!("ts-native-live-{}", std::process::id()));
    gateway.search = crate::search::SearchController::shared(&root);
    gateway
        .search
        .save(
            serde_json::from_value(json!({"enabled":true,"engine":"bing","mode":"native"}))
                .unwrap(),
        )
        .unwrap();
    let mut body = if anthropic {
        json!({"model":"auto","max_tokens":1000,"messages":[{"role":"user","content":"Search the web for Python official documentation. Give one brief sentence and cite the official documentation URL."}],"tools":[{"type":"web_search_20250305","name":"web_search","max_uses":1}],"tool_choice":{"type":"tool","name":"web_search"}})
    } else {
        json!({"model":"auto","max_output_tokens":1000,"reasoning":{"effort":"none"},"input":"Search the web for Python official documentation. Give one brief sentence and cite the official documentation URL.","tools":[{"type":"web_search"}],"tool_choice":"required","max_tool_calls":1})
    };
    body["stream"] = json!(true);
    let mut wire = String::new();
    let mut http_status = None;
    let mut error_message = Value::Null;
    let mut json_shape = Value::Null;
    let started = std::time::Instant::now();
    let mut first_delta_ms = None;
    let ctx = RequestContext::detached(
        std::time::Duration::from_secs(90),
        std::time::Duration::from_secs(90),
    );
    gateway.chat_scoped_without_body_log(
        &ctx,
        None,
        None,
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
                Reply::Chunk(chunk) => {
                    if first_delta_ms.is_none()
                        && (chunk.contains("text_delta")
                            || chunk.contains("response.output_text.delta"))
                    {
                        first_delta_ms = Some(started.elapsed().as_millis());
                    }
                    wire.push_str(&chunk);
                }
                Reply::BeginJson(reply) => {
                    http_status = Some(reply.status);
                    let error = serde_json::from_str::<Value>(&reply.body).unwrap_or(Value::Null);
                    json_shape = json!({"bytes":reply.body.len(),"keys":error.as_object().map(|object| object.keys().collect::<Vec<_>>()),"code":error.get("code"),"error_type":error["error"].as_str()});
                    error_message = error
                        .get("message")
                        .or_else(|| error["error"].get("message"))
                        .cloned()
                        .unwrap_or(Value::Null);
                }
                Reply::BeginStream => {}
            }
            true
        },
    );
    let mut completed_search_calls = 0;
    let mut url_annotations = 0;
    for line in wire.lines().filter_map(|line| line.strip_prefix("data: ")) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if event["type"] == "error" {
            error_message = event
                .get("message")
                .or_else(|| event["error"].get("message"))
                .cloned()
                .unwrap_or(Value::Null);
        }
        if event["type"] == "response.output_item.done"
            && event["item"]["type"] == "web_search_call"
            && event["item"]["status"] == "completed"
            && event["item"]["action"]["sources"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        {
            completed_search_calls += 1;
        }
        if event["type"] == "content_block_start"
            && event["content_block"]["type"] == "web_search_tool_result"
            && event["content_block"]["content"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        {
            completed_search_calls += 1;
        }
        if event["type"] == "response.output_text.annotation.added"
            && event["annotation"]["type"] == "url_citation"
        {
            url_annotations += 1;
        }
    }
    let records = recorder.0.lock().unwrap();
    let record = records.last().unwrap();
    let completed = wire.contains(if anthropic {
        "event: message_stop"
    } else {
        "event: response.completed"
    });
    let evidence = wire.contains("https://docs.python.org");
    println!(
        "{}",
        json!({"upstream":upstream,"model":model,"client":if anthropic {"anthropic"} else {"responses"},"status":record.status,"json_status":http_status,"error_message":error_message,"json_shape":json_shape,"error_code":record.error_code,"attempts":record.attempts,"browser_operations":record.browser_searches.len(),"completed":completed,"official_source_present":evidence,"completed_search_calls":completed_search_calls,"url_annotations":url_annotations,"first_delta_ms":first_delta_ms,"elapsed_ms":started.elapsed().as_millis(),"wire_bytes":wire.len()})
    );
    drop(records);
    drop(gateway);
    std::fs::remove_dir_all(root).unwrap();
    assert!(
        completed && evidence && completed_search_calls > 0 && first_delta_ms.is_some(),
        "Live native search did not complete with the expected source."
    );
}
