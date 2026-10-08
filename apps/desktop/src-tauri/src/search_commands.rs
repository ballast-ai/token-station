//! Browser search activation verifies the real model and browser path.
use crate::agent_integration::commands::{
    refresh_current_model_metadata, runtime_from_app, AgentCommandState, AgentProxyRuntime,
};
use crate::AppStateManaged;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Manager, State};
use token_station_cli::search::{
    SearchController, SearchMode, SearchResponse, SearchSettings, SearchStatus,
};

static SETTINGS_OPERATION: Mutex<()> = Mutex::new(());

#[derive(Serialize)]
pub(crate) struct SearchActivation {
    status: SearchStatus,
    verified: bool,
    managed_codex_updated: usize,
    execution: Option<&'static str>,
}

#[tauri::command]
pub(crate) fn get_search_status(state: State<'_, AppStateManaged>) -> Result<SearchStatus, String> {
    let inner = state.0.lock().map_err(|_| "Cannot read search settings.")?;
    Ok(SearchController::shared(&inner.data_dir()).status())
}

#[tauri::command]
pub(crate) async fn save_search_settings(
    app: tauri::AppHandle,
    settings: SearchSettings,
) -> Result<SearchActivation, String> {
    tauri::async_runtime::spawn_blocking(move || activate(&app, settings))
        .await
        .map_err(|_| "Search activation stopped. Check the current settings.".to_owned())?
}

fn activate(app: &tauri::AppHandle, settings: SearchSettings) -> Result<SearchActivation, String> {
    let _operation = SETTINGS_OPERATION
        .try_lock()
        .map_err(|_| "Search settings are busy. Try again.")?;
    let state = app.state::<AppStateManaged>();
    let agents = app.state::<AgentCommandState>();
    let controller = {
        let inner = state.0.lock().map_err(|_| "Cannot read search settings.")?;
        inner.ensure_editable()?;
        SearchController::shared(&inner.data_dir())
    };
    let previous = controller.settings();
    let previous_runtime = runtime_from_app(&state);
    if settings.enabled {
        if settings.mode == SearchMode::Local && !controller.status().chrome_available {
            return Err("Install Google Chrome. Then verify search again.".into());
        }
        previous_runtime
            .as_ref()
            .map_err(|error| error.message.clone())?;
    }
    {
        let _inner = state
            .0
            .lock()
            .map_err(|_| "Cannot update search settings.")?;
        controller.save(settings.clone())?;
    }
    let mut execution = None;
    let result = (|| {
        let runtime = match runtime_from_app(&state) {
            Ok(runtime) => runtime,
            Err(_) if !settings.enabled => return Ok(0),
            Err(error) => return Err(error.message),
        };
        if settings.enabled {
            execution = Some(verify_gateway_search(&runtime)?);
            let current = runtime_from_app(&state).map_err(|error| error.message)?;
            if current.fingerprint() != runtime.fingerprint() {
                return Err("The proxy changed during verification. Try again.".into());
            }
        }
        refresh_current_model_metadata(&state, &agents, Some("codex"), Some(&runtime))
            .map_err(|error| error.message)
    })();
    match result {
        Ok(count) => Ok(SearchActivation {
            status: controller.status(),
            verified: settings.enabled,
            managed_codex_updated: count,
            execution,
        }),
        Err(error) => {
            {
                let _inner = state
                    .0
                    .lock()
                    .map_err(|_| "Cannot restore search settings.")?;
                controller.save(previous).map_err(|_| {
                    format!("{error} Search settings could not be restored. Check Settings.")
                })?;
            }
            // A client can connect while the probe runs. Reconcile those owned files too.
            if let Ok(runtime) = runtime_from_app(&state) {
                refresh_current_model_metadata(&state, &agents, Some("codex"), Some(&runtime)).map_err(|_| format!("{error} Search settings were restored. Codex configuration needs review on the Agents page."))?;
            }
            Err(format!("{error} Previous search settings were restored."))
        }
    }
}

