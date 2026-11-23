//! Validate Codex tool declarations embedded in the Responses input sequence.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{capability, invalid};

pub(super) const PRIVATE_TOOLS: &str = "token_station_private_response_tools";

/// Return a codec projection only for the observed leading developer declaration.
pub(super) fn project(body: &Value) -> Result<Option<Value>, String> {
    let Some(input) = body["input"].as_array() else {
        return Ok(None);
    };
    let declarations: Vec<_> = input
        .iter()
        .enumerate()
        .filter(|(_, item)| item["type"] == "additional_tools")
        .collect();
    if declarations.is_empty() {
        return Ok(None);
    }
    if declarations.len() != 1 || declarations[0].0 != 0 {
        return Err(capability(
            "additional_tools requires one leading declaration",
        ));
    }
    let declaration = declarations[0].1;
    if declaration["role"] != "developer"
        || declaration["id"]
            .as_str()
            .is_none_or(|id| id.is_empty() || id.len() > 256)
        || declaration.as_object().is_none_or(|object| {
            object
                .keys()
                .any(|key| !["type", "role", "id", "tools"].contains(&key.as_str()))
        })
    {
        return Err(invalid(
            "additional_tools requires a developer role, bounded ID, and declaration fields only",
        ));
    }
    let embedded = declaration["tools"]
        .as_array()
        .ok_or_else(|| invalid("additional_tools.tools must be an array"))?;
    let mut tools = match body.get("tools") {
        None => Vec::new(),
        Some(Value::Array(tools)) => tools.clone(),
        Some(_) => return Err(invalid("tools must be an array")),
    };
    // The strict declaration vocabulary applies only to embedded tools. Existing
    // top-level special tools keep the shared codec's established validation.
    validate_tools(embedded, None, &mut BTreeSet::new(), &mut BTreeSet::new())?;
    tools.extend_from_slice(embedded);
    let mut namespaces = BTreeSet::new();
    for tool in tools.iter().filter(|tool| tool["type"] == "namespace") {
        if let Some(name) = tool["name"].as_str() {
            if !namespaces.insert(name) {
                return Err(invalid("Merged tool namespace is duplicated"));
            }
        }
    }
    check_flat_identities(&Value::Array(tools.clone()), None, &mut BTreeSet::new())?;
    let mut projected = body.clone();
    projected["input"] = Value::Array(input[1..].to_vec());
    projected["tools"] = Value::Array(tools);
    Ok(Some(projected))
}

