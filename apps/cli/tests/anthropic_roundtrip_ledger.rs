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

use std::collections::{BTreeMap, BTreeSet};
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

/// Every sample every gate runs over: the curated shapes, then the generated
/// sweep. Keeping them in one place means a class found by the sweep is also
/// held to the load-bearing check, and vice versa.
fn all_bodies() -> impl Iterator<Item = (String, Value)> {
    corpus()
        .into_iter()
        .map(|(name, body)| (name.to_owned(), body))
        .chain(
            (1..=SWEEP_CASES).map(|seed| (format!("generated seed {seed}"), generated_body(seed))),
        )
}

/// The sweep as a gate: over the whole generated space, every divergence must
/// fall to a registered eraser. A new one survives and turns this red, with the
/// seed that reproduces it.
#[test]
fn the_generated_sweep_finds_no_unregistered_class() {
    let mut unlisted: BTreeMap<String, u64> = BTreeMap::new();
    for seed in 1..=SWEEP_CASES {
        let body = generated_body(seed);
        let Outcome::Divergent {
            passthrough,
            component,
        } = observe(&body)
        else {
            continue;
        };
        let (left, right) = (erased(passthrough), erased(component));
        if left == right {
            continue;
        }
        let signature = difference_signature(&left, &right)
            .into_iter()
            .collect::<Vec<_>>()
            .join(" | ");
        unlisted.entry(signature).or_insert(seed);
    }
    assert!(
        unlisted.is_empty(),
        "{} unregistered class(es) survived every eraser:\n{}",
        unlisted.len(),
        unlisted
            .iter()
            .map(|(signature, seed)| format!("  seed {seed}: {signature}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
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
// **R-1 — retired.** Assistant `thinking` blocks used to vanish silently:
// `agent-anthropic` put them in `extensions["anthropic_thinking_blocks"]`, an
// adapter-local channel older than the IR's own `ContentPart::Thinking`, while
// the component read the typed field. The two never met. The fix was to stop
// diverting them — they ride in `content` parts like every other block now, and
// the extension is gone with them. This eraser is retired rather than kept as
// dead normalisation, which is the evidence the class is actually eliminated:
// `every_registered_class_is_still_load_bearing` is what demanded the
// retirement, and the 200,000-body sweep confirmed the fix introduced nothing
// new in its place.
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

/// R-2: rewrite a forced `tool_choice` to the `auto` the translate path emits.
fn erase_r2_degraded_tool_choice(value: &mut Value) {
    let Some(choice) = value.get_mut("tool_choice") else {
        return;
    };
    if choice.get("type").and_then(Value::as_str) == Some("tool") {
        *choice = json!({"type": "auto"});
    }
}

/// R-3: a block-array `system` is flattened to the string the IR carries.
fn erase_r3_flattened_system(value: &mut Value) {
    let Some(system) = value.get_mut("system") else {
        return;
    };
    let Some(blocks) = system.as_array() else {
        return;
    };
    let all_text = blocks.iter().all(|block| {
        block.get("type").and_then(Value::as_str) == Some("text")
            && block.get("text").and_then(Value::as_str).is_some()
    });
    if !all_text {
        return;
    }
    let joined: String = blocks
        .iter()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    *system = json!(joined);
}

/// R-4: `metadata` is dropped — the IR has no slot for it, so `user_id` and its
/// siblings never reach the upstream on the translated route.
fn erase_r4_dropped_metadata(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.remove("metadata");
    }
}

/// R-5: a bare-string `content` comes back as a one-element text array.
fn erase_r5_string_content_becomes_array(value: &mut Value) {
    let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        if let Some(text) = content.as_str() {
            *content = json!([{"type": "text", "text": text}]);
        }
    }
}

/// R-6: `tool_use` blocks are re-ordered to the end of the turn.
///
/// The IR splits a message into `content` parts and a separate `tool_calls`
/// vector, so a renderer can only append the calls after the parts — the
/// interleaving the caller sent is not representable. Registered rather than
/// condoned: block order carries meaning in an assistant turn, and this is the
/// same structural gap that makes retiring the `anthropic_thinking_blocks`
/// extension a shape question rather than a swap.
fn erase_r6_tool_use_reordered(value: &mut Value) {
    let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages {
        let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
            continue;
        };
        let is_tool_use =
            |block: &Value| block.get("type").and_then(Value::as_str) == Some("tool_use");
        let (calls, rest): (Vec<Value>, Vec<Value>) = blocks.iter().cloned().partition(is_tool_use);
        blocks.clear();
        blocks.extend(rest);
        blocks.extend(calls);
    }
}

/// A registered class: its label, and the narrow rewrite that erases exactly it.
type Eraser = (&'static str, fn(&mut Value));

const ERASERS: &[Eraser] = &[
    (
        "R-2 a forced tool_choice degrades to auto",
        erase_r2_degraded_tool_choice,
    ),
    (
        "R-3 a block-array system is flattened",
        erase_r3_flattened_system,
    ),
    ("R-4 metadata is dropped", erase_r4_dropped_metadata),
    (
        "R-5 a string content becomes a text array",
        erase_r5_string_content_becomes_array,
    ),
    (
        "R-6 tool_use blocks are re-ordered to the end",
        erase_r6_tool_use_reordered,
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
        let load_bearing = all_bodies().any(|(_, body)| {
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

// -- generated sweep -----------------------------------------------------------
//
// The curated corpus above proves the harness; it does not bound the shape
// space. 5a's sweep grew its ledger from six classes to twelve and two of the
// additions outranked most of the original list, so the same lesson applies
// here: a hand-written list is a starting point, not a census.
//
// The generator is a seeded walk over a grammar of Messages bodies rather than
// a `proptest` dependency. Determinism is the point — a gate that finds a new
// class on one run and not the next is not a gate — and the shape space here is
// discrete enough that a deliberate grammar covers it better than sampling.

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        // xorshift64*, deterministic and dependency-free.
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next_u64() % n as u64).unwrap_or(0)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next_u64() % 100 < percent
    }
}

fn text_block(rng: &mut Rng) -> Value {
    json!({"type": "text", "text": format!("t{}", rng.below(4))})
}

fn user_block(rng: &mut Rng) -> Value {
    match rng.below(5) {
        0 => json!({
            "type": "image",
            "source": {"type": "base64", "media_type": "image/png", "data": "aGk="}
        }),
        1 => json!({"type": "document", "source": {"type": "text", "data": "d"}}),
        2 => json!({"type": "search_result", "content": []}),
        3 => json!({"type": "wholly_unknown_block", "payload": {"k": rng.below(3)}}),
        _ => text_block(rng),
    }
}

fn assistant_block(rng: &mut Rng) -> Value {
    match rng.below(6) {
        0 => json!({
            "type": "thinking",
            "thinking": format!("th{}", rng.below(3)),
            "signature": format!("sig{}", rng.below(3))
        }),
        1 => json!({"type": "redacted_thinking", "data": "opaque"}),
        2 => json!({"type": "tool_use", "id": "tu_1", "name": "lookup", "input": {}}),
        3 => json!({"type": "server_tool_use", "id": "st_1", "name": "web_search", "input": {}}),
        4 => json!({"type": "web_search_tool_result", "tool_use_id": "st_1", "content": []}),
        _ => text_block(rng),
    }
}

fn content(rng: &mut Rng, assistant: bool) -> Value {
    if rng.chance(25) {
        return json!(format!("s{}", rng.below(3)));
    }
    let count = 1 + rng.below(3);
    let blocks: Vec<Value> = (0..count)
        .map(|_| {
            if assistant {
                assistant_block(rng)
            } else {
                user_block(rng)
            }
        })
        .collect();
    Value::Array(blocks)
}

fn generated_body(seed: u64) -> Value {
    let rng = &mut Rng(seed | 1);
    let turns = 1 + rng.below(4);
    let mut messages = Vec::new();
    for turn in 0..turns {
        let assistant = turn % 2 == 1;
        messages.push(json!({
            "role": if assistant { "assistant" } else { "user" },
            "content": content(rng, assistant)
        }));
    }
    let mut body = json!({
        "model": "claude-sonnet-4-5",
        "max_tokens": 256 * (1 + rng.below(4)),
        "messages": messages
    });
    let object = body.as_object_mut().expect("object");
    if rng.chance(30) {
        object.insert(
            "system".to_owned(),
            if rng.chance(50) {
                json!("be brief")
            } else {
                json!([{"type": "text", "text": "be brief"}])
            },
        );
    }
    if rng.chance(40) {
        object.insert(
            "tools".to_owned(),
            json!([{
                "name": "lookup",
                "description": "look it up",
                "input_schema": {"type": "object", "properties": {}}
            }]),
        );
        if rng.chance(60) {
            object.insert(
                "tool_choice".to_owned(),
                match rng.below(3) {
                    0 => json!({"type": "auto"}),
                    1 => json!({"type": "any"}),
                    _ => json!({"type": "tool", "name": "lookup"}),
                },
            );
        }
    }
    if rng.chance(35) {
        object.insert("temperature".to_owned(), json!(0.5));
    }
    if rng.chance(25) {
        object.insert("top_p".to_owned(), json!(0.9));
    }
    if rng.chance(20) {
        object.insert("stop_sequences".to_owned(), json!(["STOP"]));
    }
    if rng.chance(15) {
        object.insert("metadata".to_owned(), json!({"user_id": "u1"}));
    }
    body
}

/// Where two bodies differ, as a set of tagged paths. Two observations with the
/// same signature are the same class, so a sweep reports classes rather than
/// twenty thousand dumps.
fn difference_signature(left: &Value, right: &Value) -> BTreeSet<String> {
    fn walk(left: &Value, right: &Value, path: &str, out: &mut BTreeSet<String>) {
        match (left, right) {
            (Value::Object(a), Value::Object(b)) => {
                for key in a.keys().chain(b.keys()).collect::<BTreeSet<_>>() {
                    let next = format!("{path}/{key}");
                    match (a.get(key), b.get(key)) {
                        (Some(l), Some(r)) => walk(l, r, &next, out),
                        (Some(_), None) => {
                            out.insert(format!("{next}: only on the passthrough side"));
                        }
                        (None, Some(_)) => {
                            out.insert(format!("{next}: only on the component side"));
                        }
                        (None, None) => {}
                    }
                }
            }
            (Value::Array(a), Value::Array(b)) => {
                if a.len() == b.len() {
                    for (index, (l, r)) in a.iter().zip(b).enumerate() {
                        walk(l, r, &format!("{path}/{index}"), out);
                    }
                } else {
                    out.insert(format!("{path}: array length {} vs {}", a.len(), b.len()));
                }
            }
            _ => {
                if left != right {
                    let kind = |value: &Value| match value {
                        Value::Null => "null",
                        Value::Bool(_) => "bool",
                        Value::Number(_) => "number",
                        Value::String(_) => "string",
                        Value::Array(_) => "array",
                        Value::Object(_) => "object",
                    };
                    out.insert(format!("{path}: {} vs {}", kind(left), kind(right)));
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(left, right, "", &mut out);
    out
}

/// How many bodies the sweep walks. Deliberately a constant: the gate must not
/// depend on wall-clock budget or an environment variable nobody sets in CI.
const SWEEP_CASES: u64 = 200_000;

/// Discovery, not a gate: walks the generated space and reports each *distinct*
/// class that survives every registered eraser, with the seed that reproduces it.
#[test]
#[ignore = "discovery pass; run explicitly to grow the ledger"]
fn sweep_for_unregistered_request_divergences() {
    let mut seen: BTreeMap<String, u64> = BTreeMap::new();
    let mut refusals: BTreeMap<String, u64> = BTreeMap::new();
    for seed in 1..=SWEEP_CASES {
        let body = generated_body(seed);
        match observe(&body) {
            Outcome::Equivalent => {}
            Outcome::Refused(reason) => {
                *refusals.entry(reason).or_insert(0) += 1;
            }
            Outcome::Divergent {
                passthrough,
                component,
            } => {
                let (left, right) = (erased(passthrough), erased(component));
                if left == right {
                    continue;
                }
                let signature = difference_signature(&left, &right)
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(" | ");
                seen.entry(signature).or_insert(seed);
            }
        }
    }
    println!("-- refusals by reason --");
    for (reason, count) in &refusals {
        println!("  {count:>6}  {reason}");
    }
    println!("-- unregistered classes (signature -> first seed) --");
    for (signature, seed) in &seen {
        println!("  seed {seed:>6}  {signature}");
    }
    println!(
        "-- {} unregistered classes over {SWEEP_CASES} generated bodies --",
        seen.len()
    );
}
