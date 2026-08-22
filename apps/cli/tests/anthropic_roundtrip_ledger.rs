//! Stage C's request-direction round-trip ledger, offline.
//!
//! The `anthropic-native` passthrough forwards the caller's Messages body
//! verbatim — only `model` is rewritten (`gateway.rs`, "Verbatim body, except
//! the caller's model is remapped to the routed one"). So the ground truth for
//! stage C is not another translator's opinion: it is the bytes the client
//! actually sent. That makes the property a round trip, `f(g(x)) ≟ x`, rather
//! than the translator-versus-translator diff S4 and the enterprise host's 5a
//! and 5b batches ran.
//!
//! `g` is `agent-anthropic`'s `normalize_inbound`; `f` is the official South
//! Anthropic component. Neither half needs an upstream, so this whole file runs
//! offline against zero traffic and touches no production path — which is why
//! it comes before any shadow wiring.
//!
//! Equivalence is byte equality after a normalisation that erases only the
//! registered divergence classes, the technique the enterprise host proved out
//! in 5a: anything unregistered survives normalisation and turns the gate red,
//! and retiring a normaliser is the evidence a class was really eliminated
//! rather than hidden.
//!
//! Two implementation notes:
//!
//! - `f` runs through the component's **JSON ABI**, not its typed seam. The
//!   typed seam speaks the kernel mirror's protocol types while this workspace
//!   defines the IR itself; going through JSON keeps those two packages from
//!   meeting, and it is the shape the runtime actually uses, so the ledger
//!   measures what production would measure.
//! - `f` is the `AnthropicReferenceV1` implementation rather than the wasm
//!   component. South's `anthropic_sandbox_parity_v1` already pins the
//!   sandboxed component to this implementation byte for byte, so the
//!   translation is the same and no wasm build is paid for here.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use serde_json::{Value, json};
use south_component_conformance::abi::build_http_request_json;
use south_component_conformance::reference_anthropic::AnthropicReferenceV1;
use token_station_conformance::AgentAdapter;
use token_station_plugin_runtime::{AgentPlugin, PluginRuntime, RuntimeLimits};
use token_station_protocol::{AgentRequestEnvelope, Extensions, HeaderDigest, Principal};

/// The model the router is pretending to have chosen. The passthrough rewrites
/// the caller's `model` to this before forwarding, so the ground truth carries
/// it too.
const ROUTED_MODEL: &str = "claude-sonnet-4-5";
const UPSTREAM: &str = "https://api.anthropic.com/v1";

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
}

fn agent_plugin() -> &'static AgentPlugin {
    static PLUGIN: OnceLock<AgentPlugin> = OnceLock::new();
    PLUGIN.get_or_init(|| {
        let source = repo_root().join("plugins/official/agent-anthropic");
        let status = Command::new("cargo")
            .args(["build", "--release", "--target", "wasm32-wasip2"])
            .current_dir(&source)
            .status()
            .expect("cargo is on PATH");
        assert!(
            status.success(),
            "agent-anthropic must build; run `rustup target add wasm32-wasip2` if missing"
        );
        let package =
            std::env::temp_dir().join(format!("ts-anthropic-ledger-{}", std::process::id()));
        std::fs::create_dir_all(&package).expect("temp dir is writable");
        std::fs::copy(source.join("manifest.json"), package.join("manifest.json"))
            .expect("manifest copies");
        std::fs::copy(
            source.join("target/wasm32-wasip2/release/agent_anthropic.wasm"),
            package.join("adapter.wasm"),
        )
        .expect("wasm copies");
        let runtime = PluginRuntime::new(RuntimeLimits::default()).expect("engine builds");
        AgentPlugin::load(&runtime, &package).expect("the official agent package loads")
    })
}

/// `ProviderEndpoint` serialises as a bare string, so the component's config is
/// this small.
fn component_config() -> String {
    json!({"provider": "anthropic", "base_url": UPSTREAM}).to_string()
}

/// What the passthrough would put on the wire for this caller body.
fn ground_truth(client_body: &Value) -> Value {
    let mut forwarded = client_body.clone();
    forwarded["model"] = json!(ROUTED_MODEL);
    forwarded
}

