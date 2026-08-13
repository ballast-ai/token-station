use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::plan::{file_revision_hash, ConfigSource};
use super::safe_fs::{ensure_private_dir, sync_parent, verify_private_file, write_atomic_private};
use super::types::SnapshotRecord;

const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const INDEX_SCHEMA_VERSION: u32 = 1;
const ALGORITHM: &str = "AES-256-GCM";
const KEY_ID: &str = "snapshot-master-key-v1";
const INDEX_FILE: &str = "index.json";
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_ENVELOPE_BYTES: u64 = 20 * 1024 * 1024;
const RETAIN_UNPINNED_PER_TARGET: usize = 5;

pub trait MasterKeyStore: Send + Sync {
    fn load_or_create(&self, allow_create: bool) -> Result<Zeroizing<[u8; 32]>, String>;
    fn load(&self) -> Result<Zeroizing<[u8; 32]>, String>;
}

impl<T: MasterKeyStore + ?Sized> MasterKeyStore for Arc<T> {
    fn load_or_create(&self, allow_create: bool) -> Result<Zeroizing<[u8; 32]>, String> {
        self.as_ref().load_or_create(allow_create)
    }

    fn load(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        self.as_ref().load()
    }
}

pub struct FileMasterKeyStore {
    path: PathBuf,
}

impl FileMasterKeyStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl MasterKeyStore for FileMasterKeyStore {
    fn load_or_create(&self, allow_create: bool) -> Result<Zeroizing<[u8; 32]>, String> {
        if self.path.exists() {
            verify_private_file(&self.path)
                .map_err(|_| "快照主密钥文件权限过宽或不是普通文件".to_string())?;
            let raw =
                std::fs::read(&self.path).map_err(|_| "无法读取快照主密钥文件".to_string())?;
            let key: [u8; 32] = raw
                .try_into()
                .map_err(|_| "快照主密钥文件长度无效,或已损坏".to_string())?;
            return Ok(Zeroizing::new(key));
        }
        if !allow_create {
            return Err("快照主密钥缺失,已有快照不可安全解密".to_string());
        }
        let mut generated = Zeroizing::new([0_u8; 32]);
        getrandom::fill(generated.as_mut()).map_err(|_| "生成快照主密钥失败".to_string())?;
        write_atomic_private(&self.path, generated.as_ref())
            .map_err(|_| "写入快照主密钥文件失败".to_string())?;
        Ok(generated)
    }

    fn load(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        verify_private_file(&self.path)
            .map_err(|_| "快照主密钥文件权限过宽或不是普通文件".to_string())?;
        let raw = std::fs::read(&self.path).map_err(|_| "无法读取快照主密钥文件".to_string())?;
        let key: [u8; 32] = raw
            .try_into()
            .map_err(|_| "快照主密钥文件长度无效,或已损坏".to_string())?;
        Ok(Zeroizing::new(key))
    }
}

#[cfg(windows)]
impl MasterKeyStore for FileMasterKeyStore {
    fn load_or_create(&self, allow_create: bool) -> Result<Zeroizing<[u8; 32]>, String> {
        if self.path.exists() {
            verify_private_file(&self.path)
                .map_err(|_| "快照主密钥文件不是普通文件".to_string())?;
            let raw =
                std::fs::read(&self.path).map_err(|_| "无法读取快照主密钥文件".to_string())?;
            let key: [u8; 32] = raw
                .try_into()
                .map_err(|_| "快照主密钥文件长度无效,或已损坏".to_string())?;
            return Ok(Zeroizing::new(key));
        }
        if !allow_create {
            return Err("快照主密钥缺失,已有快照不可安全解密".to_string());
        }
        let mut generated = Zeroizing::new([0_u8; 32]);
        getrandom::fill(generated.as_mut()).map_err(|_| "生成快照主密钥失败".to_string())?;
        write_atomic_private(&self.path, generated.as_ref())
            .map_err(|_| "写入快照主密钥文件失败".to_string())?;
        Ok(generated)
    }

