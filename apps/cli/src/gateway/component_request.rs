//! Host-owned edits to a Canonical request before a provider component sees it.
//!
//! Two request fields carry trust that only the host can grant, so the host
//! settles them here, once, for every provider build:
//!
//! - `host_values` are values the host mints for a component (South KC1: the
//!   per-tenant `end_user_hash`). An agent adapter decodes client JSON, and a
//!   client top-level `host_values` key deserializes into this typed field, so
//!   whatever arrives here is client-controlled. The desktop host mints none,
//!   so it clears them.
//! - Reasoning replay markers ask the upstream to trust reasoning state it
//!   issued earlier. Each family has its own model capability: Claude signed
//!   thinking needs `reasoning_replay.claude.v1`, and `OpenAI` encrypted
//!   reasoning items need `reasoning_replay.openai.v1`. South refuses a
//!   family's markers unless the target model declares its capability, and a
//!   target must not receive state it did not issue. The markers are removed
//!   for such a target, so the component sends the turn without replay instead
//!   of failing it. The `OpenAI` items themselves
//!   (`responses_opaque_reasoning_items`) stay: references read them only
//!   under the marker, and codeassist reads its own thought state from them.

use std::borrow::Cow;

use serde_json::Value;
use token_station_protocol::{ChatRequest, Message, ProviderConfig};

/// The model capability South requires before it replays Claude signed thinking.
pub(super) const CLAUDE_REPLAY_CAPABILITY: &str = "reasoning_replay.claude.v1";
/// The model capability South requires before it replays `OpenAI` encrypted
/// reasoning items. No upstream is granted it yet: no shipped component serves
/// the `OpenAI` Responses family.
const OPENAI_REPLAY_CAPABILITY: &str = "reasoning_replay.openai.v1";
const CLAUDE_REPLAY_FAMILY: &str = "claude-signed-thinking";
const OPENAI_REPLAY_FAMILY: &str = "openai-encrypted-reasoning";
const REPLAY_FAMILY: &str = "reasoning_replay_protocol_family";
const REPLAY_LAYOUT: &str = "reasoning_replay_block_layout";

/// Where Anthropic itself serves Messages. Replay is granted only to an
/// upstream whose endpoint covers this URL: a signature goes back only to the
/// service that issued it, never to an Anthropic-compatible third party.
const OFFICIAL_ANTHROPIC_MESSAGES: &str = "https://api.anthropic.com/v1/messages";

/// Declares Claude signed-thinking replay on every model of an upstream that
/// talks to Anthropic itself through a component that can replay.
///
/// The capability is derived here, at assembly, and never written to the
/// user's configuration: it follows from the endpoint and the admitted
/// component, not from a per-model setting someone has to remember.
pub(super) fn grant_replay(config: &mut ProviderConfig, component_replays: bool) {
    if !component_replays
        || config.provider != "anthropic"
        || !config.base_url.permits(OFFICIAL_ANTHROPIC_MESSAGES)
    {
        return;
    }
    for model in &mut config.models {
        model
            .supported_parameters
            .insert(CLAUDE_REPLAY_CAPABILITY.to_owned());
    }
}

/// The capability a target model must declare before it may receive the
/// markers of `family`. An unknown family has none, so it is never accepted.
fn family_capability(family: &str) -> Option<&'static str> {
    match family {
        CLAUDE_REPLAY_FAMILY => Some(CLAUDE_REPLAY_CAPABILITY),
        OPENAI_REPLAY_FAMILY => Some(OPENAI_REPLAY_CAPABILITY),
        _ => None,
    }
}

fn target_declares(config: &ProviderConfig, request: &ChatRequest, capability: &str) -> bool {
    config.models.iter().any(|model| {
        model.model == request.model && model.supported_parameters.contains(capability)
    })
}

/// Whether `message` carries replay markers the target has not declared it
/// can take. Each family is judged by its own capability: a target that
/// replays Claude signatures must not receive an `OpenAI` turn's markers,
/// which every reference except the `OpenAI` Responses one refuses.
fn strips_markers(config: &ProviderConfig, request: &ChatRequest, message: &Message) -> bool {
    let family = message.extensions.get(REPLAY_FAMILY);
    if family.is_none() && !message.extensions.contains_key(REPLAY_LAYOUT) {
        return false;
    }
    !family
        .and_then(Value::as_str)
        .and_then(family_capability)
        .is_some_and(|capability| target_declares(config, request, capability))
}

