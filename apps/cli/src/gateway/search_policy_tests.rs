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
                let _ = write!(
                    connection,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\nevent: response.created\ndata: {}\n\n",
                    json!({"type":"response.created","response":{"id":"resp_native","status":"in_progress"}})
                );
                if !truncated {
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
        serde_json::from_value(json!({"enabled":true,"engine":"bing","mode":mode})).unwrap();
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
                    Reply::Chunk(chunk) => chunks.push_str(&chunk),
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
            && !constrained
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
            assert!(chunks.contains("response.completed"));
        }
        if cancel {
            assert_eq!(records[0].status, 499);
        }
        if truncated {
            assert!(records[0].error_code.is_some());
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
