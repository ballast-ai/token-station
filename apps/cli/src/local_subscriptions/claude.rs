//! Local Claude Code subscription references and host-owned process transport.
//! No login token is read, copied, or returned. Claude Code owns authentication.
use super::{AccountView, Connection, Model};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use token_station_private_fs::{
    create_private_file, ensure_private_dir, verify_private_dir, verify_private_file,
    verify_single_link_file, write_atomic_private,
};
use token_station_protocol::{
    ErrorCode, ErrorEnvelope, HttpMethod, HttpRequestDescriptor, HttpResponseParts,
};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, String>;
pub const API_BASE: &str = "http://127.0.0.1:9/claude-plan";
const LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct AccountManager {
    root: PathBuf,
    source: PathBuf,
    executable: Option<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Reference {
    id: String,
    source: PathBuf,
    executable: PathBuf,
    subject: String,
    email: Option<String>,
}
#[derive(Debug)]
struct Identity {
    subject: String,
    email: Option<String>,
}

fn local_error(_: impl std::fmt::Display) -> String {
    "Cannot access the local Claude subscription reference.".into()
}
fn error(code: ErrorCode, status: u16, message: &str) -> ErrorEnvelope {
    ErrorEnvelope::new(code, status, message)
}
fn capability(message: &str) -> ErrorEnvelope {
    error(ErrorCode::Capability, 400, message)
}
fn nonce() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(local_error)?;
    Ok(hex(&bytes))
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut output, byte| {
        let _ = write!(&mut output, "{byte:02x}");
        output
    })
}
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())
}
fn executable() -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path).find_map(|dir| {
            let file = dir.join(if cfg!(windows) {
                "claude.exe"
            } else {
                "claude"
            });
            if !file.is_file() {
                return None;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if fs::metadata(&file).ok()?.permissions().mode() & 0o111 == 0 {
                    return None;
                }
            }
            fs::canonicalize(file).ok()
        })
    })
}
fn identity(value: &Value) -> Result<Identity> {
    if value["loggedIn"] != true {
        return Err("missing".into());
    }
    if !matches!(
        value["authMethod"].as_str(),
        Some("claude.ai" | "oauth" | "claudeai")
    ) {
        return Err("api_key_only".into());
    }
    let email = value["email"]
        .as_str()
        .filter(|v| !v.is_empty() && v.len() <= 320 && !v.chars().any(char::is_control))
        .map(str::to_owned);
    let account = value["accountId"]
        .as_str()
        .or_else(|| value["userId"].as_str())
        .or(email.as_deref())
        .ok_or("invalid_source")?;
    let organization = value["orgId"]
        .as_str()
        .or_else(|| value["organizationId"].as_str())
        .unwrap_or("");
    let subject = format!(
        "{:x}",
        Sha256::digest(json!([account, organization]).to_string())
    );
    Ok(Identity { subject, email })
}

struct WorkDir(PathBuf);
impl WorkDir {
    fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("token-station-claude-{}", nonce()?));
        ensure_private_dir(&path).map_err(local_error)?;
        Ok(Self(path))
    }
}
impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Remove endpoint, API-key, and provider overrides before invoking local OAuth.
fn command(binary: &Path, source: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(binary);
    remove_environment_overrides(&mut command, std::env::vars_os().map(|(name, _)| name));
    command
        .env("CLAUDE_CONFIG_DIR", source)
        .env("DISABLE_AUTOUPDATER", "1")
        .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}
/// Direct process transport ignores ambient proxy and provider overrides.
fn remove_environment_overrides(
    command: &mut Command,
    environment_names: impl IntoIterator<Item = std::ffi::OsString>,
) {
    for name in environment_names {
        let key = name.to_string_lossy();
        if ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY"]
            .iter()
            .any(|proxy| key.eq_ignore_ascii_case(proxy))
            || key.starts_with("ANTHROPIC_")
            || key.starts_with("CLAUDE_CODE_")
            || key.starts_with("CLAUDE_AGENT_")
            || matches!(
                key.as_ref(),
                "CLAUDE_CONFIG_DIR" | "CLAUDE_SECURESTORAGE_CONFIG_DIR" | "IS_SANDBOX"
            )
        {
            command.env_remove(name);
        }
    }
}

