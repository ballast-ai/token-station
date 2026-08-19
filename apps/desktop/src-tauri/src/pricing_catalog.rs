//! Public model metadata and price suggestions for the Desktop control plane.
//!
//! This module is deliberately outside the gateway and pricing kernel. It reads
//! a public catalog on demand. Remote model IDs remain advisory. The existing
//! `set_model_price` command remains the only price-table write path.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CATALOG_URL: &str = "https://models.dev/api.json";
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_RESPONSE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct ModelPriceSuggestionView {
    pub model_id: String,
    pub display_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub source: String,
    pub catalog_source: String,
    pub fetched_at_ms: u64,
    pub input_per_mtok: u64,
    pub output_per_mtok: u64,
    pub cache_read_per_mtok: u64,
    pub cache_write_per_mtok: u64,
    pub reasoning_per_mtok: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct PublicProviderModelsView {
    pub providers: BTreeMap<String, Vec<String>>,
    pub source: String,
    pub fetched_at_ms: u64,
    pub unavailable_provider_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RequestedModelPriceSuggestion {
    pub requested_model_id: String,
    pub suggestion: ModelPriceSuggestionView,
}

#[derive(Deserialize)]
struct CatalogProvider {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    models: BTreeMap<String, CatalogModel>,
}

#[derive(Deserialize)]
struct CatalogModel {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    family: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    modalities: Option<CatalogModalities>,
    cost: Option<CatalogCost>,
}

#[derive(Deserialize)]
struct CatalogModalities {
    #[serde(default)]
    input: Vec<String>,
    #[serde(default)]
    output: Vec<String>,
}

#[derive(Deserialize)]
struct CatalogCost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
    reasoning: Option<f64>,
}

struct CachedCatalog {
    body: String,
    fetched_at_ms: u64,
    fresh: bool,
}

pub(crate) fn list_public_provider_models_with_cache_egress(
    data_dir: &Path,
    provider_ids: &[String],
    egress: &token_station_cli::config::EgressConfig,
    secrets: &token_station_cli::secrets::SecretStore,
) -> Result<PublicProviderModelsView, String> {
    let cache = read_cache(data_dir);
    if let Some(cached) = cache.as_ref().filter(|cache| cache.fresh) {
        if let Ok(result) = public_provider_models_from_json(
            &cached.body,
            provider_ids,
            cached.fetched_at_ms,
            "cache",
        ) {
            return Ok(result);
        }
    }

    let live = fetch_catalog(egress, secrets).and_then(|body| {
        let fetched_at_ms = now_ms();
        let result = public_provider_models_from_json(&body, provider_ids, fetched_at_ms, "live")?;
        let _ = write_cache(data_dir, body.as_bytes());
        Ok(result)
    });
    match live {
        Ok(result) => Ok(result),
        Err(live_error) => {
            let Some(cached) = cache else {
                return Err(live_error);
            };
            public_provider_models_from_json(
                &cached.body,
                provider_ids,
                cached.fetched_at_ms,
                "stale_cache",
            )
            .map_err(|cache_error| {
                format!("{live_error}；本地公共模型目录也无法读取：{cache_error}")
            })
        }
    }
}

fn public_provider_models_from_json(
    body: &str,
    provider_ids: &[String],
    fetched_at_ms: u64,
    source: &str,
) -> Result<PublicProviderModelsView, String> {
    let providers: BTreeMap<String, CatalogProvider> =
        serde_json::from_str(body).map_err(|error| format!("公共模型目录格式无效：{error}"))?;
    let mut resolved = BTreeMap::new();
    let mut unavailable = Vec::new();

    for requested_id in provider_ids {
        let Some(wanted) = normalize_public_model_provider_id(requested_id) else {
            unavailable.push(requested_id.clone());
            continue;
        };
        let Some((_, provider)) = find_provider(&providers, &wanted) else {
            unavailable.push(requested_id.clone());
            continue;
        };
        let mut models = BTreeSet::new();
        let mut invalid_provider = false;
        for (model_key, model) in &provider.models {
            if model.status.as_deref().is_some_and(|status| {
                status.eq_ignore_ascii_case("deprecated") || status.eq_ignore_ascii_case("retired")
            }) {
                continue;
            }
            if !is_chat_compatible_model(model_key, model) {
                continue;
            }
            let model_id = if model.id.trim().is_empty() {
                model_key.trim()
            } else {
                model.id.trim()
            };
            if model_id.is_empty()
                || model_id.len() > 512
                || !model_id
                    .chars()
                    .all(|character| character.is_ascii_graphic())
            {
                continue;
            }
            models.insert(model_id.to_owned());
            if models.len() > 512 {
                invalid_provider = true;
                break;
            }
        }
        if invalid_provider || models.is_empty() {
            unavailable.push(requested_id.clone());
            continue;
        }
        resolved.insert(requested_id.clone(), models.into_iter().collect());
    }

    Ok(PublicProviderModelsView {
        providers: resolved,
        source: source.to_owned(),
        fetched_at_ms,
        unavailable_provider_ids: unavailable,
    })
}

fn normalize_public_model_provider_id(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        // The public catalog has one generic Alibaba namespace. Token Station
        // uses region-specific endpoints whose model availability can differ.
        "qwen-singapore" | "qwen_singapore" | "qwen-us" | "qwen_us" => None,
        _ => Some(normalize_provider_id(value)),
    }
}