    fn load(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        verify_private_file(&self.path).map_err(|_| "快照主密钥文件不是普通文件".to_string())?;
        let raw = std::fs::read(&self.path).map_err(|_| "无法读取快照主密钥文件".to_string())?;
        let key: [u8; 32] = raw
            .try_into()
            .map_err(|_| "快照主密钥文件长度无效,或已损坏".to_string())?;
        Ok(Zeroizing::new(key))
    }
}

pub struct SnapshotRequest<'a> {
    pub operation_id: &'a str,
    pub agent_id: &'a str,
    pub target_config_path: &'a str,
    pub before_hash: String,
    pub source: &'a ConfigSource,
    pub created_at_ms: u64,
    pub connector_id: &'a str,
    pub app_version: &'a str,
    pub pinned: bool,
}

pub struct SnapshotCreateResult {
    pub record: SnapshotRecord,
    pub maintenance_warning: Option<String>,
}

pub struct DecryptedSnapshot {
    pub record: SnapshotRecord,
    pub exact_bytes: Zeroizing<Vec<u8>>,
}

pub trait SnapshotStore: Send + Sync {
    fn create(&self, request: SnapshotRequest<'_>) -> Result<SnapshotCreateResult, String>;
    fn load(&self, snapshot_id: &str) -> Result<DecryptedSnapshot, String>;
    fn list(&self, agent_id: &str, target_config_path: &str)
        -> Result<Vec<SnapshotRecord>, String>;
    fn list_agent(&self, agent_id: &str) -> Result<Vec<SnapshotRecord>, String> {
        let _ = agent_id;
        Err("快照存储不支持按 Agent 列出".to_string())
    }
    fn set_pinned(&self, snapshot_id: &str, pinned: bool) -> Result<(), String>;
}

pub struct FileSnapshotStore<K> {
    root: PathBuf,
    keys: K,
    lock: Mutex<()>,
}

impl<K> FileSnapshotStore<K> {
    #[must_use]
    pub fn new(root: PathBuf, keys: K) -> Self {
        Self {
            root,
            keys,
            lock: Mutex::new(()),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotIndex {
    schema_version: u32,
    records: Vec<SnapshotRecord>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotEnvelope {
    schema_version: u32,
    algorithm: String,
    key_id: String,
    snapshot_id: String,
    operation_id: String,
    agent_id: String,
    target_config_path: String,
    before_hash: String,
    original_existed: bool,
    original_permissions: Option<u32>,
    original_owner: Option<String>,
    created_at_ms: u64,
    connector_id: String,
    app_version: String,
    nonce_hex: String,
    ciphertext_hex: String,
}

impl SnapshotEnvelope {
    fn aad(&self) -> Vec<u8> {
        let mut aad = Vec::new();
        aad.extend_from_slice(b"token-station-snapshot-aad-v1\0");
        aad_field(&mut aad, &self.schema_version.to_be_bytes());
        for value in [
            self.algorithm.as_bytes(),
            self.key_id.as_bytes(),
            self.snapshot_id.as_bytes(),
            self.operation_id.as_bytes(),
            self.agent_id.as_bytes(),
            self.target_config_path.as_bytes(),
            self.before_hash.as_bytes(),
            self.connector_id.as_bytes(),
            self.app_version.as_bytes(),
        ] {
            aad_field(&mut aad, value);
        }
        aad_field(&mut aad, &[u8::from(self.original_existed)]);
        aad_field(
            &mut aad,
            &self.original_permissions.unwrap_or(u32::MAX).to_be_bytes(),
        );
        aad_field(
            &mut aad,
            self.original_owner.as_deref().unwrap_or("").as_bytes(),
        );
        aad_field(&mut aad, &self.created_at_ms.to_be_bytes());
        aad
    }
}

impl<K: MasterKeyStore> SnapshotStore for FileSnapshotStore<K> {
    fn create(&self, request: SnapshotRequest<'_>) -> Result<SnapshotCreateResult, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "快照存储写锁已损坏".to_string())?;
        ensure_private_dir(&self.root).map_err(|_| "创建私有快照目录失败".to_string())?;
        let mut index = self.read_index()?;
        let allow_key_creation = index.records.is_empty() && !self.has_envelope_files()?;
        let key = self.keys.load_or_create(allow_key_creation)?;
        let snapshot_id = self.unique_snapshot_id()?;
        let mut nonce_bytes = [0_u8; 12];
        getrandom::fill(&mut nonce_bytes).map_err(|_| "生成快照随机 nonce 失败".to_string())?;
        let mut envelope = SnapshotEnvelope {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            algorithm: ALGORITHM.to_string(),
            key_id: KEY_ID.to_string(),
            snapshot_id: snapshot_id.clone(),
            operation_id: request.operation_id.to_string(),
            agent_id: request.agent_id.to_string(),
            target_config_path: request.target_config_path.to_string(),
            before_hash: request.before_hash.clone(),
            original_existed: request.source.existed,
            original_permissions: request.source.original_permissions,
            original_owner: request.source.original_owner.clone(),
            created_at_ms: request.created_at_ms,
            connector_id: request.connector_id.to_string(),
            app_version: request.app_version.to_string(),
            nonce_hex: lower_hex(&nonce_bytes),
            ciphertext_hex: String::new(),
        };
        validate_request(&envelope)?;
        let mut ciphertext = request.source.exact_bytes.to_vec();
        let cipher = LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, key.as_ref())
                .map_err(|_| "初始化快照 AEAD 失败".to_string())?,
        );
        cipher
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(envelope.aad()),
                &mut ciphertext,
            )
            .map_err(|_| "加密快照失败".to_string())?;
        envelope.ciphertext_hex = lower_hex(&ciphertext);
        let rendered = render_json(&envelope, "序列化快照 envelope 失败")?;
        let envelope_hash = sha256_hex(&rendered);
        let envelope_path = self.envelope_path(&snapshot_id);
        write_atomic_private(&envelope_path, &rendered)
            .map_err(|_| "持久化加密快照失败".to_string())?;
        let record = SnapshotRecord {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            snapshot_id,
            operation_id: request.operation_id.to_string(),
            agent_id: request.agent_id.to_string(),
            target_config_path: request.target_config_path.to_string(),
            envelope_hash,
            before_hash: request.before_hash,
            original_existed: request.source.existed,
            original_permissions: request.source.original_permissions,
            original_owner: request.source.original_owner.clone(),
            created_at_ms: request.created_at_ms,
            connector_id: request.connector_id.to_string(),
            app_version: request.app_version.to_string(),
            pinned: request.pinned,
        };
        index.records.push(record.clone());
        if let Err(error) = self.write_index(&index) {
            std::fs::remove_file(&envelope_path).ok();
            sync_parent(&self.root).ok();
            return Err(error);
        }
        let maintenance_warning = self.prune(&mut index).err();
        Ok(SnapshotCreateResult {
            record,
            maintenance_warning,
        })
    }

    fn load(&self, snapshot_id: &str) -> Result<DecryptedSnapshot, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "快照存储读锁已损坏".to_string())?;
        validate_id(snapshot_id)?;
        let index = self.read_index()?;
        let record = index
            .records
            .iter()
            .find(|record| record.snapshot_id == snapshot_id)
            .cloned()
            .ok_or_else(|| "快照不存在".to_string())?;
        let path = self.envelope_path(snapshot_id);
        verify_private_file(&path).map_err(|_| "快照文件权限或类型无效".to_string())?;
        let metadata = std::fs::metadata(&path).map_err(|_| "读取快照元数据失败".to_string())?;
        if metadata.len() > MAX_ENVELOPE_BYTES {
            return Err("快照 envelope 超过安全上限".to_string());
        }
        let rendered = std::fs::read(&path).map_err(|_| "读取快照 envelope 失败".to_string())?;
        if sha256_hex(&rendered) != record.envelope_hash {
            return Err("快照 envelope 哈希不匹配".to_string());
        }
        let envelope: SnapshotEnvelope =
            serde_json::from_slice(&rendered).map_err(|_| "快照 envelope JSON 无效".to_string())?;
        validate_envelope(&envelope, &record)?;
        let nonce: [u8; 12] = decode_lower_hex(&envelope.nonce_hex)?
            .try_into()
            .map_err(|_| "快照 nonce 长度无效".to_string())?;
        let mut ciphertext = Zeroizing::new(decode_lower_hex(&envelope.ciphertext_hex)?);
        if ciphertext.len() < AES_256_GCM.tag_len() {
            return Err("快照 ciphertext 长度无效".to_string());
        }
        let key = self.keys.load()?;
        let cipher = LessSafeKey::new(
            UnboundKey::new(&AES_256_GCM, key.as_ref())
                .map_err(|_| "初始化快照 AEAD 失败".to_string())?,
        );
        let plaintext_len = cipher
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(envelope.aad()),
                ciphertext.as_mut(),
            )
            .map_err(|_| "快照认证或解密失败".to_string())?
            .len();
        ciphertext.truncate(plaintext_len);
        let restored_source = ConfigSource {
            existed: record.original_existed,
            exact_bytes: ciphertext.clone(),
            original_permissions: record.original_permissions,
            original_owner: record.original_owner.clone(),
        };
        if file_revision_hash(Path::new(&record.target_config_path), &restored_source)?
            != record.before_hash
        {
            return Err("快照明文 revision 与索引不一致".to_string());
        }
        Ok(DecryptedSnapshot {
            record,
            exact_bytes: ciphertext,
        })
    }

    fn list(
        &self,
        agent_id: &str,
        target_config_path: &str,
    ) -> Result<Vec<SnapshotRecord>, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "快照存储读锁已损坏".to_string())?;
        let mut records: Vec<_> = self
            .read_index()?
            .records
            .into_iter()
            .filter(|record| {
                record.agent_id == agent_id && record.target_config_path == target_config_path
            })
            .collect();
        records.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| right.snapshot_id.cmp(&left.snapshot_id))
        });
        Ok(records)
    }

    fn list_agent(&self, agent_id: &str) -> Result<Vec<SnapshotRecord>, String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "快照存储读锁已损坏".to_string())?;
        let mut records: Vec<_> = self
            .read_index()?
            .records
            .into_iter()
            .filter(|record| record.agent_id == agent_id)
            .collect();
        records.sort_by(|left, right| {
            right
                .created_at_ms
                .cmp(&left.created_at_ms)
                .then_with(|| right.snapshot_id.cmp(&left.snapshot_id))
        });
        Ok(records)
    }

    fn set_pinned(&self, snapshot_id: &str, pinned: bool) -> Result<(), String> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| "快照存储写锁已损坏".to_string())?;
        validate_id(snapshot_id)?;
        let mut index = self.read_index()?;
        let record = index
            .records
            .iter_mut()
            .find(|record| record.snapshot_id == snapshot_id)
            .ok_or_else(|| "快照不存在".to_string())?;
        record.pinned = pinned;
        self.write_index(&index)
    }
}

