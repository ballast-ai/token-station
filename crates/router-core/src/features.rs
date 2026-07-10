use serde::{Deserialize, Serialize};
use token_station_protocol::{AgentHint, ChatRequest, Content, ContentPart, ResponseFormat};

/// What the router looked at, and the only thing it remembers having looked at.
///
/// # Why this type is `Copy`
///
/// `String` is not `Copy`. Deriving `Copy` here therefore makes it *impossible*
/// for this struct to come to own prompt text: the moment someone adds a
/// `keyword: String` or a `first_message: String`, the derive stops compiling.
///
/// That is not a stylistic preference. [`crate::Decision`] embeds these features
/// and is what the metrics store persists and what the cloud-sync whitelist is
/// later drawn from. The client's promise is that request content never leaves
/// the device; a promise enforced by code review lasts until the first tired
/// afternoon. Removing this derive would silently reopen the door, so it is
/// asserted in a test as well.
///
/// The router *reads* content — the heuristic counts code fences, and a keyword
/// rule scans message text. That is allowed and local. What it may not do is
/// carry any of it forward.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFeatures {
    /// See [`estimate_tokens`] for what "estimated" is worth.
    pub estimated_input_tokens: u32,
    pub message_count: u32,
    pub tool_count: u32,
    pub has_images: bool,
    pub requires_json_schema: bool,
    /// Fenced code blocks, counted as pairs of triple-backtick fences.
    pub code_block_count: u32,
    pub requested_max_output_tokens: Option<u32>,
    pub hint_count: u32,
}

impl RequestFeatures {
    /// Reads the request once, and keeps only numbers.
    #[must_use]
    pub fn extract(request: &ChatRequest, hints: &[AgentHint]) -> Self {
        let mut estimated_input_tokens: u32 = 0;
        let mut code_fences: u32 = 0;
        let mut has_images = false;

        for message in &request.messages {
            match message.content.as_ref() {
                Some(Content::Text(text)) => {
                    estimated_input_tokens =
                        estimated_input_tokens.saturating_add(estimate_tokens(text));
                    code_fences = code_fences.saturating_add(count_fences(text));
                }
                Some(Content::Parts(parts)) => {
                    for part in parts {
                        match part {
                            ContentPart::Text { text } => {
                                estimated_input_tokens =
                                    estimated_input_tokens.saturating_add(estimate_tokens(text));
                                code_fences = code_fences.saturating_add(count_fences(text));
                            }
                            ContentPart::ImageUrl { .. } => has_images = true,
                        }
                    }
                }
                None => {}
            }
            for call in &message.tool_calls {
                estimated_input_tokens =
                    estimated_input_tokens.saturating_add(estimate_tokens(&call.arguments));
            }
        }

        Self {
            estimated_input_tokens,
            message_count: truncate(request.messages.len()),
            tool_count: truncate(request.tools.len()),
            has_images,
            requires_json_schema: matches!(
                request.response_format,
                Some(ResponseFormat::JsonObject | ResponseFormat::JsonSchema { .. })
            ),
            code_block_count: code_fences / 2,
            requested_max_output_tokens: request.sampling.max_output_tokens,
            hint_count: truncate(hints.len()),
        }
    }
}

/// A cheap, deliberately crude token estimate.
///
/// Four ASCII characters per token, one token per non-ASCII character. That is
/// roughly right for English and for CJK, and wrong for plenty else.
///
/// Accuracy is not the job. This number decides which upstream a request goes
/// to, and it must therefore produce the *same* answer in the local client and
/// in the server gateway — which is why it lives in this crate rather than being
/// re-invented on each side. The true count comes back from the provider in
/// [`token_station_protocol::Usage`], and that is what billing uses.
///
/// Loading a real tokenizer here would mean shipping one per model family and
/// re-deciding routing every time a vendor changes theirs.
#[must_use]
pub fn estimate_tokens(text: &str) -> u32 {
    let mut ascii: u32 = 0;
    let mut wide: u32 = 0;
    for character in text.chars() {
        if character.is_ascii() {
            ascii = ascii.saturating_add(1);
        } else {
            wide = wide.saturating_add(1);
        }
    }
    ascii.div_ceil(4).saturating_add(wide)
}

fn count_fences(text: &str) -> u32 {
    truncate(text.matches("```").count())
}

fn truncate(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{RequestFeatures, estimate_tokens};
    use token_station_protocol::{
        AgentHint, ChatRequest, Content, ContentPart, HintKind, ImageUrl, Message, ResponseFormat,
        Role, ToolDef,
    };

    fn request(messages: Vec<Message>) -> ChatRequest {
        ChatRequest::new("gpt-5.5", messages)
    }

    #[test]
    fn features_cannot_come_to_carry_prompt_text() {
        // `String` is not `Copy`. If this stops compiling, someone added a text
        // field to the one type the decision record embeds.
        const fn assert_copy<T: Copy>() {}
        assert_copy::<RequestFeatures>();
    }

    #[test]
    fn token_estimate_treats_wide_characters_as_one_token_each() {
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2, "rounds up rather than down");
        assert_eq!(estimate_tokens("北京天气"), 4);
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn code_fences_are_counted_in_pairs() {
        let features = RequestFeatures::extract(
            &request(vec![Message::text(
                Role::User,
                "before ```rust\nfn main() {}\n``` after ```py\npass\n```",
            )]),
            &[],
        );

        assert_eq!(features.code_block_count, 2);
    }

    #[test]
    fn an_unclosed_fence_is_not_a_code_block() {
        let features =
            RequestFeatures::extract(&request(vec![Message::text(Role::User, "```rust")]), &[]);

        assert_eq!(features.code_block_count, 0);
    }

    #[test]
    fn multimodal_parts_are_seen_through() {
        let message = Message {
            content: Some(Content::Parts(vec![
                ContentPart::Text {
                    text: "describe".to_owned(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "https://example/cat.png".to_owned(),
                        detail: None,
                    },
                },
            ])),
            ..Message::text(Role::User, "")
        };
        let features = RequestFeatures::extract(&request(vec![message]), &[]);

        assert!(features.has_images);
        assert_eq!(features.estimated_input_tokens, 2);
    }

    #[test]
    fn tools_json_schema_and_hints_are_counted() {
        let mut chat = request(vec![Message::text(Role::User, "hi")]);
        chat.tools = vec![ToolDef {
            name: "get_weather".to_owned(),
            description: None,
            parameters: serde_json::json!({}),
        }];
        chat.response_format = Some(ResponseFormat::JsonObject);

        let features =
            RequestFeatures::extract(&chat, &[AgentHint::new(HintKind::StepType, "planning")]);

        assert_eq!(features.tool_count, 1);
        assert!(features.requires_json_schema);
        assert_eq!(features.hint_count, 1);
        assert_eq!(features.message_count, 1);
    }
}
