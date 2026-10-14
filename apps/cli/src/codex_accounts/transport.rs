use super::{API_BASE, Result};
use crate::chatgpt_accounts::PlanModel;
use serde_json::{Value, json};
use std::{io::Read, time::Duration};
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
#[derive(Clone)]
pub(super) struct Endpoints {
    pub token: String,
    pub api: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            token: TOKEN_URL.into(),
            api: API_BASE.into(),
        }
    }
}
fn read_json(mut response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    if response.status().as_u16() != 200 {
        return Err("Codex request was not accepted. Check the login in Codex.".into());
    }
    let mut raw = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(4 * super::source::LIMIT + 1)
        .read_to_end(&mut raw)
        .map_err(|_| "Codex response could not be read.")?;
    let parsed = if raw.len() as u64 > 4 * super::source::LIMIT {
        Err("Codex response exceeds its size limit.".into())
    } else {
        serde_json::from_slice(&raw).map_err(|_| "Codex returned an invalid response.".into())
    };
    raw.fill(0);
    parsed
}
impl Endpoints {
    fn validate(&self) -> Result<()> {
        if self.token == TOKEN_URL && self.api == API_BASE {
            return Ok(());
        }
        #[cfg(test)]
        {
            let local = |s: &str| {
                url::Url::parse(s).is_ok_and(|u| {
                    u.scheme() == "http"
                        && u.host_str() == Some("127.0.0.1")
                        && u.username().is_empty()
                        && u.password().is_none()
                        && u.query().is_none()
                        && u.fragment().is_none()
                })
            };
            if local(&self.token) && local(&self.api) {
                return Ok(());
            }
        }
        Err("Untrusted Codex endpoint.".into())
    }
    fn http() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into()
    }
    pub fn refresh(&self, refresh: &str) -> Result<super::SecretJson> {
        self.validate()?;
        let payload = super::SecretJson(json!({
            "client_id":CLIENT_ID,
            "grant_type":"refresh_token",
            "refresh_token":refresh,
            "scope":"openid profile email"
        }));
        let body = super::SecretText::new(payload.to_string());
        let reply = Self::http()
            .post(&self.token)
            .header("Content-Type", "application/json")
            .send(body.expose().as_bytes())
            .map_err(|_| "Codex session refresh failed. Sign in with Codex again.")?;
        let value = super::SecretJson(read_json(reply)?);
        if value
            .get("access_token")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            return Err("Codex refresh did not return an access token.".into());
        }
        Ok(value)
    }
    pub fn models(&self, token: &str, account: &str, version: &str) -> Result<Vec<PlanModel>> {
        self.validate()?;
        let mut url = url::Url::parse(&format!("{}/models", self.api))
            .map_err(|_| "Invalid Codex endpoint.")?;
        url.query_pairs_mut().append_pair("client_version", version);
        let reply = Self::http()
            .get(url.as_str())
            .header("Authorization", &format!("Bearer {token}"))
            .header("ChatGPT-Account-Id", account)
            .header("originator", "codex_cli_rs")
            .header("version", version)
            .header(
                "User-Agent",
                &format!("codex_cli_rs/{version} (Token Station)"),
            )
            .call()
            .map_err(|_| "Codex model catalog is unavailable.")?;
        let value = read_json(reply)?;
        let models = value
            .get("models")
            .and_then(Value::as_array)
            .ok_or("Codex returned an invalid model catalog.")?;
        let mut result = Vec::new();
        for model in models {
            if model.get("visibility").and_then(Value::as_str) == Some("hide") {
                continue;
            }
            let Some(id) = model
                .get("slug")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
            else {
                return Err("Codex returned an invalid model identifier.".into());
            };
            if result.iter().any(|v: &PlanModel| v.id == id) {
                continue;
            }
            result.push(PlanModel {
                id: id.into(),
                name: model
                    .get("display_name")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
                    .unwrap_or(id)
                    .into(),
                context_window: model
                    .get("context_window")
                    .and_then(Value::as_u64)
                    .filter(|v| *v > 0),
                supports_images: model
                    .get("input_modalities")
                    .and_then(Value::as_array)
                    .map(|v| v.iter().any(|v| v.as_str() == Some("image"))),
            });
        }
        Ok(result)
    }
}
// Gregorian civil-date conversion, with the Unix epoch as its input origin.
pub(super) fn timestamp(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX / 2) + 719_468;
    let era = days / 146_097;
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    let rest = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}