fn is_chat_compatible_model(model_key: &str, model: &CatalogModel) -> bool {
    if model.modalities.as_ref().is_some_and(|modalities| {
        !modalities
            .input
            .iter()
            .any(|value| value.eq_ignore_ascii_case("text"))
            || modalities.output.is_empty()
            || !modalities
                .output
                .iter()
                .all(|value| value.eq_ignore_ascii_case("text"))
    }) {
        return false;
    }

    let searchable = format!("{} {} {}", model_key, model.id, model.family).to_ascii_lowercase();
    ![
        "embed",
        "rerank",
        "retriever",
        "retrieval",
        "whisper",
        "transcri",
        "text-to-speech",
        "speech-to-text",
        "moderation",
        "-tts",
        "_tts",
    ]
    .iter()
    .any(|marker| searchable.contains(marker))
}

pub(crate) fn suggest_with_cache_egress(
    data_dir: &Path,
    provider_id: Option<&str>,
    model_id: &str,
    egress: &token_station_cli::config::EgressConfig,
    secrets: &token_station_cli::secrets::SecretStore,
) -> Result<Option<ModelPriceSuggestionView>, String> {
    let mut suggestions = suggest_many_with_cache_egress(
        data_dir,
        provider_id,
        &[model_id.to_owned()],
        egress,
        secrets,
    )?;
    Ok(suggestions.pop().map(|value| value.suggestion))
}

pub(crate) fn suggest_many_with_cache_egress(
    data_dir: &Path,
    provider_id: Option<&str>,
    model_ids: &[String],
    egress: &token_station_cli::config::EgressConfig,
    secrets: &token_station_cli::secrets::SecretStore,
) -> Result<Vec<RequestedModelPriceSuggestion>, String> {
    let cache = read_cache(data_dir);
    if let Some(cached) = cache.as_ref().filter(|cache| cache.fresh) {
        if let Ok(suggestions) =
            suggest_many_from_json(&cached.body, provider_id, model_ids, cached.fetched_at_ms)
        {
            return Ok(with_catalog_sources(suggestions, "cache"));
        }
    }

    let live = fetch_catalog(egress, secrets).and_then(|body| {
        let fetched_at_ms = now_ms();
        let suggestions = suggest_many_from_json(&body, provider_id, model_ids, fetched_at_ms)?;
        // Never replace a known-good cache with an invalid catalog. Parsing and
        // price validation above are part of a successful refresh.
        let _ = write_cache(data_dir, body.as_bytes());
        Ok(with_catalog_sources(suggestions, "live"))
    });
    match live {
        Ok(suggestions) => Ok(suggestions),
        Err(live_error) => {
            let Some(cached) = cache else {
                return Err(live_error);
            };
            suggest_many_from_json(&cached.body, provider_id, model_ids, cached.fetched_at_ms)
                .map(|suggestions| with_catalog_sources(suggestions, "stale_cache"))
                .map_err(|cache_error| {
                    format!("{live_error}；本地价格目录也无法读取：{cache_error}")
                })
        }
    }
}

fn with_catalog_sources(
    suggestions: Vec<RequestedModelPriceSuggestion>,
    catalog_source: &str,
) -> Vec<RequestedModelPriceSuggestion> {
    suggestions
        .into_iter()
        .map(|value| RequestedModelPriceSuggestion {
            requested_model_id: value.requested_model_id,
            suggestion: with_catalog_source(value.suggestion, catalog_source),
        })
        .collect()
}