fn safety_args(command: &mut Command) {
    command.args([
        "-p",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--settings",
        r#"{"disableAllHooks":true,"enabledPlugins":{},"alwaysThinkingEnabled":false}"#,
    ]);
}
fn output_channel(
    child: &mut Child,
) -> Result<mpsc::Receiver<std::result::Result<String, String>>> {
    let stdout = child.stdout.take().ok_or_else(|| local_error("stdout"))?;
    let (send, receive) = mpsc::sync_channel(64);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout.take(LIMIT + 1));
        let mut total = 0usize;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(count) => {
                    total += count;
                    if total as u64 > LIMIT {
                        let _ =
                            send.send(Err("Claude Code output exceeded the host limit.".into()));
                        break;
                    }
                    if send.send(Ok(line)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = send.send(Err("Cannot read the Claude Code output.".into()));
                    break;
                }
            }
        }
    });
    Ok(receive)
}
fn write_json(child: &mut Child, value: &Value) -> Result<()> {
    let stdin = child.stdin.as_mut().ok_or_else(|| local_error("stdin"))?;
    serde_json::to_writer(&mut *stdin, value).map_err(local_error)?;
    stdin.write_all(b"\n").map_err(local_error)?;
    stdin.flush().map_err(local_error)
}
fn parse_models(value: &Value) -> Result<Vec<Model>> {
    let entries = value["models"]
        .as_array()
        .ok_or("Claude Code did not report its available models.")?;
    let mut models = BTreeMap::new();
    for entry in entries {
        let Some(id) = entry["value"].as_str().or_else(|| entry["id"].as_str()) else {
            continue;
        };
        if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
            continue;
        }
        models.insert(
            id.to_owned(),
            Model {
                id: id.to_owned(),
                name: entry["displayName"]
                    .as_str()
                    .or_else(|| entry["name"].as_str())
                    .unwrap_or(id)
                    .to_owned(),
                context_window: None,
                supports_images: Some(false),
                supports_tools: Some(true),
            },
        );
    }
    if models.is_empty() {
        return Err("Claude Code reported no available subscription models.".into());
    }
    Ok(models.into_values().collect())
}

