//! A2's exit criterion, executed: the official OpenAI adapters, compiled to
//! real `wasm32-wasip2` components, loaded through every gate, and held to the
//! full conformance suite — the same suite, through the same trait, that judges
//! native adapters.
//!
//! This is also the whole install pipeline in miniature: manifest gate, import
//! scan, identity gate, then the fixture suite. A package that failed any of
//! these would be a draft in the registry, not a loadable plugin.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::json;
use token_station_conformance::{
    AgentAdapter, AgentFamily, FixturePack, ProviderFamily, run_agent_suite, run_provider_suite,
};
use token_station_plugin_runtime::{
    AgentPlugin, LoadError, NoSecrets, PluginRuntime, ProviderPlugin, RuntimeLimits,
};
use token_station_protocol::{
    AgentRequestEnvelope, ErrorCode, ErrorEnvelope, Extensions, FinishReason, HeaderDigest,
    Principal, StreamEvent,
};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/plugin-runtime sits two levels below the root")
}

/// Builds one official plugin and assembles its package directory next to a
/// copy of its real manifest.
fn build_package(plugin: &str) -> PathBuf {
    let source_dir = repo_root().join("plugins/official").join(plugin);

    let status = Command::new("cargo")
        .args(["build", "--target", "wasm32-wasip2"])
        .current_dir(&source_dir)
        .status()
        .expect("cargo is on PATH");
    assert!(
        status.success(),
        "{plugin} must build; run `rustup target add wasm32-wasip2` if the target is missing"
    );

    let wasm = source_dir
        .join("target/wasm32-wasip2/debug")
        .join(format!("{}.wasm", plugin.replace('-', "_")));

    let package = std::env::temp_dir().join(format!("ts-official-{}-{plugin}", std::process::id()));
    std::fs::create_dir_all(&package).expect("temp dir is writable");
    std::fs::copy(
        source_dir.join("manifest.json"),
        package.join("manifest.json"),
    )
    .expect("manifest copies");
    std::fs::copy(&wasm, package.join("adapter.wasm")).expect("wasm copies");
    package
}

fn provider_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| build_package("provider-openai-compatible"))
}

fn agent_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| build_package("agent-openai"))
}

fn anthropic_agent_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| build_package("agent-anthropic"))
}

fn responses_agent_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| build_package("agent-openai-responses"))
}

fn gemini_agent_package() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| build_package("agent-gemini"))
}

fn runtime() -> PluginRuntime {
    PluginRuntime::new(RuntimeLimits {
        memory_bytes: 64 * 1024 * 1024,
        // Generous: the incrementality check replays the stream fixture at
        // every byte boundary, and CI machines are slow.
        call_timeout: Duration::from_secs(5),
    })
    .expect("engine builds")
}

fn envelope(protocol: &str, body: serde_json::Value) -> AgentRequestEnvelope {
    AgentRequestEnvelope {
        protocol: protocol.to_owned(),
        agent_tool: None,
        headers: HeaderDigest::default(),
        principal: Principal {
            subject: "local".to_owned(),
            tenant: None,
        },
        hints: Vec::new(),
        body,
        extensions: Extensions::new(),
    }
}

fn response_sse_events(rendered: &serde_json::Value) -> Vec<serde_json::Value> {
    rendered["data"]
        .as_str()
        .expect("rendered Responses data is a string")
        .split("\n\n")
        .filter_map(|frame| {
            frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .map(|data| serde_json::from_str(data).expect("Responses SSE data is JSON"))
        })
        .collect()
}

#[test]
fn the_official_provider_adapter_passes_the_full_suite_as_wasm() {
    let plugin =
        ProviderPlugin::load(&runtime(), provider_package(), NoSecrets).expect("loads clean");

    let fixtures = repo_root().join("plugins/official/provider-openai-compatible/fixtures");
    let pack: FixturePack<ProviderFamily> =
        FixturePack::load(&fixtures).expect("the shipped pack loads");

    let report = run_provider_suite(&plugin, &pack);

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), plugin.manifest().conformance.required_suite);
}

#[test]
fn the_official_agent_adapter_passes_the_full_suite_as_wasm() {
    let plugin = AgentPlugin::load(&runtime(), agent_package()).expect("loads clean");

    let fixtures = repo_root().join("plugins/official/agent-openai/fixtures");
    let pack: FixturePack<AgentFamily> =
        FixturePack::load(&fixtures).expect("the shipped pack loads");

    let report = run_agent_suite(&plugin, &pack);

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), plugin.manifest().conformance.required_suite);
}

