use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Mutex;

use ring::hmac;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::config_codec::{semantic_json, ConfigDocument};
use super::safe_fs::{ensure_private_dir, verify_private_file, write_atomic_private};
use super::types::ConfigPath;

const OWNERSHIP_SCHEMA_VERSION: u32 = 1;
const INDEX_SCHEMA_VERSION: u32 = 1;
const INDEX_FILE: &str = "ownership-index.json";
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipRecord {
    pub schema_version: u32,
    pub revision: u64,
    pub agent_id: String,
    pub installation_path: String,
    pub target_config_path: String,
    pub connector_id: String,
    pub baseline_snapshot_id: String,
    pub last_transaction_snapshot_id: String,
    pub before_hash: String,
    pub managed_after_hash: String,
    pub owned_paths: Vec<ConfigPath>,
    pub owned_value_macs: BTreeMap<String, String>,
    pub acquired_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub struct OwnershipKey {
    pub agent_id: String,
    pub installation_path: String,
    pub target_config_path: String,
}

impl OwnershipRecord {
    #[must_use]
    pub fn key(&self) -> OwnershipKey {
        OwnershipKey {
            agent_id: self.agent_id.clone(),
            installation_path: self.installation_path.clone(),
            target_config_path: self.target_config_path.clone(),
        }
    }
}

pub trait OwnershipStore {
    fn load(&self, key: &OwnershipKey) -> Result<Option<OwnershipRecord>, String>;
    fn list_agent_installation(
        &self,
        agent_id: &str,
        installation_path: &str,
    ) -> Result<Vec<OwnershipRecord>, String> {
        let _ = (agent_id, installation_path);
        Err("ownership 存储不支持按安装实例列出".to_string())
    }
    fn commit(
        &self,
        record: OwnershipRecord,
        expected_revision: Option<u64>,
    ) -> Result<OwnershipRecord, String>;
    fn remove(&self, key: &OwnershipKey, expected_revision: u64) -> Result<(), String>;
}

pub struct FileOwnershipStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl FileOwnershipStore {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            lock: Mutex::new(()),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnershipIndex {
    schema_version: u32,
    records: Vec<OwnershipRecord>,
}

impl OwnershipStore for FileOwnershipStore {
    fn load(&self, key: &OwnershipKey) -> Result<Option<OwnershipRecord>, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "ownership 索引读锁已损坏".to_string())?;
        Ok(self
            .read_index()?
            .records
            .into_iter()
            .find(|record| record.key() == *key))
    }

    fn list_agent_installation(
        &self,
        agent_id: &str,
        installation_path: &str,
    ) -> Result<Vec<OwnershipRecord>, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "ownership 索引读锁已损坏".to_string())?;
        let mut records: Vec<_> = self
            .read_index()?
            .records
            .into_iter()
            .filter(|record| {
                record.agent_id == agent_id && record.installation_path == installation_path
            })
            .collect();
        records.sort_by(|left, right| left.target_config_path.cmp(&right.target_config_path));
        Ok(records)
    }

    fn commit(
        &self,
        mut record: OwnershipRecord,
        expected_revision: Option<u64>,
    ) -> Result<OwnershipRecord, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "ownership 索引写锁已损坏".to_string())?;
        validate_record(&record)?;
        ensure_private_dir(&self.root).map_err(|_| "创建私有 ownership 目录失败".to_string())?;
        let mut index = self.read_index()?;
        let existing = index
            .records
            .iter()
            .position(|item| item.key() == record.key());
        match (existing, expected_revision) {
            (None, None) => record.revision = 1,
            (Some(position), Some(expected)) if index.records[position].revision == expected => {
                record.revision = expected.saturating_add(1);
                index.records.remove(position);
            }
            (Some(_), None) => return Err("该实例已有 active ownership".to_string()),
            (None, Some(_)) | (Some(_), Some(_)) => {
                return Err("ownership revision 已变化，必须重新预览".to_string());
            }
        }
        index.records.push(record.clone());
        index.records.sort_by_key(OwnershipRecord::key);
        self.write_index(&index)?;
        Ok(record)
    }

    fn remove(&self, key: &OwnershipKey, expected_revision: u64) -> Result<(), String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "ownership 索引写锁已损坏".to_string())?;
        let mut index = self.read_index()?;
        let position = index
            .records
            .iter()
            .position(|record| record.key() == *key)
            .ok_or_else(|| "ownership 不存在".to_string())?;
        if index.records[position].revision != expected_revision {
            return Err("ownership revision 已变化，必须重新预览".to_string());
        }
        index.records.remove(position);
        self.write_index(&index)
    }
}

