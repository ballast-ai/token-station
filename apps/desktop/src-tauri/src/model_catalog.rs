use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use token_station_protocol::{CapabilityState, ModelCapability, ProviderApi, ProviderEndpoint};

const CACHE_VERSION: u32 = 3;
const CACHE_FILE: &str = "model-catalog-cache.json";
const MAX_CACHE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PROVIDERS: usize = 128;
pub(crate) const MAX_MODELS_PER_PROVIDER: usize = 4_096;
pub(crate) const MAX_MODEL_ID_BYTES: usize = 512;
const MAX_PROVIDER_NAME_BYTES: usize = 128;
const MAX_BASE_URL_BYTES: usize = 2_048;
const MAX_REMOVED_MODELS: usize = 512;
const REMOVED_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(6);

static CACHE_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ModelDiscoveryView {
    pub(crate) models: Vec<String>,
    pub(crate) source: String,
    pub(crate) fetched_at_ms: Option<u64>,
    pub(crate) warning: Option<String>,
    pub(crate) capabilities_updated: bool,
    pub(crate) revision: u64,
    pub(crate) catalog: Vec<CatalogModelView>,
    pub(crate) added: Vec<String>,
    pub(crate) removed: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogSource {
    Live,
    Cache,
    Configured,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogState {
    Active,
    Stale,
    Removed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CatalogModelView {
    pub(crate) model: String,
    pub(crate) tool: CapabilityState,
    pub(crate) vision: CapabilityState,
    pub(crate) json_schema: CapabilityState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cost: Option<CatalogCostView>,
    pub(crate) source: CatalogSource,
    pub(crate) last_seen_ms: Option<u64>,
    pub(crate) catalog_state: CatalogState,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct CatalogCostView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_write: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CacheEntry {
    base_url: String,
    revision: u64,
    models: Vec<CatalogModelView>,
    fetched_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct CacheFile {
    version: u32,
    providers: BTreeMap<String, CacheEntry>,
}

#[derive(Debug, Deserialize)]
struct CacheFileV1 {
    version: u32,
    providers: BTreeMap<String, CacheEntryV1>,
}

#[derive(Debug, Deserialize)]
struct CacheEntryV1 {
    base_url: String,
    models: Vec<String>,
    fetched_at_ms: u64,
}

impl Default for CacheFile {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            providers: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
pub(crate) fn discover_with_cache(
    data_dir: &Path,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<ModelDiscoveryView, String> {
    discover_with_cache_egress(
        data_dir,
        name,
        base_url,
        api_key,
        &token_station_cli::config::EgressConfig::default(),
        &token_station_cli::secrets::SecretStore::default(),
    )
}

pub(crate) fn discover_with_cache_egress(
    data_dir: &Path,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
    egress: &token_station_cli::config::EgressConfig,
    secrets: &token_station_cli::secrets::SecretStore,
) -> Result<ModelDiscoveryView, String> {
    let name = name.trim();
    let base_url = base_url.trim().trim_end_matches('/');
    if name.is_empty() {
        return Err("请先填写供应商名称".to_owned());
    }
    if base_url.is_empty() {
        return Err("请先填写 Base URL".to_owned());
    }

    match fetch_models_with_egress(base_url, api_key, egress, secrets) {
        Ok(live_catalog) => {
            let fetched_at_ms = now_ms();
            let previous = read_cached_entry(data_dir, name, base_url);
            let (entry, added, removed) =
                merge_live_catalog(base_url, previous.as_ref(), &live_catalog, fetched_at_ms);
            let models = visible_models(&entry.models);
            let warning = write_cache(data_dir, name, entry.clone()).err();
            Ok(ModelDiscoveryView {
                models,
                source: "live".to_owned(),
                fetched_at_ms: Some(fetched_at_ms),
                warning,
                capabilities_updated: false,
                revision: entry.revision,
                catalog: entry.models,
                added,
                removed,
            })
        }
        Err(warning) => {
            if let Some(entry) = read_cached_entry(data_dir, name, base_url) {
                let catalog = stale_catalog(&entry);
                return Ok(ModelDiscoveryView {
                    models: visible_models(&catalog),
                    source: "cache".to_owned(),
                    fetched_at_ms: Some(entry.fetched_at_ms),
                    warning: Some(warning),
                    capabilities_updated: false,
                    revision: entry.revision,
                    catalog,
                    added: Vec::new(),
                    removed: Vec::new(),
                });
            }
            Ok(ModelDiscoveryView {
                models: vec![],
                source: "none".to_owned(),
                fetched_at_ms: None,
                warning: Some(warning),
                capabilities_updated: false,
                revision: 0,
                catalog: Vec::new(),
                added: Vec::new(),
                removed: Vec::new(),
            })
        }
    }
}

fn unknown_catalog_model(
    model: String,
    source: CatalogSource,
    last_seen_ms: Option<u64>,
    catalog_state: CatalogState,
) -> CatalogModelView {
    CatalogModelView {
        model,
        tool: CapabilityState::Unknown,
        vision: CapabilityState::Unknown,
        json_schema: CapabilityState::Unknown,
        context_window: None,
        max_output_tokens: None,
        cost: None,
        source,
        last_seen_ms,
        catalog_state,
    }
}

fn merge_live_catalog(
    base_url: &str,
    previous: Option<&CacheEntry>,
    live_models: &[CatalogModelView],
    fetched_at_ms: u64,
) -> (CacheEntry, Vec<String>, Vec<String>) {
    let previous_by_model: BTreeMap<&str, &CatalogModelView> = previous
        .into_iter()
        .flat_map(|entry| &entry.models)
        .map(|model| (model.model.as_str(), model))
        .collect();
    let live: BTreeSet<&str> = live_models
        .iter()
        .map(|model| model.model.as_str())
        .collect();
    let mut added = Vec::new();
    let mut catalog = Vec::new();

    for live_model in live_models {
        let model = &live_model.model;
        let mut record = previous_by_model.get(model.as_str()).map_or_else(
            || {
                added.push(model.clone());
                live_model.clone()
            },
            |existing| (*existing).clone(),
        );
        if record.catalog_state == CatalogState::Removed {
            added.push(model.clone());
        }
        record.source = CatalogSource::Live;
        record.last_seen_ms = Some(fetched_at_ms);
        record.catalog_state = CatalogState::Active;
        if live_model.tool != CapabilityState::Unknown {
            record.tool = live_model.tool;
        }
        if live_model.vision != CapabilityState::Unknown {
            record.vision = live_model.vision;
        }
        if live_model.json_schema != CapabilityState::Unknown {
            record.json_schema = live_model.json_schema;
        }
        if live_model.context_window.is_some() {
            record.context_window = live_model.context_window;
        }
        if live_model.max_output_tokens.is_some() {
            record.max_output_tokens = live_model.max_output_tokens;
        }
        if live_model.cost.is_some() {
            record.cost = live_model.cost.clone();
        }
        catalog.push(record);
    }

    let mut removed = Vec::new();
    for old in previous.into_iter().flat_map(|entry| &entry.models) {
        if !live.contains(old.model.as_str()) {
            let mut record = old.clone();
            if record.catalog_state != CatalogState::Removed {
                removed.push(record.model.clone());
            }
            record.catalog_state = CatalogState::Removed;
            catalog.push(record);
        }
    }
    catalog.sort_by(|left, right| left.model.cmp(&right.model));
    added.sort();
    added.dedup();
    removed.sort();

    (
        CacheEntry {
            base_url: base_url.to_owned(),
            revision: previous.map_or(1, |entry| entry.revision.saturating_add(1)),
            models: catalog,
            fetched_at_ms,
        },
        added,
        removed,
    )
}

fn stale_catalog(entry: &CacheEntry) -> Vec<CatalogModelView> {
    entry
        .models
        .iter()
        .cloned()
        .map(|mut model| {
            if model.catalog_state != CatalogState::Removed {
                model.catalog_state = CatalogState::Stale;
                model.source = CatalogSource::Cache;
            }
            model
        })
        .collect()
}

fn visible_models(catalog: &[CatalogModelView]) -> Vec<String> {
    catalog
        .iter()
        .filter(|model| model.catalog_state != CatalogState::Removed)
        .map(|model| model.model.clone())
        .collect()
}

#[cfg(test)]
fn fetch_models(base_url: &str, api_key: Option<&str>) -> Result<Vec<CatalogModelView>, String> {
    fetch_models_with_egress(
        base_url,
        api_key,
        &token_station_cli::config::EgressConfig::default(),
        &token_station_cli::secrets::SecretStore::default(),
    )
}

fn fetch_models_with_egress(
    base_url: &str,
    api_key: Option<&str>,
    egress: &token_station_cli::config::EgressConfig,
    secrets: &token_station_cli::secrets::SecretStore,
) -> Result<Vec<CatalogModelView>, String> {
    let endpoint =
        ProviderEndpoint::try_new(base_url).map_err(|error| format!("Base URL 不合法：{error}"))?;
    let url = endpoint.resolve(ProviderApi::Models);
    let proxy = if let Some((scheme, host, port)) = egress.proxy_parts()? {
        let protocol = match scheme.as_str() {
            "http" => ureq::ProxyProtocol::Http,
            "https" => ureq::ProxyProtocol::Https,
            "socks5" => ureq::ProxyProtocol::Socks5,
            "socks5h" => ureq::ProxyProtocol::Socks5h,
            _ => return Err("不支持的出站代理协议".to_string()),
        };
        let mut builder = ureq::Proxy::builder(protocol).host(&host).port(port);
        for entry in &egress.no_proxy {
            builder = builder.no_proxy(entry);
        }
        if let Some(auth) = &egress.auth {
            let password = secrets.resolve_egress(&auth.credential.slot)?;
            builder = builder.username(&auth.username).password(&password);
        }
        Some(
            builder
                .build()
                .map_err(|_| "出站代理配置无效".to_string())?,
        )
    } else {
        None
    };
    let http = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(DISCOVERY_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .proxy(proxy)
            .build(),
    );
    let mut request = http
        .get(&url)
        .header("accept", "application/json")
        .header("user-agent", "token-station-desktop/model-discovery");
    if let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) {
        request = request.header("authorization", &format!("Bearer {key}"));
    }

    let response = request
        .call()
        .map_err(|error| format!("模型目录请求失败：{error}"))?;
    let status = response.status().as_u16();
    if (300..400).contains(&status) {
        return Err(format!("模型目录拒绝上游重定向：HTTP {status}"));
    }
    if status >= 400 {
        return Err(status_message(status));
    }

    let mut bytes = Vec::new();
    response
        .into_body()
        .into_with_config()
        .limit(MAX_RESPONSE_BYTES)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取模型目录失败：{error}"))?;
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| "厂商返回的模型目录不是有效 JSON".to_owned())?;
    parse_models(&document)
}

fn parse_models(document: &Value) -> Result<Vec<CatalogModelView>, String> {
    let data = document
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "厂商未返回 OpenAI-compatible 的 data 模型列表".to_owned())?;
    let mut models = BTreeMap::<String, CatalogModelView>::new();
    for item in data {
        let Some(model) = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if model.len() > MAX_MODEL_ID_BYTES {
            return Err(format!("模型 ID 超过 {MAX_MODEL_ID_BYTES} 字节上限"));
        }
        let vision = explicit_image_input_state(item);
        let context_window = bounded_u32(item.get("context_window"))
            .or_else(|| bounded_u32(item.pointer("/limit/context")));
        let max_output_tokens = bounded_u32(item.get("max_output_tokens"))
            .or_else(|| bounded_u32(item.pointer("/limit/output")));
        let cost = catalog_cost(item.get("cost"));
        models
            .entry(model.to_owned())
            .and_modify(|existing| {
                if vision != CapabilityState::Unknown {
                    existing.vision = vision;
                }
                if context_window.is_some() {
                    existing.context_window = context_window;
                }
                if max_output_tokens.is_some() {
                    existing.max_output_tokens = max_output_tokens;
                }
                if cost.is_some() {
                    existing.cost = cost.clone();
                }
            })
            .or_insert_with(|| CatalogModelView {
                model: model.to_owned(),
                tool: CapabilityState::Unknown,
                vision,
                json_schema: CapabilityState::Unknown,
                context_window,
                max_output_tokens,
                cost,
                source: CatalogSource::Live,
                last_seen_ms: None,
                catalog_state: CatalogState::Active,
            });
        if models.len() > MAX_MODELS_PER_PROVIDER {
            return Err(format!(
                "模型数量超过单供应商 {MAX_MODELS_PER_PROVIDER} 个上限"
            ));
        }
    }
    Ok(models.into_values().collect())
}

fn bounded_u32(value: Option<&Value>) -> Option<u32> {
    let raw = value?.as_u64()?;
    let value = u32::try_from(raw).ok()?;
    (value > 0).then_some(value)
}

fn catalog_cost(value: Option<&Value>) -> Option<CatalogCostView> {
    let object = value?.as_object()?;
    let rate = |name: &str| {
        object
            .get(name)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 9_000_000_000.0)
    };
    let cost = CatalogCostView {
        input: rate("input"),
        output: rate("output"),
        cache_read: rate("cache_read"),
        cache_write: rate("cache_write"),
    };
    (cost.input.is_some() && cost.output.is_some()).then_some(cost)
}

fn explicit_image_input_state(model: &Value) -> CapabilityState {
    if let Some(supported) = model
        .get("supportsImage")
        .or_else(|| model.get("supports_image"))
        .or_else(|| model.get("vision"))
        .and_then(Value::as_bool)
    {
        return if supported {
            CapabilityState::Verified
        } else {
            CapabilityState::Unsupported
        };
    }

    [
        model.pointer("/architecture/input_modalities"),
        model.pointer("/modalities/input"),
        model.get("input_modalities"),
        model.get("inputModalities"),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| {
        value.as_array().map(|modalities| {
            if modalities.iter().any(|modality| {
                modality
                    .as_str()
                    .map(str::trim)
                    .is_some_and(|modality| modality.eq_ignore_ascii_case("image"))
            }) {
                CapabilityState::Verified
            } else {
                CapabilityState::Unsupported
            }
        })
    })
    .unwrap_or(CapabilityState::Unknown)
}

fn status_message(status: u16) -> String {
    match status {
        401 | 403 => "Key 无效，或当前账号没有读取模型目录的权限".to_owned(),
        404 => "该厂商未提供标准 /models 目录，请使用内置建议或手动输入".to_owned(),
        429 => "模型目录请求过于频繁，请稍后手动刷新".to_owned(),
        _ => format!("模型目录返回 HTTP {status}"),
    }
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CACHE_FILE)
}

fn load_cache(data_dir: &Path) -> CacheFile {
    let path = cache_path(data_dir);
    let Some(metadata) = std::fs::metadata(&path).ok() else {
        return CacheFile::default();
    };
    if metadata.len() > MAX_CACHE_BYTES {
        return CacheFile::default();
    }
    let Some(text) = std::fs::read_to_string(path).ok() else {
        return CacheFile::default();
    };
    if let Some(cache) = serde_json::from_str::<CacheFile>(&text)
        .ok()
        .filter(|cache| cache.version == CACHE_VERSION)
    {
        return bounded_cache(cache, now_ms());
    }
    let Some(old) = serde_json::from_str::<CacheFileV1>(&text)
        .ok()
        .filter(|cache| cache.version == 1)
    else {
        return CacheFile::default();
    };
    bounded_cache(
        CacheFile {
            version: CACHE_VERSION,
            providers: old
                .providers
                .into_iter()
                .map(|(name, entry)| {
                    let models = entry
                        .models
                        .into_iter()
                        .map(|model| {
                            unknown_catalog_model(
                                model,
                                CatalogSource::Cache,
                                Some(entry.fetched_at_ms),
                                CatalogState::Active,
                            )
                        })
                        .collect();
                    (
                        name,
                        CacheEntry {
                            base_url: entry.base_url,
                            revision: 1,
                            models,
                            fetched_at_ms: entry.fetched_at_ms,
                        },
                    )
                })
                .collect(),
        },
        now_ms(),
    )
}

fn bounded_cache(mut cache: CacheFile, now: u64) -> CacheFile {
    let removed_cutoff = now.saturating_sub(REMOVED_TTL_MS);
    cache.providers.retain(|name, entry| {
        if name.is_empty()
            || name.len() > MAX_PROVIDER_NAME_BYTES
            || entry.base_url.len() > MAX_BASE_URL_BYTES
        {
            return false;
        }
        entry.models.retain(|model| {
            !model.model.is_empty()
                && model.model.len() <= MAX_MODEL_ID_BYTES
                && (model.catalog_state != CatalogState::Removed
                    || model
                        .last_seen_ms
                        .is_some_and(|seen| seen >= removed_cutoff))
        });
        entry.models.sort_by(|left, right| {
            let left_removed = left.catalog_state == CatalogState::Removed;
            let right_removed = right.catalog_state == CatalogState::Removed;
            left_removed
                .cmp(&right_removed)
                .then_with(|| right.last_seen_ms.cmp(&left.last_seen_ms))
                .then_with(|| left.model.cmp(&right.model))
        });
        let mut removed_kept = 0usize;
        entry.models.retain(|model| {
            if model.catalog_state != CatalogState::Removed {
                return true;
            }
            removed_kept = removed_kept.saturating_add(1);
            removed_kept <= MAX_REMOVED_MODELS
        });
        entry.models.truncate(MAX_MODELS_PER_PROVIDER);
        entry
            .models
            .sort_by(|left, right| left.model.cmp(&right.model));
        true
    });
    if cache.providers.len() > MAX_PROVIDERS {
        let mut oldest: Vec<_> = cache
            .providers
            .iter()
            .map(|(name, entry)| (entry.fetched_at_ms, name.clone()))
            .collect();
        oldest.sort();
        for (_, name) in oldest
            .into_iter()
            .take(cache.providers.len() - MAX_PROVIDERS)
        {
            cache.providers.remove(&name);
        }
    }
    cache
}

fn read_cached_entry(data_dir: &Path, name: &str, base_url: &str) -> Option<CacheEntry> {
    load_cache(data_dir)
        .providers
        .remove(name)
        .filter(|entry| entry.base_url == base_url)
}

pub(crate) fn catalog_for_provider(
    data_dir: &Path,
    name: &str,
    base_url: &str,
    configured: &[ModelCapability],
) -> (u64, Vec<CatalogModelView>) {
    let cached = read_cached_entry(data_dir, name, base_url);
    let revision = cached.as_ref().map_or(0, |entry| entry.revision);
    let mut catalog: BTreeMap<String, CatalogModelView> = cached
        .into_iter()
        .flat_map(|entry| entry.models)
        .map(|model| (model.model.clone(), model))
        .collect();
    for capability in configured {
        catalog
            .entry(capability.model.clone())
            .and_modify(|model| {
                model.tool = capability.tool_state();
                model.vision = capability.vision_state();
                model.json_schema = capability.json_schema_state();
                if capability.context_window > 0 {
                    model.context_window = Some(capability.context_window);
                }
                if let Some(value) = capability
                    .extensions
                    .get("max_output_tokens")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0)
                {
                    model.max_output_tokens = Some(value);
                }
                if let Some(value) = capability
                    .extensions
                    .get("catalog_cost")
                    .and_then(|value| serde_json::from_value(value.clone()).ok())
                {
                    model.cost = Some(value);
                }
            })
            .or_insert_with(|| CatalogModelView {
                model: capability.model.clone(),
                tool: capability.tool_state(),
                vision: capability.vision_state(),
                json_schema: capability.json_schema_state(),
                context_window: (capability.context_window > 0)
                    .then_some(capability.context_window),
                max_output_tokens: capability
                    .extensions
                    .get("max_output_tokens")
                    .and_then(Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .filter(|value| *value > 0),
                cost: capability
                    .extensions
                    .get("catalog_cost")
                    .and_then(|value| serde_json::from_value(value.clone()).ok()),
                source: CatalogSource::Configured,
                last_seen_ms: None,
                catalog_state: CatalogState::Active,
            });
    }
    (revision, catalog.into_values().collect())
}

fn write_cache(data_dir: &Path, name: &str, entry: CacheEntry) -> Result<(), String> {
    let _guard = CACHE_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "模型缓存写锁已损坏".to_owned())?;
    if name.is_empty() || name.len() > MAX_PROVIDER_NAME_BYTES {
        return Err(format!("供应商名称超过 {MAX_PROVIDER_NAME_BYTES} 字节上限"));
    }
    if entry.base_url.len() > MAX_BASE_URL_BYTES {
        return Err(format!(
            "供应商 Base URL 超过 {MAX_BASE_URL_BYTES} 字节上限"
        ));
    }
    if entry.models.len() > MAX_MODELS_PER_PROVIDER
        || entry
            .models
            .iter()
            .any(|model| model.model.is_empty() || model.model.len() > MAX_MODEL_ID_BYTES)
    {
        return Err("模型缓存条目超过资源上限".to_owned());
    }
    std::fs::create_dir_all(data_dir).map_err(|error| format!("创建数据目录失败：{error}"))?;
    let mut cache = load_cache(data_dir);
    cache.providers.insert(name.to_owned(), entry);
    persist_cache(data_dir, &bounded_cache(cache, now_ms()))
}

/// Invalidates every catalog fact tied to one Provider identity.
///
/// A deleted/re-added Provider may point at the same URL with a different
/// account. Keeping the old live catalog would then turn another account's
/// observations into trusted facts, so lifecycle changes fail closed by
/// dropping this derived cache.
pub(crate) fn remove_provider(data_dir: &Path, name: &str) -> Result<bool, String> {
    let _guard = CACHE_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "模型缓存写锁已损坏".to_owned())?;
    let mut cache = load_cache(data_dir);
    if cache.providers.remove(name).is_none() {
        return Ok(false);
    }
    persist_cache(data_dir, &cache)?;
    Ok(true)
}