impl AccountManager {
    /// Creates a reference manager without reading local login credentials.
    #[must_use]
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        let source = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|p| PathBuf::from(p).join(".claude"))
            })
            .unwrap_or_default();
        Self {
            root: data_dir.as_ref().join("claude-subscription-references"),
            source,
            executable: executable(),
        }
    }
    fn empty(state: &str) -> AccountView {
        AccountView {
            id: String::new(),
            label: "Local Claude Code account".into(),
            email: None,
            state: state.into(),
        }
    }
    fn status(&self) -> Result<Reference> {
        self.status_with(Instant::now() + Duration::from_secs(15), None)
    }
    fn status_with(
        &self,
        deadline: Instant,
        cancellation: Option<&CancellationToken>,
    ) -> Result<Reference> {
        let binary = self.executable.as_ref().ok_or("missing_cli")?;
        let source = fs::canonicalize(&self.source).map_err(|_| "missing".to_owned())?;
        let work = WorkDir::new()?;
        let mut command = command(binary, &source, &work.0);
        command.args(["auth", "status", "--json"]);
        let mut process = Process(command.spawn().map_err(|_| "missing_cli".to_owned())?);
        let output = output_channel(&mut process.0)?;
        drop(process.0.stdin.take());
        let deadline = deadline.min(Instant::now() + Duration::from_secs(15));
        let mut buffer = String::new();
        loop {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return Err("cancelled".into());
            }
            if Instant::now() >= deadline {
                return Err("timeout".into());
            }
            match output.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(line)) => buffer.push_str(&line),
                Ok(Err(_)) => return Err("invalid_source".into()),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        let safe_status: Value =
            serde_json::from_str(&buffer).map_err(|_| "invalid_source".to_owned())?;
        let identity = identity(&safe_status)?;
        let digest = Sha256::digest(json!([source, binary, identity.subject]).to_string());
        let id = hex(&digest[..16]);
        Ok(Reference {
            id,
            source,
            executable: binary.clone(),
            subject: identity.subject,
            email: identity.email,
        })
    }
    fn view(reference: &Reference, state: &str) -> AccountView {
        AccountView {
            id: reference.id.clone(),
            label: reference
                .email
                .clone()
                .unwrap_or_else(|| "Local Claude Code account".into()),
            email: reference.email.clone(),
            state: state.to_owned(),
        }
    }
    #[allow(clippy::unnecessary_wraps)] // Shared channel facade requires this result contract.
    /// Reports safe identity metadata from Claude Code.
    /// # Errors
    /// Returns an error if the source, reference, or requested capability is unavailable.
    pub fn discover(&self) -> Result<AccountView> {
        Ok(match self.status() {
            Ok(r) => Self::view(&r, "ready"),
            Err(state) => Self::empty(
                if [
                    "missing_cli",
                    "missing",
                    "api_key_only",
                    "invalid_source",
                    "timeout",
                ]
                .contains(&state.as_str())
                {
                    &state
                } else {
                    "invalid_source"
                },
            ),
        })
    }
    fn path(&self, id: &str) -> Result<PathBuf> {
        if !valid_id(id) {
            return Err("Invalid Claude subscription reference.".into());
        }
        Ok(self.root.join(format!("{id}.json")))
    }
    fn lock(&self) -> Result<File> {
        ensure_private_dir(&self.root).map_err(local_error)?;
        let path = self.root.join("references.lock");
        create_private_file(&path, b"").map_err(local_error)?;
        verify_private_file(&path).map_err(local_error)?;
        verify_single_link_file(&path).map_err(local_error)?;
        let file = File::options()
            .read(true)
            .write(true)
            .open(path)
            .map_err(local_error)?;
        file.lock().map_err(local_error)?;
        Ok(file)
    }
    fn load(&self, id: &str) -> Result<Reference> {
        let path = self.path(id)?;
        verify_private_dir(&self.root).map_err(local_error)?;
        verify_private_file(&path).map_err(local_error)?;
        verify_single_link_file(&path).map_err(local_error)?;
        let mut bytes = Vec::new();
        File::open(path)
            .map_err(local_error)?
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(local_error)?;
        if bytes.len() > 16 * 1024 {
            return Err(local_error("size"));
        }
        let reference: Reference = serde_json::from_slice(&bytes).map_err(local_error)?;
        if reference.id != id {
            return Err(local_error("binding"));
        }
        Ok(reference)
    }
    fn bound(&self, id: &str) -> Result<Reference> {
        self.bound_with(id, Instant::now() + Duration::from_secs(15), None)
    }
    fn bound_with(
        &self,
        id: &str,
        deadline: Instant,
        cancellation: Option<&CancellationToken>,
    ) -> Result<Reference> {
        let reference = self.load(id)?;
        let current = self.status_with(deadline, cancellation)?;
        if current.id != reference.id
            || current.subject != reference.subject
            || current.source != reference.source
            || current.executable != reference.executable
        {
            return Err("The local Claude account changed. Reconnect the intended account.".into());
        }
        Ok(current)
    }
    /// Lists stored references and checks current login identity.
    /// # Errors
    /// Returns an error if the source, reference, or requested capability is unavailable.
    pub fn list(&self) -> Result<Vec<AccountView>> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        verify_private_dir(&self.root).map_err(local_error)?;
        let current = self.status();
        let mut accounts = vec![];
        for entry in fs::read_dir(&self.root).map_err(local_error)? {
            let entry = entry.map_err(local_error)?;
            let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_owned)
            else {
                continue;
            };
            if !valid_id(&id) {
                continue;
            }
            let reference = self.load(&id)?;
            let state = match &current {
                Ok(r) if r.id == id => "ready",
                Ok(_) => "account_changed",
                Err(_) => "missing",
            };
            accounts.push(Self::view(&reference, state));
        }
        accounts.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(accounts)
    }
    /// Connects the current subscription after native model discovery.
    /// # Errors
    /// Returns an error if the source, reference, or requested capability is unavailable.
    pub fn connect(&self) -> Result<AccountView> {
        let reference = self
            .status()
            .map_err(|_| "Open Claude Code and sign in with a Claude subscription.".to_owned())?;
        Self::probe_models(&reference)?;
        let _lock = self.lock()?;
        write_atomic_private(
            &self.path(&reference.id)?,
            &serde_json::to_vec(&reference).map_err(local_error)?,
        )
        .map_err(local_error)?;
        Ok(Self::view(&reference, "ready"))
    }
    /// Reads available model metadata from the bound local login.
    /// # Errors
    /// Returns an error if the source, reference, or requested capability is unavailable.
    pub fn models(&self, id: &str) -> Result<Vec<Model>> {
        Self::probe_models(&self.bound(id)?)
    }
    /// Returns non-secret connection metadata for the bound login.
    /// # Errors
    /// Returns an error if the source, reference, or requested capability is unavailable.
    pub fn connection(&self, id: &str) -> Result<Connection> {
        let reference = self.bound(id)?;
        Ok(Connection {
            account: Self::view(&reference, "ready"),
            base_url: API_BASE.into(),
            models: Self::probe_models(&reference)?,
            metadata: BTreeMap::from([
                ("claude_transport".into(), json!("claude-code-process")),
                ("claude_history_mode".into(), json!("structured-replay")),
            ]),
        })
    }
    /// Returns a host-only opaque reference, never an OAuth access token.
    /// # Errors
    /// Returns an error if the source, reference, or requested capability is unavailable.
    pub fn access_token(&self, id: &str) -> Result<String> {
        self.bound(id)?;
        Ok(format!("claude-local-reference:{id}"))
    }
    /// Removes only the Token Station reference.
    /// # Errors
    /// Returns an error if the source, reference, or requested capability is unavailable.
    pub fn disconnect(&self, id: &str) -> Result<()> {
        let _lock = self.lock()?;
        let path = self.path(id)?;
        if path.exists() {
            self.load(id)?;
            fs::remove_file(path).map_err(local_error)?;
        }
        Ok(())
    }
    fn probe_models(reference: &Reference) -> Result<Vec<Model>> {
        let work = WorkDir::new()?;
        let mut command = command(&reference.executable, &reference.source, &work.0);
        safety_args(&mut command);
        command.args([
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
            "--system-prompt",
            "Report initialization metadata only.",
        ]);
        let mut process = Process(command.spawn().map_err(local_error)?);
        let output = output_channel(&mut process.0)?;
        write_json(
            &mut process.0,
            &json!({"type":"control_request","request_id":"models","request":{"subtype":"initialize"}}),
        )?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if Instant::now() >= deadline {
                return Err("Claude Code model discovery timed out.".into());
            }
            match output.recv_timeout(Duration::from_millis(50)) {
                Ok(Ok(line)) => {
                    let value: Value = serde_json::from_str(&line).map_err(local_error)?;
                    if value["type"] == "control_response"
                        && value["response"]["request_id"] == "models"
                    {
                        if value["response"]["subtype"] != "success" {
                            return Err("Claude Code rejected model discovery.".into());
                        }
                        return parse_models(&value["response"]["response"]);
                    }
                }
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Claude Code ended before model discovery completed.".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    /// Executes a Messages descriptor with caller-owned tools.
    /// # Errors
    /// Returns an error if the source, reference, or requested capability is unavailable.
    pub fn execute(
        &self,
        id: &str,
        descriptor: &HttpRequestDescriptor,
        timeout: Duration,
        cancellation: Option<&CancellationToken>,
    ) -> std::result::Result<HttpResponseParts, ErrorEnvelope> {
        if descriptor.method != HttpMethod::Post
            || descriptor.url != format!("{API_BASE}/v1/messages")
        {
            return Err(capability(
                "Claude subscription requests require the fixed host process endpoint.",
            ));
        }
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return Err(error(
                ErrorCode::Timeout,
                499,
                "The Claude request was cancelled.",
            ));
        }
        let deadline = Instant::now() + timeout.min(Duration::from_mins(10));
        let reference = self
            .bound_with(id, deadline, cancellation)
            .map_err(|state| match state.as_str() {
                "cancelled" => error(ErrorCode::Timeout, 499, "The Claude request was cancelled."),
                "timeout" => error(ErrorCode::Timeout, 504, "The Claude request timed out."),
                _ => error(
                    ErrorCode::Auth,
                    401,
                    "Open Claude Code and reconnect its subscription login.",
                ),
            })?;
        let body = descriptor
            .body
            .as_ref()
            .ok_or_else(|| capability("Claude subscription requests require a message body."))?;
        process::execute(&reference, body, deadline, cancellation)
    }
}

#[path = "claude/process.rs"]
mod process;
#[cfg(test)]
use process::{McpServer, prepare, response_sse};
#[cfg(test)]
#[path = "claude/tests.rs"]
mod tests;
