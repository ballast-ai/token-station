use axum::body::Body;
use axum::extract::{Request, State as AxumState};
use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::Router;
#[cfg(target_os = "macos")]
use flate2::read::GzDecoder;
use getrandom::fill as fill_random;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{BufRead, BufReader};
#[cfg(target_os = "macos")]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use tauri::State;
use tokio::sync::oneshot;
use zeroize::Zeroizing;

use crate::agent_integration::commands::runtime_from_app;
use crate::{AgentIntegrationPaths, AppStateManaged};

const CURSOR_APPLICATION_USER_KEY: &str =
    "src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl.persistentStorage.applicationUser";
const CURSOR_OPENAI_KEY: &str = "secret://cursorAuth/openAIKey";
const CURSOR_MODEL: &str = "tokenstation/auto";
#[cfg(target_os = "macos")]
const CLOUDFLARED_VERSION: &str = "2026.8.0";
#[cfg(target_os = "macos")]
const CLOUDFLARED_ASSET_URL: &str = "https://github.com/cloudflare/cloudflared/releases/download/2026.8.0/cloudflared-darwin-arm64.tgz";
#[cfg(target_os = "macos")]
const CLOUDFLARED_ASSET_SIZE: usize = 19_214_411;
#[cfg(target_os = "macos")]
const CLOUDFLARED_MAX_ARCHIVE_BYTES: u64 = 24 * 1024 * 1024;
#[cfg(target_os = "macos")]
const CLOUDFLARED_ASSET_SHA256: &str =
    "6244b4b199515690f93e170110d219d8d141184ba847179980c2f5906800c931";
#[cfg(target_os = "macos")]
const CLOUDFLARED_BINARY_SHA256: &str =
    "145790f4f8a6413f69ce08800c401bc15a2a18afcc3b5ffea0a861623566c0a9";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CursorProviderState {
    Disconnected,
    Connected,
    RepairRequired,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CursorProviderStatusView {
    pub state: CursorProviderState,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CursorSettingsBackup {
    schema_version: u32,
    created_at_ms: u64,
    application_user: String,
    encrypted_openai_key: Option<String>,
}

impl CursorSettingsBackup {
    fn restore(&self, db_path: &Path) -> Result<(), String> {
        restore_cursor_database(
            db_path,
            &self.application_user,
            self.encrypted_openai_key.as_deref(),
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CursorManagedRecord {
    schema_version: u32,
    public_origin: String,
    backup: CursorSettingsBackup,
}

#[derive(Clone)]
struct BridgeContext {
    cursor_token: Arc<Zeroizing<String>>,
    gateway_token: Arc<Zeroizing<String>>,
    gateway_origin: String,
    client: reqwest::Client,
}

struct ActiveTunnel {
    public_origin: String,
    cloudflared: Child,
    shutdown: Option<oneshot::Sender<()>>,
}

impl ActiveTunnel {
    fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = self.cloudflared.kill();
        let _ = self.cloudflared.wait();
    }
}

#[derive(Default)]
pub(crate) struct CursorTunnelState(Mutex<Option<ActiveTunnel>>);

impl Drop for CursorTunnelState {
    fn drop(&mut self) {
        if let Ok(active) = self.0.get_mut() {
            if let Some(active) = active.take() {
                active.stop();
            }
        }
    }
}

pub(crate) fn cursor_request_allowed(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("GET", "/agents/cursor/v1/models")
            | ("POST", "/agents/cursor/v1/chat/completions")
            | ("POST", "/agents/cursor/v1/responses")
    )
}

pub(crate) fn parse_trycloudflare_origin(line: &str) -> Option<String> {
    let regex = Regex::new(r"https://[a-z0-9]+(?:-[a-z0-9]+)+\.trycloudflare\.com(?:\b|/)").ok()?;
    let matched = regex.find(line)?.as_str().trim_end_matches('/');
    Some(matched.to_string())
}

fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn bridge_request(
    AxumState(context): AxumState<BridgeContext>,
    request: Request,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    if !cursor_request_allowed(method.as_str(), &path) {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("static response builds");
    }
    let supplied = bearer_token(request.headers()).unwrap_or_default();
    if !bool::from(supplied.as_bytes().ct_eq(context.cursor_token.as_bytes())) {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::empty())
            .expect("static response builds");
    }

    let content_type = request.headers().get(header::CONTENT_TYPE).cloned();
    let accept = request.headers().get(header::ACCEPT).cloned();
    let body = match axum::body::to_bytes(request.into_body(), 32 * 1024 * 1024).await {
        Ok(body) => body,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(Body::empty())
                .expect("static response builds")
        }
    };
    let target = format!("{}{}", context.gateway_origin.trim_end_matches('/'), path);
    let mut upstream = context
        .client
        .request(method, target)
        .bearer_auth(context.gateway_token.as_str());
    if let Some(content_type) = content_type {
        upstream = upstream.header(header::CONTENT_TYPE, content_type);
    }
    if let Some(accept) = accept {
        upstream = upstream.header(header::ACCEPT, accept);
    }
    let upstream = match upstream.body(body).send().await {
        Ok(response) => response,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Body::empty())
                .expect("static response builds")
        }
    };
    let status = upstream.status();
    let response_content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let mut response = Response::builder().status(status);
    if let Some(content_type) = response_content_type {
        response = response.header(header::CONTENT_TYPE, content_type);
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .expect("upstream response builds")
}

