use super::*;

fn constrained_runtime() -> PluginRuntime {
    PluginRuntime::new(RuntimeLimits {
        memory_bytes: 16 * 1024 * 1024,
        call_timeout: Duration::from_secs(5),
    })
    .unwrap()
}

fn request(content: &str) -> AgentRequestEnvelope {
    envelope(
        "openai-chat-completions",
        json!({"model":"fixture","messages":[{"role":"user","content":content}]}),
    )
}

fn protocol_request(protocol: &str, content: &str) -> AgentRequestEnvelope {
    envelope(
        protocol,
        if protocol == "openai-responses" {
            json!({"model":"fixture","input":content})
        } else {
            json!({"model":"fixture","max_tokens":128,"messages":[{"role":"user","content":content}]})
        },
    )
}

fn context(protocol: &str, id: &str) -> serde_json::Value {
    json!({"protocol":protocol,"stream_id":id,"response_id":format!("response-{id}"),"model":"fixture"})
}

fn delta(content: &str) -> StreamEvent {
    StreamEvent::Delta {
        index: 0,
        content: content.into(),
    }
}

fn done() -> StreamEvent {
    StreamEvent::Done {
        finish_reason: Some(FinishReason::Stop),
        stop_sequence: None,
    }
}

#[test]
fn a_trapped_official_agent_recovers_matching_and_normalization() {
    let plugin = AgentPlugin::load(&constrained_runtime(), agent_package()).unwrap();
    let large = request(&"x".repeat(8 * 1024 * 1024));
    let failure = plugin.normalize_inbound(&large).unwrap_err();
    assert_eq!(failure.code, ErrorCode::Internal);
    assert_eq!(
        failure.message,
        "adapter trapped or exceeded a resource limit"
    );
    let matched =
        plugin.match_inbound(&json!({"method":"POST","path":"/v1/chat/completions","headers":{}}));
    assert!(
        matched.is_ok(),
        "A trapped guest poisoned subsequent matching: {matched:?}"
    );
    assert!(matched.unwrap().matched);
    let normalized = plugin.normalize_inbound(&request("small")).unwrap();
    assert_eq!(normalized.model, "fixture");
    let large_head = json!({"path":"/v1/chat/completions","noise":"x".repeat(8 * 1024 * 1024)});
    assert_eq!(
        plugin.match_inbound(&large_head).unwrap_err().code,
        ErrorCode::Internal
    );
    plugin
        .normalize_inbound(&request("small after match trap"))
        .unwrap();
}

#[test]
fn a_trapped_terminal_event_cannot_restart_the_old_stream() {
    let plugin = AgentPlugin::load(&constrained_runtime(), responses_agent_package()).unwrap();
    let context = json!({"protocol":"openai-responses","stream_id":"terminal-trap","response_id":"response-terminal-trap","model":"fixture"});
    plugin
        .render_stream_event(
            &StreamEvent::Delta {
                index: 0,
                content: "before trap".into(),
            },
            &context,
        )
        .unwrap();
    let failure = plugin
        .render_stream_event(
            &StreamEvent::Done {
                finish_reason: Some(FinishReason::Stop),
                stop_sequence: Some("x".repeat(8 * 1024 * 1024)),
            },
            &context,
        )
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::Internal);
    plugin
        .normalize_inbound(&envelope(
            "openai-responses",
            json!({"model":"fixture","input":"small"}),
        ))
        .unwrap();
    let terminal = plugin.render_stream_event(
        &StreamEvent::Done {
            finish_reason: Some(FinishReason::Stop),
            stop_sequence: None,
        },
        &context,
    );
    assert!(
        terminal.is_err(),
        "The reset stream reported success without its old history: {terminal:?}"
    );
    assert_eq!(terminal.unwrap_err().code, ErrorCode::TransportTruncated);
}