impl<K: MasterKeyStore> FileSnapshotStore<K> {
    fn read_index(&self) -> Result<SnapshotIndex, String> {
        let path = self.root.join(INDEX_FILE);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                verify_private_file(&path).map_err(|_| "快照索引权限或类型无效".to_string())?;
                if metadata.len() > MAX_INDEX_BYTES {
                    return Err("快照索引超过安全上限".to_string());
                }
                let bytes = std::fs::read(&path).map_err(|_| "读取快照索引失败".to_string())?;
                let index: SnapshotIndex =
                    serde_json::from_slice(&bytes).map_err(|_| "快照索引 JSON 无效".to_string())?;
                validate_index(index)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SnapshotIndex {
                schema_version: INDEX_SCHEMA_VERSION,
                records: Vec::new(),
            }),
            Err(_) => Err("读取快照索引元数据失败".to_string()),
        }
    }

    fn write_index(&self, index: &SnapshotIndex) -> Result<(), String> {
        let rendered = render_json(index, "序列化快照索引失败")?;
        write_atomic_private(&self.root.join(INDEX_FILE), &rendered)
            .map_err(|_| "原子写入快照索引失败".to_string())
    }

    fn has_envelope_files(&self) -> Result<bool, String> {
        let entries = std::fs::read_dir(&self.root).map_err(|_| "扫描快照目录失败".to_string())?;
        Ok(entries.filter_map(Result::ok).any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".snapshot.json"))
        }))
    }

    fn unique_snapshot_id(&self) -> Result<String, String> {
        for _ in 0..16 {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random).map_err(|_| "生成 snapshot ID 失败".to_string())?;
            let id = lower_hex(&random);
            if !self.envelope_path(&id).exists() {
                return Ok(id);
            }
        }
        Err("无法生成唯一 snapshot ID".to_string())
    }

    fn envelope_path(&self, snapshot_id: &str) -> PathBuf {
        self.root.join(format!("{snapshot_id}.snapshot.json"))
    }

    fn prune(&self, index: &mut SnapshotIndex) -> Result<(), String> {
        let mut remove = BTreeSet::new();
        let groups: BTreeSet<_> = index
            .records
            .iter()
            .map(|record| (record.agent_id.clone(), record.target_config_path.clone()))
            .collect();
        for (agent_id, target) in groups {
            let mut eligible: Vec<_> = index
                .records
                .iter()
                .filter(|record| {
                    !record.pinned
                        && record.agent_id == agent_id
                        && record.target_config_path == target
                })
                .collect();
            eligible.sort_by(|left, right| {
                right
                    .created_at_ms
                    .cmp(&left.created_at_ms)
                    .then_with(|| right.snapshot_id.cmp(&left.snapshot_id))
            });
            remove.extend(
                eligible
                    .into_iter()
                    .skip(RETAIN_UNPINNED_PER_TARGET)
                    .map(|record| record.snapshot_id.clone()),
            );
        }
        if remove.is_empty() {
            return Ok(());
        }
        let original = index.records.clone();
        index
            .records
            .retain(|record| !remove.contains(&record.snapshot_id));
        if let Err(error) = self.write_index(index) {
            index.records = original;
            return Err(format!("快照保留策略索引提交失败：{error}"));
        }
        let mut failed = false;
        for snapshot_id in remove {
            if std::fs::remove_file(self.envelope_path(&snapshot_id)).is_err() {
                failed = true;
            }
        }
        if sync_parent(&self.root).is_err() {
            failed = true;
        }
        if failed {
            Err("快照索引已提交，但旧快照文件清理未完全成功".to_string())
        } else {
            Ok(())
        }
    }
}

