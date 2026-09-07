use crate::*;

#[tauri::command]
pub(crate) fn get_web_search_target(state: State<'_, AppStateManaged>) -> Result<Value, String> {
    let inner = state.0.lock().unwrap();
    Ok(inner
        .draft
        .get("web_search_target")
        .cloned()
        .unwrap_or(Value::Null))
}

fn edit_search_target(draft: &mut Value, upstream: &str, model: &str) -> Result<(), String> {
    if upstream.is_empty() {
        draft
            .as_object_mut()
            .ok_or("Configuration is invalid")?
            .remove("web_search_target");
        return Ok(());
    }
    let provider = draft["upstreams"]
        .get_mut(upstream)
        .ok_or("Search provider is not configured")?;
    if provider["managed_route"].as_bool() == Some(true) {
        return Err(
            "Managed providers require an administrator-declared search backend".to_owned(),
        );
    }
    let dialect = match provider["provider"].as_str() {
        Some("anthropic") => "anthropic-native",
        Some("openai-compatible") => "responses-native",
        _ => return Err("Search requires an Anthropic or OpenAI Responses provider".to_owned()),
    };
    provider["api_dialect"] = json!(dialect);
    draft["web_search_target"] = json!({"upstream":upstream,"model":model});
    Ok(())
}

#[tauri::command]
pub(crate) fn set_web_search_target(
    state: State<'_, AppStateManaged>,
    upstream: String,
    model: String,
) -> Result<StateView, String> {
    let mut inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let previous_draft = inner.draft.clone();
    let previous_config_state = inner.config_state.clone();
    inner.edit_validated_draft(|candidate| {
        edit_search_target(&mut candidate.draft, &upstream, &model)?;
        candidate.materialize()?.validate()
    })?;
    if let Err(error) = inner.save_draft() {
        inner.draft = previous_draft;
        inner.config_state = previous_config_state;
        return Err(error);
    }
    Ok(inner.snapshot())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_search_declares_protocol_without_changing_main_route() {
        let mut draft = json!({"routing":{"mode":"direct"},"upstreams":{"search":{"provider":"openai-compatible"}}});
        edit_search_target(&mut draft, "search", "search-model").unwrap();
        assert_eq!(
            draft["upstreams"]["search"]["api_dialect"],
            "responses-native"
        );
        assert_eq!(draft["routing"], json!({"mode":"direct"}));
        edit_search_target(&mut draft, "", "").unwrap();
        assert!(draft.get("web_search_target").is_none());
    }
}
