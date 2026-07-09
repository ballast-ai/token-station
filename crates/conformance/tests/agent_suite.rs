//! `agent-protocol-v1`, run against a reference adapter and against adapters
//! broken in exactly one way.
//!
//! The agent suite has fewer bespoke gates than the provider suite, and that is
//! not an oversight. An `agent-adapter` names no credential and addresses no
//! upstream, so there is no `EndpointConfinement` to check and no error
//! retriability to get wrong. What is left — translate faithfully,
//! deterministically, and survive a field you do not know — is checked here.

use std::cell::Cell;
use std::fmt::Display;
use std::path::Path;

use serde_json::{Map, Value, json};
use token_station_conformance::{
    AdapterResult, AgentAdapter, AgentFamily, Check, FixturePack, Report, run_agent_suite,
};
use token_station_plugin_api::{AdapterKind, AdapterMetadata};
use token_station_protocol::{
    AgentHint, AgentRequestEnvelope, ChatRequest, ChatResponse, Content, ContentPart, ErrorCode,
    ErrorEnvelope, Extensions, HintKind, Message, Role, Sampling, StreamEvent, ToolCall, ToolDef,
};

// -- the reference adapter ----------------------------------------------------

fn internal(detail: impl Display) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Internal, 500, detail.to_string())
}

fn invalid(detail: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::InvalidRequest, 400, detail)
}

/// The headers this adapter reads a routing hint out of.
const HINT_HEADERS: &[(&str, HintKind)] = &[
    ("x-agent-step", HintKind::StepType),
    ("x-agent-task", HintKind::TaskType),
    ("x-agent-preference", HintKind::Preference),
];

fn tool_calls_from_openai(raw: &Value) -> Vec<ToolCall> {
    raw.as_array()
        .into_iter()
        .flatten()
        .map(|call| ToolCall {
            id: call["id"].as_str().unwrap_or_default().to_owned(),
            name: call["function"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            arguments: call["function"]["arguments"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        })
        .collect()
}

fn tool_calls_to_openai(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": "function",
                    "function": {"name": call.name, "arguments": call.arguments},
                })
            })
            .collect(),
    )
}

fn content_to_json(content: Option<&Content>) -> Value {
    match content {
        Some(Content::Text(text)) => json!(text),
        Some(Content::Parts(parts)) => json!(parts),
        None => Value::Null,
    }
}

struct OpenAiClient;

impl AgentAdapter for OpenAiClient {
    fn metadata(&self) -> AdapterMetadata {
        AdapterMetadata::new(
            "agent-openai",
            "1.0.0",
            AdapterKind::Agent,
            "agent-adapter-v1",
        )
    }