#[test]
fn the_official_responses_agent_adapter_passes_the_full_suite_as_wasm() {
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");

    let fixtures = repo_root().join("plugins/official/agent-openai-responses/fixtures");
    let pack: FixturePack<AgentFamily> =
        FixturePack::load(&fixtures).expect("the shipped pack loads");

    let report = run_agent_suite(&plugin, &pack);

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), plugin.manifest().conformance.required_suite);
}

#[test]
fn the_official_anthropic_agent_adapter_passes_the_full_suite_as_wasm() {
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");

    let fixtures = repo_root().join("plugins/official/agent-anthropic/fixtures");
    let pack: FixturePack<AgentFamily> =
        FixturePack::load(&fixtures).expect("the shipped pack loads");

    let report = run_agent_suite(&plugin, &pack);

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), plugin.manifest().conformance.required_suite);
}

#[test]
fn the_official_gemini_agent_adapter_passes_the_full_suite_as_wasm() {
    let plugin = AgentPlugin::load(&runtime(), gemini_agent_package()).expect("loads clean");
    let fixtures = repo_root().join("plugins/official/agent-gemini/fixtures");
    let pack: FixturePack<AgentFamily> =
        FixturePack::load(&fixtures).expect("the shipped pack loads");

    let report = run_agent_suite(&plugin, &pack);

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), plugin.manifest().conformance.required_suite);
}

#[test]
fn gemini_model_and_stream_mode_come_from_the_transport_path() {
    let plugin = AgentPlugin::load(&runtime(), gemini_agent_package()).expect("loads clean");
    let mut request = envelope(
        "google-gemini-generate-content",
        json!({"contents": [{"role": "user", "parts": [{"text": "hello"}]}]}),
    );
    request.extensions.insert(
        "transport_path".to_owned(),
        json!("/agents/gemini-cli/v1beta/models/gemini-2.5-pro:streamGenerateContent?alt=sse"),
    );

    let normalized = plugin.normalize_inbound(&request).expect("normalizes");

    assert_eq!(normalized.model, "gemini-2.5-pro");
    assert!(normalized.stream);
}

#[test]
fn responses_structured_output_is_rejected_by_the_real_wasm() {
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");
    let request = |format: serde_json::Value| {
        envelope(
            "openai-responses",
            serde_json::json!({
                "model": "auto",
                "input": "return one value",
                "text": {"format": format}
            }),
        )
    };

    let plain = plugin
        .normalize_inbound(&request(serde_json::json!({"type": "text"})))
        .expect("plain text remains supported");
    assert_eq!(plain.response_format, None);

    for format in [
        serde_json::json!({"type": "json_schema", "name": "answer", "schema": {"type": "object"}}),
        serde_json::json!({"type": "json_object"}),
        serde_json::json!({"type": "future_structured_format"}),
    ] {
        let error = plugin
            .normalize_inbound(&request(format))
            .expect_err("structured output must not be downgraded to text");
        assert_eq!(error.code, ErrorCode::Capability);
        assert_eq!(error.http_status, 400);
    }

    for format in [serde_json::json!({}), serde_json::json!("json_schema")] {
        let invalid = plugin
            .normalize_inbound(&request(format))
            .expect_err("a malformed format is refused");
        assert_eq!(invalid.code, ErrorCode::InvalidRequest);
    }
}

#[test]
fn chat_completions_structured_output_is_rejected_by_the_real_wasm() {
    let plugin = AgentPlugin::load(&runtime(), agent_package()).expect("loads clean");
    let request = |format: serde_json::Value| {
        envelope(
            "openai-chat-completions",
            serde_json::json!({
                "model": "auto",
                "messages": [{"role": "user", "content": "return one value"}],
                "response_format": format
            }),
        )
    };

    let plain = plugin
        .normalize_inbound(&request(serde_json::json!({"type": "text"})))
        .expect("plain text remains supported");
    assert_eq!(plain.response_format, None);

    for format in [
        serde_json::json!({"type": "json_schema", "json_schema": {"name": "answer", "schema": {"type": "object"}}}),
        serde_json::json!({"type": "json_object"}),
        serde_json::json!({"type": "future_structured_format"}),
    ] {
        let error = plugin
            .normalize_inbound(&request(format))
            .expect_err("structured output must not be downgraded to text");
        assert_eq!(error.code, ErrorCode::Capability);
        assert_eq!(error.http_status, 400);
    }

    for format in [serde_json::json!({}), serde_json::json!("json_schema")] {
        let invalid = plugin
            .normalize_inbound(&request(format))
            .expect_err("a malformed format is refused");
        assert_eq!(invalid.code, ErrorCode::InvalidRequest);
    }
}

