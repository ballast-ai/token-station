use super::{AccountView, Result, local_error, now};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) const LIMIT: u64 = 1_048_576;
pub(super) struct Snapshot {
    pub document: Value,
    pub bytes: Vec<u8>,
    pub subject: String,
    pub account: String,
    pub email: Option<String>,
    pub expiry: u64,
    pub source: PathBuf,
}
impl Drop for Snapshot {
    fn drop(&mut self) {
        super::erase(&mut self.document);
        self.bytes.fill(0);
    }
}
fn owned_directory(path: &Path) -> Result<()> {
    let m = fs::metadata(path).map_err(local_error)?;
    if !m.is_dir() {
        return Err("Codex source directory is invalid.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o022 != 0 {
            return Err("Codex source directory has unsafe ownership or write permissions.".into());
        }
    }
    #[cfg(not(unix))]
    token_station_private_fs::verify_private_dir(path).map_err(local_error)?;
    Ok(())
}
pub(super) fn owned_file(path: &Path) -> Result<()> {
    owned_directory(path.parent().ok_or("Invalid Codex source path.")?)?;
    let m = fs::symlink_metadata(path).map_err(local_error)?;
    if !m.is_file() || m.file_type().is_symlink() {
        return Err("Codex authentication source must be a regular file.".into());
    }
    token_station_private_fs::verify_single_link_file(path).map_err(local_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if m.uid() != rustix::process::geteuid().as_raw() || m.mode() & 0o022 != 0 {
            return Err(
                "Codex authentication source has unsafe ownership or write permissions.".into(),
            );
        }
    }
    #[cfg(not(unix))]
    token_station_private_fs::verify_private_file(path).map_err(local_error)?;
    Ok(())
}
pub(super) fn bytes(path: &Path, limit: u64) -> Result<Vec<u8>> {
    owned_file(path)?;
    let before = fs::symlink_metadata(path).map_err(local_error)?;
    let file = File::open(path).map_err(local_error)?;
    let opened = file.metadata().map_err(local_error)?;
    #[cfg(not(unix))]
    let _ = &opened;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            return Err("Codex source changed while opening it.".into());
        }
    }
    let mut out = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut out)
        .map_err(local_error)?;
    owned_file(path)?;
    let after = fs::symlink_metadata(path).map_err(local_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if after.dev() != opened.dev() || after.ino() != opened.ino() {
            out.fill(0);
            return Err("Codex source changed while reading it.".into());
        }
    }
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        out.fill(0);
        return Err("Codex source changed while reading it.".into());
    }
    if out.len() as u64 > limit {
        return Err("Codex source exceeds its size limit.".into());
    }
    Ok(out)
}
pub(super) fn storage(home: &Path) -> Result<bool> {
    let p = home.join("config.toml");
    if let Err(error) = fs::symlink_metadata(&p) {
        return if error.kind() == std::io::ErrorKind::NotFound {
            Ok(true)
        } else {
            Err(local_error(error))
        };
    }
    let b = bytes(&p, LIMIT)?;
    let text = std::str::from_utf8(&b).map_err(local_error)?;
    let doc: toml::Value = toml::from_str(text).map_err(local_error)?;
    Ok(match doc.get("cli_auth_credentials_store") {
        None => true,
        Some(v) => v.as_str() == Some("file"),
    })
}
pub(super) fn header_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
pub(super) fn version_value(value: &str) -> bool {
    if value.len() > 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
    {
        return false;
    }
    let core = value.split(['-', '+']).next().unwrap_or_default();
    let fields: Vec<_> = core.split('.').collect();
    fields.len() == 3
        && fields
            .iter()
            .all(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
}
pub(super) fn client_version(home: &Path) -> String {
    let cache = home.join("models_cache.json");
    if let Ok(raw) = bytes(&cache, 4 * LIMIT)
        && let Ok(value) = serde_json::from_slice::<Value>(&raw)
        && let Some(version) = value
            .get("client_version")
            .and_then(Value::as_str)
            .filter(|v| version_value(v))
    {
        return version.to_owned();
    }
    // A bounded version query does not load or export authentication material.
    let Ok(mut child) = Command::new("codex")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return String::new();
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return String::new();
                }
                let mut text = String::new();
                if let Some(stdout) = child.stdout.take() {
                    let _ = stdout.take(256).read_to_string(&mut text);
                }
                return text
                    .split_whitespace()
                    .find(|v| version_value(v))
                    .unwrap_or_default()
                    .into();
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return String::new();
            }
        }
    }
}
fn claims(token: &str) -> Result<Value> {
    let pieces: Vec<_> = token.split('.').collect();
    if pieces.len() != 3 || pieces[1].len() > 65536 {
        return Err("Codex authentication token metadata is invalid.".into());
    }
    let raw = URL_SAFE_NO_PAD.decode(pieces[1]).map_err(local_error)?;
    serde_json::from_slice(&raw).map_err(local_error)
}
impl Snapshot {
    pub fn parse(source: PathBuf, bytes: Vec<u8>) -> Result<Self> {
        let document: Value = serde_json::from_slice(&bytes).map_err(local_error)?;
        if document
            .get("auth_mode")
            .and_then(Value::as_str)
            .is_some_and(|m| m != "chatgpt")
            || document
                .get("OPENAI_API_KEY")
                .and_then(Value::as_str)
                .is_some_and(|v| !v.is_empty())
        {
            return Err("api_key_only".into());
        }
        let tokens = document
            .get("tokens")
            .and_then(Value::as_object)
            .ok_or("invalid_source")?;
        let access = tokens
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or("invalid_source")?;
        let id = tokens
            .get("id_token")
            .and_then(Value::as_str)
            .ok_or("invalid_source")?;
        let identity = claims(id)?;
        let subject = identity
            .get("sub")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
            .ok_or("invalid_source")?
            .to_owned();
        let access_claims = claims(access)?;
        if access_claims
            .get("sub")
            .and_then(Value::as_str)
            .is_some_and(|v| v != subject)
        {
            return Err("Codex token subject identities disagree.".into());
        }
        let claim_account = identity
            .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
            .and_then(Value::as_str);
        let account = tokens
            .get("account_id")
            .and_then(Value::as_str)
            .or(claim_account)
            .filter(|v| header_value(v))
            .ok_or("invalid_source")?
            .to_owned();
        for claimed in [
            claim_account,
            access_claims
                .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
                .and_then(Value::as_str),
        ]
        .into_iter()
        .flatten()
        {
            if claimed != account {
                return Err("Codex token account identities disagree.".into());
            }
        }
        let expiry = access_claims
            .get("exp")
            .and_then(Value::as_u64)
            .ok_or("invalid_source")?;
        let email = identity
            .get("email")
            .and_then(Value::as_str)
            .filter(|v| v.len() <= 320 && !v.chars().any(char::is_control))
            .map(str::to_owned);
        Ok(Self {
            document,
            bytes,
            subject,
            account,
            email,
            expiry,
            source,
        })
    }
    pub fn read(home: &Path) -> Result<Self> {
        if !storage(home)? {
            return Err("unsupported_storage".into());
        }
        let source = home.canonicalize().map_err(local_error)?.join("auth.json");
        Self::parse(source.clone(), bytes(&source, LIMIT)?)
    }
    pub fn id(&self) -> String {
        let mut hash = Sha256::new();
        for part in [
            self.source.to_string_lossy().as_ref(),
            &self.subject,
            &self.account,
        ] {
            hash.update(part.as_bytes());
            hash.update([0]);
        }
        format!("{:x}", hash.finalize())[..32].to_owned()
    }
    pub fn view(&self, version: String) -> AccountView {
        AccountView {
            id: self.id(),
            label: self
                .email
                .clone()
                .unwrap_or_else(|| "Local Codex account".into()),
            email: self.email.clone(),
            state: if self.expiry > now() + 60 {
                "ready"
            } else {
                "expired"
            }
            .into(),
            account_id: self.account.clone(),
            client_version: version,
        }
    }
    pub fn token(&self, key: &str) -> Option<&str> {
        self.document
            .get("tokens")?
            .get(key)?
            .as_str()
            .filter(|v| !v.is_empty())
    }
}
