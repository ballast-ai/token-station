use serde::{Deserialize, Serialize};
use token_station_protocol::{AgentHint, ChatRequest, Content, ContentPart, ResponseFormat, Role};

use crate::lexicon;

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
    /// Dims 2–10 of the ported V1 complexity heuristic (V2-P2-3): hit counts
    /// against the crate's fixed bilingual vocabulary (`lexicon`), scanned
    /// over the LAST user message only — V1's scope, kept so conversation
    /// history cannot inflate the score. Counts, never text.
    #[serde(default)]
    pub reasoning_marker_count: u32,
    #[serde(default)]
    pub technical_term_count: u32,
    /// Hits on the easy-request list. The only count that argues DOWN.
    #[serde(default)]
    pub simple_indicator_count: u32,
    /// Code vocabulary hits — the fence-less complement to `code_block_count`.
    #[serde(default)]
    pub code_keyword_count: u32,
    #[serde(default)]
    pub math_term_count: u32,
    #[serde(default)]
    pub creative_term_count: u32,
    /// Multi-step framing strength: 10 = numbered list / "step N", 6 =
    /// first-then sequencing, 0 = none. Points, so the weight stays integer.
    #[serde(default)]
    pub multi_step_signal: u32,
    /// Question marks (ASCII and fullwidth) in the last user message.
    #[serde(default)]
    pub question_count: u32,
    /// Structured-output vocabulary appears in SYSTEM text (V1 dim 10).
    #[serde(default)]
    pub system_format_hint: bool,
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

        let user_text = last_user_text(request).to_lowercase();
        let system_text = system_text(request).to_lowercase();

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
            reasoning_marker_count: lexicon::count_matches(&user_text, lexicon::REASONING_MARKERS),
            technical_term_count: lexicon::count_matches(&user_text, lexicon::TECHNICAL_TERMS),
            simple_indicator_count: lexicon::count_matches(&user_text, lexicon::SIMPLE_INDICATORS),
            code_keyword_count: lexicon::count_matches(&user_text, lexicon::CODE_KEYWORDS),
            math_term_count: lexicon::count_matches(&user_text, lexicon::MATH_TERMS),
            creative_term_count: lexicon::count_matches(&user_text, lexicon::CREATIVE_TERMS),
            multi_step_signal: multi_step_signal(&user_text),
            question_count: truncate(
                user_text.matches('?').count() + user_text.matches('\u{ff1f}').count(),
            ),
            system_format_hint: lexicon::count_matches(&system_text, lexicon::FORMAT_TERMS) > 0,
        }
    }
}

/// The text of the LAST user message — the request being asked now, which is
/// what V1 scored. Parts are concatenated; non-text parts contribute nothing.
fn last_user_text(request: &ChatRequest) -> String {
    for message in request.messages.iter().rev() {
        if message.role != Role::User {
            continue;
        }
        return match message.content.as_ref() {
            Some(Content::Text(text)) => text.clone(),
            Some(Content::Parts(parts)) => {
                let mut text = String::new();
                for part in parts {
                    if let ContentPart::Text { text: t } = part {
                        text.push_str(t);
                        text.push(' ');
                    }
                }
                text
            }
            None => String::new(),
        };
    }
    String::new()
}

/// All system-role text, concatenated (V1 dim 10 scans instructions only).
fn system_text(request: &ChatRequest) -> String {
    let mut text = String::new();
    for message in &request.messages {
        if message.role != Role::System {
            continue;
        }
        match message.content.as_ref() {
            Some(Content::Text(t)) => {
                text.push_str(t);
                text.push(' ');
            }
            Some(Content::Parts(parts)) => {
                for part in parts {
                    if let ContentPart::Text { text: t } = part {
                        text.push_str(t);
                        text.push(' ');
                    }
                }
            }
            None => {}
        }
    }
    text
}

/// V1 dim 6: 10 for explicit numbered/step framing, 6 for first-then
/// sequencing, 0 otherwise. Both languages count.
fn multi_step_signal(user_text: &str) -> u32 {
    // Substring, not word-boundary: "1." tokenizes to "1" under the word
    // splitter, which made V1's numbered-list check dead code. Fixed in the
    // port — a decimal like "3.14" can false-positive, which a heuristic
    // survives; a check that never fires cannot.
    let numbered = user_text.contains("1.") && user_text.contains("2.");
    let step_words = lexicon::has_phrase(user_text, "step 1")
        || lexicon::has_phrase(user_text, "step 2")
        || user_text.contains("第一步")
        || user_text.contains("第二步");
    if numbered || step_words {
        return 10;
    }
    let sequenced = (lexicon::has_phrase(user_text, "first")
        && lexicon::has_phrase(user_text, "then"))
        || (user_text.contains("首先") && user_text.contains("然后"));
    if sequenced { 6 } else { 0 }
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
    fn ported_dimensions_score_the_last_user_message_only() {
        let features = RequestFeatures::extract(
            &request(vec![
                // History full of scary vocabulary that must NOT count.
                Message::text(Role::User, "prove the theorem about distributed consensus"),
                Message::text(Role::Assistant, "done"),
                // The message being asked now.
                Message::text(
                    Role::User,
                    "First analyze the architecture, then evaluate the tradeoffs. \
                     What breaks? What scales? Any deadlock risk?",
                ),
            ]),
            &[],
        );
        assert_eq!(features.reasoning_marker_count, 2); // analyze, evaluate
        assert!(features.technical_term_count >= 2); // architecture, deadlock
        assert_eq!(features.math_term_count, 0); // "prove"/"theorem" are history
        assert_eq!(features.multi_step_signal, 6); // first … then
        assert_eq!(features.question_count, 3);
    }

    #[test]
    fn ported_dimensions_read_chinese_vocabulary() {
        let features = RequestFeatures::extract(
            &request(vec![
                Message::text(Role::System, "所有输出必须是结构化 JSON 格式"),
                Message::text(Role::User, "请一步一步推理,证明这个定理,并分析利弊?"),
            ]),
            &[],
        );
        assert!(features.reasoning_marker_count >= 3); // step-by-step, reasoning, analysis, pros and cons
        assert!(features.math_term_count >= 2); // proof and theorem
        assert!(features.system_format_hint);
        assert_eq!(features.question_count, 1); // fullwidth ?
    }

    #[test]
    fn simple_talk_and_numbered_steps_are_seen() {
        let easy = RequestFeatures::extract(
            &request(vec![Message::text(
                Role::User,
                "hello, what is rust? thanks",
            )]),
            &[],
        );
        assert!(easy.simple_indicator_count >= 2);

        let stepped = RequestFeatures::extract(
            &request(vec![Message::text(
                Role::User,
                "please do this: 1. parse the file 2. sort the rows",
            )]),
            &[],
        );
        assert_eq!(stepped.multi_step_signal, 10);
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