impl FileOwnershipStore {
    fn read_index(&self) -> Result<OwnershipIndex, String> {
        let path = self.root.join(INDEX_FILE);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                verify_private_file(&path)
                    .map_err(|_| "ownership 索引权限或类型无效".to_string())?;
                if metadata.len() > MAX_INDEX_BYTES {
                    return Err("ownership 索引超过安全上限".to_string());
                }
                let bytes =
                    std::fs::read(&path).map_err(|_| "读取 ownership 索引失败".to_string())?;
                let index: OwnershipIndex = serde_json::from_slice(&bytes)
                    .map_err(|_| "ownership 索引 JSON 无效".to_string())?;
                validate_index(index)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(OwnershipIndex {
                schema_version: INDEX_SCHEMA_VERSION,
                records: Vec::new(),
            }),
            Err(_) => Err("读取 ownership 索引元数据失败".to_string()),
        }
    }

    fn write_index(&self, index: &OwnershipIndex) -> Result<(), String> {
        let mut rendered =
            serde_json::to_vec(index).map_err(|_| "序列化 ownership 索引失败".to_string())?;
        rendered.push(b'\n');
        write_atomic_private(&self.root.join(INDEX_FILE), &rendered)
            .map_err(|_| "原子写入 ownership 索引失败".to_string())
    }
}

fn validate_index(index: OwnershipIndex) -> Result<OwnershipIndex, String> {
    if index.schema_version != INDEX_SCHEMA_VERSION {
        return Err("不支持的 ownership 索引 schema".to_string());
    }
    let mut keys = BTreeSet::new();
    for record in &index.records {
        validate_record(record)?;
        if !keys.insert(record.key()) {
            return Err("ownership 索引包含重复实例".to_string());
        }
    }
    Ok(index)
}

fn validate_record(record: &OwnershipRecord) -> Result<(), String> {
    if record.schema_version != OWNERSHIP_SCHEMA_VERSION
        || record.agent_id.is_empty()
        || record.installation_path.is_empty()
        || record.target_config_path.is_empty()
        || record.connector_id.is_empty()
        || record.baseline_snapshot_id.len() != 32
        || record.last_transaction_snapshot_id.len() != 32
        || record.before_hash.len() != 64
        || record.managed_after_hash.len() != 64
        || record.owned_paths.is_empty()
        || record.owned_paths.len() != record.owned_value_macs.len()
        || record
            .owned_paths
            .iter()
            .any(|path| !record.owned_value_macs.contains_key(&path.to_string()))
    {
        return Err("ownership record 无效".to_string());
    }
    Ok(())
}

pub fn compute_owned_value_macs(
    document: &ConfigDocument,
    owned_paths: &[ConfigPath],
    master_key: &Zeroizing<[u8; 32]>,
) -> Result<BTreeMap<String, String>, String> {
    let canonical = semantic_json(document)?;
    let key = hmac::Key::new(hmac::HMAC_SHA256, master_key.as_ref());
    let mut result = BTreeMap::new();
    for path in owned_paths {
        let mut message = Vec::new();
        message.extend_from_slice(b"token-station-owned-value-v1\0");
        encode_field(&mut message, path.to_string().as_bytes());
        match json_value_at(&canonical, path) {
            Some(value) => {
                message.push(1);
                let bytes =
                    serde_json::to_vec(value).map_err(|_| "序列化 owned value 失败".to_string())?;
                encode_field(&mut message, &bytes);
            }
            None => message.push(0),
        }
        result.insert(
            path.to_string(),
            lower_hex(hmac::sign(&key, &message).as_ref()),
        );
    }
    Ok(result)
}

