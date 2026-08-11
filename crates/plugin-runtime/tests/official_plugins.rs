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
    AgentRequestEnvelope, ChatResponse, ErrorCode, ErrorEnvelope, Extensions, FinishReason,
    HeaderDigest, Principal, ResponseFormat, StreamEvent, ToolChoice,
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
fn gemini_hosted_tools_fail_closed_and_name_the_tool() {
    // Google Search grounding, URL Context and Code Execution run on Google's
    // servers and have no Canonical IR shape. They must fail closed with a
    // Capability error that names the tool, while plain functionDeclarations
    // still normalize.
    let plugin = AgentPlugin::load(&runtime(), gemini_agent_package()).expect("loads clean");
    let request = |tools: serde_json::Value| {
        let mut request = envelope(
            "google-gemini-generate-content",
            json!({
                "contents": [{"role": "user", "parts": [{"text": "hello"}]}],
                "tools": tools
            }),
        );
        request.extensions.insert(
            "transport_path".to_owned(),
            json!("/agents/gemini-cli/v1beta/models/gemini-2.5-pro:generateContent"),
        );
        request
    };

    let ok = plugin
        .normalize_inbound(&request(json!([
            {"functionDeclarations": [{"name": "lookup", "parameters": {"type": "object"}}]}
        ])))
        .expect("functionDeclarations remain supported");
    assert_eq!(ok.tools.len(), 1);
    assert_eq!(ok.tools[0].name, "lookup");

    for tools in [
        json!([{"google_search": {}}]),
        json!([{"url_context": {}}]),
        json!([{"code_execution": {}}]),
    ] {
        let error = plugin
            .normalize_inbound(&request(tools))
            .expect_err("hosted tools have no IR representation");
        assert_eq!(error.code, ErrorCode::Capability);
        assert_eq!(error.http_status, 400);
    }
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
fn responses_structured_output_is_typed_by_the_real_wasm() {
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
    assert_eq!(plain.response_format, Some(ResponseFormat::Text));

    let json_object = plugin
        .normalize_inbound(&request(serde_json::json!({"type": "json_object"})))
        .expect("JSON object output remains typed");
    assert_eq!(
        json_object.response_format,
        Some(ResponseFormat::JsonObject)
    );

    let json_schema = plugin
        .normalize_inbound(&request(serde_json::json!({
            "type": "json_schema",
            "name": "answer",
            "strict": true,
            "schema": {"type": "object"}
        })))
        .expect("JSON Schema output remains typed");
    assert_eq!(
        json_schema.response_format,
        Some(ResponseFormat::JsonSchema {
            json_schema: serde_json::json!({
                "name": "answer",
                "strict": true,
                "schema": {"type": "object"}
            })
        })
    );

    let unsupported = plugin
        .normalize_inbound(&request(
            serde_json::json!({"type": "future_structured_format"}),
        ))
        .expect_err("unknown structured output must not be downgraded to text");
    assert_eq!(unsupported.code, ErrorCode::Capability);
    assert_eq!(unsupported.http_status, 400);

    for format in [serde_json::json!({}), serde_json::json!("json_schema")] {
        let invalid = plugin
            .normalize_inbound(&request(format))
            .expect_err("a malformed format is refused");
        assert_eq!(invalid.code, ErrorCode::InvalidRequest);
    }
}

#[test]
fn responses_builtin_tools_are_translated_to_functions() {
    // Codex pairs function tools with built-ins (local_shell/custom/namespace/
    // tool_search) and hosted tools. Rather than drop them (a shrunken tool set)
    // or fail the whole request, we translate every tool to a function the model
    // can call — matching CC Switch — so Codex is usable on any provider.
    // Namespace children are lifted to <namespace>__<child>; a collision is a
    // hard error, never a silent overwrite.
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");
    let request = |tools: serde_json::Value| {
        envelope(
            "openai-responses",
            serde_json::json!({
                "model": "auto",
                "input": "read the marker",
                "tools": tools
            }),
        )
    };
    let names = |req: &token_station_protocol::ChatRequest| {
        req.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>()
    };

    // A plain function tool survives untouched.
    let ok = plugin
        .normalize_inbound(&request(serde_json::json!([
            {"type": "function", "name": "read_marker", "parameters": {"type": "object"}}
        ])))
        .expect("function tools remain supported");
    assert_eq!(names(&ok), vec!["read_marker"]);

    // Mixed request: function + namespace(child) + local_shell all reach the
    // model as function tools; the namespace child is flattened.
    let mixed = plugin
        .normalize_inbound(&request(serde_json::json!([
            {"type": "function", "name": "read_marker", "parameters": {"type": "object"}},
            {"type": "namespace", "name": "container", "tools": [
                {"type": "function", "name": "nested", "parameters": {}}
            ]},
            {"type": "local_shell"}
        ])))
        .expect("built-in tools are translated, not dropped or refused");
    assert_eq!(
        names(&mixed),
        vec![
            "read_marker",
            "container__nested",
            "__token_station_responses_local_shell",
        ]
    );

    // Codex client tools remain callable through the translated provider.
    for (tools, expected) in [
        (
            serde_json::json!([{"type": "custom", "name": "grep"}]),
            "grep",
        ),
        (serde_json::json!([{"type": "tool_search"}]), "tool_search"),
    ] {
        let req = plugin
            .normalize_inbound(&request(tools))
            .expect("client-executed tools are translated");
        assert_eq!(names(&req), vec![expected]);
    }

    for hosted in [
        "web_search",
        "file_search",
        "code_interpreter",
        "image_generation",
        "computer_use_preview",
        "mcp",
    ] {
        let error = plugin
            .normalize_inbound(&request(serde_json::json!([{"type": hosted}])))
            .expect_err("provider-hosted tools require native execution semantics");
        assert_eq!(error.code, ErrorCode::Capability);
        assert!(error.message.contains(hosted), "{}", error.message);
    }

    let disabled_web_search = plugin
        .normalize_inbound(&request(serde_json::json!([
            {"type": "web_search", "external_web_access": false}
        ])))
        .expect("a disabled hosted-tool marker remains inert");
    assert!(disabled_web_search.tools.is_empty());

    // A namespace flatten collision is a hard InvalidRequest, never a silent drop.
    let collision = plugin
        .normalize_inbound(&request(serde_json::json!([
            {"type": "function", "name": "container__nested", "parameters": {}},
            {"type": "namespace", "name": "container", "tools": [
                {"type": "function", "name": "nested", "parameters": {}}
            ]}
        ])))
        .expect_err("a flatten collision must fail loudly");
    assert_eq!(collision.code, ErrorCode::InvalidRequest);

    // A tool object without a `type` is still a hard InvalidRequest.
    let invalid = plugin
        .normalize_inbound(&request(serde_json::json!([{"name": "typeless"}])))
        .expect_err("a tool without a type is malformed");
    assert_eq!(invalid.code, ErrorCode::InvalidRequest);
}

#[test]
fn responses_custom_tool_round_trips() {
    // A custom (freeform-grammar) tool must survive the whole loop: declared as
    // a function with a fixed { input } schema on the way in, restored to a
    // `custom_tool_call` on the way out, and its replayed history accepted on
    // the next turn. Matches CC Switch's Codex↔Chat custom-tool handling.
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");

    // Request: the custom tool becomes a function with the fixed { input:string }
    // schema so the model always emits an `input` string we can unwrap.
    let normalized = plugin
        .normalize_inbound(&envelope(
            "openai-responses",
            json!({
                "model": "auto",
                "input": "go",
                "tools": [{"type": "custom", "name": "code_exec", "description": "run code"}]
            }),
        ))
        .expect("custom tool normalizes");
    assert_eq!(normalized.tools.len(), 1);
    assert_eq!(normalized.tools[0].name, "code_exec");
    assert_eq!(normalized.tools[0].parameters["required"], json!(["input"]));
    assert_eq!(
        normalized.tools[0].parameters["properties"]["input"]["type"],
        json!("string")
    );
    // The original tool definition is embedded in the description so a grammar /
    // format survives the schema-less function path.
    let description = normalized.tools[0]
        .description
        .as_deref()
        .unwrap_or_default();
    assert!(description.contains("Original tool definition:"));
    assert!(description.contains("\"name\":\"code_exec\""));

    // Request history: a prior custom_tool_call + its output are accepted (they
    // used to be a hard capability error) and become an assistant tool call
    // carrying the wrapped { input } arguments plus a tool result message.
    let replayed = plugin
        .normalize_inbound(&envelope(
            "openai-responses",
            json!({
                "model": "auto",
                "tools": [{"type": "custom", "name": "code_exec"}],
                "input": [
                    {"type": "custom_tool_call", "call_id": "c1", "name": "code_exec", "input": "print(1)"},
                    {"type": "custom_tool_call_output", "call_id": "c1", "output": "1"}
                ]
            }),
        ))
        .expect("custom tool history is accepted");
    let assistant = replayed
        .messages
        .iter()
        .find(|message| !message.tool_calls.is_empty())
        .expect("the replayed custom_tool_call becomes an assistant tool call");
    assert_eq!(assistant.tool_calls[0].name, "code_exec");
    assert_eq!(assistant.tool_calls[0].arguments, r#"{"input":"print(1)"}"#);
    assert!(
        replayed
            .messages
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("c1")),
        "the custom_tool_call_output becomes a tool result message"
    );

    // Response: a function_call named after the custom tool is restored to a
    // custom_tool_call with the unwrapped input string.
    let response: ChatResponse = serde_json::from_value(json!({
        "id": "resp-1",
        "model": "auto",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "c9",
                    "name": "code_exec",
                    "arguments": "{\"input\":\"print(2)\"}"
                }]
            }
        }]
    }))
    .expect("valid ChatResponse");
    let context = json!({
        "response_id": "resp-1",
        "model": "auto",
        "inbound_tools": [{"type": "custom", "name": "code_exec"}]
    });
    let rendered = plugin
        .render_response(&response, &context)
        .expect("renders the custom tool call");
    let item = &rendered["output"][0];
    assert_eq!(item["type"], json!("custom_tool_call"));
    assert_eq!(item["name"], json!("code_exec"));
    assert_eq!(item["call_id"], json!("c9"));
    assert_eq!(item["input"], json!("print(2)"));

    // Without the custom tool in context the same call stays a function_call —
    // a map miss must never invent a custom_tool_call.
    let plain = plugin
        .render_response(
            &response,
            &json!({"response_id": "resp-1", "model": "auto"}),
        )
        .expect("renders");
    assert_eq!(plain["output"][0]["type"], json!("function_call"));
}

