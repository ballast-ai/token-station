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
//! - Claude signed-thinking replay markers ask the upstream to trust a
//!   signature. South refuses the markers unless the target model declares
//!   `reasoning_replay.claude.v1`, and a target without it must not receive a
//!   signature it did not issue. The markers are removed for such a target, so
//!   the component sends the turn without replay instead of failing it.

use std::borrow::Cow;

use token_station_protocol::{ChatRequest, ProviderConfig};

/// The model capability South requires before it replays Claude signed thinking.
pub(super) const CLAUDE_REPLAY_CAPABILITY: &str = "reasoning_replay.claude.v1";
const REPLAY_FAMILY: &str = "reasoning_replay_protocol_family";
const REPLAY_LAYOUT: &str = "reasoning_replay_block_layout";

fn target_accepts_replay(config: &ProviderConfig, request: &ChatRequest) -> bool {
    config.models.iter().any(|model| {
        model.model == request.model
            && model
                .supported_parameters
                .contains(CLAUDE_REPLAY_CAPABILITY)
    })
}

fn carries_replay_markers(request: &ChatRequest) -> bool {
    request.messages.iter().any(|message| {
        message.extensions.contains_key(REPLAY_FAMILY)
            || message.extensions.contains_key(REPLAY_LAYOUT)
    })
}

/// The request as the target's component may see it.
pub(super) fn seal<'a>(config: &ProviderConfig, request: &'a ChatRequest) -> Cow<'a, ChatRequest> {
    let strip_markers = carries_replay_markers(request) && !target_accepts_replay(config, request);
    if request.host_values.is_empty() && !strip_markers {
        return Cow::Borrowed(request);
    }
    let mut sealed = request.clone();
    sealed.host_values = token_station_protocol::ComponentValues::new();
    if strip_markers {
        for message in &mut sealed.messages {
            message.extensions.remove(REPLAY_FAMILY);
            message.extensions.remove(REPLAY_LAYOUT);
        }
    }
    Cow::Owned(sealed)
}

#[cfg(test)]
mod tests {
    use super::{CLAUDE_REPLAY_CAPABILITY, REPLAY_FAMILY, REPLAY_LAYOUT, seal};
    use serde_json::json;
    use std::borrow::Cow;
    use token_station_protocol::{ChatRequest, Message, ProviderConfig, Role};

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

    #[test]
    fn markers_are_removed_when_another_model_declares_replay() {
        let mut request = replayed_request();
        request.model = "claude-other".to_owned();
        let sealed = seal(&config(&[CLAUDE_REPLAY_CAPABILITY]), &request);
        assert!(!sealed.messages[1].extensions.contains_key(REPLAY_FAMILY));
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
