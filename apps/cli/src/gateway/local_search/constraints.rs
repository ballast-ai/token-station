//! Validate model tool selections before the host starts managed retrieval.
use super::{ErrorCode, ErrorEnvelope};
use serde_json::Value;

fn rejected(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}

fn namespace(item: &Value) -> Result<Option<&str>, ()> {
    match item.get("namespace") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(name)) if !name.is_empty() => Ok(Some(name)),
        _ => Err(()),
    }
}

fn declaration_kind(call: &Value) -> Option<&str> {
    match call["type"].as_str()? {
        "function_call" => Some("function"),
        "custom_tool_call" => Some("custom"),
        "local_shell_call" => Some("local_shell"),
        "tool_search_call" => Some("tool_search"),
        _ => None,
    }
}

fn declared(working: &Value, call: &Value, anthropic: bool) -> bool {
    let Some(tools) = working["tools"].as_array() else {
        return false;
    };
    let Ok(scope) = namespace(call) else {
        return false;
    };
    let name = call["name"].as_str().filter(|name| !name.is_empty());
    if anthropic {
        return call["type"] == "tool_use"
            && scope.is_none()
            && name.is_some_and(|name| tools.iter().any(|tool| tool["name"] == name));
    }
    let Some(kind) = declaration_kind(call) else {
        return false;
    };
    if matches!(kind, "local_shell" | "tool_search") {
        return scope.is_none() && tools.iter().any(|tool| tool["type"] == kind);
    }
    let Some(name) = name else {
        return false;
    };
    tools.iter().any(|tool| {
        if let Some(scope) = scope {
            tool["type"] == "namespace"
                && tool["name"] == scope
                && tool["tools"].as_array().is_some_and(|children| {
                    children
                        .iter()
                        .any(|child| child["type"] == kind && child["name"] == name)
                })
        } else {
            tool["type"] == kind && tool["name"] == name
        }
    })
}

fn selected(choice: &Value, call: &Value, anthropic: bool) -> bool {
    if anthropic {
        return choice["name"]
            .as_str()
            .is_some_and(|name| call["name"] == name);
    }
    let Some(kind) = declaration_kind(call) else {
        return false;
    };
    if choice["type"] != kind {
        return false;
    }
    if matches!(kind, "local_shell" | "tool_search") {
        return true;
    }
    // The original Responses choice is flat. Canonical function choices can
    // also retain the nested OpenAI function shape during a host retry.
    let identity = if choice["function"].is_object() {
        &choice["function"]
    } else {
        choice
    };
    identity["name"]
        .as_str()
        .is_some_and(|name| call["name"] == name)
        && namespace(identity).is_ok_and(|scope| namespace(call) == Ok(scope))
}