fn validate_index(index: SnapshotIndex) -> Result<SnapshotIndex, String> {
    if index.schema_version != INDEX_SCHEMA_VERSION {
        return Err("不支持的快照索引 schema".to_string());
    }
    let mut ids = BTreeSet::new();
    for record in &index.records {
        validate_id(&record.snapshot_id)?;
        if record.schema_version != SNAPSHOT_SCHEMA_VERSION || !ids.insert(&record.snapshot_id) {
            return Err("快照索引包含无效或重复记录".to_string());
        }
    }
    Ok(index)
}

fn validate_request(envelope: &SnapshotEnvelope) -> Result<(), String> {
    validate_id(&envelope.snapshot_id)?;
    if envelope.operation_id.is_empty()
        || envelope.agent_id.is_empty()
        || envelope.target_config_path.is_empty()
        || envelope.before_hash.len() != 64
        || envelope.connector_id.is_empty()
        || envelope.app_version.is_empty()
    {
        return Err("快照请求元数据无效".to_string());
    }
    Ok(())
}

fn validate_envelope(envelope: &SnapshotEnvelope, record: &SnapshotRecord) -> Result<(), String> {
    validate_request(envelope)?;
    if envelope.schema_version != SNAPSHOT_SCHEMA_VERSION
        || envelope.algorithm != ALGORITHM
        || envelope.key_id != KEY_ID
        || envelope.snapshot_id != record.snapshot_id
        || envelope.operation_id != record.operation_id
        || envelope.agent_id != record.agent_id
        || envelope.target_config_path != record.target_config_path
        || envelope.before_hash != record.before_hash
        || envelope.original_existed != record.original_existed
        || envelope.original_permissions != record.original_permissions
        || envelope.original_owner != record.original_owner
        || envelope.created_at_ms != record.created_at_ms
        || envelope.connector_id != record.connector_id
        || envelope.app_version != record.app_version
    {
        return Err("快照 envelope 与索引绑定不一致".to_string());
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), String> {
    if value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err("snapshot ID 无效".to_string())
    }
}

