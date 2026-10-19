//! Reversible function names at provider request boundaries.
use super::{ChatRequest, json};

/// Alias after normalization, including names flattened from Responses namespaces.
pub(super) fn alias_client_tools(
    request: &mut ChatRequest,
) -> std::collections::BTreeMap<String, String> {
    use sha2::{Digest, Sha256};
    let mut occupied: std::collections::BTreeSet<String> = request
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .chain(
            request
                .messages
                .iter()
                .flat_map(|message| message.tool_calls.iter())
                .map(|call| call.name.clone()),
        )
        .collect();
    let originals = occupied.clone();
    let mut forward = std::collections::BTreeMap::new();
    for name in originals {
        if name.len() <= 64
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            continue;
        }
        let digest = format!("{:x}", Sha256::digest(name.as_bytes()));
        let base = format!("ts_alias_{}", &digest[..40]);
        let mut alias = base.clone();
        let mut suffix = 0_u32;
        while occupied.contains(&alias) {
            suffix += 1;
            alias = format!("{base}_{suffix}");
        }
        occupied.insert(alias.clone());
        forward.insert(name, alias);
    }
    for tool in &mut request.tools {
        if let Some(alias) = forward.get(&tool.name) {
            tool.name.clone_from(alias);
        }
    }
    if let Some(strictness) = request
        .extensions
        .get_mut("responses_tool_strict")
        .and_then(serde_json::Value::as_object_mut)
    {
        *strictness = std::mem::take(strictness)
            .into_iter()
            .map(|(name, value)| (forward.get(&name).cloned().unwrap_or(name), value))
            .collect();
    }
    for message in &mut request.messages {
        for call in &mut message.tool_calls {
            if let Some(alias) = forward.get(&call.name) {
                call.name.clone_from(alias);
            }
        }
    }
    if let Some(token_station_protocol::ToolChoice::Other(choice)) = &mut request.tool_choice {
        let identity = if choice["function"].is_object() {
            &mut choice["function"]
        } else {
            choice
        };
        if let Some(alias) = identity["name"].as_str().and_then(|name| forward.get(name)) {
            identity["name"] = json!(alias);
        }
    }
    forward
        .into_iter()
        .map(|(name, alias)| (alias, name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_tool_strictness_with_its_function_name() {
        let name = "client.function.with.dots";
        let mut request: ChatRequest = serde_json::from_value(json!({"model":"test","messages":[],
            "tools":[{"name":name,"parameters":{"type":"object"}}],
            "responses_tool_strict":{name:true}}))
        .unwrap();
        let aliases = alias_client_tools(&mut request);
        let alias = &request.tools[0].name;
        assert_eq!(aliases[alias], name);
        assert_eq!(request.extensions["responses_tool_strict"][alias], true);
        assert!(
            request.extensions["responses_tool_strict"]
                .get(name)
                .is_none()
        );
    }

    #[test]
    fn aliases_history_even_when_the_current_round_retires_tools() {
        let original = "long_tool_name_".repeat(8);
        let mut request: ChatRequest = serde_json::from_value(json!({"model":"test","messages":[{"role":"assistant","tool_calls":[{"id":"call_a","name":original,"arguments":"{\"same\":\"text\"}"}]}]})).unwrap();
        let aliases = alias_client_tools(&mut request);
        let call = &request.messages[0].tool_calls[0];
        assert!(call.name.len() <= 64);
        assert_eq!(aliases[&call.name], original);
        assert_eq!(call.id, "call_a");
        assert_eq!(call.arguments, "{\"same\":\"text\"}");
    }

    #[test]
    fn restores_a_flat_named_choice_and_does_not_alias_parameter_text() {
        let name = "client.function.with.dots";
        let mut request: ChatRequest = serde_json::from_value(json!({"model":"test","messages":[],"tools":[{"name":name,"parameters":{"type":"object","description":name}}],"tool_choice":{"type":"function","name":name}})).unwrap();
        let aliases = alias_client_tools(&mut request);
        assert_eq!(aliases.len(), 1);
        let choice = serde_json::to_value(&request.tool_choice).unwrap();
        assert_eq!(choice["name"], request.tools[0].name);
        assert_eq!(request.tools[0].parameters["description"], name);
    }
}