fn persist_cache(data_dir: &Path, cache: &CacheFile) -> Result<(), String> {
    let mut rendered = serde_json::to_string_pretty(&cache)
        .map_err(|error| format!("序列化模型缓存失败：{error}"))?;
    rendered.push('\n');
    if u64::try_from(rendered.len()).unwrap_or(u64::MAX) > MAX_CACHE_BYTES {
        return Err(format!(
            "模型缓存超过 {} MiB 上限",
            MAX_CACHE_BYTES / 1024 / 1024
        ));
    }

    crate::agent_integration::safe_fs::write_atomic_private(
        &cache_path(data_dir),
        rendered.as_bytes(),
    )
    .map_err(|error| format!("保存模型缓存失败：{error}"))
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        catalog_for_provider, discover_with_cache, fetch_models, parse_models, read_cached_entry,
        remove_provider, status_message, unknown_catalog_model, write_cache, CacheEntry,
        CatalogSource, CatalogState, MAX_MODELS_PER_PROVIDER, MAX_MODEL_ID_BYTES,
    };
    use serde_json::json;
    use std::io::{Read, Write};
    use token_station_protocol::{CapabilityState, ModelCapability};

    fn scratch(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "token-station-model-cache-{}-{name}-{nonce}",
            std::process::id()
        ))
    }

    fn serve_once(status: u16, body: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("test port binds");
        let address = listener.local_addr().expect("test address exists");
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("one request arrives");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let response = format!(
                "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("test response writes");
        });
        format!("http://{address}")
    }

    fn serve_sequence(bodies: Vec<&'static str>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("test port binds");
        let address = listener.local_addr().expect("test address exists");
        std::thread::spawn(move || {
            for body in bodies {
                let (mut stream, _) = listener.accept().expect("request arrives");
                let mut request = [0_u8; 2048];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 200 Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("test response writes");
            }
        });
        format!("http://{address}")
    }

    fn cache_entry(base_url: &str, models: &[&str], fetched_at_ms: u64) -> CacheEntry {
        CacheEntry {
            base_url: base_url.to_owned(),
            revision: 1,
            models: models
                .iter()
                .map(|model| {
                    unknown_catalog_model(
                        (*model).to_owned(),
                        CatalogSource::Cache,
                        Some(fetched_at_ms),
                        CatalogState::Active,
                    )
                })
                .collect(),
            fetched_at_ms,
        }
    }

    #[test]
    fn standard_model_directories_preserve_explicit_image_modalities() {
        let models = parse_models(&json!({
            "data": [
                {
                    "id": "z-model",
                    "architecture": {"input_modalities": ["text", "image"]}
                },
                {
                    "id": " a-model ",
                    "architecture": {"input_modalities": ["text"]}
                },
                {"id": "z-model"},
                {"object": "model"}
            ]
        }))
        .expect("standard model list parses");

        assert_eq!(models[0].model, "a-model");
        assert_eq!(models[0].vision, CapabilityState::Unsupported);
        assert_eq!(models[1].model, "z-model");
        assert_eq!(models[1].vision, CapabilityState::Verified);
    }

    #[test]
    fn wecoding_model_directory_preserves_limits_and_known_cost_fields() {
        let models = parse_models(&json!({
            "data": [{
                "id": "glm-5.2",
                "context_window": 257550,
                "max_output_tokens": 32768,
                "limit": {"context": 257550, "output": 32768},
                "cost": {"cache_read": 0.04, "input": 0.2, "output": 0.6, "think": 0.8}
            }]
        }))
        .expect("Wecoding model metadata parses");

        let model = &models[0];
        assert_eq!(model.context_window, Some(257_550));
        assert_eq!(model.max_output_tokens, Some(32_768));
        assert_eq!(model.cost.as_ref().and_then(|cost| cost.input), Some(0.2));
        assert_eq!(model.cost.as_ref().and_then(|cost| cost.output), Some(0.6));
        assert_eq!(
            model.cost.as_ref().and_then(|cost| cost.cache_read),
            Some(0.04)
        );
        assert_eq!(model.cost.as_ref().and_then(|cost| cost.cache_write), None);
    }

    #[test]
    fn invalid_or_zero_model_limits_are_left_unknown() {
        let models = parse_models(&json!({
            "data": [{
                "id": "unsafe",
                "context_window": 0,
                "max_output_tokens": 4294967296_u64,
                "cost": {"input": -1, "output": "secret"}
            }]
        }))
        .expect("invalid optional metadata does not reject the catalog");

        assert_eq!(models[0].context_window, None);
        assert_eq!(models[0].max_output_tokens, None);
        assert_eq!(models[0].cost, None);
    }

    #[test]
    fn partial_catalog_costs_remain_unknown_until_input_and_output_are_present() {
        let models = parse_models(&json!({
            "data": [
                {"id": "input-only", "cost": {"input": 0.2}},
                {"id": "output-only", "cost": {"output": 0.6}},
                {"id": "cache-only", "cost": {"cache_read": 0.04}},
                {"id": "complete", "cost": {"input": 0.2, "output": 0.6}},
                {"id": "duplicate", "cost": {"input": 0.2}},
                {"id": "duplicate", "cost": {"output": 0.6}}
            ]
        }))
        .expect("partial costs do not reject the model directory");

        for model in ["cache-only", "duplicate", "input-only", "output-only"] {
            assert_eq!(
                models.iter().find(|item| item.model == model).unwrap().cost,
                None,
                "{model} must not expose an incomplete price"
            );
        }
        let complete = models
            .iter()
            .find(|item| item.model == "complete")
            .unwrap()
            .cost
            .as_ref()
            .unwrap();
        assert_eq!(complete.input, Some(0.2));
        assert_eq!(complete.output, Some(0.6));
    }

    #[test]
    fn model_directory_rejects_unbounded_cardinality_and_identifier_size() {
        let oversized = "x".repeat(MAX_MODEL_ID_BYTES + 1);
        let error = parse_models(&json!({"data": [{"id": oversized}]})).unwrap_err();
        assert!(error.contains("模型 ID"));

        let data: Vec<_> = (0..=MAX_MODELS_PER_PROVIDER)
            .map(|index| json!({"id": format!("model-{index}")}))
            .collect();
        let error = parse_models(&json!({"data": data})).unwrap_err();
        assert!(error.contains("模型数量"));
    }

    #[test]
    fn nonstandard_documents_are_refused_without_echoing_the_document() {
        let error = parse_models(&json!({"secret": "sk-never-log"}))
            .expect_err("a missing data list is not compatible");

        assert!(error.contains("data"), "{error}");
        assert!(!error.contains("sk-never-log"), "{error}");
    }

    #[test]
    fn live_model_directories_use_the_standard_models_endpoint() {
        let base = serve_once(200, r#"{"data":[{"id":"model-b"},{"id":"model-a"}]}"#);

        let models = fetch_models(&base, Some("sk-test-only")).expect("live directory parses");
        assert_eq!(
            models
                .iter()
                .map(|model| model.model.as_str())
                .collect::<Vec<_>>(),
            ["model-a", "model-b"]
        );
    }

    #[test]
    fn status_errors_are_actionable_and_never_echo_credentials() {
        let base = serve_once(401, r#"{"error":"sk-secret-must-not-escape"}"#);

        let error = fetch_models(&base, Some("sk-secret-must-not-escape"))
            .expect_err("unauthorized directory is refused");
        assert!(error.contains("Key 无效"), "{error}");
        assert!(!error.contains("sk-secret-must-not-escape"), "{error}");
    }

    #[test]
    fn cache_matches_both_provider_name_and_base_url() {
        let dir = scratch("round-trip");
        write_cache(
            &dir,
            "moonshot",
            cache_entry("https://api.moonshot.cn/v1", &["kimi-k2.6"], 42),
        )
        .expect("cache writes");

        let hit = read_cached_entry(&dir, "moonshot", "https://api.moonshot.cn/v1")
            .expect("matching cache is returned");
        assert_eq!(hit.models[0].model, "kimi-k2.6");
        assert!(read_cached_entry(&dir, "moonshot", "https://example.com/v1").is_none());

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn concurrent_cache_updates_preserve_every_provider() {
        let dir = scratch("concurrent");
        let first_dir = dir.clone();
        let first = std::thread::spawn(move || {
            write_cache(
                &first_dir,
                "deepseek",
                cache_entry("https://api.deepseek.com/v1", &["deepseek-chat"], 1),
            )
        });
        let second_dir = dir.clone();
        let second = std::thread::spawn(move || {
            write_cache(
                &second_dir,
                "moonshot",
                cache_entry("https://api.moonshot.cn/v1", &["moonshot-v1-8k"], 2),
            )
        });
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        assert!(read_cached_entry(&dir, "deepseek", "https://api.deepseek.com/v1").is_some());
        assert!(read_cached_entry(&dir, "moonshot", "https://api.moonshot.cn/v1").is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn provider_identity_changes_invalidate_only_its_catalog() {
        let dir = scratch("identity-invalidation");
        write_cache(
            &dir,
            "old-account",
            cache_entry("https://api.example.test/v1", &["private-model"], 1),
        )
        .unwrap();
        write_cache(
            &dir,
            "other",
            cache_entry("https://other.example.test/v1", &["other-model"], 2),
        )
        .unwrap();

        assert!(remove_provider(&dir, "old-account").unwrap());
        assert!(!remove_provider(&dir, "old-account").unwrap());
        assert!(
            read_cached_entry(&dir, "old-account", "https://api.example.test/v1").is_none(),
            "a re-added Provider cannot inherit the deleted account's catalog"
        );
        assert!(
            read_cached_entry(&dir, "other", "https://other.example.test/v1").is_some(),
            "unrelated Provider catalogs remain intact"
        );

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_damaged_cache_is_treated_as_empty() {
        let dir = scratch("damaged");
        std::fs::create_dir_all(&dir).expect("scratch directory exists");
        std::fs::write(dir.join("model-catalog-cache.json"), "not-json")
            .expect("damaged cache fixture writes");

        assert!(read_cached_entry(&dir, "moonshot", "https://api.moonshot.cn/v1").is_none());

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn discovery_prefers_live_then_falls_back_to_matching_cache_or_none() {
        let dir = scratch("discovery-fallbacks");
        let live_base = serve_once(200, r#"{"data":[{"id":"live-model"}]}"#);
        let live = discover_with_cache(&dir, " fixture ", &format!("{live_base}/"), Some(" key "))
            .expect("live discovery succeeds");
        assert_eq!(live.models, ["live-model"]);
        assert_eq!(live.source, "live");
        assert!(live.fetched_at_ms.is_some());
        assert!(live.warning.is_none());

        let cached = discover_with_cache(&dir, "fixture", "http://127.0.0.1:9", None)
            .expect("network failure uses the matching cache only when its URL matches");
        assert_eq!(cached.source, "none");
        assert!(cached.warning.is_some());
        write_cache(
            &dir,
            "offline",
            cache_entry("http://127.0.0.1:9", &["cached-model"], 42),
        )
        .unwrap();
        let cached = discover_with_cache(&dir, "offline", "http://127.0.0.1:9", None).unwrap();
        assert_eq!(cached.models, ["cached-model"]);
        assert_eq!(cached.source, "cache");
        assert_eq!(cached.fetched_at_ms, Some(42));
        assert!(discover_with_cache(&dir, "", "http://example.invalid", None).is_err());
        assert!(discover_with_cache(&dir, "provider", "", None).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn live_refresh_versions_the_catalog_and_retains_removed_models() {
        let dir = scratch("catalog-diff");
        let base = serve_sequence(vec![
            r#"{"data":[{"id":"model-a"},{"id":"model-b"}]}"#,
            r#"{"data":[{"id":"model-b"},{"id":"model-c"}]}"#,
        ]);

        let first = discover_with_cache(&dir, "fixture", &base, None).unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(first.added, ["model-a", "model-b"]);
        assert!(first.removed.is_empty());

        let second = discover_with_cache(&dir, "fixture", &base, None).unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(second.added, ["model-c"]);
        assert_eq!(second.removed, ["model-a"]);
        assert_eq!(second.models, ["model-b", "model-c"]);
        assert_eq!(
            second
                .catalog
                .iter()
                .find(|model| model.model == "model-a")
                .unwrap()
                .catalog_state,
            CatalogState::Removed
        );

        let stale = discover_with_cache(&dir, "fixture", &base, None).unwrap();
        assert_eq!(stale.source, "cache");
        assert!(stale.catalog.iter().any(|model| {
            model.model == "model-b" && model.catalog_state == CatalogState::Stale
        }));
        assert!(stale.catalog.iter().any(|model| {
            model.model == "model-a" && model.catalog_state == CatalogState::Removed
        }));

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn v1_cache_migrates_and_configured_capabilities_overlay_without_fake_live_evidence() {
        let dir = scratch("v1-migration");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("model-catalog-cache.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "providers": {
                    "fixture": {
                        "base_url": "https://example.test/v1",
                        "models": ["cached-model"],
                        "fetched_at_ms": 42
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let configured = ModelCapability {
            model: "manual-model".to_owned(),
            tool_state: Some(CapabilityState::Declared),
            ..ModelCapability::default()
        };

        let (revision, catalog) =
            catalog_for_provider(&dir, "fixture", "https://example.test/v1", &[configured]);

        assert_eq!(revision, 1);
        let cached = catalog
            .iter()
            .find(|model| model.model == "cached-model")
            .unwrap();
        assert_eq!(cached.source, CatalogSource::Cache);
        assert_eq!(cached.tool, CapabilityState::Unknown);
        let manual = catalog
            .iter()
            .find(|model| model.model == "manual-model")
            .unwrap();
        assert_eq!(manual.source, CatalogSource::Configured);
        assert_eq!(manual.tool, CapabilityState::Declared);
        assert!(manual.last_seen_ms.is_none());

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn discovery_rejects_invalid_json_and_reports_status_and_cache_commit_failures() {
        let invalid = serve_once(200, "not-json");
        assert!(fetch_models(&invalid, None)
            .unwrap_err()
            .contains("有效 JSON"));
        assert!(status_message(404).contains("/models"));
        assert!(status_message(429).contains("频繁"));
        assert!(status_message(503).contains("503"));

        let dir = scratch("rename-failure");
        std::fs::create_dir_all(dir.join("model-catalog-cache.json")).unwrap();
        let error = write_cache(
            &dir,
            "fixture",
            cache_entry("https://example.invalid/v1", &["model"], 1),
        )
        .unwrap_err();
        assert!(error.contains("保存模型缓存失败"));
        assert_eq!(
            std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