fn with_catalog_source(
    mut suggestion: ModelPriceSuggestionView,
    catalog_source: &str,
) -> ModelPriceSuggestionView {
    suggestion.catalog_source = catalog_source.to_owned();
    suggestion
}

#[cfg(test)]
fn suggest_from_json(
    body: &str,
    provider_id: Option<&str>,
    model_id: &str,
    fetched_at_ms: u64,
) -> Result<Option<ModelPriceSuggestionView>, String> {
    let providers: BTreeMap<String, CatalogProvider> =
        serde_json::from_str(body).map_err(|error| format!("公开价格目录格式无效：{error}"))?;
    suggest_from_providers(&providers, provider_id, model_id, fetched_at_ms)
}

fn suggest_many_from_json(
    body: &str,
    provider_id: Option<&str>,
    model_ids: &[String],
    fetched_at_ms: u64,
) -> Result<Vec<RequestedModelPriceSuggestion>, String> {
    let providers: BTreeMap<String, CatalogProvider> =
        serde_json::from_str(body).map_err(|error| format!("公开价格目录格式无效：{error}"))?;
    let mut suggestions = Vec::new();
    for model_id in model_ids {
        if let Some(suggestion) =
            suggest_from_providers(&providers, provider_id, model_id, fetched_at_ms)?
        {
            suggestions.push(RequestedModelPriceSuggestion {
                requested_model_id: model_id.clone(),
                suggestion,
            });
        }
    }
    Ok(suggestions)
}

fn suggest_from_providers(
    providers: &BTreeMap<String, CatalogProvider>,
    provider_id: Option<&str>,
    model_id: &str,
    fetched_at_ms: u64,
) -> Result<Option<ModelPriceSuggestionView>, String> {
    let raw_model = model_id.trim();
    if raw_model.is_empty() {
        return Ok(None);
    }

    let explicit_provider = provider_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_provider_id);
    let prefix = model_provider_prefix(raw_model)
        .as_deref()
        .map(normalize_provider_id);
    let selected_provider = explicit_provider
        .or(prefix.clone())
        .or_else(|| infer_official_provider(raw_model).map(str::to_owned));

    if let Some(provider) = selected_provider {
        let Some((provider_key, entry)) = find_provider(providers, &provider) else {
            // A caller-supplied namespace is an explicit boundary. Do not
            // silently price it as another provider's model.
            return Ok(None);
        };
        return find_in_provider(provider_key, entry, raw_model, fetched_at_ms);
    }

    let mut matches = Vec::new();
    for (provider_key, provider) in providers {
        if let Some(suggestion) =
            find_in_provider(provider_key, provider, raw_model, fetched_at_ms)?
        {
            matches.push(suggestion);
            if matches.len() > 1 {
                return Ok(None);
            }
        }
    }
    Ok(matches.pop())
}

fn find_provider<'a>(
    providers: &'a BTreeMap<String, CatalogProvider>,
    wanted: &str,
) -> Option<(&'a str, &'a CatalogProvider)> {
    let entry_id = |key: &'a str, provider: &'a CatalogProvider| {
        if provider.id.is_empty() {
            key
        } else {
            provider.id.as_str()
        }
    };
    providers
        .iter()
        .find_map(|(key, provider)| {
            let id = entry_id(key, provider);
            (key.eq_ignore_ascii_case(wanted) || id.eq_ignore_ascii_case(wanted))
                .then_some((key.as_str(), provider))
        })
        .or_else(|| {
            providers.iter().find_map(|(key, provider)| {
                let id = entry_id(key, provider);
                (normalize_provider_id(key) == wanted || normalize_provider_id(id) == wanted)
                    .then_some((key.as_str(), provider))
            })
        })
}

