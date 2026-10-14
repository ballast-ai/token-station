//! References to local Copilot editor and CLI identities. Credentials remain host-owned.
use super::{AccountView, Connection, Model};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use token_station_private_fs::{
    create_private_file, ensure_private_dir, verify_private_dir, verify_private_file,
    verify_single_link_file, write_atomic_private,
};
#[derive(Clone)]
struct SecretText(String);
impl SecretText {
    fn new(s: String) -> Self {
        Self(s)
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
pub const API_BASE: &str = "https://api.githubcopilot.com";
const LIMIT: u64 = 4 * 1024 * 1024;
#[derive(Clone)]
pub struct AccountManager {
    root: PathBuf,
    config: PathBuf,
    cli_home: PathBuf,
    sessions: Arc<Mutex<BTreeMap<String, Session>>>,
    #[cfg(test)]
    fixture: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Reference {
    id: String,
    source: PathBuf,
    key: String,
    login: String,
    kind: String,
    base_url: String,
}
struct Snapshot {
    source: PathBuf,
    key: String,
    login: String,
    kind: String,
    token: SecretText,
}
#[derive(Clone)]
struct Session {
    token: SecretText,
    expires: u64,
    base_url: String,
    source_digest: String,
}
fn fail(_: impl std::fmt::Display) -> String {
    "Cannot access the local Copilot account source.".into()
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
fn valid_login(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 39
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}
fn valid_api(value: &str) -> bool {
    matches!(
        value,
        "https://api.githubcopilot.com"
            | "https://api.individual.githubcopilot.com"
            | "https://api.business.githubcopilot.com"
            | "https://api.enterprise.githubcopilot.com"
    )
}
fn erase(value: &mut Value) {
    match value {
        Value::String(s) => drop(south_core::SecretValue::new(std::mem::take(s))),
        Value::Array(a) => a.iter_mut().for_each(erase),
        Value::Object(o) => o.values_mut().for_each(erase),
        _ => (),
    }
}
struct SecretJson(Value);
impl Drop for SecretJson {
    fn drop(&mut self) {
        erase(&mut self.0);
    }
}
fn read_source(path: &Path) -> Result<SecretJson> {
    let parent = path
        .parent()
        .ok_or("Copilot source directory is invalid.")?;
    let directory = fs::symlink_metadata(parent).map_err(fail)?;
    if directory.is_symlink() || !directory.is_dir() {
        return Err("Copilot source directory is invalid.".into());
    }
    verify_private_file(path).map_err(fail)?;
    verify_single_link_file(path).map_err(fail)?;
    let before = fs::symlink_metadata(path).map_err(fail)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = rustix::process::geteuid().as_raw();
        if before.uid() != owner || directory.uid() != owner || directory.mode() & 0o022 != 0 {
            return Err("Copilot source ownership or directory permissions are unsafe.".into());
        }
    }
    let file = File::open(path).map_err(fail)?;
    let opened = file.metadata().map_err(fail)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if before.dev() != opened.dev() || before.ino() != opened.ino() {
            return Err("Copilot source changed while opening it.".into());
        }
    }
    let mut raw = Vec::new();
    let parsed = (|| {
        file.take(LIMIT + 1).read_to_end(&mut raw).map_err(fail)?;
        verify_private_file(path).map_err(fail)?;
        verify_single_link_file(path).map_err(fail)?;
        let after = fs::symlink_metadata(path).map_err(fail)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if after.dev() != opened.dev() || after.ino() != opened.ino() {
                return Err("Copilot source changed while reading it.".into());
            }
        }
        if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
            return Err("Copilot source changed while reading it.".into());
        }
        if raw.len() as u64 > LIMIT {
            return Err("Copilot source exceeds its size limit.".into());
        }
        serde_json::from_slice(&raw).map(SecretJson).map_err(fail)
    })();
    raw.fill(0);
    parsed
}

