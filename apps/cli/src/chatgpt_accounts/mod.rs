//! Host-owned Sign in with `ChatGPT` registrations. No Agent credentials are imported.
//! All network operations are synchronous. Async callers must use blocking workers.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use token_station_private_fs::{
    create_private_file, ensure_private_dir, verify_private_dir, verify_private_file,
    verify_single_link_file, write_atomic_private,
};

const ISSUER: &str = "https://auth.openai.com";
pub const API_BASE: &str = "https://api.openai.com/v1";
pub const SLOT: &str = "chatgpt_access_token";
const RESOURCE: &str = API_BASE;
const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const LIMIT: u64 = 1_048_576;
const LOGIN_LIFETIME: Duration = Duration::from_mins(10);

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct AccountView {
    pub id: String,
    pub email: Option<String>,
    pub label: String,
    pub state: String,
    pub expires_at: Option<u64>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct LoginAttempt {
    pub id: String,
    pub authorization_url: String,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct LoginStatus {
    pub id: String,
    pub state: String,
    pub account_id: Option<String>,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PlanModel {
    pub id: String,
    pub name: String,
    pub context_window: Option<u64>,
    pub supports_images: Option<bool>,
}
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SignOutResult {
    pub remote_revoked: bool,
    pub warning: Option<String>,
}

// Deliberately neither Debug nor Serialize on the manager or attempt internals.
#[derive(Clone)]
pub struct AccountManager {
    root: PathBuf,
    endpoints: Endpoints,
}
#[derive(Clone)]
struct Endpoints {
    issuer: String,
    api: String,
}
#[derive(Serialize, Deserialize)]
struct Record {
    id: String,
    client_id: String,
    subject: String,
    email: Option<String>,
    host_id: String,
    generation: u64,
    state: String,
    access: Option<String>,
    refresh: Option<String>,
    id_token: Option<String>,
    scopes: Vec<String>,
    expires_at: Option<u64>,
    #[serde(default)]
    remote_revoked: Option<bool>,
}
impl Drop for Record {
    fn drop(&mut self) {
        erase(&mut self.access);
        erase(&mut self.refresh);
        erase(&mut self.id_token);
    }
}
fn erase(value: &mut Option<String>) {
    if let Some(value) = value.take() {
        drop(south_core::SecretValue::new(value));
    }
}
struct AttemptState {
    status: LoginStatus,
    created: Instant,
}
type Attempt = Arc<Mutex<AttemptState>>;
static ATTEMPTS: OnceLock<Mutex<HashMap<(PathBuf, String), Attempt>>> = OnceLock::new();
fn attempts() -> &'static Mutex<HashMap<(PathBuf, String), Attempt>> {
    ATTEMPTS.get_or_init(Mutex::default)
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn opaque() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "Secure random generation failed.")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn identifier() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Secure random generation failed.")?;
    let mut encoded = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        write!(&mut encoded, "{b:02x}").map_err(local_error)?;
    }
    Ok(encoded)
}
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn local_error(_: impl std::fmt::Display) -> String {
    "Cannot access protected ChatGPT account state.".into()
}

