//! Buffered stop-sequence compatibility for the public plan transport.

use std::borrow::Cow;
use token_station_protocol::{
    ChatRequest, ChatResponse, Content, ErrorCode, ErrorEnvelope, FinishReason,
};

fn active(provider: &str, request: &ChatRequest) -> bool {
    provider == "chatgpt-plan" && !request.sampling.stop.is_empty()
}

pub(super) fn prepare<'a>(
    provider: &str,
    request: &'a ChatRequest,
) -> Result<Cow<'a, ChatRequest>, ErrorEnvelope> {
    if !active(provider, request) {
        return Ok(Cow::Borrowed(request));
    }
    if request.stream || !request.tools.is_empty() {
        return Err(ErrorEnvelope::new(
            ErrorCode::Capability,
            400,
            "ChatGPT plan stop sequences require a buffered request without tools.",
        ));
    }
    if request.sampling.stop.len() > 16
        || request.sampling.stop.iter().any(String::is_empty)
        || request.sampling.stop.iter().map(String::len).sum::<usize>() > 4096
    {
        return Err(ErrorEnvelope::new(
            ErrorCode::Capability,
            400,
            "Stop sequences must be nonempty, with at most 16 entries and 4096 total bytes.",
        ));
    }
    // The original request stays available at the response boundary. No stop
    // parameter is sent to an endpoint that cannot enforce it natively.
    let mut projected = request.clone();
    projected.sampling.stop.clear();
    Ok(Cow::Owned(projected))
}

pub(super) fn apply(
    provider: &str,
    request: &ChatRequest,
    response: &mut ChatResponse,
) -> Result<(), ErrorEnvelope> {
    if !active(provider, request) {
        return Ok(());
    }
    // Called only after the provider confirms response.completed. A matching
    // delimiter cannot hide an upstream refusal, failure, or truncated stream.
    for choice in &mut response.choices {
        if !choice.message.tool_calls.is_empty() {
            return Err(ErrorEnvelope::new(
                ErrorCode::ProviderProtocolError,
                502,
                "A buffered stop request returned unexpected tool calls.",
            ));
        }
        let Some(Content::Text(text)) = &mut choice.message.content else {
            return Err(ErrorEnvelope::new(
                ErrorCode::ProviderProtocolError,
                502,
                "A buffered stop request returned non-text content.",
            ));
        };
        if let Some((index, stop)) = request
            .sampling
            .stop
            .iter()
            .filter_map(|stop| text.find(stop).map(|index| (index, stop)))
            .min_by_key(|(index, _)| *index)
        {
            text.truncate(index);
            choice.finish_reason = Some(FinishReason::StopSequence);
            choice.stop_sequence = Some(stop.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use token_station_protocol::{Content, ErrorCode, FinishReason};

    fn request() -> ChatRequest {
        serde_json::from_value(json!({"model":"test","messages":[{"role":"user","content":"Classify this synthetic action."}],"sampling":{"stop":["</block>","</severity>"]}})).unwrap()
    }
    fn response(text: &str) -> ChatResponse {
        serde_json::from_value(json!({"id":"r","model":"test","choices":[{"index":0,"message":{"role":"assistant","content":text},"finish_reason":"stop"}],"usage":{"input_tokens":100,"output_tokens":30}})).unwrap()
    }

    #[test]
    fn strips_only_the_plan_copy_and_preserves_instructions() {
        let request = request();
        let prepared = prepare("chatgpt-plan", &request).unwrap();
        assert!(prepared.sampling.stop.is_empty());
        assert_eq!(prepared.messages, request.messages);
        assert_eq!(request.sampling.stop.len(), 2);
        assert!(matches!(
            prepare("anthropic", &request).unwrap(),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn preserves_both_classifier_decisions_and_upstream_usage() {
        for decision in ["yes", "no"] {
            let mut response = response(&format!("<block>{decision}</block>ignored"));
            let usage = response.usage;
            apply("chatgpt-plan", &request(), &mut response).unwrap();
            let choice = &response.choices[0];
            assert_eq!(
                choice.message.content,
                Some(Content::Text(format!("<block>{decision}")))
            );
            assert_eq!(choice.finish_reason, Some(FinishReason::StopSequence));
            assert_eq!(choice.stop_sequence.as_deref(), Some("</block>"));
            assert_eq!(response.usage, usage);
        }
    }

    #[test]
    fn earliest_match_is_unicode_safe_and_absent_match_preserves_output() {
        let mut response = response("中文<severity>0</severity></block>");
        apply("chatgpt-plan", &request(), &mut response).unwrap();
        assert_eq!(
            response.choices[0].message.content,
            Some(Content::Text("中文<severity>0".into()))
        );
        assert_eq!(
            response.choices[0].stop_sequence.as_deref(),
            Some("</severity>")
        );
        let mut absent = self::response("ordinary answer");
        let original = absent.clone();
        apply("chatgpt-plan", &request(), &mut absent).unwrap();
        assert_eq!(absent, original);
    }

    #[test]
    fn unsupported_request_shapes_fail_closed() {
        let mut request = request();
        request.stream = true;
        assert_eq!(
            prepare("chatgpt-plan", &request).unwrap_err().code,
            ErrorCode::Capability
        );
        request.stream = false;
        request.tools = serde_json::from_value(json!([{"name":"read","parameters":{}}])).unwrap();
        assert!(prepare("chatgpt-plan", &request).is_err());
        request.tools.clear();
        request.sampling.stop = vec![String::new()];
        assert!(prepare("chatgpt-plan", &request).is_err());
        request.sampling.stop = vec!["x".repeat(4097)];
        assert!(prepare("chatgpt-plan", &request).is_err());
        request.sampling.stop = vec!["x".into(); 17];
        assert!(prepare("chatgpt-plan", &request).is_err());
    }

    #[test]
    fn unexpected_tools_and_nontext_output_fail_closed() {
        let mut response = response("<block>no</block>");
        response.choices[0].message.tool_calls =
            serde_json::from_value(json!([{"id":"c","name":"read","arguments":"{}"}])).unwrap();
        assert_eq!(
            apply("chatgpt-plan", &request(), &mut response)
                .unwrap_err()
                .code,
            ErrorCode::ProviderProtocolError
        );
        response.choices[0].message.tool_calls.clear();
        response.choices[0].message.content = Some(Content::Parts(vec![]));
        assert!(apply("chatgpt-plan", &request(), &mut response).is_err());
    }
}