fn find_in_provider(
    provider_key: &str,
    provider: &CatalogProvider,
    requested_model: &str,
    fetched_at_ms: u64,
) -> Result<Option<ModelPriceSuggestionView>, String> {
    let candidates = model_candidates(requested_model, provider_key);
    for candidate in candidates {
        let found = provider.models.iter().find(|(key, model)| {
            key.eq_ignore_ascii_case(&candidate)
                || (!model.id.is_empty() && model.id.eq_ignore_ascii_case(&candidate))
        });
        let Some((model_key, model)) = found else {
            continue;
        };
        let Some(cost) = model.cost.as_ref() else {
            return Ok(None);
        };
        let Some(input) = cost.input else {
            return Ok(None);
        };
        let Some(output) = cost.output else {
            return Ok(None);
        };
        return Ok(Some(ModelPriceSuggestionView {
            model_id: if model.id.is_empty() {
                model_key.clone()
            } else {
                model.id.clone()
            },
            display_name: if model.name.is_empty() {
                model_key.clone()
            } else {
                model.name.clone()
            },
            provider_id: if provider.id.is_empty() {
                provider_key.to_owned()
            } else {
                provider.id.clone()
            },
            provider_name: if provider.name.is_empty() {
                provider_key.to_owned()
            } else {
                provider.name.clone()
            },
            source: "models.dev".to_owned(),
            catalog_source: String::new(),
            fetched_at_ms,
            input_per_mtok: usd_per_mtok_to_micros(input)?,
            output_per_mtok: usd_per_mtok_to_micros(output)?,
            cache_read_per_mtok: optional_usd_to_micros(cost.cache_read)?,
            cache_write_per_mtok: optional_usd_to_micros(cost.cache_write)?,
            reasoning_per_mtok: cost.reasoning.map(usd_per_mtok_to_micros).transpose()?,
        }));
    }
    Ok(None)
}

fn optional_usd_to_micros(value: Option<f64>) -> Result<u64, String> {
    value
        .map(usd_per_mtok_to_micros)
        .transpose()
        .map(Option::unwrap_or_default)
}

fn usd_per_mtok_to_micros(value: f64) -> Result<u64, String> {
    let micros = value * 1_000_000.0;
    if !value.is_finite() || value < 0.0 || micros > u64::MAX as f64 {
        return Err("公开价格目录包含越界金额".to_owned());
    }
    Ok(micros.round() as u64)
}

fn normalize_provider_id(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "google-ai" | "google-generative-ai" | "gemini" => "google".to_owned(),
        "zhipu" | "zhipu-ai" | "bigmodel" | "glm-cn" | "glm_cn" => "zhipuai".to_owned(),
        "glm" => "zai".to_owned(),
        "glm-coding" | "glm_coding" => "zai-coding-plan".to_owned(),
        "moonshot" | "kimi-global" | "kimi_global" => "moonshotai".to_owned(),
        "kimi" => "moonshotai-cn".to_owned(),
        "qwen" => "alibaba-cn".to_owned(),
        "qwen-singapore" | "qwen_singapore" | "qwen-us" | "qwen_us" => "alibaba".to_owned(),
        "minimax-cn" | "minimax_cn" => "minimax-cn".to_owned(),
        "minimax-global" | "minimax_global" => "minimax".to_owned(),
        "nvidia-nim" | "nvidia_nim" => "nvidia".to_owned(),
        "siliconflow" => "siliconflow-cn".to_owned(),
        "siliconflow-global" | "siliconflow_global" => "siliconflow".to_owned(),
        "together" => "togetherai".to_owned(),
        "fireworks" => "fireworks-ai".to_owned(),
        "stepfun-plan" | "stepfun_plan" => "stepfun-step-plan".to_owned(),
        "xiaomi-mimo" | "xiaomi_mimo" => "xiaomi".to_owned(),
        "novita" => "novita-ai".to_owned(),
        other => other.to_owned(),
    }
}

fn model_provider_prefix(model_id: &str) -> Option<String> {
    let (prefix, _) = model_id.trim().split_once('/')?;
    (!prefix.is_empty()).then(|| prefix.to_ascii_lowercase())
}

fn infer_official_provider(model_id: &str) -> Option<&'static str> {
    let model = model_id
        .trim()
        .split_once('/')
        .map_or(model_id.trim(), |(_, suffix)| suffix)
        .to_ascii_lowercase();
    if model.starts_with("gpt-")
        || model.starts_with("chatgpt-")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
    {
        Some("openai")
    } else if model.starts_with("claude-") {
        Some("anthropic")
    } else if model.starts_with("gemini-") {
        Some("google")
    } else if model.starts_with("deepseek-") {
        Some("deepseek")
    } else if model.starts_with("glm-") {
        Some("zhipuai")
    } else if model.starts_with("minimax-") {
        Some("minimax")
    } else if model.starts_with("kimi-") || model.starts_with("moonshot-") {
        Some("moonshotai")
    } else {
        None
    }
}

