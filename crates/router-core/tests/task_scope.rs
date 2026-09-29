//! Public regressions for task difficulty, independent from context admission.
use token_station_protocol::{ChatRequest, Message, Role};
use token_station_router_core::{Heuristic, RequestFeatures};

fn heuristic() -> Heuristic {
    serde_json::from_str(r#"{
      "weights": {"tokens_per_point":100,"per_tool":0,"json_schema":0,"image":0,"per_code_block":8,"per_extra_turn":3,
        "per_reasoning_marker":10,"per_math_term":12,"per_technical_term":8},
      "threshold":22,"above":"high","below":"low"
    }"#).unwrap()
}
fn features(messages: Vec<Message>) -> RequestFeatures {
    RequestFeatures::extract(&ChatRequest::new("auto", messages), &[])
}
fn user(text: &str) -> Message {
    Message::text(Role::User, text)
}

#[test]
fn an_explicit_new_task_does_not_inherit_history_length_fences_or_turns() {
    let task = "Only return the ticket title.";
    let alone = features(vec![user(task)]);
    let history = features(vec![
        Message::text(Role::System, "```text\nscaffolding\n```".repeat(100)),
        user("Analyze distributed consensus and prove the theorem."),
        Message::text(Role::Assistant, "```rust\nfn main() {}\n```".repeat(400)),
        Message::text(Role::Tool, "analyze architecture ".repeat(1000)),
        user(task),
    ]);
    assert_eq!(heuristic().score(&history), heuristic().score(&alone));
    assert!(history.estimated_input_tokens > alone.estimated_input_tokens + 5000);
    assert!(history.code_block_count > alone.code_block_count);
    assert_eq!(history.message_count, 5);
}

#[test]
fn exact_continuations_preserve_the_nearest_substantive_task() {
    let task = "Analyze distributed consensus and prove the theorem.";
    let original = features(vec![user(task)]);
    for cue in ["Continue.", "继续！", "Please continue", "接着", "go on"] {
        let continued = features(vec![
            user(task),
            Message::text(Role::Assistant, "partial"),
            user("继续"),
            Message::text(Role::Assistant, "partial"),
            user(cue),
        ]);
        assert_eq!(
            heuristic().score(&continued),
            heuristic().score(&original),
            "{cue}"
        );
    }
}

#[test]
fn continuation_words_inside_a_new_instruction_do_not_inherit() {
    for task in [
        "Do not continue. Reply OK.",
        "继续，把标题改成已暂停。",
        "Translate the word continue.",
    ] {
        let alone = features(vec![user(task)]);
        let changed = features(vec![
            user("Analyze distributed consensus and prove the theorem."),
            user(task),
        ]);
        assert_eq!(alone.math_term_count, changed.math_term_count);
        assert_eq!(alone.reasoning_marker_count, changed.reasoning_marker_count);
    }
}

#[test]
fn tool_output_size_does_not_replace_the_active_user_task() {
    let task = "Return only the total from the tool result.";
    let alone = features(vec![user(task)]);
    let with_tool = features(vec![
        user(task),
        Message::text(Role::Tool, "```\nprove theorem\n```".repeat(400)),
    ]);
    assert_eq!(heuristic().score(&alone), heuristic().score(&with_tool));
    assert!(with_tool.estimated_input_tokens > alone.estimated_input_tokens);
}

#[test]
fn missing_user_and_legacy_serialized_features_keep_legacy_scoring() {
    let without_user = features(vec![Message::text(Role::Assistant, "x".repeat(4000))]);
    assert_eq!(heuristic().score(&without_user), 10);
    let legacy: RequestFeatures = serde_json::from_str(
        r#"{
      "estimated_input_tokens":2000,"conversation_tokens":1000,"message_count":3,
      "tool_count":0,"has_images":false,"requires_json_schema":false,
      "code_block_count":1,"requested_max_output_tokens":null,"hint_count":0
    }"#,
    )
    .unwrap();
    assert_eq!(heuristic().score(&legacy), 24);
}

#[test]
fn continuations_stop_at_the_latest_task_boundary() {
    let title = "Only return the title.";
    let continued = features(vec![
        user("Prove the theorem."),
        user(title),
        user("continue"),
    ]);
    assert_eq!(
        heuristic().score(&continued),
        heuristic().score(&features(vec![user(title)]))
    );
}

#[test]
fn brevity_does_not_override_reasoning_or_unknown_task_complexity() {
    for task in [
        "Only output JSON after you analyze the architecture and evaluate tradeoffs.",
        "Only return the number after proving the theorem.",
        "Only output a design for distributed consensus.",
        "只输出数值，首先分析约束然后给出证明。",
    ] {
        let result = features(vec![
            Message::text(Role::Tool, "x".repeat(10000)),
            user(task),
        ]);
        assert_eq!(result.task_tokens, None, "{task}");
    }
}

#[test]
fn multipart_scope_keeps_image_and_schema_admission_flags() {
    use token_station_protocol::{Content, ContentPart, ImageUrl, ResponseFormat};
    let mut message = user("unused");
    message.content = Some(Content::Parts(vec![
        ContentPart::Text {
            text: "Only return the title.".into(),
        },
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://example.invalid/test.png".into(),
                detail: None,
            },
        },
    ]));
    let mut request = ChatRequest::new("auto", vec![message]);
    request.response_format = Some(ResponseFormat::JsonObject);
    let result = RequestFeatures::extract(&request, &[]);
    assert!(result.has_images);
    assert!(result.requires_json_schema);
    assert!(result.task_tokens.is_some());
    assert_eq!(
        serde_json::from_value::<RequestFeatures>(serde_json::to_value(result).unwrap()).unwrap(),
        result
    );
}