#[test]
fn responses_namespace_tool_round_trips() {
    // A namespace child is flattened to `<ns>__<child>` on the way in (covered
    // by responses_builtin_tools_are_translated_to_functions); here we prove the
    // other half: the flat call is restored to the bare child name + namespace
    // field on the way out, and a replayed namespace-qualified call flattens
    // back on the next turn. Mirrors CC Switch's namespace restore.
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");
    let inbound_tools = json!([
        {"type": "namespace", "name": "container", "tools": [
            {"type": "function", "name": "nested", "parameters": {"type": "object"}}
        ]}
    ]);

    let response: ChatResponse = serde_json::from_value(json!({
        "id": "resp-1",
        "model": "auto",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{"id": "n1", "name": "container__nested", "arguments": "{}"}]
            }
        }]
    }))
    .expect("valid ChatResponse");
    let context = json!({
        "response_id": "resp-1",
        "model": "auto",
        "inbound_tools": inbound_tools
    });
    let rendered = plugin
        .render_response(&response, &context)
        .expect("renders the namespace call");
    let item = &rendered["output"][0];
    assert_eq!(item["type"], json!("function_call"));
    assert_eq!(item["name"], json!("nested"));
    assert_eq!(item["namespace"], json!("container"));
    assert_eq!(item["call_id"], json!("n1"));

    // Request history: the replayed namespace-qualified function_call flattens
    // back to the `<ns>__<child>` name the tool is declared under this turn.
    let replayed = plugin
        .normalize_inbound(&envelope(
            "openai-responses",
            json!({
                "model": "auto",
                "tools": inbound_tools,
                "input": [
                    {"type": "function_call", "call_id": "n1", "name": "nested", "namespace": "container", "arguments": "{}"},
                    {"type": "function_call_output", "call_id": "n1", "output": "ok"}
                ]
            }),
        ))
        .expect("namespace history is accepted");
    let assistant = replayed
        .messages
        .iter()
        .find(|message| !message.tool_calls.is_empty())
        .expect("the replayed namespace call becomes an assistant tool call");
    assert_eq!(assistant.tool_calls[0].name, "container__nested");
}