fn render_json<T: Serialize>(value: &T, message: &str) -> Result<Vec<u8>, String> {
    let mut rendered = serde_json::to_vec(value).map_err(|_| message.to_string())?;
    rendered.push(b'\n');
    Ok(rendered)
}

fn aad_field(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

fn decode_lower_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2)
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err("快照十六进制字段无效".to_string());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err("快照十六进制字段无效".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    struct MemoryKeys {
        key: Mutex<Option<[u8; 32]>>,
        fail: AtomicBool,
    }

    impl MemoryKeys {
        fn available() -> Self {
            Self {
                key: Mutex::new(Some([7_u8; 32])),
                fail: AtomicBool::new(false),
            }
        }
    }

    impl MasterKeyStore for MemoryKeys {
        fn load_or_create(&self, allow_create: bool) -> Result<Zeroizing<[u8; 32]>, String> {
            if self.fail.load(Ordering::SeqCst) {
                return Err("keychain locked".to_string());
            }
            let mut key = self.key.lock().unwrap();
            if key.is_none() && allow_create {
                *key = Some([8_u8; 32]);
            }
            key.map(Zeroizing::new)
                .ok_or_else(|| "master key missing".to_string())
        }

        fn load(&self) -> Result<Zeroizing<[u8; 32]>, String> {
            self.load_or_create(false)
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let mut random = [0_u8; 8];
        getrandom::fill(&mut random).unwrap();
        std::env::temp_dir().join(format!(
            "token-station-snapshot-{label}-{}-{}",
            std::process::id(),
            lower_hex(&random)
        ))
    }

    fn request<'a>(
        source: &'a ConfigSource,
        created_at_ms: u64,
        pinned: bool,
    ) -> SnapshotRequest<'a> {
        let target = Path::new("/tmp/.claude/settings.json");
        SnapshotRequest {
            operation_id: "01aabbccddeeff001122334455667788",
            agent_id: "claude-code",
            target_config_path: "/tmp/.claude/settings.json",
            before_hash: file_revision_hash(target, source).unwrap(),
            source,
            created_at_ms,
            connector_id: "claude-code-v1",
            app_version: "0.1.0",
            pinned,
        }
    }

    #[test]
    fn snapshot_round_trip_is_encrypted_randomized_private_and_tamper_evident() {
        let root = scratch("roundtrip");
        let store = FileSnapshotStore::new(root.clone(), MemoryKeys::available());
        let marker = b"vk-plaintext-must-not-appear".to_vec();
        let source =
            ConfigSource::existing(marker.clone(), Some(0o640), Some("501:20".to_string()));
        let first = store.create(request(&source, 1, false)).unwrap().record;
        let second = store.create(request(&source, 2, false)).unwrap().record;
        assert_ne!(first.snapshot_id, second.snapshot_id);
        let first_file = std::fs::read(store.envelope_path(&first.snapshot_id)).unwrap();
        let second_file = std::fs::read(store.envelope_path(&second.snapshot_id)).unwrap();
        assert!(!first_file
            .windows(marker.len())
            .any(|window| window == marker));
        assert_ne!(first_file, second_file);
        assert_eq!(
            store
                .load(&first.snapshot_id)
                .unwrap()
                .exact_bytes
                .as_slice(),
            marker
        );

        let path = store.envelope_path(&first.snapshot_id);
        let mut tampered = std::fs::read(&path).unwrap();
        let index = tampered.len() / 2;
        tampered[index] ^= 1;
        write_atomic_private(&path, &tampered).unwrap();
        assert!(store.load(&first.snapshot_id).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(root.join(INDEX_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn snapshot_key_failure_is_closed_and_existing_blobs_forbid_key_regeneration() {
        let root = scratch("key-failure");
        let keys = MemoryKeys::available();
        keys.fail.store(true, Ordering::SeqCst);
        let store = FileSnapshotStore::new(root.clone(), keys);
        let source = ConfigSource::existing(b"secret".to_vec(), Some(0o600), None);
        assert!(store.create(request(&source, 1, false)).is_err());
        assert!(!root.join(INDEX_FILE).exists());

        ensure_private_dir(&root).unwrap();
        write_atomic_private(&root.join("00.snapshot.json"), b"orphan").unwrap();
        let missing = FileSnapshotStore::new(
            root.clone(),
            MemoryKeys {
                key: Mutex::new(None),
                fail: AtomicBool::new(false),
            },
        );
        let error = missing
            .create(request(&source, 2, false))
            .err()
            .expect("missing key with existing blob is rejected");
        assert!(error.contains("missing"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn snapshot_retains_five_unpinned_and_all_pinned() {
        let root = scratch("retention");
        let store = FileSnapshotStore::new(root.clone(), MemoryKeys::available());
        let source = ConfigSource::existing(b"fixture".to_vec(), Some(0o600), None);
        let pinned = store.create(request(&source, 0, true)).unwrap().record;
        for created in 1..=6 {
            store.create(request(&source, created, false)).unwrap();
        }
        let records = store
            .list("claude-code", "/tmp/.claude/settings.json")
            .unwrap();
        assert_eq!(records.len(), 6);
        assert!(records
            .iter()
            .any(|record| record.snapshot_id == pinned.snapshot_id));
        assert_eq!(records.iter().filter(|record| !record.pinned).count(), 5);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn snapshot_listing_pinning_arc_keys_and_default_capability_are_covered() {
        struct CreateOnlyStore;

        impl SnapshotStore for CreateOnlyStore {
            fn create(
                &self,
                _request: SnapshotRequest<'_>,
            ) -> Result<SnapshotCreateResult, String> {
                Err("unused".to_string())
            }

            fn load(&self, _snapshot_id: &str) -> Result<DecryptedSnapshot, String> {
                Err("unused".to_string())
            }

            fn list(
                &self,
                _agent_id: &str,
                _target_config_path: &str,
            ) -> Result<Vec<SnapshotRecord>, String> {
                Ok(Vec::new())
            }

            fn set_pinned(&self, _snapshot_id: &str, _pinned: bool) -> Result<(), String> {
                Ok(())
            }
        }

        assert!(CreateOnlyStore.list_agent("claude-code").is_err());
        let keys = Arc::new(MemoryKeys::available());
        assert_eq!(keys.load_or_create(false).unwrap().as_ref(), &[7_u8; 32]);
        assert_eq!(keys.load().unwrap().as_ref(), &[7_u8; 32]);

        let root = scratch("list-pin");
        let store = FileSnapshotStore::new(root.clone(), keys);
        assert_eq!(store.root(), root.as_path());
        let source = ConfigSource::existing(b"fixture".to_vec(), Some(0o600), None);
        let older = store.create(request(&source, 10, false)).unwrap().record;
        let newer = store.create(request(&source, 20, false)).unwrap().record;
        assert_eq!(
            store
                .list_agent("claude-code")
                .unwrap()
                .into_iter()
                .map(|record| record.snapshot_id)
                .collect::<Vec<_>>(),
            vec![newer.snapshot_id.clone(), older.snapshot_id.clone()]
        );
        assert!(store.list_agent("codex").unwrap().is_empty());
        store.set_pinned(&older.snapshot_id, true).unwrap();
        assert!(
            store
                .list("claude-code", "/tmp/.claude/settings.json")
                .unwrap()
                .into_iter()
                .find(|record| record.snapshot_id == older.snapshot_id)
                .unwrap()
                .pinned
        );
        store.set_pinned(&older.snapshot_id, false).unwrap();
        assert!(store.set_pinned("invalid", true).is_err());
        assert!(store
            .set_pinned("00000000000000000000000000000000", true)
            .is_err());
        assert!(store.load("invalid").is_err());
        assert!(store.load("00000000000000000000000000000000").is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn snapshot_validation_rejects_malformed_index_envelope_and_hex() {
        assert!(validate_id("0123456789abcdef0123456789abcdef").is_ok());
        for invalid in [
            "",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdeg",
            "0123456789ABCDEF0123456789ABCDEF",
        ] {
            assert!(validate_id(invalid).is_err());
        }
        assert_eq!(decode_lower_hex("00af10").unwrap(), vec![0, 175, 16]);
        for invalid in ["0", "GG", "0A", "zz"] {
            assert!(decode_lower_hex(invalid).is_err());
        }
        assert!(hex_nibble(b'g').is_err());

        assert!(validate_index(SnapshotIndex {
            schema_version: 99,
            records: Vec::new(),
        })
        .is_err());
        let root = scratch("validation");
        let store = FileSnapshotStore::new(root.clone(), MemoryKeys::available());
        let source = ConfigSource::existing(b"source".to_vec(), Some(0o600), None);
        let created = store.create(request(&source, 7, false)).unwrap().record;
        let mut duplicate = created.clone();
        duplicate.schema_version = 99;
        assert!(validate_index(SnapshotIndex {
            schema_version: INDEX_SCHEMA_VERSION,
            records: vec![duplicate],
        })
        .is_err());
        assert!(validate_index(SnapshotIndex {
            schema_version: INDEX_SCHEMA_VERSION,
            records: vec![created.clone(), created.clone()],
        })
        .is_err());

        let bytes = std::fs::read(store.envelope_path(&created.snapshot_id)).unwrap();
        let mut envelope: SnapshotEnvelope = serde_json::from_slice(&bytes).unwrap();
        assert!(validate_request(&envelope).is_ok());
        assert!(validate_envelope(&envelope, &created).is_ok());
        envelope.operation_id.clear();
        assert!(validate_request(&envelope).is_err());
        envelope.operation_id = created.operation_id.clone();
        envelope.algorithm = "AES-128-GCM".to_string();
        assert!(validate_envelope(&envelope, &created).is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn snapshot_index_and_envelope_corruption_fail_closed() {
        let root = scratch("corruption");
        ensure_private_dir(&root).unwrap();
        write_atomic_private(&root.join(INDEX_FILE), b"not-json").unwrap();
        let store = FileSnapshotStore::new(root.clone(), MemoryKeys::available());
        assert!(store.list_agent("claude-code").is_err());

        write_atomic_private(
            &root.join(INDEX_FILE),
            br#"{"schema_version":2,"records":[]}"#,
        )
        .unwrap();
        assert!(store.list_agent("claude-code").is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
