//! Explicit text-and-tools boundary for Responses clients on Anthropic native search routes.

use super::{ErrorCode, ErrorEnvelope, MAX_UPSTREAM_BODY, Value};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn unsupported(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::Capability, 400, message)
}
fn invalid(message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCode::ProviderProtocolError, 502, message)
}

fn text(value: &Value) -> Result<String, ErrorEnvelope> {
    if let Some(text) = value.as_str() {
        return Ok(text.into());
    }
    let mut parts = Vec::new();
    for block in value
        .as_array()
        .ok_or_else(|| unsupported("The reverse search bridge requires text content."))?
    {
        if !matches!(
            block["type"].as_str(),
            Some("text" | "input_text" | "output_text")
        ) {
            return Err(unsupported(
                "The reverse search bridge supports text and direct functions only.",
            ));
        }
        parts.push(
            block["text"]
                .as_str()
                .ok_or_else(|| unsupported("Search text content is invalid."))?
                .to_owned(),
        );
    }
    Ok(parts.join("\n"))
}

fn declared_function(request: &Value, name: &str) -> bool {
    request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|tool| tool["type"] == "function" && tool["name"] == name)
}

fn allows_call(request: &Value, name: &str, search: bool) -> bool {
    let choice = &request["tool_choice"];
    if choice == "none" {
        return false;
    }
    if choice.is_object() {
        if matches!(
            choice["type"].as_str(),
            Some("web_search" | "web_search_preview")
        ) {
            return search;
        }
        if choice["type"] == "function" {
            return !search && choice["name"] == name;
        }
        return false;
    }
    true
}