#[test]
fn responses_stream_restores_custom_and_namespace_tool_calls() {
    // The streaming path restores the same shapes as the non-streaming one: a
    // custom tool streams on the `custom_tool_call_input` family (never the
    // function_call family), and a namespace child streams as a function_call
    // bearing the bare child name plus a `namespace` field.
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");
    let context = json!({
        "protocol": "openai-responses",
        "stream_id": "s1",
        "response_id": "resp-s1",
        "model": "routed",
        "inbound_tools": [
            {"type": "custom", "name": "code_exec"},
            {"type": "namespace", "name": "container", "tools": [{"type": "function", "name": "nested"}]}
        ]
    });
    let tool =
        |index, id: Option<&str>, name: Option<&str>, args: &str| StreamEvent::ToolCallDelta {
            index,
            id: id.map(str::to_owned),
            name: name.map(str::to_owned),
            arguments_delta: args.to_owned(),
        };

    // Custom tool: announced as a custom_tool_call with a ctc_ id, and its
    // arguments must NOT stream on the function_call family.
    let added = response_sse_events(
        &plugin
            .render_stream_event(
                &tool(0, Some("cc"), Some("code_exec"), r#"{"input":"print(1)"}"#),
                &context,
            )
            .expect("custom tool starts"),
    );
    let added_item = added
        .iter()
        .find(|event| event["type"] == "response.output_item.added")
        .expect("output_item.added present");
    assert_eq!(added_item["item"]["type"], json!("custom_tool_call"));
    assert_eq!(added_item["item"]["id"], json!("ctc_cc"));
    assert!(
        added
            .iter()
            .all(|event| event["type"] != "response.function_call_arguments.delta"),
        "a custom tool's arguments must not stream on the function_call family"
    );

    // Namespace child: a function_call with the bare child name + namespace.
    let ns = response_sse_events(
        &plugin
            .render_stream_event(
                &tool(1, Some("nn"), Some("container__nested"), "{}"),
                &context,
            )
            .expect("namespace tool starts"),
    );
    let ns_item = ns
        .iter()
        .find(|event| event["type"] == "response.output_item.added")
        .expect("output_item.added present");
    assert_eq!(ns_item["item"]["type"], json!("function_call"));
    assert_eq!(ns_item["item"]["name"], json!("nested"));
    assert_eq!(ns_item["item"]["namespace"], json!("container"));

    // Done: the custom tool's buffered input is delivered on its own family, and
    // both items close in their restored shapes.
    let done = response_sse_events(
        &plugin
            .render_stream_event(
                &StreamEvent::Done {
                    finish_reason: Some(FinishReason::ToolCalls),
                    stop_sequence: None,
                },
                &context,
            )
            .expect("stream completes"),
    );
    assert!(
        done.iter().any(
            |event| event["type"] == "response.custom_tool_call_input.done"
                && event["input"] == json!("print(1)")
        ),
        "the custom tool's input is delivered at done"
    );
    let custom_done = done
        .iter()
        .find(|event| {
            event["type"] == "response.output_item.done"
                && event["item"]["type"] == json!("custom_tool_call")
        })
        .expect("custom tool closes as a custom_tool_call");
    assert_eq!(custom_done["item"]["input"], json!("print(1)"));
    let ns_done = done
        .iter()
        .find(|event| {
            event["type"] == "response.output_item.done"
                && event["item"]["type"] == json!("function_call")
        })
        .expect("namespace tool closes as a function_call");
    assert_eq!(ns_done["item"]["namespace"], json!("container"));
}

#[test]
fn responses_tool_search_round_trips() {
    // tool_search is proxied as a function with a query/limit schema and
    // restored to a client-executed tool_search_call; its replayed history is
    // accepted. Mirrors CC Switch's tool_search handling.
    let plugin = AgentPlugin::load(&runtime(), responses_agent_package()).expect("loads clean");

    let normalized = plugin
        .normalize_inbound(&envelope(
            "openai-responses",
            json!({"model": "auto", "input": "go", "tools": [{"type": "tool_search"}]}),
        ))
        .expect("tool_search normalizes");
    assert_eq!(normalized.tools[0].name, "tool_search");
    assert_eq!(normalized.tools[0].parameters["required"], json!(["query"]));

    let response: ChatResponse = serde_json::from_value(json!({
        "id": "r",
        "model": "auto",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "tool_calls": [{
                    "id": "t1",
                    "name": "tool_search",
                    "arguments": "{\"query\":\"gmail\",\"limit\":5}"
                }]
            }
        }]
    }))
    .expect("valid ChatResponse");
    let context = json!({
        "response_id": "r",
        "model": "auto",
        "inbound_tools": [{"type": "tool_search"}]
    });
    let rendered = plugin
        .render_response(&response, &context)
        .expect("renders the tool_search call");
    let item = &rendered["output"][0];
    assert_eq!(item["type"], json!("tool_search_call"));
    assert_eq!(item["execution"], json!("client"));
    assert_eq!(item["call_id"], json!("t1"));
    assert_eq!(item["arguments"]["query"], json!("gmail"));
    assert_eq!(item["arguments"]["limit"], json!(5));

    let replayed = plugin
        .normalize_inbound(&envelope(
            "openai-responses",
            json!({
                "model": "auto",
                "tools": [{"type": "tool_search"}],
                "input": [
                    {"type": "tool_search_call", "call_id": "t1", "arguments": {"query": "gmail"}},
                    {"type": "tool_search_output", "call_id": "t1", "output": "[]"}
                ]
            }),
        ))
        .expect("tool_search history is accepted");
    let assistant = replayed
        .messages
        .iter()
        .find(|message| !message.tool_calls.is_empty())
        .expect("the replayed tool_search_call becomes an assistant tool call");
    assert_eq!(assistant.tool_calls[0].name, "tool_search");
    assert!(
        replayed
            .messages
            .iter()
            .any(|message| message.tool_call_id.as_deref() == Some("t1")),
        "the tool_search_output becomes a tool result message"
    );
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
fn anthropic_server_tools_are_refused_but_client_tools_remain_functions() {
    // Canonical IR has no execution/result representation for Anthropic server
    // tools. Refuse those before routing instead of pretending an empty-schema
    // function is equivalent. Client-executed schema tools remain functions.
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");
    let request = |tools: serde_json::Value| {
        envelope(
            "anthropic-messages",
            serde_json::json!({
                "model": "claude-x",
                "max_tokens": 256,
                "messages": [{"role": "user", "content": "hi"}],
                "tools": tools
            }),
        )
    };

    let ok = plugin
        .normalize_inbound(&request(serde_json::json!([
            {"name": "read_file", "description": "read", "input_schema": {"type": "object"}}
        ])))
        .expect("user tools remain supported");
    assert_eq!(ok.tools.len(), 1);
    assert_eq!(ok.tools[0].name, "read_file");

    let custom = plugin
        .normalize_inbound(&request(serde_json::json!([
            {"type": "custom", "name": "grep", "input_schema": {"type": "object"}}
        ])))
        .expect("type==custom is a user tool");
    assert_eq!(custom.tools[0].name, "grep");

    for tool_type in [
        "web_search_20250305",
        "web_fetch_20250910",
        "code_execution_20250522",
        "tool_search_tool_regex_20251119",
        "mcp_20250812",
    ] {
        let error = plugin
            .normalize_inbound(&request(serde_json::json!([{
                "type": tool_type,
                "name": tool_type.split('_').next().unwrap()
            }])))
            .expect_err("server tools require native Anthropic passthrough");
        assert_eq!(error.code, ErrorCode::Capability);
        assert!(error.message.contains(tool_type));
    }

    for (tools, expected) in [
        (
            serde_json::json!([{"type": "bash_20250124", "name": "bash"}]),
            "bash",
        ),
        (
            serde_json::json!([{"type": "computer_20250124", "name": "computer"}]),
            "computer",
        ),
        (
            serde_json::json!([{"type": "memory_20250818", "name": "memory"}]),
            "memory",
        ),
    ] {
        let req = plugin
            .normalize_inbound(&request(tools))
            .expect("client-executed tools are translated to function tools");
        assert_eq!(req.tools.len(), 1);
        assert_eq!(req.tools[0].name, expected);
    }

    // A malformed user tool (no type, no input_schema) is still InvalidRequest.
    let invalid = plugin
        .normalize_inbound(&request(serde_json::json!([{"name": "broken"}])))
        .expect_err("a user tool without input_schema is malformed");
    assert_eq!(invalid.code, ErrorCode::InvalidRequest);
}