async fn start_bridge(context: BridgeContext) -> Result<(u16, oneshot::Sender<()>), String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("无法启动 Cursor 本地桥接：{error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("无法读取 Cursor 桥接端口：{error}"))?
        .port();
    let app = Router::new().fallback(bridge_request).with_state(context);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tauri::async_runtime::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
    });
    Ok((port, shutdown_tx))
}

#[cfg(target_os = "macos")]
fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(target_os = "macos")]
fn ensure_cloudflared(root: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;

    let bin_dir = root.join("bin").join(CLOUDFLARED_VERSION);
    crate::agent_integration::safe_fs::ensure_private_dir(&bin_dir)
        .map_err(|_| "无法创建 cloudflared 私有缓存目录".to_string())?;
    let binary_path = bin_dir.join("cloudflared");
    if binary_path.is_file() {
        let bytes = fs::read(&binary_path).map_err(|error| error.to_string())?;
        if sha256_hex(&bytes) == CLOUDFLARED_BINARY_SHA256 {
            fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
            return Ok(binary_path);
        }
        return Err("cloudflared 缓存摘要不一致，已拒绝执行".to_string());
    }

    let mut response = ureq::get(CLOUDFLARED_ASSET_URL)
        .call()
        .map_err(|error| format!("下载 cloudflared 失败：{error}"))?;
    let archive_bytes = response
        .body_mut()
        .with_config()
        .limit(CLOUDFLARED_MAX_ARCHIVE_BYTES)
        .read_to_vec()
        .map_err(|error| format!("读取 cloudflared 下载包失败：{error}"))?;
    if archive_bytes.len() != CLOUDFLARED_ASSET_SIZE {
        return Err("cloudflared 下载包大小不一致，已拒绝解压".to_string());
    }
    if sha256_hex(&archive_bytes) != CLOUDFLARED_ASSET_SHA256 {
        return Err("cloudflared 下载包摘要不一致，已拒绝解压".to_string());
    }
    let decoder = GzDecoder::new(archive_bytes.as_slice());
    let mut archive = tar::Archive::new(decoder);
    let mut binary = None;
    for entry in archive
        .entries()
        .map_err(|error| format!("读取 cloudflared 压缩包失败：{error}"))?
    {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path().map_err(|error| error.to_string())?;
        if path.file_name().and_then(|name| name.to_str()) == Some("cloudflared") {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            binary = Some(bytes);
            break;
        }
    }
    let binary = binary.ok_or_else(|| "cloudflared 压缩包缺少可执行文件".to_string())?;
    if sha256_hex(&binary) != CLOUDFLARED_BINARY_SHA256 {
        return Err("cloudflared 可执行文件摘要不一致，已拒绝执行".to_string());
    }
    crate::agent_integration::safe_fs::write_atomic_private(&binary_path, &binary)
        .map_err(|_| "无法写入 cloudflared 私有缓存".to_string())?;
    fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    Ok(binary_path)
}

#[cfg(not(target_os = "macos"))]
fn ensure_cloudflared(_root: &Path) -> Result<PathBuf, String> {
    Err("Cursor 公网接入当前只支持 macOS".to_string())
}

fn cloudflared_args(bridge_port: u16) -> Vec<String> {
    vec![
        "tunnel".to_string(),
        "--no-autoupdate".to_string(),
        "--config".to_string(),
        "/dev/null".to_string(),
        "--edge-ip-version".to_string(),
        "4".to_string(),
        "--url".to_string(),
        format!("http://127.0.0.1:{bridge_port}"),
    ]
}

fn start_cloudflared(binary: &Path, bridge_port: u16) -> Result<(Child, String), String> {
    let mut child = Command::new(binary)
        .args(cloudflared_args(bridge_port))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 cloudflared：{error}"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "无法读取 cloudflared 启动日志".to_string())?;
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut sent = false;
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if !sent {
                if let Some(origin) = parse_trycloudflare_origin(&line) {
                    let _ = sender.send(origin);
                    sent = true;
                }
            }
        }
    });
    match receiver.recv_timeout(Duration::from_secs(30)) {
        Ok(origin) => Ok((child, origin)),
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("等待 Cloudflare Quick Tunnel 地址超时".to_string())
        }
    }
}