pub(super) fn validate(
    working: &Value,
    original: &Value,
    calls: &[Value],
    anthropic: bool,
) -> Result<(), ErrorEnvelope> {
    // Host preparation changes the wire hint. Enforce only the client's
    // explicit parallel prohibition, which must survive every search round.
    let disallow_parallel = if anthropic {
        original["tool_choice"]["disable_parallel_tool_use"] == true
    } else {
        original["parallel_tool_calls"] == false
    };
    if disallow_parallel && calls.len() > 1 {
        return Err(rejected(
            "The model returned parallel tool calls when they were disabled.",
        ));
    }
    if calls.iter().any(|call| !declared(working, call, anthropic)) {
        return Err(rejected(
            "The model returned an undeclared tool or an invalid tool identity.",
        ));
    }
    let choice = &working["tool_choice"];
    let kind = choice.as_str().or_else(|| choice["type"].as_str());
    match kind {
        None if choice.is_null() => Ok(()),
        Some("auto") => Ok(()),
        Some("none") if calls.is_empty() => Ok(()),
        Some("none") => Err(rejected(
            "The model called a tool when tool use was disabled.",
        )),
        Some("required" | "any") if !calls.is_empty() => Ok(()),
        Some("required" | "any") => Err(rejected("The model omitted the required tool call.")),
        Some("tool" | "function" | "custom" | "local_shell" | "tool_search") => {
            if calls.is_empty() || calls.iter().any(|call| !selected(choice, call, anthropic)) {
                return Err(rejected(
                    "The model did not follow the selected tool constraint.",
                ));
            }
            Ok(())
        }
        _ => Err(rejected(
            "The model tool selection cannot be checked against this tool choice.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(anthropic: bool) -> Value {
        if anthropic {
            json!({"tools":[{"name":"token_station_browser_search","input_schema":{"type":"object"}},{"name":"edit","input_schema":{"type":"object"}}],"tool_choice":{"type":"auto"}})
        } else {
            json!({"tools":[{"type":"function","name":"token_station_browser_search","parameters":{"type":"object"}},{"type":"function","name":"edit","parameters":{"type":"object"}}],"tool_choice":"auto"})
        }
    }

    fn call(name: &str, anthropic: bool) -> Value {
        if anthropic {
            json!({"type":"tool_use","name":name,"id":format!("call_{name}"),"input":{}})
        } else {
            json!({"type":"function_call","name":name,"call_id":format!("call_{name}"),"arguments":"{}"})
        }
    }

    fn rejected(working: &Value, original: &Value, calls: &[Value], anthropic: bool) {
        let error =
            validate(working, original, calls, anthropic).expect_err("must reject tool selection");
        assert_eq!(error.code, ErrorCode::ProviderProtocolError);
        assert_eq!(error.http_status, 502);
        assert!(!error.message.contains("private_tool_name"));
    }

    #[test]
    fn none_rejects_every_call_but_accepts_no_calls() {
        for anthropic in [false, true] {
            let mut body = request(anthropic);
            body["tool_choice"] = if anthropic {
                json!({"type":"none"})
            } else {
                json!("none")
            };
            rejected(
                &body,
                &body,
                &[call("token_station_browser_search", anthropic)],
                anthropic,
            );
            assert!(validate(&body, &body, &[], anthropic).is_ok());
        }
    }

    #[test]
    fn forced_search_rejects_mixed_client_calls_and_absent_calls() {
        for anthropic in [false, true] {
            let mut body = request(anthropic);
            body["tool_choice"] = json!({"type":if anthropic {"tool"} else {"function"},"name":"token_station_browser_search"});
            let search = call("token_station_browser_search", anthropic);
            assert!(validate(&body, &body, std::slice::from_ref(&search), anthropic).is_ok());
            rejected(&body, &body, &[search, call("edit", anthropic)], anthropic);
            rejected(&body, &body, &[], anthropic);
        }
    }

    #[test]
    fn every_call_must_match_a_declared_tool_and_wire_type() {
        for anthropic in [false, true] {
            let body = request(anthropic);
            rejected(
                &body,
                &body,
                &[call("private_tool_name", anthropic)],
                anthropic,
            );
            let mut wrong_type = call("edit", anthropic);
            wrong_type["type"] = json!(if anthropic {
                "function_call"
            } else {
                "custom_tool_call"
            });
            rejected(&body, &body, &[wrong_type], anthropic);
            rejected(
                &body,
                &body,
                &[json!({"type":"function_call","name":null})],
                anthropic,
            );
        }
    }

    #[test]
    fn explicit_parallel_prohibition_survives_host_normalization() {
        for anthropic in [false, true] {
            let working = request(anthropic);
            let calls = [
                call("token_station_browser_search", anthropic),
                call("edit", anthropic),
            ];
            assert!(validate(&working, &working, &calls, anthropic).is_ok());
            let mut original = working.clone();
            if anthropic {
                original["tool_choice"]["disable_parallel_tool_use"] = json!(true);
            } else {
                original["parallel_tool_calls"] = json!(false);
            }
            rejected(&working, &original, &calls, anthropic);
            assert!(validate(&working, &original, &calls[..1], anthropic).is_ok());
        }
    }

    #[test]
    fn namespace_and_custom_tools_require_exact_identity_and_kind() {
        let body = json!({"tools":[{"type":"namespace","name":"files","tools":[{"type":"function","name":"read"},{"type":"custom","name":"patch"}]},{"type":"custom","name":"edit"}],"tool_choice":"auto"});
        for (name, kind, namespace) in [
            ("read", "function_call", Some("files")),
            ("patch", "custom_tool_call", Some("files")),
            ("edit", "custom_tool_call", None),
        ] {
            let valid = json!({"type":kind,"name":name,"namespace":namespace,"call_id":"call_fixture","input":"raw","arguments":"{}"});
            assert!(validate(&body, &body, std::slice::from_ref(&valid), false).is_ok());
            for replacement in [json!("other"), json!(42)] {
                let mut invalid = valid.clone();
                invalid["namespace"] = replacement;
                rejected(&body, &body, &[invalid], false);
            }
            let mut invalid = valid.clone();
            invalid["type"] = json!(if kind == "function_call" {
                "custom_tool_call"
            } else {
                "function_call"
            });
            rejected(&body, &body, &[invalid], false);
        }
        rejected(&body, &body, &[call("read", false)], false);
        rejected(&body, &body, &[call("files__read", false)], false);
    }

    #[test]
    fn required_and_any_need_at_least_one_call() {
        for anthropic in [false, true] {
            let mut body = request(anthropic);
            body["tool_choice"] = if anthropic {
                json!({"type":"any"})
            } else {
                json!("required")
            };
            rejected(&body, &body, &[], anthropic);
            assert!(validate(&body, &body, &[call("edit", anthropic)], anthropic).is_ok());
        }
    }

    #[test]
    fn later_auto_round_can_return_a_client_call_after_forced_search() {
        for anthropic in [false, true] {
            let working = request(anthropic);
            let mut original = working.clone();
            original["tool_choice"] = json!({"type":if anthropic {"tool"} else {"function"},"name":"token_station_browser_search"});
            assert!(validate(&working, &original, &[call("edit", anthropic)], anthropic).is_ok());
            assert!(validate(&working, &original, &[], anthropic).is_ok());
        }
    }

    #[test]
    fn named_namespace_and_custom_choices_do_not_accept_other_tools() {
        let mut body = json!({"tools":[{"type":"namespace","name":"files","tools":[{"type":"function","name":"read"}]},{"type":"custom","name":"edit"}],"tool_choice":{"type":"function","namespace":"files","name":"read"}});
        let read = json!({"type":"function_call","namespace":"files","name":"read"});
        let edit = json!({"type":"custom_tool_call","name":"edit"});
        assert!(validate(&body, &body, std::slice::from_ref(&read), false).is_ok());
        rejected(&body, &body, std::slice::from_ref(&edit), false);
        body["tool_choice"] = json!({"type":"custom","name":"edit"});
        assert!(validate(&body, &body, &[edit], false).is_ok());
        rejected(&body, &body, &[read], false);
    }

    #[test]
    fn builtin_call_families_require_their_own_declarations() {
        let body =
            json!({"tools":[{"type":"local_shell"},{"type":"tool_search"}],"tool_choice":"auto"});
        for kind in ["local_shell_call", "tool_search_call"] {
            let call = json!({"type":kind,"call_id":"call_builtin"});
            assert!(validate(&body, &body, std::slice::from_ref(&call), false).is_ok());
            rejected(&request(false), &request(false), &[call], false);
        }
    }
}