/// What the translated route would put on the wire for the same body.
fn round_tripped(client_body: &Value) -> Result<Value, String> {
    let envelope = AgentRequestEnvelope {
        protocol: "anthropic-messages".to_owned(),
        agent_tool: None,
        headers: HeaderDigest::default(),
        principal: Principal {
            subject: "local".to_owned(),
            tenant: None,
        },
        hints: Vec::new(),
        body: client_body.clone(),
        extensions: Extensions::new(),
    };
    let mut request = agent_plugin()
        .normalize_inbound(&envelope)
        .map_err(|envelope| envelope.message.clone())?;
    // Routing happens after normalisation, so the ground truth's rewritten model
    // is applied here rather than becoming a divergence class of its own.
    ROUTED_MODEL.clone_into(&mut request.model);

    let request_json = serde_json::to_string(&request).expect("the host IR serialises");
    let descriptor_json =
        build_http_request_json(&AnthropicReferenceV1, &request_json, &component_config())
            .map_err(|error| format!("component refused: {error}"))?;
    let descriptor: Value =
        serde_json::from_str(&descriptor_json).expect("the component returns JSON");
    descriptor
        .get("body")
        .filter(|body| !body.is_null())
        .cloned()
        .ok_or_else(|| "the component produced no body".to_owned())
}

/// One observation, classified. `Refused` is the class S4's shadow did not need
/// and this one cannot do without: `normalize_inbound` rejects shapes the
/// passthrough serves today, and each rejection measures the coverage gap rather
/// than reporting a diff.
#[derive(Debug)]
enum Outcome {
    Equivalent,
    Divergent {
        passthrough: Value,
        component: Value,
    },
    Refused(String),
}

fn observe(client_body: &Value) -> Outcome {
    let truth = ground_truth(client_body);
    match round_tripped(client_body) {
        Err(reason) => Outcome::Refused(reason),
        Ok(rendered) => {
            // serde_json's Map is a BTreeMap, so serialisation already fixes key
            // order; canonicalising further would only hide real differences.
            if truth == rendered {
                Outcome::Equivalent
            } else {
                Outcome::Divergent {
                    passthrough: truth,
                    component: rendered,
                }
            }
        }
    }
}

/// The curated corpus. Random search over generated bodies is the next step and
/// the one that finds the classes hand-written cases miss — 5a's sweep grew its
/// ledger from six classes to twelve, and two of the additions outranked most of
/// the original list. These cases stand the harness up and cover the shapes the
/// reconnaissance already named.
#[allow(clippy::too_many_lines)] // a literal corpus reads worse split in half
fn corpus() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal text turn",
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "messages": [{"role": "user", "content": "hello"}]
            }),
        ),
        (
            "content as a block array",
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "messages": [{
                    "role": "user",
                    "content": [{"type": "text", "text": "hello"}]
                }]
            }),
        ),
        (
            "top-level system string",
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "system": "be brief",
                "messages": [{"role": "user", "content": "hello"}]
            }),
        ),
        (
            "assistant turn carrying thinking",
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "messages": [
                    {"role": "user", "content": "hello"},
                    {"role": "assistant", "content": [
                        {"type": "thinking", "thinking": "weighing it", "signature": "sig-abc"},
                        {"type": "text", "text": "hi"}
                    ]},
                    {"role": "user", "content": "again"}
                ]
            }),
        ),
        (
            "tool_use then tool_result",
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "tools": [{
                    "name": "lookup",
                    "description": "look something up",
                    "input_schema": {"type": "object", "properties": {}}
                }],
                "messages": [
                    {"role": "user", "content": "look it up"},
                    {"role": "assistant", "content": [
                        {"type": "tool_use", "id": "tu_1", "name": "lookup", "input": {}}
                    ]},
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "tu_1", "content": "done"}
                    ]}
                ]
            }),
        ),
        (
            "server-tool history on a follow-up turn",
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "messages": [
                    {"role": "user", "content": "search it"},
                    {"role": "assistant", "content": [
                        {"type": "server_tool_use", "id": "st_1", "name": "web_search", "input": {}},
                        {"type": "web_search_tool_result", "tool_use_id": "st_1", "content": []}
                    ]},
                    {"role": "user", "content": "and again"}
                ]
            }),
        ),
        (
            "sampling parameters",
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "temperature": 0.5,
                "top_p": 0.9,
                "stop_sequences": ["STOP"],
                "messages": [{"role": "user", "content": "hello"}]
            }),
        ),
        (
            "tool_choice naming a tool",
            json!({
                "model": "claude-sonnet-4-5",
                "max_tokens": 1024,
                "tools": [{
                    "name": "lookup",
                    "input_schema": {"type": "object", "properties": {}}
                }],
                "tool_choice": {"type": "tool", "name": "lookup"},
                "messages": [{"role": "user", "content": "hello"}]
            }),
        ),
    ]
}