/// The request as the target's component may see it.
pub(super) fn seal<'a>(config: &ProviderConfig, request: &'a ChatRequest) -> Cow<'a, ChatRequest> {
    let strip_markers = request
        .messages
        .iter()
        .any(|message| strips_markers(config, request, message));
    if request.host_values.is_empty() && !strip_markers {
        return Cow::Borrowed(request);
    }
    let mut sealed = request.clone();
    sealed.host_values = token_station_protocol::ComponentValues::new();
    for (message, original) in sealed.messages.iter_mut().zip(&request.messages) {
        if strips_markers(config, request, original) {
            message.extensions.remove(REPLAY_FAMILY);
            message.extensions.remove(REPLAY_LAYOUT);
        }
    }
    Cow::Owned(sealed)
}

#[cfg(test)]
mod tests {
    use super::{
        CLAUDE_REPLAY_CAPABILITY, OPENAI_REPLAY_CAPABILITY, REPLAY_FAMILY, REPLAY_LAYOUT,
        grant_replay, seal,
    };
    use serde_json::json;
    use std::borrow::Cow;
    use token_station_protocol::{ChatRequest, Message, ProviderConfig, Role};

    const OPENAI_ITEMS: &str = "responses_opaque_reasoning_items";

    fn config(parameters: &[&str]) -> ProviderConfig {
        serde_json::from_value(json!({
            "provider": "anthropic",
            "base_url": "https://api.anthropic.com/v1",
            "models": [{
                "model": "claude-test",
                "supported_parameters": parameters,
            }],
        }))
        .expect("provider config")
    }

    fn replayed_request() -> ChatRequest {
        let mut assistant = Message::text(Role::Assistant, "answer");
        assistant
            .extensions
            .insert(REPLAY_FAMILY.to_owned(), json!("claude-signed-thinking"));
        assistant.extensions.insert(
            REPLAY_LAYOUT.to_owned(),
            json!([{"kind": "content", "ordinal": 0}]),
        );
        ChatRequest::new(
            "claude-test",
            vec![
                Message::text(Role::User, "question"),
                assistant,
                Message::text(Role::User, "next"),
            ],
        )
    }

    #[test]
    fn a_plain_request_is_not_copied() {
        let request = ChatRequest::new("claude-test", vec![Message::text(Role::User, "hi")]);
        assert!(matches!(seal(&config(&[]), &request), Cow::Borrowed(_)));
    }

    #[test]
    fn markers_reach_a_target_that_declares_replay() {
        let request = replayed_request();
        let sealed = seal(&config(&[CLAUDE_REPLAY_CAPABILITY]), &request);
        assert!(matches!(sealed, Cow::Borrowed(_)));
        assert_eq!(
            sealed.messages[1].extensions[REPLAY_FAMILY],
            "claude-signed-thinking"
        );
    }

    #[test]
    fn markers_are_removed_for_a_target_without_replay() {
        let request = replayed_request();
        let sealed = seal(&config(&["reasoning_effort"]), &request);
        for message in &sealed.messages {
            assert!(!message.extensions.contains_key(REPLAY_FAMILY));
            assert!(!message.extensions.contains_key(REPLAY_LAYOUT));
        }
        assert_eq!(request.messages[1].extensions.len(), 2, "the caller's copy");
    }

    fn openai_turn(request: &mut ChatRequest) {
        let assistant = &mut request.messages[1];
        assistant.extensions.clear();
        assistant.extensions.insert(
            REPLAY_FAMILY.to_owned(),
            json!("openai-encrypted-reasoning"),
        );
        assistant.extensions.insert(
            REPLAY_LAYOUT.to_owned(),
            json!([{"kind": "reasoning_item", "index": 0}, {"kind": "content", "ordinal": 0}]),
        );
        assistant.extensions.insert(
            OPENAI_ITEMS.to_owned(),
            json!([{"type": "reasoning", "id": "rs_1", "summary": [], "encrypted_content": "opaque"}]),
        );
    }