pub fn ownership_matches(
    record: &OwnershipRecord,
    document: &ConfigDocument,
    master_key: &Zeroizing<[u8; 32]>,
) -> Result<bool, String> {
    Ok(
        compute_owned_value_macs(document, &record.owned_paths, master_key)?
            == record.owned_value_macs,
    )
}

fn json_value_at<'a>(
    root: &'a serde_json::Value,
    path: &ConfigPath,
) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in &path.segments {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

fn encode_field(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch() -> PathBuf {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).unwrap();
        std::env::temp_dir().join(format!(
            "token-station-ownership-{}-{}",
            std::process::id(),
            lower_hex(&random)
        ))
    }
    fn path(parts: &[&str]) -> ConfigPath {
        ConfigPath {
            segments: parts.iter().map(|part| (*part).to_string()).collect(),
        }
    }
    fn record(macs: BTreeMap<String, String>) -> OwnershipRecord {
        let owned_paths = macs
            .keys()
            .map(|value| ConfigPath {
                segments: value
                    .trim_start_matches('/')
                    .split('/')
                    .map(str::to_string)
                    .collect(),
            })
            .collect();
        OwnershipRecord {
            schema_version: 1,
            revision: 0,
            agent_id: "claude-code".to_string(),
            installation_path: "/opt/claude".to_string(),
            target_config_path: "/tmp/settings.json".to_string(),
            connector_id: "claude-code-v1".to_string(),
            baseline_snapshot_id: "01".repeat(16),
            last_transaction_snapshot_id: "02".repeat(16),
            before_hash: "a".repeat(64),
            managed_after_hash: "b".repeat(64),
            owned_paths,
            owned_value_macs: macs,
            acquired_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn ownership_index_contains_only_keyed_digests_and_enforces_revision() {
        let root = scratch();
        let store = FileOwnershipStore::new(root.clone());
        let secret = "vk-ownership-secret";
        let document = ConfigDocument::Json(json!({ "env": { "TOKEN": secret } }));
        let owned = vec![path(&["env", "TOKEN"])];
        let key = Zeroizing::new([9_u8; 32]);
        let macs = compute_owned_value_macs(&document, &owned, &key).unwrap();
        let committed = store.commit(record(macs), None).unwrap();
        assert_eq!(committed.revision, 1);
        let bytes = std::fs::read(root.join(INDEX_FILE)).unwrap();
        assert!(!bytes
            .windows(secret.len())
            .any(|window| window == secret.as_bytes()));
        let error = store
            .commit(committed.clone(), Some(0))
            .err()
            .expect("stale revision is rejected");
        assert!(error.contains("revision"));
        let updated = store.commit(committed.clone(), Some(1)).unwrap();
        assert_eq!(updated.revision, 2);
        store.remove(&updated.key(), 2).unwrap();
        assert!(store.load(&updated.key()).unwrap().is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn ownership_detects_managed_value_changes_but_ignores_unowned_changes() {
        let key = Zeroizing::new([3_u8; 32]);
        let owned = vec![path(&["provider", "tokenstation"])];
        let original = ConfigDocument::Json(
            json!({"provider":{"tokenstation":{"apiKey":"secret"}},"theme":"dark"}),
        );
        let record = record(compute_owned_value_macs(&original, &owned, &key).unwrap());
        let unowned_changed = ConfigDocument::Json(
            json!({"provider":{"tokenstation":{"apiKey":"secret"}},"theme":"light"}),
        );
        let owned_changed = ConfigDocument::Json(
            json!({"provider":{"tokenstation":{"apiKey":"other"}},"theme":"dark"}),
        );
        assert!(ownership_matches(&record, &unowned_changed, &key).unwrap());
        assert!(!ownership_matches(&record, &owned_changed, &key).unwrap());
    }
}