/// Discovery, not a gate: prints every observation so the ledger can be written
/// from what the round trip actually does rather than from what it ought to do.
/// The gate that replaces this is added once the classes it finds are registered
/// and ruled on.
#[test]
#[ignore = "discovery pass; run explicitly to regenerate the ledger"]
fn discover_request_direction_divergences() {
    for (name, body) in corpus() {
        match observe(&body) {
            Outcome::Equivalent => println!("== {name}\n   EQUIVALENT"),
            Outcome::Refused(reason) => println!("== {name}\n   REFUSED: {reason}"),
            Outcome::Divergent {
                passthrough,
                component,
            } => {
                println!(
                    "== {name}\n   PASSTHROUGH: {}\n   COMPONENT  : {}",
                    serde_json::to_string(&passthrough).expect("printable"),
                    serde_json::to_string(&component).expect("printable"),
                );
            }
        }
    }
}

// -- the ledger ---------------------------------------------------------------
//
// Two classes, both found by the first discovery pass over the curated corpus,
// both landing exactly on the fault lines the reconnaissance named.
//
// **R-1 — assistant `thinking` blocks are dropped, silently.** `agent-anthropic`
// puts them in `extensions["anthropic_thinking_blocks"]` (an adapter-local
// channel that predates the IR gaining `ContentPart::Thinking`), while the
// component reads the typed field. The two channels never meet, so the block
// vanishes with no error. This is the worst class in the ledger: silent content
// loss on the exact multi-turn continuation the passthrough exists to serve.
// Retiring it is stage B's job.
//
// **R-2 — a forced `tool_choice` degrades to `auto`.** Not an accident, and not
// an IR limitation: `validate_tool_choice` says so itself — "`ToolChoice::Other`
// can carry any shape; the real constraint is the downstream OpenAI-compatible
// chat provider." Someone already corrected the error message that blamed
// Canonical IR but could not correct the behaviour, because there was no
// Anthropic renderer to route to. Stage C supplies one; stage A′ then stops the
// degradation.
//
// Erasers run on **both** sides and stay narrow, so a new difference in the same
// area still survives. When a class is really fixed its eraser stops being
// load-bearing, which `every_registered_class_is_still_load_bearing` reports —
// retiring the eraser is then the evidence the class is gone rather than hidden.

/// R-1: drop `thinking` / `redacted_thinking` blocks from every content array.
fn erase_r1_dropped_thinking(value: &mut Value) {
    match value {
        Value::Array(items) => {
            items.retain(|item| {
                !matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("thinking" | "redacted_thinking")
                )
            });
            for item in items {
                erase_r1_dropped_thinking(item);
            }
        }
        Value::Object(fields) => {
            for (_, field) in fields.iter_mut() {
                erase_r1_dropped_thinking(field);
            }
        }
        _ => {}
    }
}

/// R-2: rewrite a forced `tool_choice` to the `auto` the translate path emits.
fn erase_r2_degraded_tool_choice(value: &mut Value) {
    let Some(choice) = value.get_mut("tool_choice") else {
        return;
    };
    if choice.get("type").and_then(Value::as_str) == Some("tool") {
        *choice = json!({"type": "auto"});
    }
}

