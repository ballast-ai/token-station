//! References local Google logins. Tokens are never stored in Token Station records.
use super::{AccountView, Connection, Kind, Model};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use token_station_private_fs::{ensure_private_dir, verify_private_file, write_atomic_private};
type Result<T> = std::result::Result<T, String>;
const LIMIT: u64 = 1024 * 1024;
const GEMINI_CLIENT: &str =
    "681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com";
const AG_CLIENT: &str = "1071006060591-tmhssin2h21lcre235vtolojh4g403ep.apps.googleusercontent.com";
#[derive(Clone)]
pub struct AccountManager {
    root: PathBuf,
    home: PathBuf,
    kind: Kind,
    endpoints: Endpoints,
    cache: Arc<Mutex<BTreeMap<String, CachedToken>>>,
}
struct CachedToken {
    access: String,
    email: String,
    expires_at: u64,
    checked_at: u64,
}
impl Drop for CachedToken {
    fn drop(&mut self) {
        drop(south_core::SecretValue::new(std::mem::take(
            &mut self.access,
        )));
    }
}
#[derive(Clone)]
struct Endpoints {
    token: String,
    user: String,
    prod: String,
    daily: String,
}
impl Default for Endpoints {
    fn default() -> Self {
        Self {
            token: "https://oauth2.googleapis.com/token".into(),
            user: "https://www.googleapis.com/oauth2/v2/userinfo?alt=json".into(),
            prod: "https://cloudcode-pa.googleapis.com".into(),
            daily: "https://daily-cloudcode-pa.googleapis.com".into(),
        }
    }
}
#[derive(Serialize, Deserialize)]
struct Record {
    id: String,
    source: String,
    fingerprint: String,
    email: String,
}
struct Login {
    source: String,
    access: String,
    refresh: String,
    expiry: u64,
    project: Option<String>,
}
impl Drop for Login {
    fn drop(&mut self) {
        drop(south_core::SecretValue::new(std::mem::take(
            &mut self.access,
        )));
        drop(south_core::SecretValue::new(std::mem::take(
            &mut self.refresh,
        )));
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn field(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().to_owned()
}
fn missing(kind: Kind) -> String {
    match kind {
        Kind::GeminiCli => "Sign in with Gemini CLI on this computer. Then retry the connection.",
        _ => "Sign in with Antigravity on this computer. Then retry the connection.",
    }
    .into()
}
fn failure() -> String {
    "The local Google subscription could not be verified. Sign in with its application and retry."
        .into()
}
fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    let mut raw = Vec::new();
    fs::File::open(path)
        .map_err(|_| failure())?
        .take(LIMIT + 1)
        .read_to_end(&mut raw)
        .map_err(|_| failure())?;
    if raw.len() as u64 > LIMIT {
        return Err(failure());
    }
    Ok(raw)
}
fn hash(v: &str) -> String {
    format!("{:x}", Sha256::digest(v.as_bytes()))
}
impl AccountManager {
    #[must_use]
    pub fn new(data_dir: impl AsRef<Path>, kind: Kind) -> Self {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .unwrap_or_default();
        let suffix = if kind == Kind::GeminiCli {
            "gemini-cli"
        } else {
            "antigravity"
        };
        Self {
            root: data_dir.as_ref().join("local-subscriptions").join(suffix),
            home,
            kind,
            endpoints: Endpoints::default(),
            cache: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    fn valid_kind(&self) -> Result<()> {
        if !matches!(self.kind, Kind::GeminiCli | Kind::Antigravity)
            || self.home.as_os_str().is_empty()
        {
            return Err("Invalid Google subscription kind or local home directory.".into());
        }
        Ok(())
    }
    fn agent() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into()
    }
    fn json(reply: ureq::http::Response<ureq::Body>) -> Result<Value> {
        let mut reply = reply;
        if reply.status().as_u16() != 200 {
            return Err(failure());
        }
        let mut raw = Vec::new();
        reply
            .body_mut()
            .as_reader()
            .take(4 * LIMIT + 1)
            .read_to_end(&mut raw)
            .map_err(|_| failure())?;
        let value = if raw.len() as u64 > 4 * LIMIT {
            Err(failure())
        } else {
            serde_json::from_slice(&raw).map_err(|_| failure())
        };
        raw.fill(0);
        value
    }
    fn post(&self, access: &str, base: &str, method: &str, body: Value) -> Result<Value> {
        let url = format!("{base}/v1internal:{method}");
        let payload = body.to_string();
        drop(body);
        let reply = Self::agent()
            .post(&url)
            .header("Authorization", &format!("Bearer {access}"))
            .header("Content-Type", "application/json")
            .header(
                "User-Agent",
                if self.kind == Kind::GeminiCli {
                    "GeminiCLI/0.61.0 (Token Station; terminal)"
                } else {
                    "antigravity/hub/2.9.1"
                },
            )
            .send(payload)
            .map_err(|_| failure())?;
        Self::json(reply)
    }
    fn authorized(&self, login: &mut Login) -> Result<String> {
        let fingerprint = hash(&login.refresh);
        let mut cache = self.cache.lock().map_err(|_| failure())?;
        if let Some(token) = cache.get(&fingerprint)
            && token.expires_at > now() + 120
            && token.checked_at + 300 > now()
        {
            login.access.clone_from(&token.access);
            login.expiry = token.expires_at;
            return Ok(token.email.clone());
        }

        if login.access.is_empty() || login.expiry <= now() + 120 {
            let client = if self.kind == Kind::GeminiCli {
                GEMINI_CLIENT
            } else {
                AG_CLIENT
            };
            let secret = if self.kind == Kind::GeminiCli {
                "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"
            } else {
                "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"
            };
            let mut form = url::form_urlencoded::Serializer::new(String::new());
            form.extend_pairs([
                ("grant_type", "refresh_token"),
                ("refresh_token", login.refresh.as_str()),
                ("client_id", client),
                ("client_secret", secret),
            ]);
            let payload = form.finish();
            let reply = Self::agent()
                .post(&self.endpoints.token)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .send(payload.as_bytes())
                .map_err(|_| failure())?;
            drop(south_core::SecretValue::new(payload));
            let mut v = Self::json(reply)?;
            login.access = v["access_token"].take().as_str().unwrap_or_default().into();
            login.expiry = now().saturating_add(v["expires_in"].as_u64().unwrap_or(0));
            if login.access.is_empty() || login.expiry <= now() {
                return Err(failure());
            }
        }
        let reply = Self::agent()
            .get(&self.endpoints.user)
            .header("Authorization", &format!("Bearer {}", login.access))
            .call()
            .map_err(|_| failure())?;
        let v = Self::json(reply)?;
        let email = field(&v, "email");
        if email.is_empty() || email.len() > 254 || email.chars().any(char::is_control) {
            return Err(failure());
        }
        cache.insert(
            fingerprint,
            CachedToken {
                access: login.access.clone(),
                email: email.clone(),
                expires_at: login.expiry,
                checked_at: now(),
            },
        );
        Ok(email)
    }
    fn sources(&self) -> Vec<(String, PathBuf)> {
        if self.kind == Kind::GeminiCli {
            return vec![(
                "gemini-file".into(),
                self.home.join(".gemini/oauth_creds.json"),
            )];
        }
        let mut out = vec![(
            "antigravity-file".into(),
            self.home
                .join(".gemini/antigravity-cli/antigravity-oauth-token"),
        )];
        let db = if cfg!(target_os = "macos") {
            self.home
                .join("Library/Application Support/Antigravity/User/globalStorage/state.vscdb")
        } else if cfg!(windows) {
            self.home
                .join("AppData/Roaming/Antigravity/User/globalStorage/state.vscdb")
        } else {
            self.home
                .join(".config/Antigravity/User/globalStorage/state.vscdb")
        };
        out.push(("antigravity-db".into(), db));
        out
    }
    fn local(&self, source: Option<&str>) -> Result<Login> {
        self.valid_kind()?;
        for (name, path) in self.sources() {
            if source.is_some_and(|s| s != name) {
                continue;
            }
            if !path.exists() {
                continue;
            }
            let info = fs::symlink_metadata(&path).map_err(|_| failure())?;
            if !info.file_type().is_file() {
                return Err(failure());
            }
            return if name == "antigravity-db" {
                Self::database(&path)
            } else {
                let mut raw = read_bounded(&path)?;
                let v = serde_json::from_slice(&raw).map_err(|_| failure());
                raw.fill(0);
                self.parse_login(&name, &v?)
            };
        }
        if self.kind == Kind::Antigravity && source.is_none_or(|s| s == "antigravity-keyring") {
            let mut raw = keyring()?;
            let text = std::str::from_utf8(&raw).map_err(|_| failure())?.trim();
            let mut decoded = if let Some(b64) = text.strip_prefix("go-keyring-base64:") {
                STANDARD.decode(b64).map_err(|_| failure())?
            } else {
                text.as_bytes().to_vec()
            };
            let v = serde_json::from_slice(&decoded).map_err(|_| failure());
            raw.fill(0);
            decoded.fill(0);
            return self.parse_login("antigravity-keyring", &v?);
        }
        Err(missing(self.kind))
    }
    fn parse_login(&self, source: &str, v: &Value) -> Result<Login> {
        if self.kind == Kind::GeminiCli
            && let Some(jwt) = v["id_token"].as_str()
        {
            let claims = jwt
                .split('.')
                .nth(1)
                .and_then(|s| URL_SAFE_NO_PAD.decode(s).ok())
                .and_then(|s| serde_json::from_slice::<Value>(&s).ok());
            if claims
                .as_ref()
                .and_then(|v| v.get("azp").or_else(|| v.get("aud")))
                .and_then(Value::as_str)
                .is_some_and(|id| id != GEMINI_CLIENT)
            {
                return Err(
                    "The local OAuth file belongs to another application. Sign in with Gemini CLI."
                        .into(),
                );
            }
        }
        let token = v.get("token").unwrap_or(v);
        let refresh = field(token, "refresh_token");
        if refresh.is_empty() || refresh.len() > 16 * 1024 {
            return Err(missing(self.kind));
        }
        let expiry = token["expiry_date"]
            .as_u64()
            .map(|n| n / 1000)
            .or_else(|| {
                token["expiry"]
                    .as_str()
                    .and_then(|s| {
                        time::OffsetDateTime::parse(
                            s,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .ok()
                    })
                    .and_then(|t| u64::try_from(t.unix_timestamp()).ok())
            })
            .unwrap_or(0);
        let mut project = v
            .get("project_id")
            .or_else(|| v.get("project"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if self.kind == Kind::GeminiCli
            && project.is_none()
            && let Ok(raw) = read_bounded(&self.home.join(".gemini/.env"))
            && let Ok(text) = std::str::from_utf8(&raw)
        {
            project = text.lines().find_map(|l| {
                l.trim()
                    .strip_prefix("GOOGLE_CLOUD_PROJECT=")
                    .map(|v| v.trim().trim_matches(['\'', '"']).to_owned())
            });
        }
        Ok(Login {
            source: source.into(),
            refresh,
            access: field(token, "access_token"),
            expiry,
            project,
        })
    }
    fn database(path: &Path) -> Result<Login> {
        let db = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| failure())?;
        let get = |key: &str| {
            db.query_row(
                "SELECT value FROM ItemTable WHERE key=?1 AND length(value)<=?2",
                rusqlite::params![key, 1_048_576_i64],
                |row| row.get::<_, String>(0),
            )
            .map_err(|_| failure())
        };
        let mut oauth = if let Ok(s) = get("antigravityUnifiedStateSync.oauthToken") {
            decode_topic(&s)?
        } else {
            let s = get("jetskiStateSync.agentManagerInitState")?;
            let raw = STANDARD.decode(s).map_err(|_| failure())?;
            proto_field(&raw, 6)?.ok_or_else(failure)?.to_vec()
        };
        let access = proto_field(&oauth, 1)?
            .and_then(|s| std::str::from_utf8(s).ok())
            .unwrap_or_default()
            .to_owned();
        let refresh = proto_field(&oauth, 3)?
            .and_then(|s| std::str::from_utf8(s).ok())
            .filter(|s| !s.is_empty())
            .ok_or_else(failure)?
            .to_owned();
        let expiry = proto_field(&oauth, 4)?
            .and_then(|s| proto_varint(s, 1).ok().flatten())
            .unwrap_or(0);
        oauth.fill(0);
        Ok(Login {
            source: "antigravity-db".into(),
            access,
            refresh,
            expiry,
            project: None,
        })
    }
    fn record(&self, id: &str) -> Result<Record> {
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid Google subscription reference.".into());
        }
        let path = self.root.join(format!("{id}.json"));
        verify_private_file(&path).map_err(|_| failure())?;
        let r: Record = serde_json::from_slice(&read_bounded(&path)?).map_err(|_| failure())?;
        if r.id != id
            || !self.sources().iter().any(|(s, _)| *s == r.source)
                && r.source != "antigravity-keyring"
        {
            return Err(failure());
        }
        Ok(r)
    }
    fn bound(&self, id: &str) -> Result<(Record, Login)> {
        let r = self.record(id)?;
        let mut login = self.local(Some(&r.source))?;
        if hash(&login.refresh) != r.fingerprint {
            return Err("The local Google account changed. Reconnect this subscription.".into());
        }
        let email = self.authorized(&mut login)?;
        if email != r.email {
            return Err("The local Google account changed. Reconnect this subscription.".into());
        }
        Ok((r, login))
    }
    fn view(r: &Record, state: &str) -> AccountView {
        AccountView {
            id: r.id.clone(),
            label: r.email.clone(),
            email: Some(r.email.clone()),
            state: state.into(),
        }
    }
    /// Inspect a local login candidate without contacting Google or creating a binding.
    /// # Errors
    /// The local login, protected reference, or Google account cannot be verified.
    pub fn discover(&self) -> Result<AccountView> {
        self.valid_kind()?;
        match self.local(None) {
            Ok(login) => Ok(AccountView {
                id: hash(&format!(
                    "{:?}:{}:{}",
                    self.kind,
                    login.source,
                    hash(&login.refresh)
                )),
                label: if self.kind == Kind::GeminiCli {
                    "Local Gemini CLI account"
                } else {
                    "Local Antigravity account"
                }
                .into(),
                email: None,
                state: "ready".into(),
            }),
            Err(error) => Ok(AccountView {
                id: String::new(),
                label: if self.kind == Kind::GeminiCli {
                    "Local Gemini CLI account"
                } else {
                    "Local Antigravity account"
                }
                .into(),
                email: None,
                state: if error.contains("keyring") && !error.contains("Sign in") {
                    "unsupported_storage"
                } else if self.sources().iter().any(|(_, path)| path.exists()) {
                    "invalid_source"
                } else {
                    "missing"
                }
                .into(),
            }),
        }
    }
    /// Return safe metadata for saved references, including stale references.
    /// # Errors
    /// The local login, protected reference, or Google account cannot be verified.
    pub fn list(&self) -> Result<Vec<AccountView>> {
        self.valid_kind()?;
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for item in fs::read_dir(&self.root).map_err(|_| failure())? {
            let p = item.map_err(|_| failure())?.path();
            if p.extension().is_none_or(|s| s != "json") {
                continue;
            }
            let id = p.file_stem().and_then(|s| s.to_str()).ok_or_else(failure)?;
            let r = self.record(id)?;
            let valid = self
                .local(Some(&r.source))
                .is_ok_and(|l| hash(&l.refresh) == r.fingerprint);
            out.push(Self::view(
                &r,
                if valid { "ready" } else { "account_changed" },
            ));
        }
        Ok(out)
    }
    /// Bind the verified login after project and model discovery succeed.
    /// # Errors
    /// The local login, protected reference, or Google account cannot be verified.
    pub fn connect(&self) -> Result<AccountView> {
        let mut login = self.local(None)?;
        let email = self.authorized(&mut login)?;
        let (_project, models) = self.catalog(&login)?;
        if models.is_empty() {
            return Err("Code Assist returned no models for this local account.".into());
        }
        let fingerprint = hash(&login.refresh);
        let id = hash(&format!(
            "{:?}:{}:{email}:{fingerprint}",
            self.kind, login.source
        ));
        let r = Record {
            id,
            source: login.source.clone(),
            fingerprint,
            email,
        };
        ensure_private_dir(&self.root).map_err(|_| failure())?;
        write_atomic_private(
            &self.root.join(format!("{}.json", r.id)),
            &serde_json::to_vec(&r).map_err(|_| failure())?,
        )
        .map_err(|_| failure())?;
        Ok(Self::view(&r, "ready"))
    }
    /// # Errors
    /// The local login, protected reference, or Google account cannot be verified.
    pub fn models(&self, id: &str) -> Result<Vec<Model>> {
        let (_, login) = self.bound(id)?;
        self.catalog(&login).map(|(_, m)| m)
    }
    /// # Errors
    /// The local login, protected reference, or Google account cannot be verified.
    /// # Errors
    /// The local login, protected reference, or Google account cannot be verified.
    pub fn connection(&self, id: &str) -> Result<Connection> {
        let (r, login) = self.bound(id)?;
        let (project, models) = self.catalog(&login)?;
        let mut metadata = BTreeMap::new();
        metadata.insert("google_project".into(), json!(project));
        metadata.insert(
            "google_client_version".into(),
            json!(if self.kind == Kind::GeminiCli {
                "0.61.0"
            } else {
                "2.9.1"
            }),
        );
        Ok(Connection {
            account: Self::view(&r, "ready"),
            base_url: if self.kind == Kind::GeminiCli {
                self.endpoints.prod.clone()
            } else {
                self.endpoints.daily.clone()
            },
            models,
            metadata,
        })
    }
    /// # Errors
    /// The local login, protected reference, or Google account cannot be verified.
    pub fn access_token(&self, id: &str) -> Result<String> {
        let (_, mut login) = self.bound(id)?;
        Ok(std::mem::take(&mut login.access))
    }
    /// # Errors
    /// The local login, protected reference, or Google account cannot be verified.
    pub fn disconnect(&self, id: &str) -> Result<()> {
        self.record(id)?;
        fs::remove_file(self.root.join(format!("{id}.json"))).map_err(|_| failure())
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Keep the vendor project handshake and account catalog in one reviewable sequence."
    )]
    fn catalog(&self, l: &Login) -> Result<(String, Vec<Model>)> {
        let mut meta = if self.kind == Kind::GeminiCli {
            json!({"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"})
        } else {
            json!({"ideType":"ANTIGRAVITY"})
        };
        let mut body = json!({"metadata":meta});
        if let Some(project) = &l.project {
            body["cloudaicompanionProject"] = json!(project);
            meta["duetProject"] = json!(project);
            body["metadata"] = meta;
        }
        let load = self.post(&l.access, &self.endpoints.prod, "loadCodeAssist", body)?;
        let mut project = load["cloudaicompanionProject"]
            .as_str()
            .or_else(|| load["cloudaicompanionProject"]["id"].as_str())
            .map(str::to_owned)
            .or_else(|| l.project.clone());
        if project.is_none() {
            let tier=load["allowedTiers"].as_array().and_then(|ts|ts.iter().find(|t|t["isDefault"].as_bool()==Some(true))).ok_or("Code Assist requires a Google Cloud project. Configure it in the local application.")?;
            if tier["userDefinedCloudaicompanionProject"].as_bool() == Some(true) {
                return Err(
                    "Configure GOOGLE_CLOUD_PROJECT in the local Gemini CLI .env file. Then retry."
                        .into(),
                );
            }
            let req = if self.kind == Kind::GeminiCli {
                json!({"tierId":tier["id"],"metadata":{"ideType":"IDE_UNSPECIFIED","platform":"PLATFORM_UNSPECIFIED","pluginType":"GEMINI"}})
            } else {
                json!({"tier_id":tier["id"],"metadata":{"ide_type":"ANTIGRAVITY","ide_name":"antigravity","ide_version":"2.9.1"}})
            };
            let base = if self.kind == Kind::GeminiCli {
                &self.endpoints.prod
            } else {
                &self.endpoints.daily
            };
            for _ in 0..5 {
                let v = self.post(&l.access, base, "onboardUser", req.clone())?;
                if v["done"].as_bool() == Some(true) {
                    project = v["response"]["cloudaicompanionProject"]
                        .as_str()
                        .or_else(|| v["response"]["cloudaicompanionProject"]["id"].as_str())
                        .map(str::to_owned);
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        let project = project
            .filter(|p| !p.is_empty() && p.len() <= 256 && !p.chars().any(char::is_control))
            .ok_or("Sign in with the local Google application to complete Code Assist setup.")?;
        let models = if self.kind == Kind::Antigravity {
            let mut req = json!({"project":project});
            if let Some(tier) = load["paidTier"]["id"]
                .as_str()
                .or_else(|| load["currentTier"]["id"].as_str())
                .filter(|s| !matches!(*s, "free-tier" | "legacy-tier"))
            {
                req["entitlement"] = json!({"userTier":tier});
            }
            let v = self
                .post(
                    &l.access,
                    &self.endpoints.daily,
                    "fetchAvailableModels",
                    req.clone(),
                )
                .or_else(|_| {
                    req.as_object_mut().map(|o| o.remove("entitlement"));
                    self.post(
                        &l.access,
                        &self.endpoints.daily,
                        "fetchAvailableModels",
                        req,
                    )
                })?;
            v["models"]
                .as_object()
                .ok_or("Code Assist returned an invalid model catalog.")?
                .iter()
                .filter(|(id, m)| {
                    !id.starts_with("chat_")
                        && !id.starts_with("tab_")
                        && m["disabled"].as_bool() != Some(true)
                })
                .map(|(id, m)| Model {
                    id: id.clone(),
                    name: m["displayName"].as_str().unwrap_or(id).into(),
                    context_window: m["maxTokens"].as_u64(),
                    supports_images: m["supportsImages"].as_bool(),
                    supports_tools: Some(true),
                })
                .collect()
        } else {
            let v = self.post(
                &l.access,
                &self.endpoints.prod,
                "retrieveUserQuota",
                json!({"project":project}),
            )?;
            let mut models = BTreeMap::new();
            for b in v["buckets"]
                .as_array()
                .ok_or("Code Assist returned no observable model catalog.")?
            {
                if let Some(id) = b["modelId"]
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.ends_with("_vertex"))
                {
                    models.insert(
                        id.to_owned(),
                        Model {
                            id: id.into(),
                            name: id.into(),
                            context_window: None,
                            supports_images: None,
                            supports_tools: Some(true),
                        },
                    );
                }
            }
            models.into_values().collect()
        };
        Ok((project, models))
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "Keep platform-specific read-only keyring commands and bounded process handling together."
)]
fn keyring() -> Result<Vec<u8>> {
    use std::process::{Command, Stdio};
    #[cfg(target_os = "macos")]
    let mut c = {
        let mut c = Command::new("/usr/bin/security");
        c.args([
            "find-generic-password",
            "-s",
            "gemini",
            "-a",
            "antigravity",
            "-w",
        ]);
        c
    };
    #[cfg(target_os = "linux")]
    let mut c = {
        let mut c = Command::new("/usr/bin/secret-tool");
        c.args(["lookup", "service", "gemini", "username", "antigravity"]);
        c
    };
    #[cfg(target_os = "windows")]
    let mut c = {
        // Read only the fixed generic credential. No secret enters command arguments.
        let script = r#"
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class TokenStationAgyCredential {
 [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
 public struct Credential {
  public uint Flags; public uint Type; public IntPtr Target; public IntPtr Comment;
  public uint TimeLow; public uint TimeHigh; public uint Size; public IntPtr Blob;
  public uint Persist; public uint Count; public IntPtr Attributes; public IntPtr Alias; public IntPtr User;
 }
 [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
 public static extern bool CredReadW(string target, uint type, uint flags, out IntPtr pointer);
 [DllImport("advapi32.dll")] public static extern void CredFree(IntPtr pointer);
 public static byte[] Read() {
  IntPtr p;
  if (!CredReadW("gemini:antigravity", 1, 0, out p)) throw new Exception("Credential unavailable");
  try {
   Credential c=(Credential)Marshal.PtrToStructure(p, typeof(Credential));
   if (c.Size > 1048576) throw new Exception("Credential too large");
   byte[] b=new byte[c.Size]; Marshal.Copy(c.Blob,b,0,b.Length); return b;
  } finally { CredFree(p); }
 }
}
'@
$b = [TokenStationAgyCredential]::Read()
[Console]::OpenStandardOutput().Write($b, 0, $b.Length)
[Array]::Clear($b, 0, $b.Length)
"#;
        let mut c = Command::new("powershell.exe");
        c.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        return Err(
            "This Antigravity keyring is not supported. Use its local file or IDE login.".into(),
        );
    }
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    {
        let mut child = c
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .spawn()
            .map_err(|_| missing(Kind::Antigravity))?;
        let stdout = child.stdout.take().ok_or_else(failure)?;
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take(LIMIT + 1)
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().map_err(|_| failure())? {
                let bytes = reader
                    .join()
                    .map_err(|_| failure())?
                    .map_err(|_| failure())?;
                if !status.success() || bytes.len() as u64 > LIMIT {
                    return Err(missing(Kind::Antigravity));
                }
                return Ok(bytes);
            }
            if started.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(
                    "Antigravity keyring access timed out. Unlock the keyring and retry.".into(),
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
fn varint(raw: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let b = *raw.get(*at).ok_or_else(failure)?;
        *at += 1;
        if shift == 63 && b > 1 {
            return Err(failure());
        }
        value |= u64::from(b & 127) << shift;
        if b & 128 == 0 {
            return Ok(value);
        }
    }
    Err(failure())
}
fn proto_field(raw: &[u8], target: u64) -> Result<Option<&[u8]>> {
    let mut at = 0;
    let mut found = None;
    while at < raw.len() {
        let key = varint(raw, &mut at)?;
        if key >> 3 == 0 {
            return Err(failure());
        }
        match key & 7 {
            0 => {
                varint(raw, &mut at)?;
            }
            1 => {
                at = at
                    .checked_add(8)
                    .filter(|n| *n <= raw.len())
                    .ok_or_else(failure)?;
            }
            2 => {
                let len = usize::try_from(varint(raw, &mut at)?).map_err(|_| failure())?;
                let end = at
                    .checked_add(len)
                    .filter(|n| *n <= raw.len())
                    .ok_or_else(failure)?;
                if key >> 3 == target {
                    if found.is_some() {
                        return Err(failure());
                    }
                    found = Some(&raw[at..end]);
                }
                at = end;
            }
            5 => {
                at = at
                    .checked_add(4)
                    .filter(|n| *n <= raw.len())
                    .ok_or_else(failure)?;
            }
            _ => return Err(failure()),
        }
    }
    Ok(found)
}
fn proto_varint(raw: &[u8], target: u64) -> Result<Option<u64>> {
    let mut at = 0;
    while at < raw.len() {
        let key = varint(raw, &mut at)?;
        if key & 7 != 0 {
            return Err(failure());
        }
        let v = varint(raw, &mut at)?;
        if key >> 3 == target {
            return Ok(Some(v));
        }
    }
    Ok(None)
}
fn decode_topic(s: &str) -> Result<Vec<u8>> {
    let mut raw = STANDARD.decode(s).map_err(|_| failure())?;
    let data = proto_field(&raw, 1)?.ok_or_else(failure)?;
    if proto_field(data, 1)? != Some(b"oauthTokenInfoSentinelKey".as_slice()) {
        return Err(failure());
    }
    let row = proto_field(data, 2)?.ok_or_else(failure)?;
    let payload = proto_field(row, 1)?.ok_or_else(failure)?;
    let out = STANDARD.decode(payload).map_err(|_| failure());
    raw.fill(0);
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        net::TcpListener,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
    };
    struct Fixture {
        manager: AccountManager,
        dir: PathBuf,
        stop: Arc<AtomicBool>,
        server: Option<thread::JoinHandle<()>>,
        switched: Arc<AtomicBool>,
    }
    impl Fixture {
        fn new(kind: Kind) -> Self {
            let mut bytes = [0u8; 16];
            getrandom::fill(&mut bytes).unwrap();
            let dir = std::env::temp_dir().join(format!(
                "token-station-google-{}",
                hash(&format!("{bytes:?}"))
            ));
            fs::create_dir_all(&dir).unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let stop = Arc::new(AtomicBool::new(false));
            let switched = Arc::new(AtomicBool::new(false));
            let done = stop.clone();
            let sw = switched.clone();
            let server = thread::spawn(move || {
                while !done.load(Ordering::Relaxed) {
                    let Ok((mut socket, _)) = listener.accept() else {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    // macOS accepted sockets inherit the listener nonblocking flag.
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut raw = Vec::new();
                    let mut b = [0u8; 2048];
                    let mut complete = false;
                    loop {
                        let n = socket.read(&mut b).unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        raw.extend_from_slice(&b[..n]);
                        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&raw[..at]);
                            let len = headers
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|s| s.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0);
                            if raw.len() >= at + 4 + len {
                                complete = true;
                                break;
                            }
                        }
                    }
                    if !complete {
                        continue;
                    }
                    let request = String::from_utf8_lossy(&raw);
                    let value = if request.starts_with("GET /user") {
                        json!({"email":if sw.load(Ordering::Relaxed){"other@example.com"}else{"fixture@example.com"}})
                    } else if request.starts_with("POST /token") {
                        json!({"access_token":"refreshed-fixture-access","expires_in":3600})
                    } else if request.contains("loadCodeAssist") {
                        json!({"cloudaicompanionProject":"fixture-project","currentTier":{"id":"free-tier"}})
                    } else if request.contains("fetchAvailableModels") {
                        json!({"models":{"gemini-fixture":{"displayName":"Fixture","maxTokens":128_000,"supportsImages":true},"tab_internal":{"displayName":"Hidden"},"disabled":{"disabled":true}}})
                    } else if request.starts_with("POST /v1internal:retrieveUserQuota ") {
                        json!({"buckets":[{"modelId":"gemini-fixture"},{"modelId":"gemini-fixture"}]})
                    } else {
                        continue;
                    };
                    let body = value.to_string();
                    write!(socket,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
                }
            });
            let mut manager = AccountManager::new(dir.join("state"), kind);
            manager.home = dir.join("home");
            manager.endpoints = Endpoints {
                token: format!("{base}/token"),
                user: format!("{base}/user"),
                prod: base.clone(),
                daily: base,
            };
            Self {
                manager,
                dir,
                stop,
                server: Some(server),
                switched,
            }
        }
        fn login(&self, refresh: &str, expired: bool) {
            let path = self.manager.sources()[0].1.clone();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let v = json!({"access_token":"fixture-access","refresh_token":refresh,"expiry_date":if expired{0}else{(now()+3600)*1000}});
            let value = if self.manager.kind == Kind::Antigravity {
                json!({"token":v})
            } else {
                v
            };
            fs::write(path, value.to_string()).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(server) = self.server.take() {
                server.join().unwrap();
            }
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }
    #[test]
    fn lifecycle_records_no_tokens_and_rejects_replacement() {
        let f = Fixture::new(Kind::GeminiCli);
        assert_eq!(f.manager.discover().unwrap().state, "missing");
        f.login("fixture-refresh", false);
        assert_eq!(f.manager.discover().unwrap().state, "ready");
        let account = f.manager.connect().unwrap();
        assert_eq!(f.manager.models(&account.id).unwrap().len(), 1);
        assert_eq!(
            f.manager.connection(&account.id).unwrap().metadata["google_project"],
            "fixture-project"
        );
        assert_eq!(
            f.manager.access_token(&account.id).unwrap(),
            "fixture-access"
        );
        let record =
            fs::read_to_string(f.manager.root.join(format!("{}.json", account.id))).unwrap();
        assert!(!record.contains("fixture-refresh"));
        assert!(!record.contains("fixture-access"));
        f.login("replacement-refresh", false);
        assert!(f.manager.access_token(&account.id).is_err());
        assert_eq!(f.manager.list().unwrap()[0].state, "account_changed");
        f.manager.disconnect(&account.id).unwrap();
        assert!(f.manager.list().unwrap().is_empty());
        assert!(f.manager.sources()[0].1.exists());
    }
    #[test]
    fn refresh_is_memory_only_and_remote_identity_is_bound() {
        let f = Fixture::new(Kind::Antigravity);
        f.login("fixture-refresh", true);
        let source = f.manager.sources()[0].1.clone();
        let before = fs::read(&source).unwrap();
        let account = f.manager.connect().unwrap();
        assert_eq!(
            f.manager.access_token(&account.id).unwrap(),
            "refreshed-fixture-access"
        );
        assert_eq!(fs::read(source).unwrap(), before);
        assert_eq!(f.manager.models(&account.id).unwrap().len(), 1);
        f.switched.store(true, Ordering::Relaxed);
        f.manager.cache.lock().unwrap().clear();
        assert!(f.manager.access_token(&account.id).is_err());
    }
    fn pb(n: u8, value: &[u8]) -> Vec<u8> {
        assert!(value.len() < 128);
        [
            vec![n << 3 | 2, u8::try_from(value.len()).unwrap()],
            value.to_vec(),
        ]
        .concat()
    }
    #[test]
    fn database_reads_new_and_old_oauth_formats_without_writing() {
        for new in [true, false] {
            let f = Fixture::new(Kind::Antigravity);
            let path = f.manager.sources()[1].1.clone();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            let oauth = [pb(1, b"db-access"), pb(3, b"db-refresh"), pb(4, &[8, 127])].concat();
            let value = if new {
                STANDARD.encode(pb(
                    1,
                    &[
                        pb(1, b"oauthTokenInfoSentinelKey"),
                        pb(2, &pb(1, STANDARD.encode(&oauth).as_bytes())),
                    ]
                    .concat(),
                ))
            } else {
                STANDARD.encode(pb(6, &oauth))
            };
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY,value TEXT)",
                [],
            )
            .unwrap();
            db.execute(
                "INSERT INTO ItemTable VALUES (?1,?2)",
                rusqlite::params![
                    if new {
                        "antigravityUnifiedStateSync.oauthToken"
                    } else {
                        "jetskiStateSync.agentManagerInitState"
                    },
                    value
                ],
            )
            .unwrap();
            drop(db);
            let before = fs::read(&path).unwrap();
            let login = AccountManager::database(&path).unwrap();
            assert_eq!(login.refresh, "db-refresh");
            assert_eq!(login.access, "db-access");
            assert_eq!(login.expiry, 127);
            assert_eq!(fs::read(path).unwrap(), before);
        }
    }
    #[test]
    fn malformed_protobuf_and_references_fail_closed() {
        assert!(proto_field(&[10, 255], 1).is_err());
        assert!(proto_field(&[10, 1, 1, 10, 1, 2], 1).is_err());
        assert!(decode_topic(&STANDARD.encode(pb(1, b"wrong"))).is_err());
        let f = Fixture::new(Kind::GeminiCli);
        assert!(f.manager.access_token("../../other").is_err());
        assert!(
            AccountManager::new(&f.dir, Kind::Copilot)
                .discover()
                .is_err()
        );
    }
    #[test]
    fn cloned_manager_reuses_memory_token_after_source_expiry() {
        let mut fixture = Fixture::new(Kind::GeminiCli);
        fixture.login("fixture-refresh", true);
        let account = fixture.manager.connect().unwrap();
        let clone = fixture.manager.clone();
        fixture.stop.store(true, Ordering::Relaxed);
        fixture.server.take().unwrap().join().unwrap();
        assert_eq!(
            clone.access_token(&account.id).unwrap(),
            "refreshed-fixture-access"
        );
        fixture.login("replacement-refresh", true);
        assert!(clone.access_token(&account.id).is_err());
    }
    #[test]
    fn incomplete_fixture_request_cannot_receive_a_quota_success() {
        let fixture = Fixture::new(Kind::Antigravity);
        let url = url::Url::parse(&fixture.manager.endpoints.prod).unwrap();
        let mut socket = std::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        socket.write_all(b"POST /v1internal:").unwrap();
        let mut reply = Vec::new();
        let _ = socket.read_to_end(&mut reply);
        assert!(
            reply.is_empty(),
            "An incomplete HTTP request received a successful fixture response."
        );
    }
    #[test]
    fn fragmented_fixture_request_preserves_its_code_assist_target() {
        let fixture = Fixture::new(Kind::Antigravity);
        let url = url::Url::parse(&fixture.manager.endpoints.prod).unwrap();
        let mut socket = std::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        socket.write_all(b"POST /v1internal:").unwrap();
        thread::sleep(Duration::from_millis(50));
        socket
            .write_all(b"loadCodeAssist HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
        let mut reply = String::new();
        socket.read_to_string(&mut reply).unwrap();
        let (_, body) = reply.split_once("\r\n\r\n").unwrap();
        let body: Value = serde_json::from_str(body).unwrap();
        assert_eq!(body["cloudaicompanionProject"], "fixture-project");
    }
}