#[test]
fn anthropic_stream_state_is_isolated_and_cleaned_by_stream_id() {
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");
    let context = |stream_id: &str, response_id: &str| {
        serde_json::json!({
            "protocol": "anthropic-messages",
            "stream_id": stream_id,
            "response_id": response_id,
            "model": "routed-model"
        })
    };
    let delta = |content: &str| StreamEvent::Delta {
        index: 0,
        content: content.to_owned(),
    };

    let a_first = plugin
        .render_stream_event(&delta("a1"), &context("stream-a", "msg-a"))
        .expect("stream A starts");
    let b_first = plugin
        .render_stream_event(&delta("b1"), &context("stream-b", "msg-b"))
        .expect("stream B starts");
    let a_second = plugin
        .render_stream_event(&delta("a2"), &context("stream-a", "msg-a"))
        .expect("stream A resumes");

    assert!(a_first["data"].as_str().unwrap().contains("message_start"));
    assert!(b_first["data"].as_str().unwrap().contains("message_start"));
    assert!(!a_second["data"].as_str().unwrap().contains("message_start"));
    assert!(a_second["data"].as_str().unwrap().contains("a2"));

    plugin
        .render_stream_event(
            &StreamEvent::Done {
                finish_reason: Some(FinishReason::Stop),
                stop_sequence: None,
            },
            &context("stream-a", "msg-a"),
        )
        .expect("done cleans stream A");
    plugin
        .render_stream_event(
            &StreamEvent::Error {
                error: ErrorEnvelope::new(ErrorCode::UpstreamUnavailable, 502, "unavailable"),
            },
            &context("stream-b", "msg-b"),
        )
        .expect("error cleans stream B");

    let a_restarted = plugin
        .render_stream_event(&delta("a3"), &context("stream-a", "msg-a-2"))
        .expect("stream A can restart after done");
    let b_restarted = plugin
        .render_stream_event(&delta("b2"), &context("stream-b", "msg-b-2"))
        .expect("stream B can restart after error");
    assert!(
        a_restarted["data"]
            .as_str()
            .unwrap()
            .contains("message_start")
    );
    assert!(
        b_restarted["data"]
            .as_str()
            .unwrap()
            .contains("message_start")
    );
}

#[test]
fn responses_stream_state_is_isolated_and_cleaned_by_stream_id() {
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");
    let context = |stream_id: &str, response_id: &str| {
        serde_json::json!({
            "protocol": "openai-responses",
            "stream_id": stream_id,
            "response_id": response_id,
            "model": "routed-model"
        })
    };
    let delta = |content: &str| StreamEvent::Delta {
        index: 0,
        content: content.to_owned(),
    };

    let a_first = plugin
        .render_stream_event(&delta("a1"), &context("stream-a", "resp-a"))
        .expect("stream A starts");
    let b_first = plugin
        .render_stream_event(&delta("b1"), &context("stream-b", "resp-b"))
        .expect("stream B starts");
    let a_second = plugin
        .render_stream_event(&delta("a2"), &context("stream-a", "resp-a"))
        .expect("stream A resumes");

    assert!(
        a_first["data"]
            .as_str()
            .unwrap()
            .contains("response.created")
    );
    assert!(
        b_first["data"]
            .as_str()
            .unwrap()
            .contains("response.created")
    );
    assert!(
        !a_second["data"]
            .as_str()
            .unwrap()
            .contains("response.created")
    );

    let a_done = plugin
        .render_stream_event(
            &StreamEvent::Done {
                finish_reason: Some(FinishReason::Stop),
                stop_sequence: None,
            },
            &context("stream-a", "resp-a"),
        )
        .expect("done cleans stream A");
    assert!(a_done["data"].as_str().unwrap().contains("a1a2"));

    plugin
        .render_stream_event(
            &StreamEvent::Error {
                error: ErrorEnvelope::new(ErrorCode::UpstreamUnavailable, 502, "unavailable"),
            },
            &context("stream-b", "resp-b"),
        )
        .expect("error cleans stream B");

    let a_restarted = plugin
        .render_stream_event(&delta("a3"), &context("stream-a", "resp-a-2"))
        .expect("stream A restarts");
    let b_restarted = plugin
        .render_stream_event(&delta("b2"), &context("stream-b", "resp-b-2"))
        .expect("stream B restarts");
    assert!(
        a_restarted["data"]
            .as_str()
            .unwrap()
            .contains("response.created")
    );
    assert!(
        b_restarted["data"]
            .as_str()
            .unwrap()
            .contains("response.created")
    );
}

