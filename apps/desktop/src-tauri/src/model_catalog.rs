use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use token_station_protocol::{ProviderApi, ProviderEndpoint};

const CACHE_VERSION: u32 = 1;
const CACHE_FILE: &str = "model-catalog-cache.json";
const MAX_RESPONSE_BYTES: u64 = 2 * 1024 * 1024;
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(6);

static CACHE_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ModelDiscoveryView {
    pub(crate) models: Vec<String>,
    pub(crate) source: String,
    pub(crate) fetched_at_ms: Option<u64>,
    pub(crate) warning: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CacheEntry {
    base_url: String,
    models: Vec<String>,
    fetched_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct CacheFile {
    version: u32,
    providers: BTreeMap<String, CacheEntry>,
}

impl Default for CacheFile {
    fn default() -> Self {
        Self {
            version: CACHE_VERSION,
            providers: BTreeMap::new(),
        }
    }
}

pub(crate) fn discover_with_cache(
    data_dir: &Path,
    name: &str,
    base_url: &str,
    api_key: Option<&str>,
) -> Result<ModelDiscoveryView, String> {
    let name = name.trim();
    let base_url = base_url.trim().trim_end_matches('/');
    if name.is_empty() {
        return Err("请先填写供应商名称".to_owned());
    }
    if base_url.is_empty() {
        return Err("请先填写 Base URL".to_owned());
    }

    match fetch_models(base_url, api_key) {
        Ok(models) => {
            let fetched_at_ms = now_ms();
            let warning = write_cache(
                data_dir,
                name,
                CacheEntry {
                    base_url: base_url.to_owned(),
                    models: models.clone(),
                    fetched_at_ms,
                },
            )
            .err();
            Ok(ModelDiscoveryView {
                models,
                source: "live".to_owned(),
                fetched_at_ms: Some(fetched_at_ms),
                warning,
            })
        }
        Err(warning) => {
            if let Some(entry) = read_cached_entry(data_dir, name, base_url) {
                return Ok(ModelDiscoveryView {
                    models: entry.models,
                    source: "cache".to_owned(),
                    fetched_at_ms: Some(entry.fetched_at_ms),
                    warning: Some(warning),
                });
            }
            Ok(ModelDiscoveryView {
                models: vec![],
                source: "none".to_owned(),
                fetched_at_ms: None,
                warning: Some(warning),
            })
        }
    }
}

fn fetch_models(base_url: &str, api_key: Option<&str>) -> Result<Vec<String>, String> {
    let endpoint = ProviderEndpoint::try_new(base_url)
        .map_err(|error| format!("Base URL 不合法：{error}"))?;
    let url = endpoint.resolve(ProviderApi::Models);
    let http = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(DISCOVERY_TIMEOUT))
            .http_status_as_error(false)
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

fn parse_models(document: &Value) -> Result<Vec<String>, String> {
    let data = document
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "厂商未返回 OpenAI-compatible 的 data 模型列表".to_owned())?;
    let models: BTreeSet<String> = data
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(models.into_iter().collect())
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
    std::fs::read_to_string(cache_path(data_dir))
        .ok()
        .and_then(|text| serde_json::from_str::<CacheFile>(&text).ok())
        .filter(|cache| cache.version == CACHE_VERSION)
        .unwrap_or_default()
}

fn read_cached_entry(data_dir: &Path, name: &str, base_url: &str) -> Option<CacheEntry> {
    load_cache(data_dir)
        .providers
        .remove(name)
        .filter(|entry| entry.base_url == base_url)
}

fn write_cache(data_dir: &Path, name: &str, entry: CacheEntry) -> Result<(), String> {
    let _guard = CACHE_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "模型缓存写锁已损坏".to_owned())?;
    std::fs::create_dir_all(data_dir).map_err(|error| format!("创建数据目录失败：{error}"))?;
    let mut cache = load_cache(data_dir);
    cache.providers.insert(name.to_owned(), entry);
    let mut rendered = serde_json::to_string_pretty(&cache)
        .map_err(|error| format!("序列化模型缓存失败：{error}"))?;
    rendered.push('\n');

    let path = cache_path(data_dir);
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = data_dir.join(format!(
        ".{CACHE_FILE}.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    std::fs::write(&temporary, rendered).map_err(|error| format!("写模型缓存失败：{error}"))?;
    if let Err(error) = std::fs::rename(&temporary, &path) {
        std::fs::remove_file(&temporary).ok();
        return Err(format!("保存模型缓存失败：{error}"));
    }
    Ok(())
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
        discover_with_cache, fetch_models, parse_models, read_cached_entry, status_message,
        write_cache, CacheEntry,
    };
    use serde_json::json;
    use std::io::{Read, Write};

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

    #[test]
    fn standard_model_directories_are_trimmed_deduplicated_and_sorted() {
        let models = parse_models(&json!({
            "data": [
                {"id": "z-model"},
                {"id": " a-model "},
                {"id": "z-model"},
                {"object": "model"}
            ]
        }))
        .expect("standard model list parses");

        assert_eq!(models, ["a-model", "z-model"]);
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
        assert_eq!(models, ["model-a", "model-b"]);
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
            CacheEntry {
                base_url: "https://api.moonshot.cn/v1".to_owned(),
                models: vec!["kimi-k2.6".to_owned()],
                fetched_at_ms: 42,
            },
        )
        .expect("cache writes");

        let hit = read_cached_entry(&dir, "moonshot", "https://api.moonshot.cn/v1")
            .expect("matching cache is returned");
        assert_eq!(hit.models, ["kimi-k2.6"]);
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
                CacheEntry {
                    base_url: "https://api.deepseek.com/v1".to_owned(),
                    models: vec!["deepseek-chat".to_owned()],
                    fetched_at_ms: 1,
                },
            )
        });
        let second_dir = dir.clone();
        let second = std::thread::spawn(move || {
            write_cache(
                &second_dir,
                "moonshot",
                CacheEntry {
                    base_url: "https://api.moonshot.cn/v1".to_owned(),
                    models: vec!["moonshot-v1-8k".to_owned()],
                    fetched_at_ms: 2,
                },
            )
        });
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        assert!(read_cached_entry(&dir, "deepseek", "https://api.deepseek.com/v1").is_some());
        assert!(read_cached_entry(&dir, "moonshot", "https://api.moonshot.cn/v1").is_some());
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
            CacheEntry {
                base_url: "http://127.0.0.1:9".to_owned(),
                models: vec!["cached-model".to_owned()],
                fetched_at_ms: 42,
            },
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
            CacheEntry {
                base_url: "https://example.invalid/v1".to_owned(),
                models: vec!["model".to_owned()],
                fetched_at_ms: 1,
            },
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