fn validate_tools(
    tools: &[Value],
    namespace: Option<&str>,
    identities: &mut BTreeSet<String>,
    namespaces: &mut BTreeSet<String>,
) -> Result<(), String> {
    for tool in tools {
        let name = tool["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| invalid("Embedded tools require nonempty names"))?;
        match tool["type"].as_str() {
            Some("namespace") if namespace.is_none() => {
                if !namespaces.insert(name.to_owned()) {
                    return Err(invalid("Embedded tool namespace is duplicated"));
                }
                let nested = tool["tools"].as_array().ok_or_else(|| invalid("Tool namespace requires an array"))?;
                validate_tools(nested, Some(name), identities, namespaces)?;
            }
            Some("function" | "custom") => {
                let identity = namespace.map_or_else(|| Ok(name.to_owned()), |namespace| qualified(namespace, name))?;
                if !identities.insert(identity) {
                    return Err(invalid("Embedded tool identity is duplicated or ambiguous"));
                }
            }
            _ => return Err(capability("additional_tools supports function, custom, and single-level namespace declarations")),
        }
    }
    Ok(())
}

pub(super) fn qualified(namespace: &str, name: &str) -> Result<String, String> {
    let valid = |value: &str, limit| {
        !value.is_empty()
            && value.len() <= limit
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    };
    if !valid(namespace, 64) || !valid(name, 128) {
        return Err(invalid("Custom tool namespace or name is invalid"));
    }
    let name = if namespace.ends_with("__") {
        format!("{namespace}{name}")
    } else {
        format!("{namespace}__{name}")
    };
    if name.len() > 128 {
        return Err(invalid("Qualified custom tool name exceeds 128 bytes"));
    }
    Ok(name)
}

pub(super) fn custom_names(
    tools: &Value,
) -> Result<std::collections::BTreeMap<String, (String, String)>, String> {
    let mut names = std::collections::BTreeMap::new();
    for namespace in tools
        .as_array()
        .into_iter()
        .flatten()
        .filter(|tool| tool["type"] == "namespace")
    {
        for tool in namespace["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|tool| tool["type"] == "custom")
        {
            let namespace = namespace["name"]
                .as_str()
                .ok_or_else(|| invalid("Custom namespace requires a name"))?;
            let name = tool["name"]
                .as_str()
                .ok_or_else(|| invalid("Custom tool requires a name"))?;
            if names
                .insert(
                    qualified(namespace, name)?,
                    (namespace.to_owned(), name.to_owned()),
                )
                .is_some()
            {
                return Err(invalid("Qualified custom tool identity is duplicated"));
            }
        }
    }
    if !names.is_empty() {
        let mut identities = BTreeSet::new();
        check_flat_identities(tools, None, &mut identities)?;
    }
    Ok(names)
}

fn check_flat_identities(
    tools: &Value,
    namespace: Option<&str>,
    identities: &mut BTreeSet<String>,
) -> Result<(), String> {
    for tool in tools.as_array().into_iter().flatten() {
        match tool["type"].as_str() {
            Some("namespace") => {
                check_flat_identities(&tool["tools"], tool["name"].as_str(), identities)?
            }
            Some("function" | "custom") => {
                let name = tool["name"]
                    .as_str()
                    .ok_or_else(|| invalid("Tool requires a name"))?;
                let name = namespace.map_or_else(
                    || Ok(name.to_owned()),
                    |namespace| qualified(namespace, name),
                )?;
                if !identities.insert(name) {
                    return Err(invalid(
                        "Qualified tool identity is duplicated or ambiguous",
                    ));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The pinned shared codec supports top-level custom tools and namespaced functions.
pub(super) fn codec_tools(tools: &Value) -> Value {
    let Some(tools) = tools.as_array() else {
        return tools.clone();
    };
    let mut result = Vec::new();
    for tool in tools {
        if tool["type"] != "namespace" {
            result.push(tool.clone());
            continue;
        }
        let Some(children) = tool["tools"].as_array() else {
            result.push(tool.clone());
            continue;
        };
        let mut functions = Vec::new();
        for child in children {
            if child["type"] == "custom" {
                let mut custom = child.clone();
                if let (Some(namespace), Some(name)) =
                    (tool["name"].as_str(), child["name"].as_str())
                {
                    if let Ok(name) = qualified(namespace, name) {
                        custom["name"] = Value::String(name);
                    }
                }
                result.push(custom);
            } else {
                functions.push(child.clone());
            }
        }
        if !functions.is_empty() {
            let mut namespace = tool.clone();
            namespace["tools"] = Value::Array(functions);
            result.push(namespace);
        }
    }
    Value::Array(result)
}

pub(super) fn project_custom_history(body: &mut Value) -> Result<(), String> {
    for item in body["input"].as_array_mut().into_iter().flatten() {
        if item["type"] != "custom_tool_call" || item.get("namespace").is_none() {
            continue;
        }
        let namespace = item["namespace"]
            .as_str()
            .ok_or_else(|| invalid("Custom call namespace must be a string"))?;
        let name = item["name"]
            .as_str()
            .ok_or_else(|| invalid("Custom call name must be a string"))?;
        item["name"] = Value::String(qualified(namespace, name)?);
        item.as_object_mut().unwrap().remove("namespace");
    }
    Ok(())
}

pub(super) fn restore(
    value: &mut Value,
    names: &std::collections::BTreeMap<String, (String, String)>,
) {
    if value["type"] == "custom_tool_call" {
        if let Some((namespace, name)) = value["name"].as_str().and_then(|name| names.get(name)) {
            value["namespace"] = Value::String(namespace.clone());
            value["name"] = Value::String(name.clone());
        }
    }
    match value {
        Value::Object(object) => {
            for value in object.values_mut() {
                restore(value, names);
            }
        }
        Value::Array(array) => {
            for value in array {
                restore(value, names);
            }
        }
        _ => {}
    }
}

pub(super) fn restore_stream(encoded: String, tools: &Value) -> Result<String, String> {
    let names = custom_names(tools)?;
    if names.is_empty() {
        return Ok(encoded);
    }
    let mut envelope: Value = serde_json::from_str(&encoded).map_err(super::internal)?;
    if let Some(data) = envelope["data"].as_str() {
        let mut lines = Vec::new();
        for line in data.split('\n') {
            if let Some(json) = line.strip_prefix("data: ") {
                if let Ok(mut value) = serde_json::from_str::<Value>(json) {
                    restore(&mut value, &names);
                    lines.push(format!("data: {value}"));
                    continue;
                }
            }
            lines.push(line.to_owned());
        }
        envelope["data"] = Value::String(lines.join("\n"));
    }
    super::to_output(&envelope)
}