    fn normalize_inbound(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<ChatRequest> {
        let body = &envelope.body;
        let model = body["model"]
            .as_str()
            .ok_or_else(|| invalid("request declares no model"))?
            .to_owned();

        let mut messages = Vec::new();
        for raw in body["messages"].as_array().into_iter().flatten() {
            let role = match raw["role"].as_str() {
                Some("system") => Role::System,
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                Some("tool") => Role::Tool,
                _ => return Err(invalid("message declares no known role")),
            };
            let content = match &raw["content"] {
                Value::String(text) => Some(Content::Text(text.clone())),
                parts @ Value::Array(_) => {
                    let parsed: Vec<ContentPart> =
                        serde_json::from_value(parts.clone()).map_err(internal)?;
                    Some(Content::Parts(parsed))
                }
                _ => None,
            };
            messages.push(Message {
                role,
                content,
                tool_calls: tool_calls_from_openai(&raw["tool_calls"]),
                tool_call_id: raw["tool_call_id"].as_str().map(str::to_owned),
                name: raw["name"].as_str().map(str::to_owned),
                extensions: Extensions::new(),
            });
        }

        let tools = body["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|tool| ToolDef {
                name: tool["function"]["name"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                description: tool["function"]["description"].as_str().map(str::to_owned),
                parameters: tool["function"]["parameters"].clone(),
            })
            .collect();

        Ok(ChatRequest {
            model,
            messages,
            tools,
            response_format: None,
            sampling: Sampling {
                temperature: body["temperature"].as_f64(),
                top_p: body["top_p"].as_f64(),
                max_output_tokens: body["max_tokens"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok()),
                stop: body["stop"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect(),
            },
            stream: body["stream"].as_bool().unwrap_or(false),
            extensions: Extensions::new(),
        })
    }

    fn extract_agent_hint(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<Vec<AgentHint>> {
        // A hint is routing *input*. Naming a provider here would not make the
        // router honour it, so there is nothing to guard against.
        Ok(HINT_HEADERS
            .iter()
            .filter_map(|(header, kind)| {
                envelope
                    .headers
                    .value(header)
                    .map(|value| AgentHint::new(*kind, value))
            })
            .collect())
    }

    fn render_response(&self, response: &ChatResponse, _context: &Value) -> AdapterResult<Value> {
        let choices: Vec<Value> = response
            .choices
            .iter()
            .map(|choice| {
                let mut message = Map::new();
                message.insert("role".to_owned(), json!(choice.message.role));
                message.insert(
                    "content".to_owned(),
                    content_to_json(choice.message.content.as_ref()),
                );
                if !choice.message.tool_calls.is_empty() {
                    message.insert(
                        "tool_calls".to_owned(),
                        tool_calls_to_openai(&choice.message.tool_calls),
                    );
                }
                json!({
                    "index": choice.index,
                    "message": Value::Object(message),
                    "finish_reason": choice.finish_reason,
                })
            })
            .collect();

        Ok(json!({
            "id": response.id,
            "object": "chat.completion",
            "model": response.model,
            "choices": choices,
            "usage": {
                "prompt_tokens": response.usage.input_tokens,
                "completion_tokens": response.usage.output_tokens,
                "total_tokens": response.usage.total(),
            },
        }))
    }

    fn render_stream_event(&self, event: &StreamEvent, _context: &Value) -> AdapterResult<Value> {
        // Templated rather than serialized: the entry protocol's key order is
        // part of its wire format, and a `Map` would sort it.
        let data = match event {
            StreamEvent::Delta { index, content } => format!(
                "data: {{\"choices\":[{{\"index\":{index},\"delta\":{{\"content\":{}}}}}]}}\n\n",
                serde_json::to_string(content).map_err(internal)?
            ),
            StreamEvent::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => {
                let mut function = format!(
                    "\"arguments\":{}",
                    serde_json::to_string(arguments_delta).map_err(internal)?
                );
                if let Some(name) = name {
                    function = format!(
                        "\"name\":{},{function}",
                        serde_json::to_string(name).map_err(internal)?
                    );
                }
                let identity = match id {
                    Some(id) => format!("\"id\":{},", serde_json::to_string(id).map_err(internal)?),
                    None => String::new(),
                };
                format!(
                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":\
                     [{{\"index\":{index},{identity}\"function\":{{{function}}}}}]}}}}]}}\n\n"
                )
            }
            StreamEvent::Usage { usage } => format!(
                "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{},\"completion_tokens\":{}}}}}\n\n",
                usage.input_tokens, usage.output_tokens
            ),
            StreamEvent::Done { finish_reason } => {
                let reason = match finish_reason {
                    Some(reason) => serde_json::to_string(reason).map_err(internal)?,
                    None => "null".to_owned(),
                };
                format!(
                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":{reason}}}]}}\
                     \n\ndata: [DONE]\n\n"
                )
            }
            StreamEvent::Error { error } => format!(
                "data: {}\n\n",
                serde_json::to_string(&self.map_inbound_error(error, &Value::Null)?)
                    .map_err(internal)?
            ),
        };

        Ok(json!({ "data": data }))
    }

    fn map_inbound_error(&self, error: &ErrorEnvelope, _context: &Value) -> AdapterResult<Value> {
        let code = serde_json::to_value(error.code).map_err(internal)?;
        Ok(json!({
            "error": {
                "message": error.message,
                "type": code,
                "code": code,
            },
        }))
    }
}

// -- adapters broken in exactly one way ---------------------------------------

/// Extracts the right hints once, then forgets them.
struct Nondeterministic {
    calls: Cell<u32>,
}

impl AgentAdapter for Nondeterministic {
    fn metadata(&self) -> AdapterMetadata {
        OpenAiClient.metadata()
    }
    fn normalize_inbound(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<ChatRequest> {
        OpenAiClient.normalize_inbound(envelope)
    }
    fn extract_agent_hint(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<Vec<AgentHint>> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        if call % 2 == 1 {
            return Ok(Vec::new());
        }
        OpenAiClient.extract_agent_hint(envelope)
    }
    fn render_response(&self, response: &ChatResponse, context: &Value) -> AdapterResult<Value> {
        OpenAiClient.render_response(response, context)
    }
    fn render_stream_event(&self, event: &StreamEvent, context: &Value) -> AdapterResult<Value> {
        OpenAiClient.render_stream_event(event, context)
    }
    fn map_inbound_error(&self, error: &ErrorEnvelope, context: &Value) -> AdapterResult<Value> {
        OpenAiClient.map_inbound_error(error, context)
    }
}

/// Refuses an envelope carrying a field this ABI version does not model.
struct BrittleAgainstNewerPeers;

impl AgentAdapter for BrittleAgainstNewerPeers {
    fn metadata(&self) -> AdapterMetadata {
        OpenAiClient.metadata()
    }
    fn normalize_inbound(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<ChatRequest> {
        if !envelope.extensions.is_empty() {
            return Err(invalid("unrecognised envelope field"));
        }
        OpenAiClient.normalize_inbound(envelope)
    }
    fn extract_agent_hint(&self, envelope: &AgentRequestEnvelope) -> AdapterResult<Vec<AgentHint>> {
        OpenAiClient.extract_agent_hint(envelope)
    }
    fn render_response(&self, response: &ChatResponse, context: &Value) -> AdapterResult<Value> {
        OpenAiClient.render_response(response, context)
    }
    fn render_stream_event(&self, event: &StreamEvent, context: &Value) -> AdapterResult<Value> {
        OpenAiClient.render_stream_event(event, context)
    }
    fn map_inbound_error(&self, error: &ErrorEnvelope, context: &Value) -> AdapterResult<Value> {
        OpenAiClient.map_inbound_error(error, context)
    }
}

// -- the suite ----------------------------------------------------------------

fn pack() -> FixturePack<AgentFamily> {
    FixturePack::load(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/openai"
    )))
    .expect("the self-test pack must load")
}

fn failed_checks(report: &Report) -> Vec<Check> {
    let mut checks: Vec<Check> = report.failures().map(|outcome| outcome.check).collect();
    checks.sort_unstable();
    checks.dedup();
    checks
}

#[test]
fn the_reference_adapter_passes_every_gate() {
    let report = run_agent_suite(&OpenAiClient, &pack());

    assert!(report.is_passing(), "{report}");
    assert_eq!(report.suite(), "agent-protocol-v1");
}

#[test]
fn the_pack_reaches_every_gate_the_agent_suite_has() {
    let report = run_agent_suite(&OpenAiClient, &pack());
    let mut reached: Vec<Check> = report.outcomes().iter().map(|o| o.check).collect();
    reached.sort_unstable();
    reached.dedup();

    assert_eq!(
        reached,
        vec![
            Check::Coverage,
            Check::FixtureMatch,
            Check::Determinism,
            Check::UnknownFieldTolerance,
        ]
    );
}

#[test]
fn one_fixture_directory_serves_both_roles() {
    // Every provider fixture in the same directory was skipped, not refused.
    let pack = pack();
    let names: Vec<&str> = pack.cases().iter().map(|case| case.name.as_str()).collect();

    assert!(names.iter().all(|name| name.starts_with("agent.")));
    assert_eq!(names.len(), 5);
}

#[test]
fn a_pack_missing_a_family_is_refused_by_name() {
    let pruned = FixturePack::from_cases(
        pack()
            .cases()
            .iter()
            .filter(|case| !case.name.starts_with("agent.error"))
            .cloned()
            .collect(),
    );
    let report = run_agent_suite(&OpenAiClient, &pruned);

    assert!(!report.is_passing());
    let missing = report
        .failures()
        .find(|outcome| outcome.check == Check::Coverage)
        .expect("coverage must fail");
    assert_eq!(missing.case, "agent.error");
}

#[test]
fn a_nondeterministic_adapter_is_caught_even_though_it_matches_the_fixture() {
    let adapter = Nondeterministic {
        calls: Cell::new(0),
    };
    let report = run_agent_suite(&adapter, &pack());

    assert_eq!(failed_checks(&report), vec![Check::Determinism]);
}

#[test]
fn an_adapter_that_refuses_an_unmodelled_field_is_caught() {
    let report = run_agent_suite(&BrittleAgainstNewerPeers, &pack());

    assert_eq!(failed_checks(&report), vec![Check::UnknownFieldTolerance]);
}

#[test]
fn a_hint_is_read_from_a_header_whose_value_survived_redaction() {
    let envelope: AgentRequestEnvelope = serde_json::from_str(include_str!(
        "fixtures/openai/agent.hint.step-header.input.json"
    ))
    .expect("valid envelope");

    // The credential header arrived and is visible by name; its value is gone.
    assert!(envelope.headers.contains("authorization"));
    assert_eq!(envelope.headers.value("authorization"), None);

    let hints = OpenAiClient
        .extract_agent_hint(&envelope)
        .expect("hints extract");
    assert_eq!(hints.len(), 1);
    assert_eq!(hints[0].kind, HintKind::StepType);
}