#[test]
fn stream_epochs_survive_domain_errors_but_not_traps() {
    for (package, protocol, start) in [
        (
            responses_agent_package(),
            "openai-responses",
            "response.created",
        ),
        (
            anthropic_agent_package(),
            "anthropic-messages",
            "message_start",
        ),
    ] {
        let plugin = AgentPlugin::load(&constrained_runtime(), package).unwrap();
        let first = context(protocol, "domain-error");
        plugin.render_stream_event(&delta("a1"), &first).unwrap();
        let domain_error = plugin
            .normalize_inbound(&envelope(protocol, json!({})))
            .unwrap_err();
        assert_eq!(domain_error.code, ErrorCode::InvalidRequest);
        let continued = plugin.render_stream_event(&delta("a2"), &first).unwrap();
        assert!(!continued["data"].as_str().unwrap().contains(start));
        let terminal = plugin.render_stream_event(&done(), &first).unwrap();
        if protocol == "openai-responses" {
            assert!(terminal["data"].as_str().unwrap().contains("a1a2"));
        }
        let second = context(protocol, "stale-delta");
        let third = context(protocol, "stale-terminal");
        plugin.render_stream_event(&delta("b1"), &second).unwrap();
        plugin.render_stream_event(&delta("c1"), &third).unwrap();
        let failure = plugin
            .normalize_inbound(&protocol_request(protocol, &"x".repeat(8 * 1024 * 1024)))
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::Internal);
        plugin
            .normalize_inbound(&protocol_request(protocol, "small"))
            .unwrap();
        for (event, old) in [
            (delta("lost"), &second),
            (done(), &third),
            (done(), &second),
        ] {
            assert_eq!(
                plugin.render_stream_event(&event, old).unwrap_err().code,
                ErrorCode::TransportTruncated
            );
        }
        let fresh = context(protocol, "fresh");
        let output = plugin
            .render_stream_event(&delta("fresh text"), &fresh)
            .unwrap();
        assert!(output["data"].as_str().unwrap().contains(start));
        let output = plugin.render_stream_event(&done(), &fresh).unwrap();
        assert!(!output["data"].as_str().unwrap().contains("b1"));
    }
}

#[test]
fn active_stream_limits_do_not_evict_previous_epochs() {
    let plugin = AgentPlugin::load(&constrained_runtime(), agent_package()).unwrap();
    for sequence in 0..1024 {
        plugin
            .render_stream_event(
                &delta("x"),
                &context("openai-chat-completions", &format!("active-{sequence}")),
            )
            .unwrap();
    }
    let extra = context("openai-chat-completions", "over-limit");
    assert_eq!(
        plugin
            .render_stream_event(&delta("x"), &extra)
            .unwrap_err()
            .code,
        ErrorCode::Capability
    );
    let oversized = context("openai-chat-completions", &"x".repeat(1025));
    assert_eq!(
        plugin
            .render_stream_event(&delta("x"), &oversized)
            .unwrap_err()
            .code,
        ErrorCode::InvalidRequest
    );
    let previous = context("openai-chat-completions", "active-0");
    plugin
        .render_stream_event(&delta("still live"), &previous)
        .unwrap();
    plugin.render_stream_event(&done(), &previous).unwrap();
    plugin.render_stream_event(&delta("x"), &extra).unwrap();
    plugin
        .normalize_inbound(&request(&"x".repeat(8 * 1024 * 1024)))
        .unwrap_err();
    plugin.normalize_inbound(&request("small")).unwrap();
    let fresh = context("openai-chat-completions", "after-trap");
    assert_eq!(
        plugin
            .render_stream_event(&delta("x"), &fresh)
            .unwrap_err()
            .code,
        ErrorCode::Capability
    );
    let stale = context("openai-chat-completions", "active-1");
    assert_eq!(
        plugin
            .render_stream_event(&done(), &stale)
            .unwrap_err()
            .code,
        ErrorCode::TransportTruncated
    );
    plugin.render_stream_event(&delta("fresh"), &fresh).unwrap();
}

#[test]
fn unidentified_streams_fail_closed_after_instance_recovery() {
    let plugin = AgentPlugin::load(&constrained_runtime(), agent_package()).unwrap();
    let anonymous = json!({"model":"fixture","response_id":"anonymous-response"});
    plugin
        .render_stream_event(&delta("before"), &anonymous)
        .unwrap();
    plugin
        .normalize_inbound(&request(&"x".repeat(8 * 1024 * 1024)))
        .unwrap_err();
    plugin.normalize_inbound(&request("small")).unwrap();
    for _ in 0..2 {
        assert_eq!(
            plugin
                .render_stream_event(&done(), &anonymous)
                .unwrap_err()
                .code,
            ErrorCode::TransportTruncated
        );
    }
    plugin
        .render_stream_event(
            &delta("new identified stream"),
            &context("openai-chat-completions", "identified"),
        )
        .unwrap();
}
