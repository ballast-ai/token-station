//! The gates, and the order they run in.
//!
//! Every check reduces to invoking one adapter function on one fixture input and
//! looking at what came back, so the two suites share a shape: turn a case into a
//! closure `input -> output`, then ask the same questions of it. Only the
//! questions that need a *typed* view of the input or the output — endpoint
//! confinement, auth-error retriability, stream incrementality — reach past that
//! closure.

use serde::Deserialize;
use serde_json::Value;
use token_station_protocol::{
    AgentRequestEnvelope, ChatRequest, ChatResponse, ErrorEnvelope, HttpRequestDescriptor,
    HttpResponseParts, ProviderConfig, StreamChunk, StreamEvent,
};

use crate::adapter::{AgentAdapter, ProviderAdapter, StreamParser};
use crate::fixture::{AgentFamily, Case, Family, FixturePack, ProviderFamily};
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
struct RequestInput {
    provider_config: ProviderConfig,
    chat_request: ChatRequest,
}

#[derive(Deserialize)]
struct StreamInput {
    chunks: Vec<String>,
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

// -- provider -----------------------------------------------------------------

/// Runs `provider-protocol-v1` against a loaded `provider-adapter`.
///
/// Never panics on a bad adapter or a bad fixture: both become failures in the
/// report, because a host running this at install time must not be taken down by
/// the package it is vetting.
#[must_use]
pub fn run_provider_suite(
    adapter: &dyn ProviderAdapter,
    pack: &FixturePack<ProviderFamily>,
) -> Report {
    let mut outcomes = coverage(pack);
    let mut a_credential_was_rejected_somewhere = false;

    for case in pack.cases() {
        let invoke = |input: &Value| invoke_provider(adapter, case.family, input);

        outcomes.extend(shared_checks(case, &invoke));

        match case.family {
            ProviderFamily::Request => {
                outcomes.push(endpoint_confinement(case, &invoke(&case.input)));
            }
            ProviderFamily::Error => {
                if let Some(outcome) = auth_errors_are_not_retriable(case, &invoke(&case.input)) {
                    a_credential_was_rejected_somewhere = true;
                    outcomes.push(outcome);
                }
            }
            ProviderFamily::Stream => {
                outcomes.push(stream_incrementality(adapter, case));
            }
            ProviderFamily::Capabilities | ProviderFamily::Response => {}
        }
    }

    // A gate that never runs describes nothing. Without a fixture that rejects a
    // credential, an adapter passes `AuthErrorsAreNotRetriable` by never being
    // asked — so the missing fixture is itself the failure.
    if !a_credential_was_rejected_somewhere {
        outcomes.push(Outcome::failed(
            Check::AuthErrorsAreNotRetriable,
            "provider.error",
            "no fixture maps a 401 or 403, so the check that keeps a rejected credential from \
             being replayed across every configured upstream never ran",
        ));
    }

    Report::new("provider-protocol-v1", outcomes)
}

fn invoke_provider(
    adapter: &dyn ProviderAdapter,
    family: ProviderFamily,
    input: &Value,
) -> Invoked {
    match family {
        ProviderFamily::Capabilities => {
            let config: ProviderConfig = parse(input)?;
            encode(
                &adapter
                    .model_capabilities(&config)
                    .map_err(|e| adapter_error(&e))?,
            )
        }
        ProviderFamily::Request => {
            let RequestInput {
                provider_config,
                chat_request,
            } = parse(input)?;
            encode(
                &adapter
                    .build_http_request(&chat_request, &provider_config)
                    .map_err(|e| adapter_error(&e))?,
            )
        }
        ProviderFamily::Response => {
            let parts: HttpResponseParts = parse(input)?;
            encode(
                &adapter
                    .parse_response(&parts)
                    .map_err(|e| adapter_error(&e))?,
            )
        }
        ProviderFamily::Error => {
            let parts: HttpResponseParts = parse(input)?;
            encode(
                &adapter
                    .map_provider_error(&parts)
                    .map_err(|e| adapter_error(&e))?,
            )
        }
        ProviderFamily::Stream => {
            let StreamInput { chunks } = parse(input)?;
            encode(&feed(adapter.stream_parser().as_mut(), &chunks)?)
        }
    }
}

fn feed(
    parser: &mut dyn StreamParser,
    chunks: &[impl AsRef<str>],
) -> Result<Vec<StreamEvent>, Failure> {
    let mut events = Vec::new();
    for chunk in chunks {
        let chunk = StreamChunk {
            data: chunk.as_ref().to_owned(),
        };
        events.extend(
            parser
                .parse_chunk(&chunk)
                .map_err(|error| adapter_error(&error))?,
        );
    }
    Ok(events)
}

/// The check that makes [`ProviderConfig::authorize`] a gate rather than a
/// suggestion.
///
/// Run against what the adapter *built*, not against the fixture's expected
/// descriptor. An adapter that fails `FixtureMatch` and confines its request is
/// broken; one that matches the fixture and does not is dangerous, and the two
/// must be distinguishable in the report.
fn endpoint_confinement(case: &Case<ProviderFamily>, built: &Invoked) -> Outcome {
    let check = Check::EndpointConfinement;

    let input: RequestInput = match parse(&case.input) {
        Ok(input) => input,
        Err(failure) => return Outcome::failed(check, &case.name, failure.detail()),
    };
    let built = match built {
        Ok(built) => built,
        Err(failure) => return Outcome::failed(check, &case.name, failure.detail()),
    };
    let descriptor: HttpRequestDescriptor = match parse(built) {
        Ok(descriptor) => descriptor,
        Err(failure) => return Outcome::failed(check, &case.name, failure.detail()),
    };

    match input.provider_config.authorize(&descriptor) {
        Ok(()) => Outcome::passed(check, &case.name),
        Err(refusal) => Outcome::failed(check, &case.name, refusal.to_string()),
    }
}

/// A rejected credential must never be retried on another upstream.
///
/// `None` when the case says nothing about credentials. Only `401` and `403`
/// unambiguously do: a `429` may legitimately map to `RateLimit` or `Capacity`,
/// both retriable, and the fixture already pins which.
fn auth_errors_are_not_retriable(case: &Case<ProviderFamily>, mapped: &Invoked) -> Option<Outcome> {
    let check = Check::AuthErrorsAreNotRetriable;

    let status = case.input.get("status").and_then(Value::as_u64)?;
    if !matches!(status, 401 | 403) {
        return None;
    }

    let Ok(mapped) = mapped else {
        return Some(Outcome::failed(
            check,
            &case.name,
            "could not map the error to inspect",
        ));
    };
    let envelope: ErrorEnvelope = match parse(mapped) {
        Ok(envelope) => envelope,
        Err(failure) => return Some(Outcome::failed(check, &case.name, failure.detail())),
    };

    if envelope.code.is_retriable_elsewhere() {
        return Some(Outcome::failed(
            check,
            &case.name,
            format!(
                "status {status} mapped to `{:?}`, which the router retries on another upstream; \
                 a rejected credential would be replayed across every configured provider",
                envelope.code
            ),
        ));
    }
    Some(Outcome::passed(check, &case.name))
}

/// However the body is split, the events must be the same.
///
/// The fixture's own chunking is one arbitrary split among many; the network
/// picks a different one every time. So the body is rejoined and re-split at
/// every character boundary, each time through a fresh parser, and every run
/// must reproduce the expected events. Splitting at index `0` also feeds an
/// empty chunk, which a parser must tolerate rather than treat as end-of-stream.
fn stream_incrementality(adapter: &dyn ProviderAdapter, case: &Case<ProviderFamily>) -> Outcome {
    let check = Check::StreamIncrementality;

    let StreamInput { chunks } = match parse(&case.input) {
        Ok(input) => input,
        Err(failure) => return Outcome::failed(check, &case.name, failure.detail()),
    };
    let body: String = chunks.concat();

    let splits = body
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(body.len()));

    for split in splits {
        let (head, tail) = body.split_at(split);
        let events = match feed(adapter.stream_parser().as_mut(), &[head, tail]) {
            Ok(events) => events,
            Err(failure) => {
                return Outcome::failed(
                    check,
                    &case.name,
                    format!("split at byte {split}: {}", failure.detail()),
                );
            }
        };

        let encoded = match encode(&events) {
            Ok(encoded) => encoded,
            Err(failure) => return Outcome::failed(check, &case.name, failure.detail()),
        };
        if encoded != case.expected {
            return Outcome::failed(
                check,
                &case.name,
                format!(
                    "split at byte {split} produced different events; a chunk off a socket is \
                     not a whole frame"
                ),
            );
        }
    }

    Outcome::passed(check, &case.name)
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