#[allow(clippy::too_many_lines)] // Keep the ordered wire lifecycle and validation together.
pub(super) fn to_anthropic(request: &Value) -> Result<Value, ErrorEnvelope> {
    let object = request
        .as_object()
        .ok_or_else(|| unsupported("Search request must be an object."))?;
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "model"
                | "input"
                | "instructions"
                | "tools"
                | "tool_choice"
                | "max_tool_calls"
                | "max_output_tokens"
                | "stream"
                | "parallel_tool_calls"
                | "temperature"
                | "top_p"
                | "store"
                | "include"
                | "reasoning"
        ) {
            return Err(unsupported(
                "The reverse search bridge cannot preserve one or more request fields.",
            ));
        }
    }
    if request.get("store").is_some_and(|value| value != false) {
        return Err(unsupported(
            "The reverse search bridge requires store=false or an omitted store field.",
        ));
    }
    if request.get("include").is_some_and(|value| {
        !value.as_array().is_some_and(|items| {
            items
                .iter()
                .all(|item| item == "web_search_call.action.sources")
        })
    }) {
        return Err(unsupported(
            "The reverse search bridge supports search sources in include only.",
        ));
    }
    let tools = request["tools"]
        .as_array()
        .ok_or_else(|| unsupported("Search tools are missing."))?;
    let searches: Vec<_> = tools
        .iter()
        .filter(|tool| super::local_search::hosted(tool, false))
        .collect();
    if searches.len() != 1 {
        return Err(unsupported(
            "The reverse bridge requires one hosted search declaration.",
        ));
    }
    let search = searches[0];
    if search.as_object().is_none_or(|fields| {
        fields.keys().any(|key| {
            !matches!(
                key.as_str(),
                "type" | "filters" | "user_location" | "external_web_access"
            )
        })
    }) {
        return Err(unsupported(
            "The reverse search bridge cannot preserve these search options.",
        ));
    }
    if search
        .get("external_web_access")
        .is_some_and(|value| value != true)
    {
        return Err(unsupported(
            "Anthropic native search cannot provide cached-only Responses search.",
        ));
    }
    let mut hosted = json!({"type":"web_search_20250305","name":"web_search"});
    if let Some(filters) = search.get("filters") {
        if !filters
            .as_object()
            .is_some_and(|fields| fields.keys().all(|key| key == "allowed_domains"))
        {
            return Err(unsupported(
                "The reverse bridge supports allowed_domains only.",
            ));
        }
        let domains = filters["allowed_domains"]
            .as_array()
            .filter(|items| {
                items.len() <= 100
                    && items
                        .iter()
                        .all(|item| item.as_str().is_some_and(|s| !s.is_empty()))
            })
            .ok_or_else(|| {
                unsupported("Search allowed_domains must be a bounded array of names.")
            })?;
        hosted["allowed_domains"] = json!(domains);
    }
    if let Some(location) = search.get("user_location") {
        if !location.is_object() || location["type"] != "approximate" {
            return Err(unsupported("Search location must be approximate."));
        }
        hosted["user_location"] = location.clone();
    }
    if let Some(limit) = request.get("max_tool_calls") {
        if limit.as_u64().is_none_or(|limit| limit == 0) {
            return Err(unsupported("Search call limit must be positive."));
        }
        hosted["max_uses"] = limit.clone();
    }
    let mut translated_tools = vec![hosted];
    for tool in tools
        .iter()
        .filter(|tool| !super::local_search::hosted(tool, false))
    {
        if tool["type"] != "function"
            || tool["name"]
                .as_str()
                .is_none_or(|name| name.is_empty() || name == "web_search")
            || !tool["parameters"].is_object()
            || tool.get("strict").is_some_and(|value| value != false)
            || tool.as_object().is_none_or(|fields| {
                fields.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "type" | "name" | "description" | "parameters" | "strict"
                    )
                })
            })
        {
            return Err(unsupported(
                "The reverse bridge requires ordinary non-strict client functions with unique names.",
            ));
        }
        translated_tools.push(json!({"name":tool["name"],"description":tool["description"].as_str().unwrap_or(""),"input_schema":tool["parameters"]}));
    }
    let mut names = BTreeSet::new();
    if translated_tools
        .iter()
        .any(|tool| !names.insert(tool["name"].as_str().unwrap_or("")))
    {
        return Err(unsupported("Search tool names must be unique."));
    }
    let mut system = Vec::new();
    if let Some(instructions) = request.get("instructions") {
        system.push(text(instructions)?);
    }
    let mut messages = Vec::new();
    if let Some(input) = request["input"].as_str() {
        messages.push(json!({"role":"user","content":input}));
    } else {
        for item in request["input"]
            .as_array()
            .ok_or_else(|| unsupported("Search input must be text or an array."))?
        {
            match item["type"].as_str() {
                None | Some("message") => {
                    let content = text(&item["content"])?;
                    match item["role"].as_str() {
                        Some("system" | "developer") => system.push(content),
                        Some("user" | "assistant") => messages.push(json!({"role":item["role"],"content":content})),
                        _ => return Err(unsupported("Search message role is unsupported.")),
                    }
                }
                Some("function_call") => {
                    let name = item["name"].as_str().unwrap_or("");
                    if !declared_function(request, name) { return Err(unsupported("Search history references an undeclared client function.")); }
                    let input: Value = serde_json::from_str(item["arguments"].as_str().unwrap_or("")).map_err(|_| unsupported("Search history function arguments are invalid."))?;
                    if !input.is_object() || item["call_id"].as_str().is_none_or(str::is_empty) { return Err(unsupported("Search history function call is invalid.")); }
                    messages.push(json!({"role":"assistant","content":[{"type":"tool_use","id":item["call_id"],"name":name,"input":input}]}));
                }
                Some("function_call_output") => {
                    if item["call_id"].as_str().is_none_or(str::is_empty) { return Err(unsupported("Search history function result ID is missing.")); }
                    messages.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":item["call_id"],"content":text(&item["output"])?}]}));
                }
                Some("web_search_call") => messages.push(json!({"role":"assistant","content":format!("Previous search data: {}", item["action"])})),
                _ => return Err(unsupported("The reverse search bridge cannot preserve this input item.")),
            }
        }
    }
    if messages.is_empty() {
        return Err(unsupported("Search messages are missing."));
    }
    let choice = &request["tool_choice"];
    let mut translated_choice = match choice.as_str() {
        Some("auto") | None if choice.is_null() || choice == "auto" => json!({"type":"auto"}),
        Some("required") => json!({"type":"any"}),
        Some("none") => json!({"type":"none"}),
        _ if matches!(
            choice["type"].as_str(),
            Some("web_search" | "web_search_preview")
        ) =>
        {
            json!({"type":"tool","name":"web_search"})
        }
        _ if choice["type"] == "function"
            && choice["name"]
                .as_str()
                .is_some_and(|name| declared_function(request, name)) =>
        {
            json!({"type":"tool","name":choice["name"]})
        }
        _ => {
            return Err(unsupported(
                "Search tool_choice cannot be represented on this route.",
            ));
        }
    };
    if let Some(parallel) = request.get("parallel_tool_calls") {
        translated_choice["disable_parallel_tool_use"] = json!(
            !parallel
                .as_bool()
                .ok_or_else(|| unsupported("parallel_tool_calls must be a boolean."))?
        );
    }
    let max_tokens = request.get("max_output_tokens").map_or(Ok(4096), |value| {
        value
            .as_u64()
            .filter(|value| *value > 0)
            .ok_or_else(|| unsupported("max_output_tokens must be positive."))
    })?;
    let mut result = json!({"model":request["model"],"messages":messages,"tools":translated_tools,"tool_choice":translated_choice,"max_tokens":max_tokens,"stream":request["stream"].as_bool().unwrap_or(false)});
    if let Some(reasoning) = request.get("reasoning") {
        if reasoning != &json!({"effort":"none"}) {
            return Err(unsupported(
                "The reverse search bridge supports reasoning effort none only.",
            ));
        }
        result["thinking"] = json!({"type":"disabled"});
    }
    if !system.is_empty() {
        result["system"] = json!(system.join("\n"));
    }
    for field in ["temperature", "top_p"] {
        if let Some(value) = request.get(field) {
            result[field] = value.clone();
        }
    }
    Ok(result)
}