fn cursor_database_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").ok_or_else(|| "无法确定当前用户目录".to_string())?;
        Ok(PathBuf::from(home)
            .join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"))
    }
    #[cfg(not(target_os = "macos"))]
    Err("Cursor 公网接入当前只支持 macOS".to_string())
}

fn cursor_is_running() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        let status = Command::new("pgrep")
            .args(["-f", "/Applications/Cursor.app/Contents/MacOS/Cursor"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("无法检查 Cursor 运行状态：{error}"))?;
        match status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err("Cursor 运行状态检查失败".to_string()),
        }
    }
    #[cfg(not(target_os = "macos"))]
    Err("Cursor 公网接入当前只支持 macOS".to_string())
}

#[cfg(target_os = "macos")]
fn cursor_safe_storage_password() -> Result<Zeroizing<String>, String> {
    let output = Command::new("security")
        .args(["find-generic-password", "-w", "-s", "Cursor Safe Storage"])
        .output()
        .map_err(|error| format!("无法读取 Cursor Safe Storage：{error}"))?;
    if !output.status.success() {
        return Err("无法从登录钥匙串读取 Cursor Safe Storage".to_string());
    }
    let password = String::from_utf8(output.stdout)
        .map_err(|_| "Cursor Safe Storage 主密码不是 UTF-8".to_string())?;
    Ok(Zeroizing::new(password.trim_end().to_string()))
}

#[cfg(not(target_os = "macos"))]
fn cursor_safe_storage_password() -> Result<Zeroizing<String>, String> {
    Err("Cursor 公网接入当前只支持 macOS".to_string())
}

#[cfg(target_os = "macos")]
pub(crate) fn encrypt_cursor_secret(plaintext: &str, password: &str) -> Result<String, String> {
    use aes::Aes128;
    use cbc::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
    use pbkdf2::pbkdf2_hmac;
    use sha1::Sha1;

    let mut key = [0u8; 16];
    pbkdf2_hmac::<Sha1>(password.as_bytes(), b"saltysalt", 1003, &mut key);
    let iv = [0x20u8; 16];
    let ciphertext = cbc::Encryptor::<Aes128>::new(&key.into(), &iv.into())
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());
    let mut bytes = b"v10".to_vec();
    bytes.extend(ciphertext);
    serde_json::to_string(&serde_json::json!({ "type": "Buffer", "data": bytes }))
        .map_err(|error| error.to_string())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn encrypt_cursor_secret(_plaintext: &str, _password: &str) -> Result<String, String> {
    Err("Cursor 公网接入当前只支持 macOS".to_string())
}