fn verify_gateway_search(runtime: &AgentProxyRuntime) -> Result<&'static str, String> {
    let origin = runtime.gateway_origin().map_err(|error| error.message)?;
    let url = reqwest::Url::parse(&origin).map_err(|_| "Invalid local proxy address.")?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))
    {
        return Err("Search verification requires the local proxy.".into());
    }
    let http = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(None)
            .build(),
    );
    let body = json!({
        "model":"auto", "stream":false, "max_output_tokens":1024, "max_tool_calls":1,
        "input":"Search the web for Python official documentation. Use the search tool once. Return the retrieved sources.",
        "tools":[{"type":"web_search"}], "tool_choice":"required",
        "include":["web_search_call.action.sources"]
    });
    let response = http.post(&format!("{origin}/agents/codex/v1/responses"))
        .header("authorization", &format!("Bearer {}", runtime.virtual_key()))
        .header("content-type", "application/json")
        .send(body.to_string().as_bytes())
        .map_err(|_| "Search verification could not reach the model or timed out. Check the proxy and provider network.")?;
    if !response.status().is_success() {
        return Err(format!("Search verification failed (HTTP {}). Check the Codex route and provider tool support.", response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(1024 * 1024)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read the search verification response.")?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|_| "The proxy returned an invalid search response.")?;
    validate_search_evidence(&document)?;
    let local = document["output"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item["type"] == "web_search_call"
                && item["id"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("srvtoolu_ts_"))
        })
    });
    Ok(if local { "local" } else { "native" })
}

fn validate_search_evidence(document: &Value) -> Result<(), String> {
    let output = document["output"]
        .as_array()
        .ok_or("The model did not return search evidence.")?;
    let searched = output.iter().any(|item| {
        item["type"] == "web_search_call"
            && item["status"] == "completed"
            && item["action"]["sources"].as_array().is_some_and(|sources| {
                sources.iter().any(|source| {
                    source["url"]
                        .as_str()
                        .and_then(|url| reqwest::Url::parse(url).ok())
                        .is_some_and(|url| {
                            matches!(url.scheme(), "https" | "http") && url.host_str().is_some()
                        })
                })
            })
    });
    let evidence = output.iter().any(|item| {
        item["type"] == "message"
            && item["content"].as_array().is_some_and(|content| {
                content.iter().any(|part| {
                    part["type"] == "output_text"
                        && part["text"]
                            .as_str()
                            .is_some_and(|text| !text.trim().is_empty())
                })
            })
    });
    if document["status"] == "completed" && searched && evidence {
        Ok(())
    } else {
        Err("The model did not complete a search with sources. Check Chrome network access, search engine challenges, and model tool support.".into())
    }
}

#[tauri::command]
pub(crate) async fn test_browser_search(
    state: State<'_, AppStateManaged>,
    query: String,
) -> Result<SearchResponse, String> {
    let controller = {
        let inner = state.0.lock().map_err(|_| "Cannot read search settings.")?;
        inner.ensure_editable()?;
        SearchController::shared(&inner.data_dir())
    };
    tauri::async_runtime::spawn_blocking(move || {
        let _operation = SETTINGS_OPERATION
            .try_lock()
            .map_err(|_| "Search settings are busy. Try again.")?;
        controller.search(&query, &|| false)
    })
    .await
    .map_err(|_| "Browser search test failed.".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn successful_http_or_model_text_does_not_prove_search() {
        assert!(validate_search_evidence(&json!({"status":"completed", "output":[{"type":"message","content":[{"type":"output_text","text":"I searched the web"}]}]})).is_err());
    }
    #[test]
    fn requires_completed_search_sources_and_evidence() {
        let mut response = json!({"status":"completed","output":[
            {"type":"web_search_call","status":"completed","action":{"sources":[{"url":"https://docs.python.org"}]}},
            {"type":"message","content":[{"type":"output_text","text":"Python documentation"}]}]});
        assert!(validate_search_evidence(&response).is_ok());
        response["output"][0]["status"] = json!("failed");
        assert!(validate_search_evidence(&response).is_err());
        response["output"][0]["status"] = json!("completed");
        response["output"][0]["action"]["sources"] = json!([]);
        assert!(validate_search_evidence(&response).is_err());
    }
}