#[test]
fn anthropic_tool_choice_is_translated_not_refused() {
    // The old code 400'd every tool_choice except "auto" with a message that
    // wrongly blamed the Canonical IR. Now: "any" -> Required (lossless),
    // "tool" -> Auto (honest degrade on the translate path), "auto" -> Auto, and
    // a genuinely unknown type is a clean capability error naming the real
    // constraint (the chat provider), never the IR.
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");
    let request = |choice: serde_json::Value| {
        envelope(
            "anthropic-messages",
            json!({
                "model": "claude-x",
                "max_tokens": 256,
                "messages": [{"role": "user", "content": "hi"}],
                "tools": [{"name": "read_file", "description": "read", "input_schema": {"type": "object"}}],
                "tool_choice": choice
            }),
        )
    };

    let any = plugin
        .normalize_inbound(&request(json!({"type": "any"})))
        .expect("any translates");
    assert_eq!(any.tool_choice, Some(ToolChoice::Required));

    let tool = plugin
        .normalize_inbound(&request(json!({"type": "tool", "name": "read_file"})))
        .expect("a forced tool translates (degraded)");
    assert_eq!(tool.tool_choice, Some(ToolChoice::Auto));

    let auto = plugin
        .normalize_inbound(&request(json!({"type": "auto"})))
        .expect("auto translates");
    assert_eq!(auto.tool_choice, Some(ToolChoice::Auto));

    let unknown = plugin
        .normalize_inbound(&request(json!({"type": "mystery"})))
        .expect_err("an unknown tool_choice type is a capability error");
    assert_eq!(unknown.code, ErrorCode::Capability);
    assert!(
        !unknown.message.contains("Canonical IR"),
        "the error must name the chat provider, not blame the IR"
    );
}