#[cfg(all(test, target_os = "macos"))]
pub(crate) fn decrypt_cursor_secret_for_test(
    encoded: &str,
    password: &str,
) -> Result<String, String> {
    use aes::Aes128;
    use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
    use pbkdf2::pbkdf2_hmac;
    use sha1::Sha1;

    let value: serde_json::Value = serde_json::from_str(encoded).map_err(|e| e.to_string())?;
    let bytes = value["data"]
        .as_array()
        .ok_or_else(|| "Buffer data missing".to_string())?
        .iter()
        .map(|value| value.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "Buffer data invalid".to_string())?;
    if !bytes.starts_with(b"v10") {
        return Err("v10 prefix missing".to_string());
    }
    let mut key = [0u8; 16];
    pbkdf2_hmac::<Sha1>(password.as_bytes(), b"saltysalt", 1003, &mut key);
    let iv = [0x20u8; 16];
    let plaintext = cbc::Decryptor::<Aes128>::new(&key.into(), &iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(&bytes[3..])
        .map_err(|_| "decrypt failed".to_string())?;
    String::from_utf8(plaintext).map_err(|error| error.to_string())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn cursor_model_entry() -> serde_json::Value {
    serde_json::json!({
        "name": CURSOR_MODEL,
        "serverModelName": CURSOR_MODEL,
        "clientDisplayName": "Token Station Auto",
        "inputboxShortModelName": "Token Station Auto",
        "defaultOn": true,
        "supportsAgent": true,
        "supportsThinking": true,
        "supportsImages": true,
        "supportsMaxMode": false,
        "supportsNonMaxMode": true,
        "supportsPlanMode": true,
        "supportsSandboxing": true,
        "isUserAdded": true,
        "parameterDefinitions": [],
        "variants": [],
        "legacySlugs": [],
        "idAliases": [],
        "cloudAgentEffortModes": [],
        "modelPickerBadges": []
    })
}

fn cursor_model_name(model: &serde_json::Value) -> Option<&str> {
    model
        .get("name")
        .or_else(|| model.get("id"))
        .and_then(serde_json::Value::as_str)
}

fn push_unique_model_name(names: &mut Vec<String>, name: &str) {
    if name != CURSOR_MODEL && !names.iter().any(|current| current == name) {
        names.push(name.to_string());
    }
}

fn select_cursor_model(config: &mut serde_json::Value) -> Result<(), String> {
    let object = config
        .as_object_mut()
        .ok_or_else(|| "Cursor modelConfig 条目必须是 object".to_string())?;
    object.insert(
        "modelName".to_string(),
        serde_json::Value::String(CURSOR_MODEL.to_string()),
    );
    object.insert(
        "selectedModels".to_string(),
        serde_json::json!([{"modelId": CURSOR_MODEL, "parameters": []}]),
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_cursor() -> Result<(), String> {
    let status = Command::new("/usr/bin/open")
        .args(["-a", "Cursor"])
        .status()
        .map_err(|error| format!("无法启动 Cursor：{error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("无法启动 Cursor：open 命令执行失败".to_string())
    }
}

#[cfg(not(target_os = "macos"))]
fn launch_cursor() -> Result<(), String> {
    Err("Cursor 自动启动当前只支持 macOS".to_string())
}

fn write_cursor_database(
    db_path: &Path,
    original_application_user: &str,
    base_url: &str,
    encrypted_token: &str,
) -> Result<(), String> {
    let mut application_user: serde_json::Value =
        serde_json::from_str(original_application_user)
            .map_err(|_| "Cursor applicationUser JSON 无法解析".to_string())?;
    let object = application_user
        .as_object_mut()
        .ok_or_else(|| "Cursor applicationUser 必须是 JSON object".to_string())?;
    object.insert(
        "openAIBaseUrl".to_string(),
        serde_json::Value::String(base_url.to_string()),
    );
    object.insert("useOpenAIKey".to_string(), serde_json::Value::Bool(true));
    let mut hidden_models = Vec::new();
    for key in ["availableDefaultModels2", "availableAPIKeyModels"] {
        if let Some(models) = object.get(key).and_then(serde_json::Value::as_array) {
            for model in models {
                if let Some(name) = cursor_model_name(model) {
                    push_unique_model_name(&mut hidden_models, name);
                }
            }
        }
    }
    object.insert(
        "availableAPIKeyModels".to_string(),
        serde_json::Value::Array(vec![cursor_model_entry()]),
    );
    let ai_settings = object
        .entry("aiSettings")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| "Cursor aiSettings 必须是 object".to_string())?;
    for key in [
        "userAddedModels",
        "modelOverrideEnabled",
        "modelOverrideDisabled",
    ] {
        if let Some(models) = ai_settings.get(key).and_then(serde_json::Value::as_array) {
            for model in models.iter().filter_map(serde_json::Value::as_str) {
                push_unique_model_name(&mut hidden_models, model);
            }
        }
    }
    ai_settings.insert(
        "userAddedModels".to_string(),
        serde_json::json!([CURSOR_MODEL]),
    );
    ai_settings.insert(
        "modelOverrideEnabled".to_string(),
        serde_json::json!([CURSOR_MODEL]),
    );
    ai_settings.insert(
        "modelOverrideDisabled".to_string(),
        serde_json::Value::Array(
            hidden_models
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    let model_config = ai_settings
        .entry("modelConfig")
        .or_insert_with(|| serde_json::json!({"composer": {"maxMode": false}}))
        .as_object_mut()
        .ok_or_else(|| "Cursor modelConfig 必须是 object".to_string())?;
    if model_config.is_empty() {
        model_config.insert(
            "composer".to_string(),
            serde_json::json!({"maxMode": false}),
        );
    }
    for config in model_config.values_mut() {
        select_cursor_model(config)?;
    }
    let encoded = serde_json::to_string(&application_user).map_err(|error| error.to_string())?;
    let mut connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE ItemTable SET value = ?1 WHERE key = ?2",
            rusqlite::params![encoded, CURSOR_APPLICATION_USER_KEY],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Cursor applicationUser 写入目标不存在".to_string());
    }
    transaction
        .execute(
            "INSERT INTO ItemTable(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            rusqlite::params![CURSOR_OPENAI_KEY, encrypted_token],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

#[cfg(test)]
pub(crate) fn update_cursor_database(
    db_path: &Path,
    base_url: &str,
    encrypted_token: &str,
) -> Result<CursorSettingsBackup, String> {
    let backup = read_cursor_backup(db_path)?;
    write_cursor_database(db_path, &backup.application_user, base_url, encrypted_token)?;
    Ok(backup)
}

fn read_cursor_backup(db_path: &Path) -> Result<CursorSettingsBackup, String> {
    let connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let application_user: String = connection
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            [CURSOR_APPLICATION_USER_KEY],
            |row| row.get(0),
        )
        .map_err(|_| "Cursor applicationUser 配置不存在".to_string())?;
    let encrypted_openai_key: Option<String> = connection
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            [CURSOR_OPENAI_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    drop(connection);
    let backup = CursorSettingsBackup {
        schema_version: 1,
        created_at_ms: now_ms(),
        application_user,
        encrypted_openai_key,
    };
    Ok(backup)
}

fn restore_cursor_database(
    db_path: &Path,
    application_user: &str,
    encrypted_openai_key: Option<&str>,
) -> Result<(), String> {
    let mut connection = Connection::open(db_path).map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let changed = transaction
        .execute(
            "UPDATE ItemTable SET value = ?1 WHERE key = ?2",
            rusqlite::params![application_user, CURSOR_APPLICATION_USER_KEY],
        )
        .map_err(|error| error.to_string())?;
    if changed != 1 {
        return Err("Cursor applicationUser 恢复目标不存在".to_string());
    }
    match encrypted_openai_key {
        Some(value) => transaction
            .execute(
                "INSERT INTO ItemTable(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![CURSOR_OPENAI_KEY, value],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())?,
        None => transaction
            .execute("DELETE FROM ItemTable WHERE key = ?1", [CURSOR_OPENAI_KEY])
            .map(|_| ())
            .map_err(|error| error.to_string())?,
    }
    transaction.commit().map_err(|error| error.to_string())
}

fn cursor_root(paths: &AgentIntegrationPaths) -> PathBuf {
    paths.snapshot_root.join("cursor")
}

fn record_path(paths: &AgentIntegrationPaths) -> PathBuf {
    cursor_root(paths).join("active.json")
}

fn backup_path(paths: &AgentIntegrationPaths) -> PathBuf {
    cursor_root(paths).join("settings-backup.json")
}

pub(crate) fn write_backup_file(path: &Path, backup: &CursorSettingsBackup) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Cursor 备份路径缺少父目录".to_string())?;
    crate::agent_integration::safe_fs::ensure_private_dir(parent)
        .map_err(|_| "无法创建 Cursor 私有备份目录".to_string())?;
    let bytes = serde_json::to_vec(backup).map_err(|error| error.to_string())?;
    crate::agent_integration::safe_fs::write_atomic_private(path, &bytes)
        .map_err(|_| "无法写入 Cursor 私有配置备份".to_string())
}

fn read_record(path: &Path) -> Result<Option<CursorManagedRecord>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| "Cursor 接管记录无法解析".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("无法读取 Cursor 接管记录：{error}")),
    }
}

fn write_record(path: &Path, record: &CursorManagedRecord) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    crate::agent_integration::safe_fs::write_atomic_private(path, &bytes)
        .map_err(|_| "无法写入 Cursor 接管记录".to_string())
}

fn generate_cursor_token() -> Result<Zeroizing<String>, String> {
    let mut bytes = [0u8; 32];
    fill_random(&mut bytes).map_err(|error| format!("无法生成 Cursor 临时令牌：{error}"))?;
    let token = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(Zeroizing::new(token))
}

fn status_view(
    state: &CursorTunnelState,
    paths: &AgentIntegrationPaths,
) -> CursorProviderStatusView {
    let mut active = state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(running) = active.as_mut() {
        match running.cloudflared.try_wait() {
            Ok(None) => {
                return CursorProviderStatusView {
                    state: CursorProviderState::Connected,
                    message: Some(format!(
                        "Cursor 已通过 {} 接入 Token Station",
                        running.public_origin
                    )),
                }
            }
            Ok(Some(_)) | Err(_) => {
                if let Some(stopped) = active.take() {
                    stopped.stop();
                }
            }
        }
    }
    if record_path(paths).is_file() {
        CursorProviderStatusView {
            state: CursorProviderState::RepairRequired,
            message: Some("上次 Cursor 隧道已失效，请重新接入或恢复官方配置".to_string()),
        }
    } else {
        CursorProviderStatusView {
            state: CursorProviderState::Disconnected,
            message: None,
        }
    }
}

#[tauri::command]
pub(crate) fn get_cursor_provider_status(
    state: State<'_, CursorTunnelState>,
    paths: State<'_, AgentIntegrationPaths>,
) -> CursorProviderStatusView {
    status_view(&state, &paths)
}

#[tauri::command(async)]
pub(crate) async fn configure_cursor_provider(
    app_state: State<'_, AppStateManaged>,
    paths: State<'_, AgentIntegrationPaths>,
    tunnel_state: State<'_, CursorTunnelState>,
) -> Result<CursorProviderStatusView, String> {
    if matches!(
        status_view(&tunnel_state, &paths).state,
        CursorProviderState::Connected
    ) {
        return Ok(status_view(&tunnel_state, &paths));
    }
    if cursor_is_running()? {
        return Err(
            "cursor_running: Cursor 正在运行。请手动退出 Cursor 后再点一键接入。".to_string(),
        );
    }
    let db_path = cursor_database_path()?;
    if !db_path.is_file() {
        return Err("找不到 Cursor 本机配置数据库".to_string());
    }
    let runtime = runtime_from_app(&app_state).map_err(|error| error.message)?;
    let gateway_origin = runtime.gateway_origin().map_err(|error| error.message)?;
    let gateway_token = Zeroizing::new(runtime.virtual_key().to_string());
    let cursor_token = generate_cursor_token()?;
    let bridge_context = BridgeContext {
        cursor_token: Arc::new(cursor_token.clone()),
        gateway_token: Arc::new(gateway_token),
        gateway_origin,
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| format!("无法创建 Cursor 桥接客户端：{error}"))?,
    };
    let (bridge_port, shutdown) = start_bridge(bridge_context).await?;
    let root = cursor_root(&paths);
    let binary_root = root.clone();
    let binary = match tauri::async_runtime::spawn_blocking(move || {
        ensure_cloudflared(&binary_root)
    })
    .await
    {
        Ok(Ok(binary)) => binary,
        Ok(Err(error)) => {
            let _ = shutdown.send(());
            return Err(error);
        }
        Err(error) => {
            let _ = shutdown.send(());
            return Err(format!("cloudflared 准备任务失败：{error}"));
        }
    };
    let tunnel =
        match tauri::async_runtime::spawn_blocking(move || start_cloudflared(&binary, bridge_port))
            .await
        {
            Ok(tunnel) => tunnel,
            Err(error) => {
                let _ = shutdown.send(());
                return Err(format!("cloudflared 启动任务失败：{error}"));
            }
        };
    let (cloudflared, public_origin) = match tunnel {
        Ok(value) => value,
        Err(error) => {
            let _ = shutdown.send(());
            return Err(error);
        }
    };
    let base_url = format!("{public_origin}/agents/cursor/v1");
    let pending = ActiveTunnel {
        public_origin,
        cloudflared,
        shutdown: Some(shutdown),
    };
    let setup_result = (|| -> Result<(), String> {
        let password = cursor_safe_storage_password()?;
        let encrypted_token = encrypt_cursor_secret(cursor_token.as_str(), password.as_str())?;
        let managed_path = record_path(&paths);
        let existing = read_record(&managed_path)?;
        let backup = match existing.as_ref() {
            Some(record) => record.backup.clone(),
            None => read_cursor_backup(&db_path)?,
        };
        write_backup_file(&backup_path(&paths), &backup)?;
        if let Err(error) = write_cursor_database(
            &db_path,
            &backup.application_user,
            &base_url,
            &encrypted_token,
        ) {
            let _ = backup.restore(&db_path);
            return Err(error);
        }
        let record = CursorManagedRecord {
            schema_version: 1,
            public_origin: pending.public_origin.clone(),
            backup,
        };
        if let Err(error) = write_record(&managed_path, &record) {
            let _ = record.backup.restore(&db_path);
            return Err(error);
        }
        if let Err(error) = launch_cursor() {
            let _ = record.backup.restore(&db_path);
            let _ = fs::remove_file(&managed_path);
            return Err(error);
        }
        Ok(())
    })();
    if let Err(error) = setup_result {
        pending.stop();
        return Err(error);
    }
    let mut slot = tunnel_state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(previous) = slot.replace(pending) {
        previous.stop();
    }
    Ok(CursorProviderStatusView {
        state: CursorProviderState::Connected,
        message: Some(
            "Cursor HTTPS 隧道已建立，Cursor 已启动并选中 Token Station Auto。".to_string(),
        ),
    })
}

#[tauri::command(async)]
pub(crate) fn restore_cursor_provider(
    paths: State<'_, AgentIntegrationPaths>,
    tunnel_state: State<'_, CursorTunnelState>,
) -> Result<CursorProviderStatusView, String> {
    if cursor_is_running()? {
        return Err(
            "cursor_running: Cursor 正在运行。请手动退出 Cursor 后再恢复官方配置。".to_string(),
        );
    }
    let managed_path = record_path(&paths);
    let Some(record) = read_record(&managed_path)? else {
        return Ok(CursorProviderStatusView {
            state: CursorProviderState::Disconnected,
            message: Some("Cursor 当前没有 Token Station 接管记录".to_string()),
        });
    };
    let db_path = cursor_database_path()?;
    record.backup.restore(&db_path)?;
    if let Some(active) = tunnel_state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        active.stop();
    }
    fs::remove_file(&managed_path).map_err(|error| format!("无法清除 Cursor 接管记录：{error}"))?;
    Ok(CursorProviderStatusView {
        state: CursorProviderState::Disconnected,
        message: Some("已恢复 Cursor 官方配置并断开".to_string()),
    })
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::decrypt_cursor_secret_for_test;
    #[cfg(target_os = "macos")]
    use super::encrypt_cursor_secret;
    use super::{
        cursor_request_allowed, parse_trycloudflare_origin, update_cursor_database,
        write_backup_file, CursorSettingsBackup, CURSOR_APPLICATION_USER_KEY, CURSOR_MODEL,
    };
    use rusqlite::Connection;
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "token-station-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn bridge_allows_only_cursor_openai_routes() {
        assert!(cursor_request_allowed("GET", "/agents/cursor/v1/models"));
        assert!(cursor_request_allowed(
            "POST",
            "/agents/cursor/v1/chat/completions"
        ));
        assert!(cursor_request_allowed(
            "POST",
            "/agents/cursor/v1/responses"
        ));
        assert!(!cursor_request_allowed("GET", "/health"));
        assert!(!cursor_request_allowed(
            "POST",
            "/agents/claude-code/v1/chat/completions"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pinned_cloudflared_asset_fits_the_bounded_download_limit() {
        assert_eq!(super::CLOUDFLARED_ASSET_SIZE, 19_214_411);
        assert!(
            u64::try_from(super::CLOUDFLARED_ASSET_SIZE).unwrap()
                < super::CLOUDFLARED_MAX_ARCHIVE_BYTES
        );
        assert_eq!(super::CLOUDFLARED_MAX_ARCHIVE_BYTES, 24 * 1024 * 1024);
    }

    #[tokio::test]
    async fn bridge_accepts_only_the_ephemeral_cursor_token() {
        use axum::body::Body;
        use axum::http::{header, Request, StatusCode};
        use axum::routing::get;
        use axum::Router;
        use std::sync::Arc;
        use tokio::sync::oneshot;
        use zeroize::Zeroizing;

        let upstream = Router::new().route(
            "/agents/cursor/v1/models",
            get(|request: Request<Body>| async move {
                if request.headers().get(header::AUTHORIZATION)
                    == Some(&"Bearer gateway-private-key".parse().unwrap())
                {
                    (StatusCode::OK, "models-ok")
                } else {
                    (StatusCode::UNAUTHORIZED, "missing gateway key")
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_port = listener.local_addr().unwrap().port();
        let (upstream_shutdown_tx, upstream_shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            axum::serve(listener, upstream)
                .with_graceful_shutdown(async move {
                    let _ = upstream_shutdown_rx.await;
                })
                .await
                .unwrap();
        });

        let context = super::BridgeContext {
            cursor_token: Arc::new(Zeroizing::new("ephemeral-cursor-token".to_string())),
            gateway_token: Arc::new(Zeroizing::new("gateway-private-key".to_string())),
            gateway_origin: format!("http://127.0.0.1:{upstream_port}"),
            client: reqwest::Client::new(),
        };
        let (bridge_port, bridge_shutdown_tx) = super::start_bridge(context).await.unwrap();
        let client = reqwest::Client::new();
        let allowed_url = format!("http://127.0.0.1:{bridge_port}/agents/cursor/v1/models");
        let allowed = client
            .get(&allowed_url)
            .bearer_auth("ephemeral-cursor-token")
            .send()
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        assert_eq!(allowed.text().await.unwrap(), "models-ok");

        let global_key = client
            .get(&allowed_url)
            .bearer_auth("gateway-private-key")
            .send()
            .await
            .unwrap();
        assert_eq!(global_key.status(), StatusCode::UNAUTHORIZED);

        let forbidden = client
            .get(format!("http://127.0.0.1:{bridge_port}/health"))
            .bearer_auth("ephemeral-cursor-token")
            .send()
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::NOT_FOUND);

        let _ = bridge_shutdown_tx.send(());
        let _ = upstream_shutdown_tx.send(());
    }

    #[test]
    fn quick_tunnel_uses_an_isolated_ipv4_configuration() {
        assert_eq!(
            super::cloudflared_args(48787),
            vec![
                "tunnel",
                "--no-autoupdate",
                "--config",
                "/dev/null",
                "--edge-ip-version",
                "4",
                "--url",
                "http://127.0.0.1:48787",
            ]
        );
    }

    #[test]
    fn cloudflared_log_parser_accepts_only_https_quick_tunnel_origins() {
        assert_eq!(
            parse_trycloudflare_origin(
                "INF Your quick Tunnel has been created! Visit it at https://quiet-tree.trycloudflare.com"
            ),
            Some("https://quiet-tree.trycloudflare.com".to_string())
        );
        assert_eq!(
            parse_trycloudflare_origin("https://quiet-tree.example.com"),
            None
        );
        assert_eq!(
            parse_trycloudflare_origin("http://quiet-tree.trycloudflare.com"),
            None
        );
        assert_eq!(
            parse_trycloudflare_origin(
                "INF Requesting new quick Tunnel on https://api.trycloudflare.com"
            ),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_safe_storage_ciphertext_round_trips_as_cursor_buffer_json() {
        let encrypted = encrypt_cursor_secret("temporary-cursor-token", "master-password")
            .expect("encrypt Cursor secret");
        let value: serde_json::Value = serde_json::from_str(&encrypted).unwrap();
        assert_eq!(value["type"], "Buffer");
        assert!(value["data"].as_array().unwrap().len() > 3);
        assert_eq!(
            decrypt_cursor_secret_for_test(&encrypted, "master-password").unwrap(),
            "temporary-cursor-token"
        );
    }

    #[test]
    fn cursor_database_update_restores_exact_original_values() {
        let root = temp_dir("cursor-db");
        let db_path = root.join("state.vscdb");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute(
                "CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB)",
                [],
            )
            .unwrap();
        let original_application_user = json!({
            "openAIBaseUrl": "https://api.openai.com/v1",
            "useOpenAIKey": false,
            "aiSettings": {
                "userAddedModels": ["user/model"],
                "modelOverrideEnabled": ["user/model"],
                "modelConfig": {
                    "composer": {
                        "modelName": "user/model",
                        "selectedModels": [{"modelId": "user/model", "parameters": []}],
                        "maxMode": false
                    }
                }
            },
            "unrelated": { "keep": true }
        })
        .to_string();
        let original_key = "{\"type\":\"Buffer\",\"data\":[1,2,3]}";
        connection
            .execute(
                "INSERT INTO ItemTable(key, value) VALUES(?1, ?2)",
                [
                    "src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl.persistentStorage.applicationUser",
                    original_application_user.as_str(),
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable(key, value) VALUES(?1, ?2)",
                ["secret://cursorAuth/openAIKey", original_key],
            )
            .unwrap();
        drop(connection);

        let backup = update_cursor_database(
            &db_path,
            "https://quiet-tree.trycloudflare.com/agents/cursor/v1",
            "{\"type\":\"Buffer\",\"data\":[118,49,48,9]}",
        )
        .unwrap();
        assert_eq!(backup.application_user, original_application_user);
        assert_eq!(backup.encrypted_openai_key.as_deref(), Some(original_key));

        let connection = Connection::open(&db_path).unwrap();
        let connected_application_user: String = connection
            .query_row(
                "SELECT value FROM ItemTable WHERE key = ?1",
                [CURSOR_APPLICATION_USER_KEY],
                |row| row.get(0),
            )
            .unwrap();
        let connected: serde_json::Value =
            serde_json::from_str(&connected_application_user).unwrap();
        assert_eq!(
            connected["aiSettings"]["userAddedModels"],
            json!([CURSOR_MODEL])
        );
        assert_eq!(
            connected["aiSettings"]["modelOverrideEnabled"],
            json!([CURSOR_MODEL])
        );
        assert_eq!(
            connected["aiSettings"]["modelOverrideDisabled"],
            json!(["user/model"])
        );
        assert_eq!(
            connected["aiSettings"]["modelConfig"]["composer"]["modelName"],
            CURSOR_MODEL
        );
        assert_eq!(
            connected["aiSettings"]["modelConfig"]["composer"]["selectedModels"],
            json!([{"modelId": CURSOR_MODEL, "parameters": []}])
        );
        assert_eq!(
            connected["aiSettings"]["modelConfig"]["composer"]["maxMode"],
            false
        );
        drop(connection);

        backup.restore(&db_path).unwrap();
        let connection = Connection::open(&db_path).unwrap();
        let restored_application_user: String = connection
            .query_row(
                "SELECT value FROM ItemTable WHERE key = ?1",
                ["src.vs.platform.reactivestorage.browser.reactiveStorageServiceImpl.persistentStorage.applicationUser"],
                |row| row.get(0),
            )
            .unwrap();
        let restored_key: String = connection
            .query_row(
                "SELECT value FROM ItemTable WHERE key = ?1",
                ["secret://cursorAuth/openAIKey"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(restored_application_user, original_application_user);
        assert_eq!(restored_key, original_key);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn backup_type_remains_serializable_for_private_state_storage() {
        let root = temp_dir("cursor-backup");
        let backup = CursorSettingsBackup {
            schema_version: 1,
            created_at_ms: 123,
            application_user: "{}".to_string(),
            encrypted_openai_key: None,
        };
        let path = root.join("settings-backup.json");
        write_backup_file(&path, &backup).unwrap();
        let persisted: CursorSettingsBackup =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted.schema_version, 1);
        assert_eq!(persisted.application_user, "{}");
        fs::remove_dir_all(root).unwrap();
    }
}