/// A registered class: its label, and the narrow rewrite that erases exactly it.
type Eraser = (&'static str, fn(&mut Value));

const ERASERS: &[Eraser] = &[
    (
        "R-1 assistant thinking blocks are dropped",
        erase_r1_dropped_thinking,
    ),
    (
        "R-2 a forced tool_choice degrades to auto",
        erase_r2_degraded_tool_choice,
    ),
];

/// Refusals that have been looked at and are understood. A refusal is not a
/// divergence — it is a measurement of what the translated route cannot serve
/// yet — but an *unrecognised* refusal still fails the gate, because the point
/// of the ledger is that nothing goes unlisted.
const REGISTERED_REFUSALS: &[&str] = &["server-tool history block"];

fn erased(mut value: Value) -> Value {
    for (_, erase) in ERASERS {
        erase(&mut value);
    }
    value
}

/// The gate: after erasing exactly the registered classes, the two sides are
/// byte-equal. Anything unregistered survives and turns this red.
#[test]
fn the_request_direction_ledger_is_complete() {
    let mut unlisted = Vec::new();
    for (name, body) in corpus() {
        match observe(&body) {
            Outcome::Equivalent => {}
            Outcome::Refused(reason) => {
                if !REGISTERED_REFUSALS
                    .iter()
                    .any(|registered| reason.contains(registered))
                {
                    unlisted.push(format!("{name}: unregistered refusal: {reason}"));
                }
            }
            Outcome::Divergent {
                passthrough,
                component,
            } => {
                let (left, right) = (erased(passthrough), erased(component));
                if left != right {
                    unlisted.push(format!(
                        "{name}: a difference survived every registered eraser\n  \
                         passthrough: {}\n  component  : {}",
                        serde_json::to_string(&left).expect("printable"),
                        serde_json::to_string(&right).expect("printable"),
                    ));
                }
            }
        }
    }
    assert!(
        unlisted.is_empty(),
        "the ledger is incomplete:\n{}",
        unlisted.join("\n")
    );
}

/// A retired class must be retired, not left standing as dead normalisation.
/// Each eraser has to still change the outcome for at least one case; when one
/// stops mattering, that is the signal the class was really eliminated and the
/// eraser should go with it.
#[test]
fn every_registered_class_is_still_load_bearing() {
    for (index, (label, _)) in ERASERS.iter().enumerate() {
        let others: Vec<_> = ERASERS
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, entry)| entry)
            .collect();
        let load_bearing = corpus().into_iter().any(|(_, body)| {
            let Outcome::Divergent {
                passthrough,
                component,
            } = observe(&body)
            else {
                return false;
            };
            let apply = |mut value: Value| {
                for (_, erase) in &others {
                    erase(&mut value);
                }
                value
            };
            // Without this eraser the sides still differ; with the full set they
            // do not. That is what makes it load-bearing.
            apply(passthrough.clone()) != apply(component.clone())
                && erased(passthrough) == erased(component)
        });
        assert!(
            load_bearing,
            "`{label}` no longer changes any outcome — if the class is fixed, retire the eraser \
             with it rather than leaving dead normalisation behind"
        );
    }
}

/// The seam between the two protocol packages is machine-checked here rather
/// than assumed. This workspace defines the IR (`crates/protocol`); South's
/// component takes the same types from the kernel mirror, byte-identical as of
/// the P0.5 alignment. The value crosses as JSON, so a drift shows up as the
/// component failing to parse a body this host considers well-formed — and it
/// shows up here, loudly, instead of as a wall of phantom divergences.
#[test]
fn the_component_still_parses_this_workspaces_ir() {
    let body = json!({
        "model": "claude-sonnet-4-5",
        "max_tokens": 1024,
        "messages": [{"role": "user", "content": "hello"}]
    });
    match round_tripped(&body) {
        Ok(_) => {}
        Err(reason) => panic!(
            "crates/protocol and the pinned kernel revision must still agree on the IR wire \
             shape: {reason}"
        ),
    }
}
