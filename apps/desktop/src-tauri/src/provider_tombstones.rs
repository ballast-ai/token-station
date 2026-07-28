//! Recoverable Provider deletion sidecar.
//!
//! The live config drops an unreferenced Provider, but its exact JSON is first
//! archived here. Values contain only secret references, never plaintext keys.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const VERSION: u32 = 1;
const FILE: &str = "provider-tombstones.json";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Tombstone {
    deleted_at_ms: u64,
    provider: Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TombstoneFile {
    version: u32,
    providers: BTreeMap<String, Tombstone>,
}

impl Default for TombstoneFile {
    fn default() -> Self {
        Self {
            version: VERSION,
            providers: BTreeMap::new(),
        }
    }
}

pub(crate) fn archive(data_dir: &Path, name: &str, provider: &Value) -> Result<(), String> {
    let mut file = load(data_dir)?;
    if file.providers.contains_key(name) {
        return Err(format!(
            "Provider 回收站已有 `{name}`，请先恢复后再编辑，已拒绝覆盖旧恢复点"
        ));
    }
    file.providers.insert(
        name.to_owned(),
        Tombstone {
            deleted_at_ms: now_ms(),
            provider: provider.clone(),
        },
    );
    persist(data_dir, &file)
}

pub(crate) fn contains(data_dir: &Path, name: &str) -> Result<bool, String> {
    Ok(load(data_dir)?.providers.contains_key(name))
}

pub(crate) fn get(data_dir: &Path, name: &str) -> Result<Option<Value>, String> {
    Ok(load(data_dir)?
        .providers
        .get(name)
        .map(|tombstone| tombstone.provider.clone()))
}

pub(crate) fn take(data_dir: &Path, name: &str) -> Result<Option<Value>, String> {
    let mut file = load(data_dir)?;
    let Some(tombstone) = file.providers.remove(name) else {
        return Ok(None);
    };
    persist(data_dir, &file)?;
    Ok(Some(tombstone.provider))
}

pub(crate) fn discard(data_dir: &Path, name: &str) -> Result<(), String> {
    let mut file = load(data_dir)?;
    if file.providers.remove(name).is_some() {
        persist(data_dir, &file)?;
    }
    Ok(())
}

pub(crate) fn list(data_dir: &Path) -> Result<Vec<String>, String> {
    Ok(load(data_dir)?
        .providers
        .into_iter()
        .filter_map(|(name, tombstone)| {
            (tombstone.provider["access_tier"].as_str() != Some("free")).then_some(name)
        })
        .collect())
}

fn path(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE)
}

fn load(data_dir: &Path) -> Result<TombstoneFile, String> {
    let tombstone_path = path(data_dir);
    let text = match std::fs::read_to_string(&tombstone_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TombstoneFile::default());
        }
        Err(error) => return Err(format!("Provider 回收站不可读：{error}")),
    };
    let file: TombstoneFile = serde_json::from_str(&text)
        .map_err(|error| format!("Provider 回收站损坏，已拒绝覆盖：{error}"))?;
    if file.version != VERSION {
        return Err(format!(
            "Provider 回收站版本 {} 不受支持，已拒绝覆盖",
            file.version
        ));
    }
    Ok(file)
}

fn persist(data_dir: &Path, file: &TombstoneFile) -> Result<(), String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| format!("创建 Provider 回收站目录失败：{error}"))?;
    let mut rendered = serde_json::to_string_pretty(file)
        .map_err(|error| format!("序列化 Provider 回收站失败：{error}"))?;
    rendered.push('\n');
    let temporary = data_dir.join(format!(
        ".{FILE}.{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, rendered)
        .map_err(|error| format!("写 Provider 回收站失败：{error}"))?;
    if let Err(error) = std::fs::rename(&temporary, path(data_dir)) {
        std::fs::remove_file(&temporary).ok();
        return Err(format!("保存 Provider 回收站失败：{error}"));
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
    use super::{archive, contains, discard, get, list, path, take};
    use serde_json::json;

    #[test]
    fn archive_take_and_discard_are_recoverable_and_idempotent() {
        let root = std::env::temp_dir().join(format!(
            "token-station-provider-tombstone-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        let provider = json!({
            "provider": "openai-compatible",
            "base_url": "https://example.test/v1",
            "models": [{"model": "example"}]
        });

        archive(&root, "example", &provider).unwrap();
        assert!(contains(&root, "example").unwrap());
        assert_eq!(get(&root, "example").unwrap(), Some(provider.clone()));
        let replacement = json!({"provider": "another-account"});
        assert!(archive(&root, "example", &replacement)
            .unwrap_err()
            .contains("拒绝覆盖"));
        assert_eq!(take(&root, "example").unwrap(), Some(provider.clone()));
        assert!(!contains(&root, "example").unwrap());
        assert_eq!(take(&root, "example").unwrap(), None);
        archive(&root, "example", &provider).unwrap();
        discard(&root, "example").unwrap();
        assert_eq!(take(&root, "example").unwrap(), None);

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn free_providers_are_hidden_from_generic_restore_list() {
        let root = std::env::temp_dir().join(format!(
            "token-station-provider-tombstone-free-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        archive(
            &root,
            "free",
            &json!({"provider": "openai-compatible", "access_tier": "free"}),
        )
        .unwrap();
        archive(
            &root,
            "paid",
            &json!({"provider": "openai-compatible", "access_tier": "paid"}),
        )
        .unwrap();

        assert_eq!(list(&root).unwrap(), ["paid"]);
        assert!(get(&root, "free").unwrap().is_some());

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn corrupt_or_future_files_fail_closed_without_being_overwritten() {
        let root = std::env::temp_dir().join(format!(
            "token-station-provider-tombstone-corrupt-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let provider = json!({"provider": "openai-compatible"});

        for original in ["not-json", r#"{"version":99,"providers":{}}"#] {
            std::fs::write(path(&root), original).unwrap();
            assert!(archive(&root, "new", &provider).is_err());
            assert!(take(&root, "old").is_err());
            assert!(discard(&root, "old").is_err());
            assert!(list(&root).is_err());
            assert_eq!(std::fs::read_to_string(path(&root)).unwrap(), original);
        }

        std::fs::remove_dir_all(root).ok();
    }
}