impl Snapshot {
    fn reference(&self, base_url: String) -> Reference {
        let source = self.source.to_string_lossy();
        let material = format!(
            "{source}\0{}\0{}\0{}",
            self.key,
            self.login.to_ascii_lowercase(),
            self.kind
        );
        Reference {
            id: digest(material.as_bytes())[..32].into(),
            source: self.source.clone(),
            key: self.key.clone(),
            login: self.login.clone(),
            kind: self.kind.clone(),
            base_url,
        }
    }
    fn view(&self) -> AccountView {
        let r = self.reference(API_BASE.into());
        AccountView {
            id: r.id,
            label: r.login,
            email: None,
            state: "ready".into(),
        }
    }
}
impl AccountManager {
    #[must_use]
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_default();
        let config =
            std::env::var_os("XDG_CONFIG_HOME").map_or_else(|| home.join(".config"), PathBuf::from);
        let cli_home =
            std::env::var_os("COPILOT_HOME").map_or_else(|| home.join(".copilot"), PathBuf::from);
        Self {
            root: data_dir.as_ref().join("copilot-accounts"),
            config,
            cli_home,
            sessions: Arc::default(),
            #[cfg(test)]
            fixture: None,
        }
    }
    fn allowed_source(&self, path: &Path) -> bool {
        [
            self.config.join("github-copilot/apps.json"),
            self.config.join("github-copilot/hosts.json"),
            self.cli_home.join("config.json"),
        ]
        .contains(&path.to_path_buf())
    }
    fn editor(path: &Path, wanted: Option<&str>) -> Result<Snapshot> {
        let document = read_source(path)?;
        let map = document
            .0
            .as_object()
            .ok_or("Local Copilot editor login is invalid.")?;
        for (key, v) in map {
            if !(key == "github.com" || key.starts_with("github.com:"))
                || wanted.is_some_and(|k| k != key)
            {
                continue;
            }
            let login = v["user"]
                .as_str()
                .filter(|s| valid_login(s))
                .ok_or("Local Copilot login has no valid account identity.")?;
            let token = v["oauth_token"]
                .as_str()
                .filter(|s| !s.is_empty() && !s.contains(['\r', '\n']))
                .ok_or("Local Copilot editor token is missing.")?;
            return Ok(Snapshot {
                source: path.into(),
                key: key.clone(),
                login: login.into(),
                kind: "editor".into(),
                token: SecretText::new(token.to_owned()),
            });
        }
        Err("Local Copilot editor login is missing. Sign in with Copilot.".into())
    }
    fn cli(path: &Path, wanted: Option<&str>) -> Result<Snapshot> {
        let document = read_source(path)?;
        let mut users = Vec::new();
        if document.0["lastLoggedInUser"].is_object() {
            users.push(&document.0["lastLoggedInUser"]);
        }
        if let Some(more) = document.0["loggedInUsers"].as_array() {
            users.extend(more);
        }
        for user in users {
            if !matches!(
                user["host"].as_str(),
                Some("https://github.com" | "https://github.com/" | "github.com")
            ) {
                continue;
            }
            let login = user["login"]
                .as_str()
                .filter(|s| valid_login(s))
                .ok_or("Local Copilot CLI identity is invalid.")?;
            let key = format!("https://github.com:{login}");
            if wanted.is_some_and(|k| k != key) {
                continue;
            }
            let token = match document.0["copilotTokens"][&key]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                Some(t) => t.into(),
                None => cli_keychain(&key)?,
            };
            if token.contains(['\r', '\n']) {
                return Err("Local Copilot CLI token is invalid.".into());
            }
            return Ok(Snapshot {
                source: path.into(),
                key,
                login: login.into(),
                kind: "cli".into(),
                token: SecretText::new(token),
            });
        }
        Err("Local Copilot CLI login is missing. Run copilot login.".into())
    }
    fn snapshot(&self) -> Result<Snapshot> {
        for name in ["apps.json", "hosts.json"] {
            let path = self.config.join("github-copilot").join(name);
            if path.exists() {
                return Self::editor(&path, None);
            }
        }
        Self::cli(&self.cli_home.join("config.json"), None)
    }
    /// Returns safe source metadata. Does not contact an upstream.
    /// # Errors
    /// The local source cannot be inspected.
    pub fn discover(&self) -> Result<AccountView> {
        match self.snapshot() {
            Ok(s) => Ok(s.view()),
            Err(_) => Ok(AccountView {
                id: String::new(),
                label: "Local Copilot account".into(),
                email: None,
                state: if self.cli_home.join("config.json").exists()
                    || self.config.join("github-copilot/apps.json").exists()
                    || self.config.join("github-copilot/hosts.json").exists()
                {
                    "invalid_source"
                } else {
                    "missing"
                }
                .into(),
            }),
        }
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid Copilot account reference.".into());
        }
        Ok(self.root.join(format!("{id}.json")))
    }
    fn lock(&self) -> Result<File> {
        ensure_private_dir(&self.root).map_err(fail)?;
        let path = self.root.join("references.lock");
        create_private_file(&path, b"").map_err(fail)?;
        verify_private_file(&path).map_err(fail)?;
        verify_single_link_file(&path).map_err(fail)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(fail)?;
        file.lock().map_err(fail)?;
        Ok(file)
    }
    fn load(&self, id: &str) -> Result<Reference> {
        verify_private_dir(&self.root).map_err(fail)?;
        let d = read_source(&self.path(id)?)?;
        let r: Reference = serde_json::from_value(d.0.clone()).map_err(fail)?;
        if r.id != id
            || !self.allowed_source(&r.source)
            || !valid_api(&r.base_url)
            || !valid_login(&r.login)
        {
            return Err("Invalid Copilot reference metadata.".into());
        }
        Ok(r)
    }
    fn bound(&self, r: &Reference) -> Result<Snapshot> {
        if !self.allowed_source(&r.source) {
            return Err("Copilot reference source is invalid.".into());
        }
        let s = match r.kind.as_str() {
            "editor" => Self::editor(&r.source, Some(&r.key)),
            "cli" => Self::cli(&r.source, Some(&r.key)),
            _ => Err("Unknown Copilot credential source.".into()),
        }?;
        if !s.login.eq_ignore_ascii_case(&r.login) || s.reference(r.base_url.clone()).id != r.id {
            return Err(
                "The local Copilot account changed. Reconnect the intended account.".into(),
            );
        }
        Ok(s)
    }
    fn get(url: &str, token: &str, kind: &str, github: bool) -> Result<SecretJson> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into();
        let headers = identity_headers(kind);
        let mut req = agent
            .get(url)
            .header(
                "Authorization",
                &format!("{} {token}", if github { "token" } else { "Bearer" }),
            )
            .header("Accept", "application/json");
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let mut response = req
            .call()
            .map_err(|_| "Copilot request failed. Check the login in Copilot.")?;
        if response.status().as_u16() != 200 {
            return Err(
                "Copilot authorization or plan access was rejected. Sign in with Copilot again."
                    .into(),
            );
        }
        let mut bytes = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(fail)?;
        let result = if bytes.len() as u64 > LIMIT {
            Err("Copilot response exceeds its size limit.".into())
        } else {
            serde_json::from_slice(&bytes)
                .map(SecretJson)
                .map_err(|_| "Copilot returned an invalid response.".into())
        };
        bytes.fill(0);
        result
    }
    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn github_url(&self, path: &str) -> String {
        #[cfg(test)]
        if let Some(base) = &self.fixture {
            return format!("{base}{path}");
        }
        format!("https://api.github.com{path}")
    }
    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn api_url(&self, base: &str, path: &str) -> String {
        #[cfg(test)]
        if let Some(fixture) = &self.fixture {
            return format!("{fixture}{path}");
        }
        format!("{base}{path}")
    }
    fn session(&self, s: &Snapshot) -> Result<Session> {
        let source_digest = digest(s.token.expose().as_bytes());
        let id = s.reference(API_BASE.into()).id;
        let mut cache = self.sessions.lock().map_err(fail)?;
        if let Some(cached) = cache
            .get(&id)
            .filter(|v| v.expires > now() + 120 && v.source_digest == source_digest)
        {
            return Ok(cached.clone());
        }
        let user = Self::get(&self.github_url("/user"), s.token.expose(), &s.kind, true)?;
        if !user.0["login"]
            .as_str()
            .is_some_and(|login| login.eq_ignore_ascii_case(&s.login))
        {
            return Err("Copilot token identity does not match its local account.".into());
        }
        let session = if s.kind == "cli" {
            Session {
                token: s.token.clone(),
                expires: now() + 300,
                base_url: API_BASE.into(),
                source_digest,
            }
        } else {
            let reply = Self::get(
                &self.github_url("/copilot_internal/v2/token"),
                s.token.expose(),
                "editor",
                true,
            )?;
            let token = reply.0["token"]
                .as_str()
                .filter(|s| !s.is_empty() && !s.contains(['\r', '\n']))
                .ok_or("Copilot did not return a session token.")?;
            let expires = reply.0["expires_at"]
                .as_u64()
                .filter(|e| *e > now() + 60)
                .ok_or("Copilot returned an expired session.")?;
            let base = reply.0["endpoints"]["api"].as_str().unwrap_or(API_BASE);
            if !valid_api(base) {
                return Err("Copilot returned an untrusted API destination.".into());
            }
            Session {
                token: SecretText::new(token.into()),
                expires,
                base_url: base.into(),
                source_digest,
            }
        };
        cache.insert(id, session.clone());
        Ok(session)
    }
    /// Verifies the local identity and plan, then stores a token-free reference.
    /// # Errors
    /// The local login, identity, entitlement, or model catalog is invalid.
    pub fn connect(&self) -> Result<AccountView> {
        let _lock = self.lock()?;
        let s = self.snapshot()?;
        let session = self.session(&s)?;
        let raw = Self::get(
            &self.api_url(&session.base_url, "/models"),
            session.token.expose(),
            &s.kind,
            false,
        )?;
        catalog(&raw.0)?;
        let r = s.reference(session.base_url);
        self.bound(&r)?;
        write_atomic_private(&self.path(&r.id)?, &serde_json::to_vec(&r).map_err(fail)?)
            .map_err(fail)?;
        Ok(s.view())
    }
    /// Lists token-free references and detects replaced local identities.
    /// # Errors
    /// The protected reference store is invalid.
    pub fn list(&self) -> Result<Vec<AccountView>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        verify_private_dir(&self.root).map_err(fail)?;
        let mut out = Vec::new();
        for e in fs::read_dir(&self.root).map_err(fail)? {
            let p = e.map_err(fail)?.path();
            if p.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let id = p
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or("Invalid Copilot reference filename.")?;
            let r = self.load(id)?;
            out.push(AccountView {
                id: r.id.clone(),
                label: r.login.clone(),
                email: None,
                state: if self.bound(&r).is_ok() {
                    "ready"
                } else {
                    "account_changed"
                }
                .into(),
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }
    /// Returns only the observed models for the bound subscription.
    /// # Errors
    /// The account or upstream model catalog is unavailable.
    pub fn models(&self, id: &str) -> Result<Vec<Model>> {
        Ok(self.connection(id)?.models)
    }
    /// Resolves the validated destination and safe per-model endpoint metadata.
    /// # Errors
    /// The local identity, session, or catalog is invalid.
    pub fn connection(&self, id: &str) -> Result<Connection> {
        let _lock = self.lock()?;
        let r = self.load(id)?;
        let s = self.bound(&r)?;
        let session = self.session(&s)?;
        if session.base_url != r.base_url {
            return Err("The Copilot API destination changed. Reconnect the account.".into());
        }
        let raw = Self::get(
            &self.api_url(&session.base_url, "/models"),
            session.token.expose(),
            &s.kind,
            false,
        )?;
        let (models, endpoints) = catalog(&raw.0)?;
        self.bound(&r)?;
        Ok(Connection {
            account: s.view(),
            base_url: session.base_url,
            models,
            metadata: BTreeMap::from([
                ("copilot_source_kind".into(), json!(s.kind)),
                ("copilot_model_endpoints".into(), json!(endpoints)),
            ]),
        })
    }
    /// Resolves the bound inference token for host use only.
    /// # Errors
    /// The bound account is unavailable or its session is invalid.
    pub fn access_token(&self, id: &str) -> Result<String> {
        let _lock = self.lock()?;
        let r = self.load(id)?;
        let s = self.bound(&r)?;
        let session = self.session(&s)?;
        self.bound(&r)?;
        if session.base_url != r.base_url {
            return Err("The Copilot API destination changed. Reconnect the account.".into());
        }
        Ok(session.token.expose().into())
    }
    /// Removes only the reference. Leaves the Copilot login unchanged.
    /// # Errors
    /// The reference cannot be removed safely.
    pub fn disconnect(&self, id: &str) -> Result<()> {
        let path = self.path(id)?;
        let _lock = self.lock()?;
        if path.exists() {
            verify_private_file(&path).map_err(fail)?;
            verify_single_link_file(&path).map_err(fail)?;
            fs::remove_file(path).map_err(fail)?;
        }
        self.sessions.lock().map_err(fail)?.remove(id);
        Ok(())
    }
}
fn identity_headers(kind: &str) -> Vec<(&'static str, &'static str)> {
    if kind == "cli" {
        vec![
            ("Editor-Version", "copilot/1.0.88"),
            ("Copilot-Integration-Id", "copilot-developer-cli"),
            ("X-GitHub-Api-Version", "2026-07-01"),
            ("User-Agent", "copilot/1.0.88"),
        ]
    } else {
        vec![
            ("Editor-Version", "vscode/1.140.0"),
            ("Editor-Plugin-Version", "copilot-chat/0.68.0"),
            ("Copilot-Integration-Id", "vscode-chat"),
            ("X-GitHub-Api-Version", "2026-01-09"),
            ("User-Agent", "GitHubCopilotChat/0.68.0"),
        ]
    }
}
fn catalog(raw: &Value) -> Result<(Vec<Model>, BTreeMap<String, Value>)> {
    let data = raw["data"]
        .as_array()
        .ok_or("Copilot returned an invalid model catalog.")?;
    let mut models = Vec::new();
    let mut endpoints = BTreeMap::new();
    for m in data {
        let id = m["id"].as_str().unwrap_or_default();
        if id.is_empty()
            || id == "auto"
            || id.starts_with("copilot-")
            || m["capabilities"]["type"] != "chat"
            || m["vendor"] == "Experimental"
            || m["model_picker_enabled"] != true
            || m.get("policy").is_some_and(|p| p["state"] != "enabled")
        {
            continue;
        }
        let paths = match m.get("supported_endpoints").and_then(Value::as_array) {
            None => vec![json!("/chat/completions")],
            Some(paths) if paths.is_empty() => vec![json!("/chat/completions")],
            Some(paths) => paths
                .iter()
                .filter(|p| {
                    matches!(
                        p.as_str(),
                        Some("/chat/completions" | "/responses" | "/v1/messages")
                    )
                })
                .cloned()
                .collect(),
        };
        if paths.is_empty() {
            continue;
        }
        if endpoints.contains_key(id) {
            return Err("Copilot returned duplicate model identities.".into());
        }
        endpoints.insert(id.into(), json!(paths));
        let capabilities = &m["capabilities"];
        models.push(Model {
            id: id.into(),
            name: m["name"].as_str().unwrap_or(id).into(),
            context_window: capabilities["limits"]["max_context_window_tokens"].as_u64(),
            supports_images: capabilities["supports"]["vision"].as_bool(),
            supports_tools: capabilities["supports"]["tool_calls"].as_bool(),
        });
    }
    if models.is_empty() {
        return Err("Copilot has no enabled supported models. Check the plan in Copilot.".into());
    }
    Ok((models, endpoints))
}
#[cfg(target_os = "macos")]
fn cli_keychain(account: &str) -> Result<String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/security")
        .args([
            "find-generic-password",
            "-s",
            "copilot-cli",
            "-a",
            account,
            "-w",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(fail)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait().map_err(fail)? {
            Some(status) => {
                if !status.success() {
                    return Err(
                        "Copilot CLI keychain login is unavailable. Sign in with Copilot.".into(),
                    );
                }
                break;
            }
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Copilot CLI keychain access timed out.".into());
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut bytes = Vec::new();
    child
        .stdout
        .take()
        .ok_or("Copilot CLI keychain output is missing.")?
        .take(16385)
        .read_to_end(&mut bytes)
        .map_err(fail)?;
    let result = if bytes.len() > 16384 {
        Err("Copilot CLI keychain token is invalid.".into())
    } else {
        std::str::from_utf8(&bytes)
            .map(|v| v.trim_end_matches(['\r', '\n']).to_owned())
            .map_err(fail)
    };
    bytes.fill(0);
    result.and_then(|s| {
        if s.is_empty() {
            Err("Copilot CLI login token is missing.".into())
        } else {
            Ok(s)
        }
    })
}
#[cfg(not(target_os = "macos"))]
fn cli_keychain(_: &str) -> Result<String> {
    Err("Copilot CLI token storage is unsupported. Use a local Copilot editor login.".into())
}
#[cfg(test)]
#[path = "copilot/tests.rs"]
mod tests;