impl AccountManager {
    #[must_use]
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            root: data_dir.as_ref().join("chatgpt-accounts"),
            endpoints: Endpoints {
                issuer: ISSUER.into(),
                api: RESOURCE.into(),
            },
        }
    }
    fn init(&self) -> Result<()> {
        ensure_private_dir(&self.root).map_err(local_error)
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err("Invalid account identifier.".into());
        }
        Ok(self.root.join(format!("{id}.json")))
    }
    fn lock(&self, id: &str) -> Result<File> {
        self.path(id)?;
        self.init()?;
        let p = self.root.join(format!("{id}.lock"));
        create_private_file(&p, b"").map_err(local_error)?;
        verify_single_link_file(&p).map_err(local_error)?;
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&p)
            .map_err(local_error)?;
        f.lock().map_err(local_error)?;
        Ok(f)
    }
    fn load(&self, id: &str) -> Result<Record> {
        let p = self.path(id)?;
        verify_private_dir(&self.root).map_err(local_error)?;
        verify_private_file(&p).map_err(local_error)?;
        let f = File::open(p).map_err(local_error)?;
        let mut bytes = Vec::new();
        f.take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(local_error)?;
        if bytes.len() as u64 > LIMIT {
            return Err("ChatGPT account record exceeds its size limit.".into());
        }
        let r: Record = serde_json::from_slice(&bytes).map_err(local_error)?;
        if r.id != id {
            return Err("ChatGPT account identity is inconsistent.".into());
        }
        Ok(r)
    }
    fn save(&self, r: &Record) -> Result<()> {
        self.init()?;
        let b = serde_json::to_vec(r).map_err(local_error)?;
        write_atomic_private(&self.path(&r.id)?, &b).map_err(local_error)
    }
    /// Lists non-secret account metadata. Does not refresh accounts.
    /// # Errors
    /// The protected account records cannot be read.
    pub fn list(&self) -> Result<Vec<AccountView>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for item in fs::read_dir(&self.root).map_err(local_error)? {
            let p = item.map_err(local_error)?.path();
            if p.extension().and_then(|v| v.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = p
                .file_stem()
                .and_then(|v| v.to_str())
                .filter(|v| valid_id(v))
            else {
                continue;
            };
            let r = self.load(id)?;
            out.push(AccountView {
                label: r
                    .email
                    .clone()
                    .unwrap_or_else(|| format!("ChatGPT {}", &r.id[..8])),
                id: r.id.clone(),
                email: r.email.clone(),
                state: r.state.clone(),
                expires_at: r.expires_at,
            });
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }
    fn host_id(&self) -> Result<String> {
        self.init()?;
        let p = self.root.join("host-id");
        if !p.exists() {
            let raw = identifier()?;
            let h = format!(
                "urn:uuid:{}-{}-4{}-8{}-{}",
                &raw[..8],
                &raw[8..12],
                &raw[13..16],
                &raw[17..20],
                &raw[20..]
            );
            create_private_file(&p, h.as_bytes()).map_err(local_error)?;
        }
        verify_private_file(&p).map_err(local_error)?;
        let h = fs::read_to_string(p).map_err(local_error)?;
        if h.len() != 45 || !h.starts_with("urn:uuid:") {
            return Err("ChatGPT host identifier is invalid.".into());
        }
        Ok(h)
    }
    /// Starts a new registration, or reauthorizes the specified registration.
    /// # Errors
    /// The listener, registration, or private state cannot be prepared.
    pub fn start_login(&self, account_id: Option<&str>) -> Result<LoginAttempt> {
        let previous = account_id.map(|id| self.load(id)).transpose()?;
        let host_id = match &previous {
            Some(r) => r.host_id.clone(),
            None => self.host_id()?,
        };
        let listener = TcpListener::bind("127.0.0.1:0")
            .map_err(|_| "Cannot open the ChatGPT sign-in callback.")?;
        listener.set_nonblocking(true).map_err(local_error)?;
        let redirect = format!(
            "http://127.0.0.1:{}/auth/callback",
            listener.local_addr().map_err(local_error)?.port()
        );
        let id = identifier()?;
        let state = opaque()?;
        let nonce = opaque()?;
        let verifier = opaque()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut auth =
            url::Url::parse(&format!("{}/api/accounts/authorize", self.endpoints.issuer))
                .map_err(local_error)?;
        {
            let mut q = auth.query_pairs_mut();
            q.append_pair(
                "client_id",
                previous
                    .as_ref()
                    .map_or("dynamic_agent_client", |r| r.client_id.as_str()),
            );
            if previous.is_none() {
                q.append_pair("agent_name_hint", "Token Station");
            }
            q.append_pair("ext_agent_host_id", &host_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", &redirect)
                .append_pair("resource", RESOURCE)
                .append_pair("scope", SCOPES)
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("code_challenge", &challenge)
                .append_pair("code_challenge_method", "S256");
        }
        let attempt = Arc::new(Mutex::new(AttemptState {
            status: LoginStatus {
                id: id.clone(),
                state: "pending".into(),
                account_id: None,
                error: None,
            },
            created: Instant::now(),
        }));
        {
            let mut map = attempts().lock().map_err(local_error)?;
            map.retain(|_, a| {
                a.lock()
                    .is_ok_and(|s| s.created.elapsed() < Duration::from_hours(1))
            });
            if map.len() >= 64 {
                return Err("Too many ChatGPT sign-in attempts. Try again later.".into());
            }
            map.insert((self.root.clone(), id.clone()), Arc::clone(&attempt));
        }
        let manager = self.clone();
        std::thread::Builder::new()
            .name("chatgpt-sign-in".into())
            .spawn(move || {
                manager.callback(
                    listener,
                    attempt,
                    LoginInput {
                        previous,
                        host_id,
                        redirect,
                        state,
                        nonce,
                        verifier,
                    },
                );
            })
            .map_err(|_| "Cannot start the ChatGPT sign-in listener.")?;
        Ok(LoginAttempt {
            id,
            authorization_url: auth.into(),
        })
    }
    fn attempt(&self, id: &str) -> Result<Attempt> {
        attempts()
            .lock()
            .map_err(local_error)?
            .get(&(self.root.clone(), id.to_owned()))
            .cloned()
            .ok_or_else(|| "Unknown ChatGPT sign-in attempt.".into())
    }
    /// # Errors
    /// The attempt is unknown or its state cannot be read.
    pub fn login_status(&self, id: &str) -> Result<LoginStatus> {
        Ok(self
            .attempt(id)?
            .lock()
            .map_err(local_error)?
            .status
            .clone())
    }
    /// # Errors
    /// The attempt is unknown or its state cannot be updated.
    pub fn cancel_login(&self, id: &str) -> Result<()> {
        let a = self.attempt(id)?;
        let mut s = a.lock().map_err(local_error)?;
        if s.status.state == "pending" {
            s.status.state = "cancelled".into();
        }
        Ok(())
    }
    /// Returns a short-lived bearer token to the Host only. Never serialize this value.
    /// # Errors
    /// The grant is unavailable, lacks inference scope, or cannot be refreshed.
    pub fn access_token(&self, id: &str) -> Result<String> {
        let _lock = self.lock(id)?;
        let mut r = self.load(id)?;
        if r.state == "signed_out" || r.state == "reauthentication_required" {
            return Err("Sign in to this ChatGPT account again.".into());
        }
        if !r.scopes.iter().any(|s| s == DIRECT_SCOPE) {
            return Err("This ChatGPT sign-in does not permit plan inference.".into());
        }
        if r.expires_at.is_some_and(|e| e > now() + 180) {
            return r
                .access
                .take()
                .ok_or_else(|| "ChatGPT access token is missing.".into());
        }
        let refresh = r
            .refresh
            .as_ref()
            .ok_or("Sign in to this ChatGPT account again.")?;
        let response = self.token(&[
            ("grant_type", "refresh_token"),
            ("client_id", &r.client_id),
            ("refresh_token", refresh),
            ("resource", RESOURCE),
        ]);
        let tok = match response {
            Ok(t) => t,
            Err(e) => {
                if e.permanent {
                    r.state = "reauthentication_required".into();
                    erase(&mut r.access);
                    erase(&mut r.refresh);
                    erase(&mut r.id_token);
                    r.expires_at = None;
                    r.generation += 1;
                    self.save(&r)?;
                }
                return Err(e.message);
            }
        };
        if let Some(id_token) = &tok.id_token {
            let jwks = self.jwks()?;
            validate_id(
                id_token,
                &jwks,
                &self.endpoints.issuer,
                &r.client_id,
                None,
                Some(&r.subject),
            )?;
        }
        apply_tokens(&mut r, tok)?;
        r.generation += 1;
        self.save(&r)?;
        if r.state != "ready" {
            return Err("This ChatGPT sign-in does not permit plan inference.".into());
        }
        r.access
            .take()
            .ok_or_else(|| "ChatGPT access token is missing.".into())
    }
    /// Reads only this registration's current model catalog.
    /// # Errors
    /// The account or its model catalog is unavailable.
    pub fn models(&self, id: &str) -> Result<Vec<PlanModel>> {
        let access = self.access_token(id)?;
        let value = self.get(&format!("{}/models", self.endpoints.api), Some(&access))?;
        let models = value
            .get("models")
            .and_then(Value::as_array)
            .ok_or("ChatGPT returned an invalid model catalog.")?;
        let mut out = Vec::new();
        for m in models {
            if m.get("visibility").and_then(Value::as_str) != Some("list") {
                continue;
            }
            let Some(slug) = m
                .get("slug")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            out.push(PlanModel {
                id: slug.into(),
                name: m
                    .get("display_name")
                    .and_then(Value::as_str)
                    .unwrap_or(slug)
                    .into(),
                context_window: m.get("context_window").and_then(Value::as_u64),
                supports_images: m
                    .get("input_modalities")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().any(|v| v.as_str() == Some("image"))),
            });
        }
        Ok(out)
    }
    /// Clears the local session even when remote revocation fails.
    /// # Errors
    /// The local protected record cannot be cleared.
    pub fn sign_out(&self, id: &str) -> Result<SignOutResult> {
        let _lock = self.lock(id)?;
        let mut r = self.load(id)?;
        let refresh = r.refresh.take();
        if refresh.is_some() {
            r.remote_revoked = Some(false);
        }
        erase(&mut r.access);
        erase(&mut r.id_token);
        r.expires_at = None;
        r.scopes.clear();
        r.state = "signed_out".into();
        r.generation += 1;
        self.save(&r)?;
        let remote_revoked = match refresh {
            None => r.remote_revoked.unwrap_or(true),
            Some(token) => {
                let result = self.revoke(&r.client_id, &token).is_ok();
                drop(south_core::SecretValue::new(token));
                result
            }
        };
        r.remote_revoked = Some(remote_revoked);
        self.save(&r)?;
        Ok(SignOutResult{remote_revoked,warning:(!remote_revoked).then(||"Local sign-out completed. Remote revocation is unconfirmed. Remove the connection in ChatGPT settings.".into())})
    }
}