pub(super) fn response_id(body: &Value) -> String {
    format!("resp_{}", body["id"].as_str().unwrap_or("search"))
}
pub(super) fn message_id(body: &Value, index: usize) -> String {
    format!("{}_text_{index}", body["id"].as_str().unwrap_or("search"))
}

#[allow(clippy::too_many_lines)] // Keep the ordered wire lifecycle and validation together.
pub(super) fn from_anthropic(body: &Value, request: &Value) -> Result<Value, ErrorEnvelope> {
    if body["type"] != "message"
        || !matches!(
            body["stop_reason"].as_str(),
            Some("end_turn" | "stop_sequence" | "tool_use" | "max_tokens" | "refusal")
        )
    {
        return Err(invalid(
            "Native search did not return a supported completed message.",
        ));
    }
    let content = body["content"]
        .as_array()
        .filter(|blocks| blocks.len() <= 4096)
        .ok_or_else(|| invalid("Native search content is missing or too large."))?;
    let mut entries = content.len();
    for block in content {
        if !block["citations"].is_null() && !block["citations"].is_array() {
            return Err(invalid("Native search citations must be an array."));
        }
        entries = entries.saturating_add(block["citations"].as_array().map_or(0, Vec::len));
        if block["type"] == "web_search_tool_result" {
            entries = entries.saturating_add(block["content"].as_array().map_or(0, Vec::len));
        }
        if entries > 4096 {
            return Err(invalid("Native search has too many evidence items."));
        }
    }
    let mut output = Vec::new();
    let mut searches = BTreeMap::new();
    let mut answered = BTreeSet::new();
    let mut text_parts = Vec::new();
    for (ci, block) in content.iter().enumerate() {
        match block["type"].as_str() {
            Some("server_tool_use") => {
                let id = block["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| invalid("Native search call ID is missing."))?;
                if block["name"] != "web_search"
                    || !allows_call(request, "web_search", true)
                    || searches.contains_key(id)
                {
                    return Err(invalid(
                        "Native search returned an unauthorized or duplicate hosted call.",
                    ));
                }
                let query = block["input"]["query"]
                    .as_str()
                    .ok_or_else(|| invalid("Native search query is missing."))?;
                searches.insert(id.to_owned(), output.len());
                if request["max_tool_calls"]
                    .as_u64()
                    .is_some_and(|limit| searches.len() as u64 > limit)
                {
                    return Err(invalid("Native search exceeded the requested call limit."));
                }
                output.push(json!({"type":"web_search_call","id":id,"status":"in_progress","action":{"type":"search","query":query,"sources":[]}}));
            }
            Some("web_search_tool_result") => {
                let id = block["tool_use_id"].as_str().unwrap_or("");
                let index = searches
                    .get(id)
                    .ok_or_else(|| invalid("Native search result refers to an unknown call."))?;
                if !answered.insert(id.to_owned()) {
                    return Err(invalid("Native search returned duplicate tool results."));
                }
                if let Some(results) = block["content"].as_array() {
                    let sources: Vec<_> = results.iter().filter_map(|source| {
                        let url = source["url"].as_str().and_then(crate::search::public_url)?;
                        Some(json!({"type":"url","url":url,"title":source["title"].as_str().unwrap_or(&url)}))
                    }).collect();
                    output[*index]["status"] = json!("completed");
                    output[*index]["action"]["sources"] = json!(sources);
                } else if block["content"]["type"] == "web_search_tool_result_error" {
                    output[*index]["status"] = json!("failed");
                    output[*index]["error"] = block["content"].clone();
                } else {
                    return Err(invalid("Native search result content is invalid."));
                }
            }
            Some("text") => {
                let text = block["text"]
                    .as_str()
                    .ok_or_else(|| invalid("Native search text is invalid."))?;
                let mut annotations = Vec::new();
                for citation in block["citations"].as_array().into_iter().flatten() {
                    if citation["type"] != "web_search_result_location" {
                        return Err(invalid(
                            "Native search returned an unsupported citation type.",
                        ));
                    }
                    let url = citation["url"]
                        .as_str()
                        .and_then(crate::search::public_url)
                        .ok_or_else(|| invalid("Native search citation URL is invalid."))?;
                    annotations.push(json!({"type":"url_citation","url":url,"title":citation["title"].as_str().unwrap_or(&url),"start_index":0,"end_index":text.chars().count()}));
                }
                output.push(json!({"type":"message","id":message_id(body, ci),"role":"assistant","status":if body["stop_reason"] == "max_tokens" {"incomplete"} else {"completed"},"content":[{"type":"output_text","text":text,"annotations":annotations}]}));
                text_parts.push(text);
            }
            Some("tool_use") => {
                let name = block["name"].as_str().unwrap_or("");
                if !declared_function(request, name)
                    || !allows_call(request, name, false)
                    || !block["input"].is_object()
                    || block["id"].as_str().is_none_or(str::is_empty)
                {
                    return Err(invalid(
                        "Native search returned an unauthorized or invalid client function.",
                    ));
                }
                output.push(json!({"type":"function_call","id":block["id"],"call_id":block["id"],"name":name,"arguments":block["input"].to_string(),"status":"completed"}));
            }
            _ => {
                return Err(invalid(
                    "Native search returned an unsupported content block.",
                ));
            }
        }
    }
    if searches.len() != answered.len() || output.is_empty() {
        return Err(invalid(
            "Native search returned incomplete tool results or no output.",
        ));
    }
    let usage = &body["usage"];
    let usage = match (
        usage["input_tokens"].as_u64(),
        usage["output_tokens"].as_u64(),
    ) {
        (Some(input), Some(out)) => {
            let cached = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            let input = input
                .saturating_add(cached)
                .saturating_add(usage["cache_creation_input_tokens"].as_u64().unwrap_or(0));
            json!({"input_tokens":input,"input_tokens_details":{"cached_tokens":cached},"output_tokens":out,"total_tokens":input.saturating_add(out)})
        }
        _ => Value::Null,
    };
    let result = json!({"id":response_id(body),"object":"response","model":request["model"],"status":if body["stop_reason"] == "max_tokens" {"incomplete"} else {"completed"},"output":output,"output_text":text_parts.join("\n"),"usage":usage,"error":null,"incomplete_details":if body["stop_reason"] == "max_tokens" {json!({"reason":"max_output_tokens"})} else {Value::Null}});
    if result.to_string().len() as u64 > MAX_UPSTREAM_BODY {
        return Err(invalid(
            "Converted native search response exceeds the size limit.",
        ));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Value {
        json!({"model":"auto","input":"Find Rust","stream":true,"tools":[{"type":"web_search","filters":{"allowed_domains":["docs.rs"]}}],"tool_choice":{"type":"web_search"},"max_tool_calls":2,"max_output_tokens":1000})
    }
    fn answer() -> Value {
        json!({"id":"msg_test","type":"message","stop_reason":"end_turn","content":[{"type":"server_tool_use","id":"srv_1","name":"web_search","input":{"query":"Rust"}},{"type":"web_search_tool_result","tool_use_id":"srv_1","content":[{"type":"web_search_result","url":"https://docs.rs/","title":"Docs","encrypted_content":"opaque"}]},{"type":"text","text":"Rust docs","citations":[{"type":"web_search_result_location","url":"https://docs.rs/","title":"Docs","cited_text":"External quote","encrypted_index":"opaque"}]}],"usage":{"input_tokens":10,"cache_read_input_tokens":4,"cache_creation_input_tokens":2,"output_tokens":3}})
    }

    #[test]
    fn reverse_response_rejects_malformed_or_excessive_evidence() {
        let mut body = answer();
        body["content"][2]["citations"] = json!({"url":"https://docs.rs/"});
        assert!(from_anthropic(&body, &request()).is_err());
        let mut body = answer();
        body["content"][1]["content"] = json!(vec![
            json!({"type":"web_search_result","url":"https://docs.rs/"});
            4097
        ]);
        assert!(from_anthropic(&body, &request()).is_err());
    }

    #[test]
    fn reverse_request_preserves_explicit_disabled_reasoning() {
        let mut request = request();
        request["reasoning"] = json!({"effort":"none"});
        assert_eq!(
            to_anthropic(&request).unwrap()["thinking"],
            json!({"type":"disabled"})
        );
        request["reasoning"] = json!({"effort":"high"});
        assert!(to_anthropic(&request).is_err());
    }

    #[test]
    fn reverse_request_preserves_choice_filters_limits_and_streaming() {
        let body = to_anthropic(&request()).unwrap();
        assert_eq!(body["stream"], true);
        assert_eq!(body["tools"][0]["allowed_domains"], json!(["docs.rs"]));
        assert_eq!(body["tools"][0]["max_uses"], 2);
        assert_eq!(
            body["tool_choice"],
            json!({"type":"tool","name":"web_search"})
        );
        assert_eq!(body["max_tokens"], 1000);
    }

    #[test]
    fn reverse_request_rejects_unrepresentable_capabilities() {
        for (field, value) in [
            ("previous_response_id", json!("resp_old")),
            ("store", json!(true)),
            ("reasoning", json!({"effort":"high"})),
            ("text", json!({"format":{"type":"json_schema"}})),
        ] {
            let mut body = request();
            body[field] = value;
            assert!(to_anthropic(&body).is_err(), "{field}");
        }
        let mut body = request();
        body["input"] = json!([{"role":"user","content":[{"type":"input_image","image_url":"https://example.com/a.png"}]}]);
        assert!(to_anthropic(&body).is_err());
    }

    #[test]
    fn reverse_response_preserves_sources_citations_usage_and_failures() {
        let body = from_anthropic(&answer(), &request()).unwrap();
        assert_eq!(body["status"], "completed");
        assert_eq!(
            body["output"][0]["action"]["sources"][0]["url"],
            "https://docs.rs/"
        );
        assert_eq!(
            body["output"][1]["content"][0]["annotations"][0]["url"],
            "https://docs.rs/"
        );
        assert_eq!(body["usage"]["input_tokens"], 16);
        assert_eq!(body["usage"]["total_tokens"], 19);
        let mut failed = answer();
        failed["content"][1]["content"] =
            json!({"type":"web_search_tool_result_error","error_code":"too_many_requests"});
        assert_eq!(
            from_anthropic(&failed, &request()).unwrap()["output"][0]["status"],
            "failed"
        );
        failed["content"][1]["tool_use_id"] = json!("unknown");
        assert!(from_anthropic(&failed, &request()).is_err());
    }

    #[test]
    fn reverse_history_and_client_functions_remain_executable_only_when_declared() {
        let mut body = request();
        body["tools"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"function","name":"edit","parameters":{"type":"object"}}));
        body["tool_choice"] = json!("auto");
        body["input"] = json!([{"role":"user","content":"Search"},{"type":"function_call","call_id":"c1","name":"edit","arguments":"{}"},{"type":"function_call_output","call_id":"c1","output":"done"}]);
        let converted = to_anthropic(&body).unwrap();
        assert_eq!(converted["messages"][1]["content"][0]["type"], "tool_use");
        let mut answer = answer();
        answer["content"]
            .as_array_mut()
            .unwrap()
            .push(json!({"type":"tool_use","id":"c2","name":"edit","input":{}}));
        answer["stop_reason"] = json!("tool_use");
        assert_eq!(
            from_anthropic(&answer, &body).unwrap()["output"][2]["type"],
            "function_call"
        );
        assert!(from_anthropic(&answer, &request()).is_err());
    }
}
