//! Explicit Codex backend dialect. Credentials remain Host-owned.
use super::*;
use sha2::{Digest, Sha256};

pub(super) fn check_config(config: &ProviderConfig) -> ComponentResultV1<()> {
    let endpoint = config.base_url.as_str();
    let loopback = config.base_url.is_loopback()
        && endpoint
            .split_once("://")
            .and_then(|(_, rest)| rest.split_once('/'))
            .is_some_and(|(_, path)| matches!(path, "backend-api/codex" | "v1"));
    if endpoint != "https://chatgpt.com/backend-api/codex" && !loopback {
        return Err(capability(
            "Codex plan requests require the fixed official Codex endpoint",
        ));
    }
    if config.auth.as_ref().map(|auth| auth.as_str()) != Some("codex_access_token") {
        return Err(ErrorEnvelope::new(
            ErrorCode::Auth,
            401,
            "Codex plan requests require a bound local Codex credential",
        ));
    }
    metadata(config)?;
    Ok(())
}

fn metadata(config: &ProviderConfig) -> ComponentResultV1<(&str, &str)> {
    let account = config
        .extensions
        .get("codex_account_id")
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or_else(|| capability("Codex plan requires a valid account identity"))?;
    let version = config
        .extensions
        .get("codex_client_version")
        .and_then(Value::as_str)
        .filter(|value| valid_version(value))
        .ok_or_else(|| capability("Codex plan requires a valid client version"))?;
    Ok((account, version))
}
fn valid_version(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
    {
        return false;
    }
    let core = value.split(['-', '+']).next().unwrap_or_default();
    let parts = core.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

pub(super) fn headers(config: &ProviderConfig) -> ComponentResultV1<SafeHeaders> {
    let (account, version) = metadata(config)?;
    SafeHeaders::try_new([
        ("content-type", "application/json".to_owned()),
        ("accept", "text/event-stream".to_owned()),
        ("chatgpt-account-id", account.to_owned()),
        ("openai-beta", "responses=experimental".to_owned()),
        ("originator", "codex_cli_rs".to_owned()),
        ("version", version.to_owned()),
        (
            "user-agent",
            format!("codex_cli_rs/{version} (Token Station)"),
        ),
    ])
    .map_err(|_| protocol("Cannot build safe Codex request headers"))
}

pub(super) fn body(mut body: Value) -> ComponentResultV1<Value> {
    // These defaults are required by the Codex transport. Never replace explicit
    // tool selection, parallelism, reasoning effort, or caller instructions.
    let object = body
        .as_object_mut()
        .ok_or_else(|| protocol("Invalid Codex request body"))?;
    object.entry("instructions").or_insert(json!(""));
    object.entry("tool_choice").or_insert(json!("auto"));
    object.entry("parallel_tool_calls").or_insert(json!(true));
    // Codex accepts call IDs up to 64 bytes. Hash complete IDs rather than
    // truncating or trimming them, and transform calls and results identically.
    let mut bound_ids = BTreeMap::new();
    for item in object
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .into_iter()
        .flatten()
    {
        if let Some(id) = item
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        {
            let bound = if id.len() > 64 {
                format!("{:x}", Sha256::digest(id.as_bytes()))
            } else {
                id.clone()
            };
            if bound_ids
                .insert(bound.clone(), id.clone())
                .is_some_and(|previous| previous != id)
            {
                return Err(capability("Codex historical tool call identities collide"));
            }
            item["call_id"] = json!(bound);
        }
    }
    Ok(body)
}