    #[test]
    fn an_openai_turn_loses_its_markers_on_a_claude_replay_target() {
        let mut request = replayed_request();
        openai_turn(&mut request);
        for parameters in [&[CLAUDE_REPLAY_CAPABILITY][..], &[][..]] {
            let sealed = seal(&config(parameters), &request);
            let assistant = &sealed.messages[1];
            assert!(
                !assistant.extensions.contains_key(REPLAY_FAMILY),
                "{parameters:?}"
            );
            assert!(
                !assistant.extensions.contains_key(REPLAY_LAYOUT),
                "{parameters:?}"
            );
            assert_eq!(
                assistant.extensions[OPENAI_ITEMS][0]["encrypted_content"], "opaque",
                "the items stay for a component that reads its own state"
            );
        }
    }

    #[test]
    fn an_openai_turn_keeps_its_markers_on_an_openai_replay_target() {
        let mut request = replayed_request();
        openai_turn(&mut request);
        let sealed = seal(&config(&[OPENAI_REPLAY_CAPABILITY]), &request);
        assert!(matches!(sealed, Cow::Borrowed(_)));
        // The OpenAI capability does not admit a Claude turn.
        let claude = replayed_request();
        let sealed = seal(&config(&[OPENAI_REPLAY_CAPABILITY]), &claude);
        assert!(!sealed.messages[1].extensions.contains_key(REPLAY_FAMILY));
    }

    #[test]
    fn an_unknown_family_or_a_bare_layout_is_removed_everywhere() {
        let everything = [CLAUDE_REPLAY_CAPABILITY, OPENAI_REPLAY_CAPABILITY];
        let mut unknown = replayed_request();
        unknown.messages[1]
            .extensions
            .insert(REPLAY_FAMILY.to_owned(), json!("future-family"));
        let mut bare_layout = replayed_request();
        bare_layout.messages[1].extensions.remove(REPLAY_FAMILY);
        for request in [unknown, bare_layout] {
            let sealed = seal(&config(&everything), &request);
            assert!(!sealed.messages[1].extensions.contains_key(REPLAY_FAMILY));
            assert!(!sealed.messages[1].extensions.contains_key(REPLAY_LAYOUT));
        }
    }

    #[test]
    fn markers_are_removed_when_another_model_declares_replay() {
        let mut request = replayed_request();
        request.model = "claude-other".to_owned();
        let sealed = seal(&config(&[CLAUDE_REPLAY_CAPABILITY]), &request);
        assert!(!sealed.messages[1].extensions.contains_key(REPLAY_FAMILY));
    }

    fn anthropic_config(base_url: &str) -> ProviderConfig {
        let mut config = config(&[]);
        config.base_url = token_station_protocol::ProviderEndpoint::try_new(base_url).unwrap();
        config
    }

    #[test]
    fn replay_is_granted_only_to_anthropic_itself_through_a_replaying_component() {
        for base_url in ["https://api.anthropic.com", "https://api.anthropic.com/v1"] {
            let mut config = anthropic_config(base_url);
            grant_replay(&mut config, true);
            assert!(
                config.models[0]
                    .supported_parameters
                    .contains(CLAUDE_REPLAY_CAPABILITY),
                "{base_url}"
            );
        }
        let mut third_party = anthropic_config("https://api.example.com/anthropic");
        grant_replay(&mut third_party, true);
        let mut old_component = anthropic_config("https://api.anthropic.com");
        grant_replay(&mut old_component, false);
        let mut other_dialect = anthropic_config("https://api.anthropic.com");
        other_dialect.provider = "openai-compatible".to_owned();
        grant_replay(&mut other_dialect, true);
        for config in [third_party, old_component, other_dialect] {
            assert!(config.models[0].supported_parameters.is_empty());
        }
    }

    #[test]
    fn client_host_values_never_reach_a_component() {
        let request: ChatRequest = serde_json::from_value(json!({
            "model": "claude-test",
            "messages": [{"role": "user", "content": "hi"}],
            "host_values": {"end_user_hash": "client-chosen"},
        }))
        .expect("request");
        assert!(!request.host_values.is_empty(), "the client key is typed");
        let sealed = seal(&config(&[CLAUDE_REPLAY_CAPABILITY]), &request);
        assert!(sealed.host_values.is_empty());
        let wire = serde_json::to_value(sealed.as_ref()).expect("wire");
        assert!(wire.get("host_values").is_none());
    }
}
