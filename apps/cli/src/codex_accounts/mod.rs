//! Host-owned references to an existing Codex login. Registration files contain no tokens.
//! Blocking filesystem and network methods must run outside asynchronous executors.
use serde::{Deserialize, Serialize};
use source::Snapshot;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use token_station_private_fs::{
    create_private_file, ensure_private_dir, verify_private_dir, verify_private_file,
    verify_single_link_file, write_atomic_private,
};
mod source;
#[cfg(test)]
mod tests;
mod transport;

pub const API_BASE: &str = "https://chatgpt.com/backend-api/codex";
pub const SLOT: &str = "codex_access_token";
struct SecretJson(serde_json::Value);
impl std::ops::Deref for SecretJson {
    type Target = serde_json::Value;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl Drop for SecretJson {
    fn drop(&mut self) {
        erase(&mut self.0);
    }
}
fn erase(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(text) => drop(south_core::SecretValue::new(std::mem::take(text))),
        serde_json::Value::Array(items) => items.iter_mut().for_each(erase),
        serde_json::Value::Object(items) => items.values_mut().for_each(erase),
        _ => {}
    }
}
struct SecretText(String);
impl SecretText {
    fn new(value: String) -> Self {
        Self(value)
    }
    fn expose(&self) -> &str {
        &self.0
    }
}
impl Drop for SecretText {
    fn drop(&mut self) {
        drop(south_core::SecretValue::new(std::mem::take(&mut self.0)));
    }
}
type Result<T> = std::result::Result<T, String>;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccountView {
    pub id: String,
    pub label: String,
    pub email: Option<String>,
    pub state: String,
    pub account_id: String,
    pub client_version: String,
}
#[derive(Clone)]
pub struct AccountManager {
    root: PathBuf,
    home: PathBuf,
    endpoints: transport::Endpoints,
}
#[derive(Serialize, Deserialize)]
struct Reference {
    id: String,
    source: PathBuf,
    subject: String,
    account_id: String,
    email: Option<String>,
    client_version: String,
}
impl Reference {
    fn matches(&self, snapshot: &Snapshot) -> bool {
        snapshot.source == self.source
            && snapshot.subject == self.subject
            && snapshot.account == self.account_id
            && snapshot.id() == self.id
    }
    fn view(&self) -> AccountView {
        AccountView {
            id: self.id.clone(),
            label: self
                .email
                .clone()
                .unwrap_or_else(|| "Local Codex account".into()),
            email: self.email.clone(),
            state: "ready".into(),
            account_id: self.account_id.clone(),
            client_version: self.client_version.clone(),
        }
    }
}
fn local_error(_: impl std::fmt::Display) -> String {
    "Cannot access the local Codex account source.".into()
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|v| v.is_ascii_hexdigit())
}
fn open_lock_file(path: &Path) -> Result<File> {
    create_private_file(path, b"").map_err(local_error)?;
    verify_private_file(path).map_err(local_error)?;
    verify_single_link_file(path).map_err(local_error)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(local_error)
}
fn lock_file(path: &Path) -> Result<File> {
    let file = open_lock_file(path)?;
    file.lock().map_err(local_error)?;
    Ok(file)
}
impl AccountManager {
    #[must_use]
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|v| PathBuf::from(v).join(".codex"))
            })
            .unwrap_or_default();
        Self {
            root: data_dir.as_ref().join("codex-accounts"),
            home,
            endpoints: transport::Endpoints::default(),
        }
    }
    fn empty(state: &str) -> AccountView {
        AccountView {
            id: String::new(),
            label: "Local Codex account".into(),
            email: None,
            state: state.into(),
            account_id: String::new(),
            client_version: String::new(),
        }
    }
    /// Discovers safe metadata without refreshing or persisting credentials.
    /// # Errors
    /// The local source cannot be inspected safely.
    pub fn discover(&self) -> Result<AccountView> {
        if self.home.as_os_str().is_empty() || !self.home.exists() {
            return Ok(Self::empty("missing"));
        }
        match source::storage(&self.home) {
            Ok(false) => return Ok(Self::empty("unsupported_storage")),
            Err(_) => return Ok(Self::empty("invalid_source")),
            Ok(true) => {}
        }
        if !self.home.join("auth.json").exists() {
            return Ok(Self::empty("missing"));
        }
        match Snapshot::read(&self.home) {
            Ok(snapshot) => {
                let version = source::client_version(&self.home);
                if version.is_empty() {
                    return Ok(Self::empty("invalid_source"));
                }
                Ok(snapshot.view(version))
            }
            Err(error) => Ok(Self::empty(if error == "api_key_only" {
                "api_key_only"
            } else {
                "invalid_source"
            })),
        }
    }
    fn lock(&self) -> Result<File> {
        ensure_private_dir(&self.root).map_err(local_error)?;
        lock_file(&self.root.join("references.lock"))
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err("Invalid Codex account reference.".into());
        }
        Ok(self.root.join(format!("{id}.json")))
    }
    fn suppression_path(&self, id: &str) -> Result<PathBuf> {
        self.path(id)?;
        Ok(self.root.join(format!("{id}.auto-disabled")))
    }
    fn auto_connect_allowed_locked(&self, id: &str) -> Result<bool> {
        let path = self.suppression_path(id)?;
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(local_error(error)),
            Ok(_) => {}
        }
        verify_private_file(&path).map_err(local_error)?;
        verify_single_link_file(&path).map_err(local_error)?;
        if source::bytes(&path, 16)? != b"1\n" {
            return Err("Local Codex automatic connection state is invalid.".into());
        }
        Ok(false)
    }
    /// Checks persistent automatic-connection suppression without reading credentials.
    /// # Errors
    /// The account identifier or protected suppression state is invalid.
    pub fn auto_connect_allowed(&self, id: &str) -> Result<bool> {
        self.path(id)?;
        let _lock = self.lock()?;
        self.auto_connect_allowed_locked(id)
    }
    fn load(&self, id: &str) -> Result<Reference> {
        let path = self.path(id)?;
        verify_private_dir(&self.root).map_err(local_error)?;
        verify_private_file(&path).map_err(local_error)?;
        let r: Reference =
            serde_json::from_slice(&source::bytes(&path, source::LIMIT)?).map_err(local_error)?;
        if r.id != id
            || !source::header_value(&r.account_id)
            || !source::version_value(&r.client_version)
        {
            return Err("Invalid Codex account reference metadata.".into());
        }
        Ok(r)
    }
    fn bound(&self, r: &Reference) -> Result<Snapshot> {
        let snapshot = Snapshot::read(&self.home)?;
        if !r.matches(&snapshot) {
            return Err("The local Codex account changed. Reconnect the intended account.".into());
        }
        Ok(snapshot)
    }
    /// Returns the exact registration and its current source state without refreshing.
    /// # Errors
    /// The protected local reference is invalid or unavailable.
    pub fn account(&self, id: &str) -> Result<AccountView> {
        let r = self.load(id)?;
        let mut view = r.view();
        let discovered = self.discover()?;
        if discovered.state != "ready" && discovered.state != "expired" {
            view.state = discovered.state;
            return Ok(view);
        }
        match self.bound(&r) {
            Ok(snapshot) => view.state = snapshot.view(r.client_version).state,
            Err(_) => view.state = "account_changed".into(),
        }
        Ok(view)
    }
    /// Commits an automatic attachment only while its local account is ready.
    /// Returns no account when another operation holds the reference lifecycle lock.
    /// Holds the reference lifecycle lock until the callback returns.
    /// The callback must not call this manager's other locking methods.
    /// # Errors
    /// The protected account state is invalid or the attachment cannot be saved.
    pub fn with_automatic_account<T>(
        &self,
        id: &str,
        commit: impl FnOnce(&AccountView) -> Result<T>,
    ) -> Result<Option<T>> {
        self.path(id)?;
        ensure_private_dir(&self.root).map_err(local_error)?;
        let file = open_lock_file(&self.root.join("references.lock"))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(error) => return Err(local_error(error)),
        }
        let _lock = file;
        if !self.auto_connect_allowed_locked(id)? {
            return Ok(None);
        }
        let reference = self.load(id)?;
        if !source::storage(&self.home)? {
            return Ok(None);
        }
        let snapshot = Snapshot::read(&self.home)?;
        if !reference.matches(&snapshot) || snapshot.expiry <= now() + 60 {
            return Ok(None);
        }
        commit(&reference.view()).map(Some)
    }
    /// Connects the current source idempotently. Only a source reference is persisted.
    /// # Errors
    /// The source is unavailable, unsupported, or cannot be refreshed safely.
    pub fn connect(&self) -> Result<AccountView> {
        self.connect_impl(false)
    }
    /// Connects only if the user has not explicitly disconnected this source.
    /// # Errors
    /// The account is suppressed or the source cannot be connected safely.
    pub fn connect_automatically(&self) -> Result<AccountView> {
        self.connect_impl(true)
    }
    fn connect_impl(&self, automatic: bool) -> Result<AccountView> {
        let _lock = self.lock()?;
        let view = self.discover()?;
        if !matches!(view.state.as_str(), "ready" | "expired") {
            return Err(format!(
                "Local Codex connection is {}. Open Codex and check its login.",
                view.state
            ));
        }
        let snapshot = Snapshot::read(&self.home)?;
        let allowed = self.auto_connect_allowed_locked(&snapshot.id())?;
        if automatic && !allowed {
            return Err("Automatic Codex connection is disabled for this account.".into());
        }
        let r = Reference {
            id: snapshot.id(),
            source: snapshot.source.clone(),
            subject: snapshot.subject.clone(),
            account_id: snapshot.account.clone(),
            email: snapshot.email.clone(),
            client_version: view.client_version,
        };
        if snapshot.expiry <= now() + 60 {
            let token = self.resolve(&r)?;
            drop(south_core::SecretValue::new(token));
        }
        self.bound(&r)?;
        let path = self.path(&r.id)?;
        write_atomic_private(&path, &serde_json::to_vec(&r).map_err(local_error)?)
            .map_err(local_error)?;
        let view = self.account(&r.id)?;
        if !automatic && !allowed {
            // Clear the explicit opt-out only after a successful manual registration.
            fs::remove_file(self.suppression_path(&r.id)?).map_err(local_error)?;
        }
        Ok(view)
    }
    /// Lists local references and safe source states. Does not refresh accounts.
    /// # Errors
    /// A protected reference cannot be read.
    pub fn list(&self) -> Result<Vec<AccountView>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        verify_private_dir(&self.root).map_err(local_error)?;
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(local_error)? {
            let path = entry.map_err(local_error)?.path();
            if path.extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            if let Some(id) = path
                .file_stem()
                .and_then(|v| v.to_str())
                .filter(|v| valid_id(v))
            {
                out.push(self.account(id)?);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }
    /// Returns credentials to Host code only. Never serialize the returned token.
    /// # Errors
    /// The bound source is missing, changed, expired, or cannot be refreshed safely.
    pub fn access_token(&self, id: &str) -> Result<String> {
        let _lock = self.lock()?;
        self.resolve(&self.load(id)?)
    }
    fn resolve(&self, r: &Reference) -> Result<String> {
        let current = self.bound(r)?;
        if current.expiry > now() + 60 {
            return current
                .token("access_token")
                .map(str::to_owned)
                .ok_or_else(|| "Codex access token is missing.".into());
        }
        let _source_lock = lock_file(&r.source.with_file_name(".token-station-refresh.lock"))?;
        let mut original = self.bound(r)?;
        if original.expiry > now() + 60 {
            return original
                .token("access_token")
                .map(str::to_owned)
                .ok_or_else(|| "Codex access token is missing.".into());
        }
        let refresh = original
            .token("refresh_token")
            .ok_or("Codex session expired. Sign in with Codex again.")?;
        let fresh = self.endpoints.refresh(refresh)?;
        let tokens = original
            .document
            .get_mut("tokens")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("Codex token document is invalid.")?;
        for name in ["access_token", "refresh_token", "id_token"] {
            if let Some(value) = fresh
                .get(name)
                .and_then(serde_json::Value::as_str)
                .filter(|v| !v.is_empty())
            {
                tokens.insert(name.into(), serde_json::Value::String(value.into()));
            }
        }
        original.document["last_refresh"] = serde_json::Value::String(transport::timestamp(now()));
        let replacement =
            SecretText::new(serde_json::to_string_pretty(&original.document).map_err(local_error)?);
        let next = Snapshot::parse(r.source.clone(), replacement.expose().as_bytes().to_vec())?;
        if next.subject != r.subject || next.account != r.account_id || next.expiry <= now() + 60 {
            return Err(
                "Codex refresh returned an inconsistent account or expired session.".into(),
            );
        }
        // Codex itself does not honor this lock. Refuse every change observed after refresh.
        let current = self.bound(r)?;
        if current.bytes != original.bytes {
            return Err(
                "Codex changed its login during refresh. Retry after Codex finishes.".into(),
            );
        }
        source::owned_file(&r.source)?;
        write_atomic_private(&r.source, replacement.expose().as_bytes()).map_err(local_error)?;
        self.bound(r)?
            .token("access_token")
            .map(str::to_owned)
            .ok_or_else(|| "Codex access token is missing.".into())
    }
    /// Fetches only the catalog visible to the bound local account.
    /// # Errors
    /// The source or fixed official model endpoint is unavailable.
    pub fn models(&self, id: &str) -> Result<Vec<crate::chatgpt_accounts::PlanModel>> {
        let _lock = self.lock()?;
        let r = self.load(id)?;
        let token = SecretText::new(self.resolve(&r)?);
        // Account identity and token are resolved under the same registration lock.
        self.endpoints
            .models(token.expose(), &r.account_id, &r.client_version)
    }
    /// Removes only the local reference. Does not revoke or delete the Codex login.
    /// # Errors
    /// The local reference cannot be removed safely.
    pub fn disconnect(&self, id: &str) -> Result<()> {
        self.path(id)?;
        let _lock = self.lock()?;
        let p = self.path(id)?;
        let registered = match fs::symlink_metadata(&p) {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(local_error(error)),
        };
        if registered {
            verify_private_file(&p).map_err(local_error)?;
            verify_single_link_file(&p).map_err(local_error)?;
        }
        if self.auto_connect_allowed_locked(id)? {
            // Commit the opt-out first. A crash must not permit automatic reconnection.
            write_atomic_private(&self.suppression_path(id)?, b"1\n").map_err(local_error)?;
        }
        if registered {
            fs::remove_file(p).map_err(local_error)?;
        }
        Ok(())
    }
}