#[test]
fn anthropic_output_config_effort_maps_to_reasoning_effort() {
    // Anthropic output_config.effort was previously swallowed into extensions and
    // dropped at the provider. Map it to the provider-neutral reasoning_effort
    // (low/medium/high passthrough, max clamps to high for OpenAI-compatible).
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");
    let request = |effort: serde_json::Value| {
        envelope(
            "anthropic-messages",
            serde_json::json!({
                "model": "claude-x",
                "max_tokens": 256,
                "messages": [{"role": "user", "content": "hi"}],
                "output_config": {"effort": effort}
            }),
        )
    };

    for (input, expected) in [
        ("low", "low"),
        ("medium", "medium"),
        ("high", "high"),
        ("max", "high"),
    ] {
        let normalized = plugin
            .normalize_inbound(&request(serde_json::json!(input)))
            .expect("normalizes");
        assert_eq!(
            normalized.extensions.get("reasoning_effort"),
            Some(&serde_json::json!(expected)),
            "effort {input} should map to reasoning_effort {expected}"
        );
    }

    // Unknown effort values are dropped rather than sent verbatim.
    let unknown = plugin
        .normalize_inbound(&request(serde_json::json!("turbo")))
        .expect("normalizes");
    assert_eq!(unknown.extensions.get("reasoning_effort"), None);
}

#[test]
fn anthropic_server_tool_history_blocks_fail_with_capability() {
    let plugin = AgentPlugin::load(&runtime(), anthropic_agent_package()).expect("loads clean");
    let request = |block: serde_json::Value| {
        envelope(
            "anthropic-messages",
            serde_json::json!({
                "model": "claude-x",
                "max_tokens": 256,
                "messages": [{"role": "assistant", "content": [block]}]
            }),
        )
    };
    for block in [
        serde_json::json!({"type": "server_tool_use", "id": "srv_1", "name": "web_search", "input": {}}),
        serde_json::json!({"type": "web_search_tool_result", "tool_use_id": "srv_1", "content": []}),
        serde_json::json!({"type": "search_result", "content": []}),
    ] {
        let error = plugin
            .normalize_inbound(&request(block))
            .expect_err("server-tool history has no IR representation");
        assert_eq!(error.code, ErrorCode::Capability);
        assert_eq!(error.http_status, 400);
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