#[test]
fn responses_stream_assigns_global_output_indices_by_first_appearance() {
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");
    let context = |stream_id: &str| {
        serde_json::json!({
            "protocol": "openai-responses",
            "stream_id": stream_id,
            "response_id": format!("resp-{stream_id}"),
            "model": "routed-model"
        })
    };
    let text = |content: &str| StreamEvent::Delta {
        index: 0,
        content: content.to_owned(),
    };
    let tool =
        |index, id: Option<&str>, name: Option<&str>, arguments: &str| StreamEvent::ToolCallDelta {
            index,
            id: id.map(str::to_owned),
            name: name.map(str::to_owned),
            arguments_delta: arguments.to_owned(),
        };

    let text_first = response_sse_events(
        &plugin
            .render_stream_event(&text("before "), &context("text-first"))
            .expect("text starts"),
    );
    assert_eq!(text_first[1]["output_index"], serde_json::json!(0));
    assert_eq!(text_first[2]["output_index"], serde_json::json!(0));
    assert_eq!(text_first[3]["output_index"], serde_json::json!(0));

    let first_tool = response_sse_events(
        &plugin
            .render_stream_event(
                &tool(0, Some("call-0"), Some("read_zero"), "{"),
                &context("text-first"),
            )
            .expect("first tool starts"),
    );
    assert_eq!(first_tool[0]["output_index"], serde_json::json!(1));
    assert_eq!(first_tool[1]["output_index"], serde_json::json!(1));

    let resumed_text = response_sse_events(
        &plugin
            .render_stream_event(&text("after"), &context("text-first"))
            .expect("text resumes"),
    );
    assert_eq!(resumed_text[0]["output_index"], serde_json::json!(0));

    let second_tool = response_sse_events(
        &plugin
            .render_stream_event(
                &tool(1, Some("call-1"), Some("read_one"), "{}"),
                &context("text-first"),
            )
            .expect("second tool starts"),
    );
    assert_eq!(second_tool[0]["output_index"], serde_json::json!(2));
    assert_eq!(second_tool[1]["output_index"], serde_json::json!(2));

    let continued_tool = response_sse_events(
        &plugin
            .render_stream_event(&tool(0, None, None, "}"), &context("text-first"))
            .expect("first tool continues"),
    );
    assert_eq!(continued_tool[0]["output_index"], serde_json::json!(1));

    let completed = response_sse_events(
        &plugin
            .render_stream_event(
                &StreamEvent::Done {
                    finish_reason: Some(FinishReason::ToolCalls),
                    stop_sequence: None,
                },
                &context("text-first"),
            )
            .expect("stream completes"),
    );
    let done_indices: Vec<_> = completed
        .iter()
        .filter(|event| event["type"] == "response.output_item.done")
        .map(|event| event["output_index"].as_u64().unwrap())
        .collect();
    assert_eq!(done_indices, vec![0, 1, 2]);
    let final_output = completed
        .iter()
        .find(|event| event["type"] == "response.completed")
        .expect("completed event")["response"]["output"]
        .as_array()
        .expect("final output is an array");
    let final_types: Vec<_> = final_output
        .iter()
        .map(|item| item["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        final_types,
        vec!["message", "function_call", "function_call"]
    );
}

#[test]
fn responses_stream_keeps_tool_first_output_order() {
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");
    let context = serde_json::json!({
        "protocol": "openai-responses",
        "stream_id": "tool-first",
        "response_id": "resp-tool-first",
        "model": "routed-model"
    });
    let tool_first = response_sse_events(
        &plugin
            .render_stream_event(
                &StreamEvent::ToolCallDelta {
                    index: 0,
                    id: Some("call-first".to_owned()),
                    name: Some("read_first".to_owned()),
                    arguments_delta: "{}".to_owned(),
                },
                &context,
            )
            .expect("tool-first stream starts"),
    );
    assert_eq!(tool_first[1]["output_index"], serde_json::json!(0));
    let later_text = response_sse_events(
        &plugin
            .render_stream_event(
                &StreamEvent::Delta {
                    index: 0,
                    content: "later".to_owned(),
                },
                &context,
            )
            .expect("text follows tool"),
    );
    assert_eq!(later_text[0]["output_index"], serde_json::json!(1));

    let tool_first_completed = response_sse_events(
        &plugin
            .render_stream_event(
                &StreamEvent::Done {
                    finish_reason: Some(FinishReason::ToolCalls),
                    stop_sequence: None,
                },
                &context,
            )
            .expect("tool-first stream completes"),
    );
    let tool_first_done_indices: Vec<_> = tool_first_completed
        .iter()
        .filter(|event| event["type"] == "response.output_item.done")
        .map(|event| event["output_index"].as_u64().unwrap())
        .collect();
    assert_eq!(tool_first_done_indices, vec![0, 1]);
    let tool_first_output = tool_first_completed
        .iter()
        .find(|event| event["type"] == "response.completed")
        .expect("tool-first completed event")["response"]["output"]
        .as_array()
        .expect("tool-first output is an array");
    assert_eq!(tool_first_output[0]["type"], "function_call");
    assert_eq!(tool_first_output[1]["type"], "message");
}

#[test]
fn anthropic_match_inbound_owns_only_post_messages() {
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");
    let headers = HeaderDigest::redacting([
        ("authorization", "Bearer local-secret"),
        ("anthropic-version", "2023-06-01"),
    ]);

    let matched = plugin
        .match_inbound(&json!({
            "method": "POST",
            "path": "/v1/messages?beta=true",
            "headers": headers
        }))
        .expect("match call succeeds");
    assert!(matched.matched);
    assert_eq!(matched.protocol.as_deref(), Some("anthropic-messages"));

    for (method, path) in [
        ("GET", "/v1/messages"),
        ("POST", "/v1/messages/count_tokens"),
        ("POST", "/v1/chat/completions"),
    ] {
        let result = plugin
            .match_inbound(&json!({ "method": method, "path": path, "headers": headers }))
            .expect("match call succeeds");
        assert!(!result.matched, "{method} {path}");
        assert_eq!(result.protocol, None, "{method} {path}");
    }
}

#[test]
fn responses_match_inbound_owns_only_post_responses() {
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");
    let headers = HeaderDigest::redacting([("authorization", "Bearer local-secret")]);

    let matched = plugin
        .match_inbound(&json!({
            "method": "POST",
            "path": "/v1/responses?include=usage",
            "headers": headers
        }))
        .expect("match call succeeds");
    assert!(matched.matched);
    assert_eq!(matched.protocol.as_deref(), Some("openai-responses"));

    for (method, path) in [
        ("GET", "/v1/responses"),
        ("POST", "/v1/chat/completions"),
        ("POST", "/v1/responses/input_tokens"),
    ] {
        let result = plugin
            .match_inbound(&json!({ "method": method, "path": path, "headers": headers }))
            .expect("match call succeeds");
        assert!(!result.matched, "{method} {path}");
        assert_eq!(result.protocol, None, "{method} {path}");
    }
}

#[test]
fn the_two_kinds_do_not_load_through_each_other() {
    // An agent package through the provider loader and vice versa: refused at
    // the kind check, before any wasm is instantiated.
    let wrong = ProviderPlugin::load(&runtime(), agent_package(), NoSecrets);
    assert!(matches!(wrong, Err(LoadError::WrongKind(_))), "{wrong:?}");

    let wrong = AgentPlugin::load(&runtime(), provider_package());
    assert!(matches!(wrong, Err(LoadError::WrongKind(_))), "{wrong:?}");
}

#[test]
fn an_agent_component_that_asks_to_name_credentials_is_refused() {
    // The provider world may import `token-station:adapter/host`; the agent
    // world may not — an agent adapter cannot even *name* a credential. The
    // WIT declares that; this proves the loader enforces it on a compiled
    // artifact that ignored the declaration.
    let wat = r#"(component
        (import "token-station:adapter/host@1.0.0" (instance))
    )"#;
    let package =
        std::env::temp_dir().join(format!("ts-official-{}-host-grab", std::process::id()));
    std::fs::create_dir_all(&package).expect("temp dir is writable");
    std::fs::copy(
        repo_root().join("plugins/official/agent-openai/manifest.json"),
        package.join("manifest.json"),
    )
    .expect("manifest copies");
    std::fs::write(package.join("adapter.wasm"), wat).expect("wat writes");

    let refused = AgentPlugin::load(&runtime(), &package);

    match refused {
        Err(LoadError::ForbiddenImport(name)) => {
            assert!(name.starts_with("token-station:adapter/host"), "{name}");
        }
        other => panic!("expected a forbidden import, got {other:?}"),
    }
}
