//! The gates, and the order they run in.
//!
//! Every check reduces to invoking one adapter function on one fixture input and
//! looking at what came back: turn a case into a closure `input -> output`, then
//! ask the same questions of it.

use serde::Deserialize;
use serde_json::Value;
use token_station_protocol::{AgentRequestEnvelope, ChatResponse, ErrorEnvelope, StreamEvent};

use crate::adapter::AgentAdapter;
use crate::fixture::{AgentFamily, Case, Family, FixturePack};
use crate::report::{Check, Outcome, Report};

/// The key injected to prove a `v1` adapter tolerates a `v2` peer's field.
const UNKNOWN_FIELD: &str = "__conformance_unknown_field";

/// What one adapter invocation produced, with a bad fixture told apart from a
/// bad adapter.
type Invoked = Result<Value, Failure>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// The adapter answered with an error.
    Adapter(String),
    /// The fixture did not deserialize into what the family feeds the adapter.
    /// Not the adapter's fault, and reported as its own reason.
    Fixture(String),
}

impl Failure {
    fn detail(&self) -> String {
        match self {
            Self::Adapter(detail) => format!("adapter returned an error: {detail}"),
            Self::Fixture(detail) => format!("fixture is not valid input: {detail}"),
        }
    }
}

fn adapter_error(error: &ErrorEnvelope) -> Failure {
    Failure::Adapter(format!("{:?}: {}", error.code, error.message))
}

fn parse<T: for<'de> Deserialize<'de>>(input: &Value) -> Result<T, Failure> {
    serde_json::from_value(input.clone()).map_err(|source| Failure::Fixture(source.to_string()))
}

fn encode<T: serde::Serialize>(value: &T) -> Invoked {
    serde_json::to_value(value).map_err(|source| Failure::Fixture(source.to_string()))
}

#[derive(Deserialize)]
struct RenderInput {
    context: Value,
    response: ChatResponse,
}

#[derive(Deserialize)]
struct RenderStreamInput {
    context: Value,
    events: Vec<StreamEvent>,
}

#[derive(Deserialize)]
struct RenderErrorInput {
    context: Value,
    error: ErrorEnvelope,
}

// -- agent --------------------------------------------------------------------

/// Runs `agent-protocol-v1` against a loaded `agent-adapter`.
#[must_use]
pub fn run_agent_suite(adapter: &dyn AgentAdapter, pack: &FixturePack<AgentFamily>) -> Report {
    let mut outcomes = coverage(pack);

    for case in pack.cases() {
        let invoke = |input: &Value| invoke_agent(adapter, case.family, input);
        outcomes.extend(shared_checks(case, &invoke));
    }

    Report::new("agent-protocol-v1", outcomes)
}

fn invoke_agent(adapter: &dyn AgentAdapter, family: AgentFamily, input: &Value) -> Invoked {
    match family {
        AgentFamily::Normalize => {
            let envelope: AgentRequestEnvelope = parse(input)?;
            encode(
                &adapter
                    .normalize_inbound(&envelope)
                    .map_err(|e| adapter_error(&e))?,
            )
        }
        AgentFamily::Hint => {
            let envelope: AgentRequestEnvelope = parse(input)?;
            encode(
                &adapter
                    .extract_agent_hint(&envelope)
                    .map_err(|e| adapter_error(&e))?,
            )
        }
        AgentFamily::Render => {
            let RenderInput { context, response } = parse(input)?;
            encode(
                &adapter
                    .render_response(&response, &context)
                    .map_err(|e| adapter_error(&e))?,
            )
        }
        AgentFamily::Stream => {
            let RenderStreamInput { context, events } = parse(input)?;
            let mut chunks = Vec::with_capacity(events.len());
            for event in &events {
                chunks.push(
                    adapter
                        .render_stream_event(event, &context)
                        .map_err(|e| adapter_error(&e))?,
                );
            }
            encode(&chunks)
        }
        AgentFamily::Error => {
            let RenderErrorInput { context, error } = parse(input)?;
            encode(
                &adapter
                    .map_inbound_error(&error, &context)
                    .map_err(|e| adapter_error(&e))?,
            )
        }
    }
}

// -- shared -------------------------------------------------------------------

fn coverage<F: Family>(pack: &FixturePack<F>) -> Vec<Outcome> {
    let missing = pack.missing_families();
    if missing.is_empty() {
        return vec![Outcome::passed(Check::Coverage, F::KIND)];
    }
    missing
        .into_iter()
        .map(|family| {
            Outcome::failed(
                Check::Coverage,
                format!("{}.{}", F::KIND, family.token()),
                "no fixture exercises this family",
            )
        })
        .collect()
}

fn shared_checks<F: Family>(case: &Case<F>, invoke: &dyn Fn(&Value) -> Invoked) -> Vec<Outcome> {
    let first = invoke(&case.input);

    vec![
        fixture_match(case, &first),
        determinism(case, &first, &invoke(&case.input)),
        unknown_field_tolerance(case, invoke),
    ]
}

fn fixture_match<F: Family>(case: &Case<F>, actual: &Invoked) -> Outcome {
    match actual {
        Err(failure) => Outcome::failed(Check::FixtureMatch, &case.name, failure.detail()),
        Ok(actual) if *actual == case.expected => Outcome::passed(Check::FixtureMatch, &case.name),
        Ok(actual) => Outcome::failed(
            Check::FixtureMatch,
            &case.name,
            format!(
                "expected {}, produced {}",
                truncate(&case.expected),
                truncate(actual)
            ),
        ),
    }
}

fn determinism<F: Family>(case: &Case<F>, first: &Invoked, second: &Invoked) -> Outcome {
    if first == second {
        Outcome::passed(Check::Determinism, &case.name)
    } else {
        Outcome::failed(
            Check::Determinism,
            &case.name,
            "the same input produced different output twice; a suite that admitted this \
             adapter did not observe the adapter the host will run",
        )
    }
}

/// Injects a field this ABI version does not model, and requires the adapter to
/// carry on.
///
/// The field goes *inside* the IR object the family feeds the adapter, not at
/// the top of a wrapper the suite invented, which is why the pointer is per
/// family. A family with nowhere to put one passes without being asked.
fn unknown_field_tolerance<F: Family>(
    case: &Case<F>,
    invoke: &dyn Fn(&Value) -> Invoked,
) -> Outcome {
    let check = Check::UnknownFieldTolerance;
    let Some(pointer) = case.family.unknown_field_pointer() else {
        return Outcome::passed(check, &case.name);
    };

    let mut mutated = case.input.clone();
    let Some(Value::Object(target)) = mutated.pointer_mut(pointer) else {
        return Outcome::failed(
            check,
            &case.name,
            format!("fixture has no object at `{pointer}` to carry an unknown field"),
        );
    };
    target.insert(UNKNOWN_FIELD.to_owned(), Value::Bool(true));

    match invoke(&mutated) {
        Ok(_) => Outcome::passed(check, &case.name),
        Err(failure) => Outcome::failed(
            check,
            &case.name,
            format!(
                "a field this version does not model was refused: {}; a v1 adapter must \
                 degrade in front of a v2 peer, not fail",
                failure.detail()
            ),
        ),
    }
}

/// Keeps a failure readable when the payload is a whole chat response.
fn truncate(value: &Value) -> String {
    let rendered = value.to_string();
    if rendered.chars().count() <= 200 {
        return rendered;
    }
    let head: String = rendered.chars().take(200).collect();
    format!("{head}…")
}