fn model_candidates(model_id: &str, provider_id: &str) -> Vec<String> {
    let mut value = model_id.trim().to_ascii_lowercase();
    if let Some((prefix, suffix)) = value.split_once('/') {
        if normalize_provider_id(prefix) == normalize_provider_id(provider_id) {
            value = suffix.to_owned();
        }
    }
    if let Some((base, _)) = value.split_once('@') {
        value = base.to_owned();
    }
    if let Some((base, _)) = value.split_once(':') {
        value = base.to_owned();
    }

    let mut candidates = vec![value.clone()];
    let dotted = value.replace('.', "-");
    if dotted != value {
        candidates.push(dotted.clone());
    }
    let hyphenated_date = strip_date_suffix(&value);
    if hyphenated_date != value && !candidates.contains(&hyphenated_date) {
        candidates.push(hyphenated_date);
    }
    let normalized_without_date = strip_date_suffix(&dotted);
    if normalized_without_date != dotted && !candidates.contains(&normalized_without_date) {
        candidates.push(normalized_without_date);
    }
    candidates
}

fn strip_date_suffix(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 11 {
        let suffix = value.get(value.len() - 11..);
        if suffix.is_some_and(|suffix| {
            suffix.starts_with('-')
                && suffix[1..].chars().enumerate().all(|(index, ch)| {
                    if index == 4 || index == 7 {
                        ch == '-'
                    } else {
                        ch.is_ascii_digit()
                    }
                })
        }) {
            return value[..value.len() - 11].to_owned();
        }
    }
    if bytes.len() >= 9 {
        let suffix = value.get(value.len() - 9..);
        if suffix.is_some_and(|suffix| {
            suffix.starts_with('-') && suffix[1..].chars().all(|ch| ch.is_ascii_digit())
        }) {
            return value[..value.len() - 9].to_owned();
        }
    }
    value.to_owned()
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("catalogs").join("models-dev-api.json")
}

fn read_cache(data_dir: &Path) -> Option<CachedCatalog> {
    let path = cache_path(data_dir);
    let metadata = fs::metadata(&path).ok()?;
    if metadata.len() > MAX_RESPONSE_BYTES {
        return None;
    }
    let modified = metadata.modified().ok()?;
    let fetched_at_ms = modified
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()?;
    let fresh = SystemTime::now()
        .duration_since(modified)
        .map(|age| age <= CACHE_TTL)
        .unwrap_or(false);
    Some(CachedCatalog {
        body: fs::read_to_string(path).ok()?,
        fetched_at_ms,
        fresh,
    })
}

fn write_cache(data_dir: &Path, body: &[u8]) -> Result<(), String> {
    let path = cache_path(data_dir);
    crate::agent_integration::safe_fs::write_atomic_private(&path, body)
        .map_err(|error| format!("保存价格目录缓存失败：{error}"))
}