struct LoginInput {
    previous: Option<Record>,
    host_id: String,
    redirect: String,
    state: String,
    nonce: String,
    verifier: String,
}
impl Drop for LoginInput {
    fn drop(&mut self) {
        drop(south_core::SecretValue::new(std::mem::take(
            &mut self.verifier,
        )));
        drop(south_core::SecretValue::new(std::mem::take(
            &mut self.nonce,
        )));
        drop(south_core::SecretValue::new(std::mem::take(
            &mut self.state,
        )));
    }
}
impl AccountManager {
    #[allow(
        clippy::needless_pass_by_value,
        reason = "The worker owns callback resources until completion."
    )]
    fn callback(&self, listener: TcpListener, attempt: Attempt, input: LoginInput) {
        let started = Instant::now();
        while started.elapsed() < LOGIN_LIFETIME {
            if !attempt.lock().is_ok_and(|s| s.status.state == "pending") {
                return;
            }
            let (mut stream, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(40));
                    continue;
                }
                Err(_) => break,
            };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
            let mut b = vec![0u8; 8192];
            let mut n = 0;
            while n < b.len() {
                match stream.read(&mut b[n..]) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => {
                        n += k;
                        if b[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                }
            }
            let raw = String::from_utf8_lossy(&b[..n]);
            let mut line = raw.lines().next().unwrap_or_default().split_whitespace();
            let method = line.next();
            let target = line.next().unwrap_or_default();
            let parsed = url::Url::parse(&format!("http://localhost{target}"));
            let params = parsed
                .as_ref()
                .ok()
                .map(|u| u.query_pairs().into_owned().collect::<HashMap<_, _>>())
                .unwrap_or_default();
            let valid = method == Some("GET")
                && parsed.as_ref().is_ok_and(|u| u.path() == "/auth/callback")
                && params.get("state") == Some(&input.state);
            if !valid {
                reply(&mut stream, false);
                continue;
            }
            let result = if params.contains_key("error") {
                Err("ChatGPT sign-in was declined.".into())
            } else {
                params
                    .get("code")
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| "ChatGPT returned no authorization code.".into())
                    .and_then(|code| {
                        self.exchange(
                            &input,
                            code,
                            params.get("client_id").map(String::as_str),
                            &attempt,
                        )
                    })
            };
            let success = result.is_ok();
            if let Ok(mut s) = attempt.lock()
                && s.status.state == "pending"
            {
                match result {
                    Ok(id) => {
                        s.status.state = "complete".into();
                        s.status.account_id = Some(id);
                    }
                    Err(e) => {
                        s.status.state = "failed".into();
                        s.status.error = Some(e);
                    }
                }
            }
            reply(&mut stream, success);
            return;
        }
        if let Ok(mut s) = attempt.lock()
            && s.status.state == "pending"
        {
            s.status.state = "failed".into();
            s.status.error = Some("ChatGPT sign-in expired. Start again.".into());
        }
    }
    fn exchange(
        &self,
        input: &LoginInput,
        code: &str,
        callback_client: Option<&str>,
        attempt: &Attempt,
    ) -> Result<String> {
        let client = match &input.previous {
            Some(r) => {
                if callback_client.is_some_and(|id| id != r.client_id) {
                    return Err("ChatGPT returned another registration identity.".into());
                }
                r.client_id.as_str()
            }
            None => callback_client
                .filter(|id| !id.is_empty() && *id != "dynamic_agent_client")
                .ok_or("ChatGPT did not issue an application registration.")?,
        };
        let tok = self
            .token(&[
                ("grant_type", "authorization_code"),
                ("client_id", client),
                ("code", code),
                ("code_verifier", &input.verifier),
                ("redirect_uri", &input.redirect),
                ("resource", RESOURCE),
            ])
            .map_err(|e| e.message)?;
        let token = tok
            .id_token
            .as_ref()
            .ok_or("ChatGPT returned no identity token.")?;
        let jwks = self.jwks()?;
        let claims = validate_id(
            token,
            &jwks,
            &self.endpoints.issuer,
            client,
            Some(&input.nonce),
            input.previous.as_ref().map(|r| r.subject.as_str()),
        )?;
        let id = input
            .previous
            .as_ref()
            .map_or(identifier()?, |r| r.id.clone());
        let _lock = self.lock(&id)?;
        if let Some(old) = &input.previous {
            let current = self.load(&id)?;
            if current.generation != old.generation {
                return Err("The account changed during sign-in. Start again.".into());
            }
        }
        let mut state = attempt.lock().map_err(local_error)?;
        if state.status.state != "pending" {
            return Err("ChatGPT sign-in was cancelled.".into());
        }
        let mut r = Record {
            id: id.clone(),
            client_id: client.into(),
            subject: claims.sub,
            email: claims.email,
            host_id: input.host_id.clone(),
            generation: input.previous.as_ref().map_or(0, |r| r.generation + 1),
            state: "identity_only".into(),
            access: None,
            refresh: None,
            id_token: None,
            scopes: vec![],
            expires_at: None,
            remote_revoked: None,
        };
        apply_tokens(&mut r, tok)?;
        self.save(&r)?;
        // Publish completion under the same attempt lock as the credential commit.
        state.status.state = "complete".into();
        state.status.account_id = Some(id.clone());
        Ok(id)
    }
    fn http() -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .http_status_as_error(false)
            .build()
            .into()
    }
    fn allowed_url(&self, value: &str) -> Result<()> {
        let u = url::Url::parse(value).map_err(|_| "Invalid ChatGPT endpoint.")?;
        let trusted = [
            url::Url::parse(&self.endpoints.issuer).map_err(local_error)?,
            url::Url::parse(&self.endpoints.api).map_err(local_error)?,
        ];
        if !trusted.iter().any(|t| t.origin() == u.origin())
            || !u.username().is_empty()
            || u.password().is_some()
            || u.fragment().is_some()
        {
            return Err("Untrusted ChatGPT endpoint.".into());
        }
        if u.scheme() != "https" {
            #[cfg(test)]
            {
                if u.scheme() == "http" && u.host_str() == Some("127.0.0.1") {
                    return Ok(());
                }
            }
            return Err("ChatGPT endpoints require HTTPS.".into());
        }
        Ok(())
    }
    fn get(&self, url: &str, bearer: Option<&str>) -> Result<Value> {
        self.allowed_url(url)?;
        let http = Self::http();
        let mut req = http.get(url).header("Accept", "application/json");
        if let Some(token) = bearer {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
        let response = req
            .call()
            .map_err(|_| "ChatGPT connection failed. Try again.")?;
        read_json(response)
    }
    fn discovery(&self) -> Result<Value> {
        let d = self.get(
            &format!("{}/.well-known/openid-configuration", self.endpoints.issuer),
            None,
        )?;
        if d.get("issuer").and_then(Value::as_str) != Some(self.endpoints.issuer.as_str()) {
            return Err("ChatGPT discovery has another issuer.".into());
        }
        Ok(d)
    }
    fn jwks(&self) -> Result<JwkSet> {
        let d = self.discovery()?;
        let u = d
            .get("jwks_uri")
            .and_then(Value::as_str)
            .ok_or("ChatGPT discovery has no signing keys.")?;
        serde_json::from_value(self.get(u, None)?)
            .map_err(|_| "ChatGPT returned invalid signing keys.".into())
    }
    fn token(&self, form: &[(&str, &str)]) -> std::result::Result<TokenReply, TokenFailure> {
        let u = format!("{}/api/accounts/oauth/token", self.endpoints.issuer);
        self.allowed_url(&u).map_err(TokenFailure::temporary)?;
        let response = Self::http()
            .post(&u)
            .send_form(form.iter().copied())
            .map_err(|_| TokenFailure::temporary("ChatGPT token request failed. Try again."))?;
        let status = response.status().as_u16();
        let body = read_body(response).map_err(TokenFailure::temporary)?;
        if status != 200 {
            let value: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            let code = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let permanent = matches!(
                code,
                "invalid_grant"
                    | "invalid_client"
                    | "invalid_refresh_token"
                    | "refresh_token_expired"
                    | "refresh_token_reused"
                    | "refresh_token_invalidated"
            );
            return Err(TokenFailure {
                permanent,
                message: if permanent {
                    "Sign in to this ChatGPT account again."
                } else {
                    "ChatGPT did not complete the token request. Try again."
                }
                .into(),
            });
        }
        serde_json::from_slice(&body)
            .map_err(|_| TokenFailure::temporary("ChatGPT returned an invalid token response."))
    }
    fn revoke(&self, client: &str, refresh: &str) -> Result<()> {
        let d = self.discovery()?;
        let u = d
            .get("revocation_endpoint")
            .and_then(Value::as_str)
            .ok_or("ChatGPT discovery has no revocation endpoint.")?;
        self.allowed_url(u)?;
        let response = Self::http()
            .post(u)
            .send_form([
                ("token", refresh),
                ("token_type_hint", "refresh_token"),
                ("client_id", client),
            ])
            .map_err(|_| "ChatGPT revocation failed.")?;
        if response.status().as_u16() != 200 {
            return Err("ChatGPT revocation was not confirmed.".into());
        }
        Ok(())
    }
}
fn reply(stream: &mut std::net::TcpStream, success: bool) {
    let body = if success {
        "ChatGPT sign-in completed. Return to Token Station."
    } else {
        "ChatGPT sign-in did not complete. Return to Token Station."
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
}
fn read_body(mut response: ureq::http::Response<ureq::Body>) -> Result<Vec<u8>> {
    let mut b = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(LIMIT + 1)
        .read_to_end(&mut b)
        .map_err(|_| "ChatGPT response could not be read.")?;
    if b.len() as u64 > LIMIT {
        return Err("ChatGPT response exceeds its size limit.".into());
    }
    Ok(b)
}
fn read_json(response: ureq::http::Response<ureq::Body>) -> Result<Value> {
    if response.status().as_u16() != 200 {
        return Err("ChatGPT request was not accepted. Check the account and try again.".into());
    }
    serde_json::from_slice(&read_body(response)?)
        .map_err(|_| "ChatGPT returned an invalid response.".into())
}
#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    expires_in: u64,
    #[serde(default)]
    scope: Option<String>,
    token_type: String,
}
impl Drop for TokenReply {
    fn drop(&mut self) {
        drop(south_core::SecretValue::new(std::mem::take(
            &mut self.access_token,
        )));
        erase(&mut self.refresh_token);
        erase(&mut self.id_token);
    }
}
struct TokenFailure {
    message: String,
    permanent: bool,
}
impl TokenFailure {
    fn temporary(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            permanent: false,
        }
    }
}
fn apply_tokens(r: &mut Record, mut t: TokenReply) -> Result<()> {
    if t.access_token.is_empty()
        || !t.token_type.eq_ignore_ascii_case("bearer")
        || t.expires_in == 0
        || t.expires_in > 31_536_000
    {
        return Err("ChatGPT returned an invalid token response.".into());
    }
    erase(&mut r.access);
    r.access = Some(std::mem::take(&mut t.access_token));
    if let Some(refresh) = t.refresh_token.take().filter(|v| !v.is_empty()) {
        erase(&mut r.refresh);
        r.refresh = Some(refresh);
    }
    if let Some(scope) = t.scope.take() {
        r.scopes = scope.split_whitespace().map(str::to_owned).collect();
    }
    if let Some(token) = t.id_token.take() {
        erase(&mut r.id_token);
        r.id_token = Some(token);
    }
    r.remote_revoked = None;
    r.expires_at = Some(now() + t.expires_in);
    r.state = if r.scopes.iter().any(|s| s == DIRECT_SCOPE) {
        "ready"
    } else {
        "identity_only"
    }
    .into();
    Ok(())
}
#[derive(Deserialize)]
struct Identity {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    azp: Option<String>,
    aud: Value,
    iat: u64,
}
fn validate_id(
    token: &str,
    jwks: &JwkSet,
    issuer: &str,
    client: &str,
    nonce: Option<&str>,
    subject: Option<&str>,
) -> Result<Identity> {
    let bad = || "ChatGPT identity token validation failed.".to_string();
    let header = decode_header(token).map_err(|_| bad())?;
    if header.alg != Algorithm::RS256 {
        return Err(bad());
    }
    let kid = header.kid.as_ref().ok_or_else(bad)?;
    let key = jwks.find(kid).ok_or_else(bad)?;
    let decoding = DecodingKey::from_jwk(key).map_err(|_| bad())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[client]);
    validation.set_issuer(&[issuer]);
    validation.set_required_spec_claims(&["iss", "sub", "aud", "exp", "iat"]);
    validation.leeway = 30;
    validation.validate_nbf = true;
    let id = decode::<Identity>(token, &decoding, &validation)
        .map_err(|_| bad())?
        .claims;
    if id.sub.is_empty()
        || id.iat > now() + 30
        || nonce.is_some_and(|n| id.nonce.as_deref() != Some(n))
        || subject.is_some_and(|s| s != id.sub)
        || id.azp.as_deref().is_some_and(|s| s != client)
        || id
            .aud
            .as_array()
            .is_some_and(|a| a.len() > 1 && id.azp.as_deref() != Some(client))
    {
        return Err(bad());
    }
    Ok(id)
}
#[cfg(test)]
mod tests;