fn fetch_catalog(
    egress: &token_station_cli::config::EgressConfig,
    secrets: &token_station_cli::secrets::SecretStore,
) -> Result<String, String> {
    let proxy = if let Some((scheme, host, port)) = egress.proxy_parts()? {
        let protocol = match scheme.as_str() {
            "http" => ureq::ProxyProtocol::Http,
            "https" => ureq::ProxyProtocol::Https,
            "socks5" => ureq::ProxyProtocol::Socks5,
            "socks5h" => ureq::ProxyProtocol::Socks5h,
            _ => return Err("不支持的出站代理协议".to_owned()),
        };
        let mut builder = ureq::Proxy::builder(protocol).host(&host).port(port);
        for entry in &egress.no_proxy {
            builder = builder.no_proxy(entry);
        }
        if let Some(auth) = &egress.auth {
            let password = secrets.resolve_egress(&auth.credential.slot)?;
            builder = builder.username(&auth.username).password(&password);
        }
        Some(builder.build().map_err(|_| "出站代理配置无效".to_owned())?)
    } else {
        None
    };
    let http = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(proxy)
            .build(),
    );
    let response = http
        .get(CATALOG_URL)
        .header("accept", "application/json")
        .header("user-agent", "token-station-desktop/price-catalog")
        .call()
        .map_err(|error| format!("公开价格目录请求失败：{error}"))?;
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(format!("公开价格目录拒绝重定向：HTTP {status}"));
    }
    if status >= 400 {
        return Err(format!("公开价格目录请求失败：HTTP {status}"));
    }
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(MAX_RESPONSE_BYTES + 1)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取公开价格目录失败：{error}"))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err("公开价格目录响应超过 8 MiB 限制".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "公开价格目录不是 UTF-8 JSON".to_owned())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATALOG: &str = r#"{
      "openai": {
        "id": "openai",
        "name": "OpenAI",
        "models": {
          "gpt-5": {
            "id": "gpt-5",
            "name": "GPT-5",
            "cost": {"input": 1.25, "output": 10, "cache_read": 0.125}
          }
        }
      },
      "anthropic": {
        "id": "anthropic",
        "name": "Anthropic",
        "models": {
          "claude-sonnet-4-6": {
            "id": "claude-sonnet-4-6",
            "name": "Claude Sonnet 4.6",
            "cost": {
              "input": 3,
              "output": 15,
              "cache_read": 0.3,
              "cache_write": 3.75
            }
          }
        }
      }
    }"#;

    #[test]
    fn maps_desktop_regional_presets_to_current_catalog_namespaces() {
        assert_eq!(normalize_provider_id("glm_cn"), "zhipuai");
        assert_eq!(normalize_provider_id("glm"), "zai");
        assert_eq!(normalize_provider_id("glm_coding"), "zai-coding-plan");
        assert_eq!(normalize_provider_id("kimi"), "moonshotai-cn");
        assert_eq!(normalize_provider_id("qwen"), "alibaba-cn");
        assert_eq!(normalize_provider_id("qwen_singapore"), "alibaba");
    }

    #[test]
    fn finds_an_exact_provider_model_and_converts_usd_to_micros() {
        let suggestion = suggest_from_json(CATALOG, Some("openai"), "gpt-5", 42)
            .unwrap()
            .unwrap();
        assert_eq!(suggestion.provider_id, "openai");
        assert_eq!(suggestion.model_id, "gpt-5");
        assert_eq!(suggestion.input_per_mtok, 1_250_000);
        assert_eq!(suggestion.output_per_mtok, 10_000_000);
        assert_eq!(suggestion.cache_read_per_mtok, 125_000);
        assert_eq!(suggestion.cache_write_per_mtok, 0);
        assert_eq!(suggestion.reasoning_per_mtok, None);
    }

    #[test]
    fn resolves_several_requested_models_from_one_catalog_parse() {
        let suggestions = suggest_many_from_json(
            CATALOG,
            Some("anthropic"),
            &["claude-sonnet-4.6@high".to_owned(), "missing".to_owned()],
            42,
        )
        .unwrap();

        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].requested_model_id, "claude-sonnet-4.6@high");
        assert_eq!(suggestions[0].suggestion.model_id, "claude-sonnet-4-6");
        assert_eq!(suggestions[0].suggestion.cache_write_per_mtok, 3_750_000);
    }

    #[test]
    fn infers_the_official_provider_and_normalizes_safe_aliases() {
        let suggestion = suggest_from_json(CATALOG, None, "anthropic/claude-sonnet-4.6@high", 42)
            .unwrap()
            .unwrap();
        assert_eq!(suggestion.provider_id, "anthropic");
        assert_eq!(suggestion.model_id, "claude-sonnet-4-6");
        assert_eq!(suggestion.cache_write_per_mtok, 3_750_000);
    }

    #[test]
    fn normalizes_a_dot_variant_with_a_date_snapshot_suffix() {
        let suggestion = suggest_from_json(
            CATALOG,
            None,
            "anthropic/claude-sonnet-4.6-20260101@high",
            42,
        )
        .unwrap()
        .unwrap();
        assert_eq!(suggestion.model_id, "claude-sonnet-4-6");
    }

    #[test]
    fn non_ascii_model_ids_never_panic_during_date_suffix_detection() {
        assert_eq!(strip_date_suffix("模型abcdefghi"), "模型abcdefghi");
    }

    #[test]
    fn does_not_guess_when_a_model_is_ambiguous_across_providers() {
        let duplicated = CATALOG.replace(
            r#""claude-sonnet-4-6": {"#,
            r#""gpt-5": {
              "id": "gpt-5",
              "name": "GPT-5 mirror",
              "cost": {"input": 2, "output": 12}
            },
            "claude-sonnet-4-6": {"#,
        );
        assert!(suggest_from_json(&duplicated, None, "unknown/gpt-5", 42)
            .unwrap()
            .is_none());
    }

    #[test]
    fn writes_and_reads_only_the_public_catalog_cache() {
        let data_dir = std::env::temp_dir().join(format!(
            "token-station-pricing-catalog-{}-{}",
            std::process::id(),
            now_ms()
        ));
        write_cache(&data_dir, CATALOG.as_bytes()).unwrap();
        let cached = read_cache(&data_dir).unwrap();
        assert_eq!(cached.body, CATALOG);
        assert!(cached.fresh);
        std::fs::remove_dir_all(data_dir).unwrap();
    }

    #[test]
    fn lists_only_current_text_models_for_requested_provider_channels() {
        let catalog = r#"{
          "alibaba-cn": {
            "id": "alibaba-cn",
            "name": "Alibaba (China)",
            "models": {
              "glm-5.2": {
                "id": "glm-5.2",
                "name": "GLM-5.2",
                "modalities": {"input": ["text"], "output": ["text"]}
              },
              "old-model": {
                "id": "old-model",
                "name": "Old",
                "status": "deprecated",
                "modalities": {"input": ["text"], "output": ["text"]}
              },
              "image-only": {
                "id": "image-only",
                "name": "Image",
                "modalities": {"input": ["text"], "output": ["image"]}
              },
              "text-embedding-3-small": {
                "id": "text-embedding-3-small",
                "name": "Embedding",
                "family": "text-embedding",
                "modalities": {"input": ["text"], "output": ["text"]}
              },
              "whisper-large-v3": {
                "id": "whisper-large-v3",
                "name": "Whisper",
                "family": "whisper",
                "modalities": {"input": ["audio"], "output": ["text"]}
              },
              "bad-id": {
                "id": "bad\u0000id",
                "name": "Bad",
                "modalities": {"input": ["text"], "output": ["text"]}
              }
            }
          },
          "empty": {
            "id": "empty",
            "name": "Empty after filtering",
            "models": {
              "image-only": {
                "id": "image-only",
                "name": "Image",
                "modalities": {"input": ["text"], "output": ["image"]}
              }
            }
          }
        }"#;

        let result = public_provider_models_from_json(
            catalog,
            &[
                "qwen".to_owned(),
                "empty".to_owned(),
                "volcengine_ark".to_owned(),
            ],
            42,
            "live",
        )
        .unwrap();

        assert_eq!(
            result.providers.get("qwen"),
            Some(&vec!["glm-5.2".to_owned()])
        );
        assert_eq!(
            result.unavailable_provider_ids,
            vec!["empty", "volcengine_ark"]
        );
        assert_eq!(result.source, "live");
        assert_eq!(result.fetched_at_ms, 42);
    }

    #[test]
    fn maps_all_supported_desktop_channel_aliases() {
        let expected = [
            ("gemini", "google"),
            ("nvidia_nim", "nvidia"),
            ("siliconflow", "siliconflow-cn"),
            ("siliconflow_global", "siliconflow"),
            ("together", "togetherai"),
            ("fireworks", "fireworks-ai"),
            ("stepfun", "stepfun"),
            ("stepfun_plan", "stepfun-step-plan"),
            ("xiaomi_mimo", "xiaomi"),
            ("novita", "novita-ai"),
        ];

        for (desktop, public) in expected {
            assert_eq!(normalize_provider_id(desktop), public);
        }

        assert_eq!(normalize_public_model_provider_id("qwen_singapore"), None);
        assert_eq!(normalize_public_model_provider_id("qwen_us"), None);
    }

    #[test]
    fn exact_provider_ids_win_before_regional_alias_normalization() {
        let catalog = r#"{
          "siliconflow": {
            "id": "siliconflow",
            "models": {"global-model": {"id": "global-model"}}
          },
          "siliconflow-cn": {
            "id": "siliconflow-cn",
            "models": {"china-model": {"id": "china-model"}}
          }
        }"#;

        let result = public_provider_models_from_json(
            catalog,
            &["siliconflow".to_owned(), "siliconflow_global".to_owned()],
            42,
            "live",
        )
        .unwrap();

        assert_eq!(result.providers["siliconflow"], vec!["china-model"]);
        assert_eq!(result.providers["siliconflow_global"], vec!["global-model"]);
    }
}
