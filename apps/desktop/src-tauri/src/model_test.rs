use crate::*;

use sha2::{Digest, Sha256};
use token_station_cli::server::GatewayRequestLease;
use zeroize::Zeroize;

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct ModelTestMessage {
    pub(crate) role: String,
    pub(crate) content: String,
}

#[derive(Serialize)]
pub(crate) struct ModelTestReply {
    pub(crate) content: String,
    pub(crate) first_token_ms: u64,
    pub(crate) latency_ms: u64,
}

#[derive(Clone, Serialize)]
pub(crate) struct ModelTestStreamEvent {
    pub(crate) request_id: String,
    pub(crate) delta: String,
    pub(crate) first_token_ms: Option<u64>,
}

#[derive(Default)]
pub(crate) struct ModelTestStreamRegistry {
    pub(crate) active: BTreeMap<String, CancelToken>,
    pub(crate) pending_cancellations: BTreeSet<String>,
}

impl ModelTestStreamRegistry {
    pub(crate) fn register(&mut self, request_id: &str, token: CancelToken) -> Result<(), String> {
        if self.pending_cancellations.remove(request_id) {
            return Err("Model test cancelled".to_owned());
        }
        if self.active.contains_key(request_id) {
            return Err("This model test request is already active".to_owned());
        }
        if self.active.len() >= MODEL_TEST_MAX_ACTIVE_STREAMS {
            return Err("Too many model test requests are active".to_owned());
        }
        self.active.insert(request_id.to_owned(), token);
        Ok(())
    }

    pub(crate) fn cancel(&mut self, request_id: String) {
        if let Some(token) = self.active.get(&request_id).cloned() {
            token.cancel();
            return;
        }
        if self.pending_cancellations.len() >= MODEL_TEST_MAX_PENDING_CANCELLATIONS {
            let eviction = self.pending_cancellations.iter().next().cloned();
            if let Some(eviction) = eviction {
                self.pending_cancellations.remove(&eviction);
            }
        }
        self.pending_cancellations.insert(request_id);
    }
}

pub(crate) struct CachedModelTestGateway {
    pub(crate) config: Box<ClientConfig>,
    pub(crate) upstream_epochs: BTreeMap<String, u64>,
    pub(crate) secret_store_fingerprint: [u8; 32],
    pub(crate) plugin_identity_fingerprint: [u8; 32],
    pub(crate) gateway: Arc<Gateway>,
}

pub(crate) const MODEL_TEST_SECRET_STORE_MAX_BYTES: u64 = 1_048_576;

pub(crate) enum ModelTestGatewaySource {
    Running {
        gateway: Arc<Gateway>,
        running_revision: u64,
        request: GatewayRequestLease,
    },
    Draft {
        config: Box<ClientConfig>,
        upstream_epochs: BTreeMap<String, u64>,
        request: RequestContext,
    },
}

impl ModelTestGatewaySource {
    pub(crate) fn request_context(&self) -> &RequestContext {
        match self {
            Self::Running { request, .. } => request.context(),
            Self::Draft { request, .. } => request,
        }
    }
}

fn hash_model_test_secret(
    hash: &mut Sha256,
    owner: &str,
    slot: &str,
    value: Result<String, String>,
) {
    for field in [owner.as_bytes(), slot.as_bytes()] {
        hash_length_prefixed_field(hash, field);
    }
    match value {
        Ok(value) => {
            let value = Zeroizing::new(value);
            hash.update([1]);
            hash.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
            hash.update(value.as_bytes());
        }
        Err(_) => hash.update([0]),
    }
}

fn hash_length_prefixed_field(hash: &mut Sha256, field: &[u8]) {
    hash.update(u64::try_from(field.len()).unwrap_or(u64::MAX).to_be_bytes());
    hash.update(field);
}

pub(crate) struct ModelTestSecretStore {
    pub(crate) values: BTreeMap<String, String>,
}

impl Drop for ModelTestSecretStore {
    fn drop(&mut self) {
        for value in self.values.values_mut() {
            value.zeroize();
        }
    }
}

fn read_model_test_secret_store_inner(
    data_dir: &Path,
    before_open: impl FnOnce(),
) -> Result<ModelTestSecretStore, String> {
    use std::io::Read as _;

    let path = data_dir.join(secrets::SECRETS_FILE);
    let link_metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ModelTestSecretStore {
                values: BTreeMap::new(),
            });
        }
        Err(_) => return Err("The local secrets store is unreadable".to_owned()),
    };
    if link_metadata.file_type().is_symlink() || !link_metadata.is_file() {
        return Err("The local secrets store must be a regular file".to_owned());
    }
    if link_metadata.len() > MODEL_TEST_SECRET_STORE_MAX_BYTES {
        return Err("The local secrets store exceeds the model test size limit".to_owned());
    }
    before_open();
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(&path).map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return "The local secrets store must be a regular file".to_owned();
        }
        "The local secrets store is unreadable".to_owned()
    })?;
    let metadata = file
        .metadata()
        .map_err(|_| "The local secrets store is unreadable".to_owned())?;
    if !metadata.is_file() {
        return Err("The local secrets store must be a regular file".to_owned());
    }
    let mut raw = Zeroizing::new(Vec::with_capacity(
        usize::try_from(metadata.len().min(MODEL_TEST_SECRET_STORE_MAX_BYTES)).unwrap_or_default(),
    ));
    file.take(MODEL_TEST_SECRET_STORE_MAX_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|_| "The local secrets store is unreadable".to_owned())?;
    if u64::try_from(raw.len()).unwrap_or(u64::MAX) > MODEL_TEST_SECRET_STORE_MAX_BYTES {
        return Err("The local secrets store exceeds the model test size limit".to_owned());
    }
    let values = serde_json::from_slice(&raw)
        .map_err(|_| "The local secrets store is invalid JSON".to_owned())?;
    Ok(ModelTestSecretStore { values })
}

pub(crate) fn read_model_test_secret_store(
    data_dir: &Path,
) -> Result<ModelTestSecretStore, String> {
    read_model_test_secret_store_inner(data_dir, || {})
}

#[cfg(test)]
pub(crate) fn read_model_test_secret_store_with_before_open(
    data_dir: &Path,
    before_open: impl FnOnce(),
) -> Result<ModelTestSecretStore, String> {
    read_model_test_secret_store_inner(data_dir, before_open)
}

pub(crate) fn model_test_secret_store_fingerprint_with_reader(
    config: &ClientConfig,
    read_store: impl FnOnce(&Path) -> Result<ModelTestSecretStore, String>,
) -> Result<[u8; 32], String> {
    let mut hash = Sha256::new();
    let mut slots = Vec::new();
    if let Some(upstreams) = home_referenced_upstreams(config) {
        for name in upstreams {
            let Some(auth) = config
                .upstreams
                .get(&name)
                .and_then(|upstream| upstream.auth.as_ref())
                .filter(|auth| auth.store)
            else {
                continue;
            };
            slots.push((name, auth.slot.clone()));
        }
    }
    if let Some(auth) = config
        .egress
        .auth
        .as_ref()
        .map(|auth| &auth.credential)
        .filter(|auth| auth.store)
    {
        slots.push(("egress-proxy".to_owned(), auth.slot.clone()));
    }
    if slots.is_empty() {
        return Ok(hash.finalize().into());
    }
    let store = read_store(&config.data.dir)?;
    for (owner, slot) in slots {
        hash_model_test_secret(
            &mut hash,
            &owner,
            &slot,
            store
                .values
                .get(&format!("{owner}/{slot}"))
                .cloned()
                .ok_or_else(|| "secret is not in the local store".to_owned()),
        );
    }
    Ok(hash.finalize().into())
}

pub(crate) fn model_test_secret_store_fingerprint(
    config: &ClientConfig,
) -> Result<[u8; 32], String> {
    model_test_secret_store_fingerprint_with_reader(config, read_model_test_secret_store)
}

pub(crate) fn model_test_plugin_identity_fingerprint(
    config: &ClientConfig,
) -> Result<[u8; 32], String> {
    let registry = PluginRegistry::for_config(config).map_err(|error| {
        eprintln!("model test plugin discovery failed: {error}");
        "The Home route plugin identity is unavailable".to_owned()
    })?;
    let upstreams = home_referenced_upstreams(config)
        .ok_or_else(|| "The Home route plugin identity is unavailable".to_owned())?;
    let dialects = upstreams
        .iter()
        .filter_map(|name| config.upstreams.get(name))
        .map(|upstream| upstream.provider.clone())
        .collect::<BTreeSet<_>>();
    let mut hash = Sha256::new();
    for dialect in dialects {
        let binding = registry
            .provider_binding(&dialect)
            .ok_or_else(|| "The Home route references an unavailable Provider plugin".to_owned())?;
        let digest = binding.source.content_digest().map_err(|error| {
            eprintln!("model test plugin digest failed: {error}");
            "The Home route plugin identity is unavailable".to_owned()
        })?;
        let discovered_package = registry.package(&binding.package);
        for field in [
            dialect.as_bytes(),
            binding.package.as_bytes(),
            digest.as_bytes(),
        ] {
            hash_length_prefixed_field(&mut hash, field);
        }
        hash.update([
            u8::from(discovered_package.is_some_and(|package| package.conformance_passed)),
            u8::from(
                discovered_package.is_some_and(|package| package.publisher_signature_verified),
            ),
        ]);
    }
    for agent_package_name in config.plugins.effective_agents() {
        let source = registry.agent_source(&agent_package_name);
        let digest = source.content_digest().map_err(|error| {
            eprintln!("model test agent plugin digest failed: {error}");
            "The Home route plugin identity is unavailable".to_owned()
        })?;
        for field in [
            b"agent".as_slice(),
            agent_package_name.as_bytes(),
            digest.as_bytes(),
        ] {
            hash_length_prefixed_field(&mut hash, field);
        }
    }
    Ok(hash.finalize().into())
}

pub(crate) fn ensure_model_test_plugin_identity_unchanged(
    config: &ClientConfig,
    expected: [u8; 32],
) -> Result<(), String> {
    if model_test_plugin_identity_fingerprint(config)? != expected {
        return Err(
            "The Home route plugin identity changed while preparing the model test Gateway. Retry"
                .to_owned(),
        );
    }
    Ok(())
}

pub(crate) enum ModelTestRequestOwner {
    Running(GatewayRequestLease),
    Draft(RequestContext),
}

impl ModelTestRequestOwner {
    pub(crate) fn context(&self) -> &RequestContext {
        match self {
            Self::Running(request) => request.context(),
            Self::Draft(request) => request,
        }
    }
}

pub(crate) fn reusable_model_test_server<'a>(
    inner: &'a AppInner,
    config: &ClientConfig,
) -> Option<&'a RunningServer> {
    let server = match &inner.server {
        ServerLifecycle::Running { server, .. } => server,
        ServerLifecycle::Applying { old, .. } => old,
        ServerLifecycle::Stopped { .. }
        | ServerLifecycle::Starting { .. }
        | ServerLifecycle::Stopping { .. }
        | ServerLifecycle::Failed { .. } => return None,
    };
    (server.is_task_alive() && server.matches_home_gateway(config, &inner.upstream_epochs))
        .then_some(server)
}

pub(crate) fn model_test_gateway_source(
    inner: &AppInner,
) -> Result<ModelTestGatewaySource, String> {
    let config = Box::new(inner.materialize()?);
    let home_router = config.home_router_config()?;
    let route_uses_pending_credential = home_router
        .pools
        .values()
        .flatten()
        .chain(home_router.quota_accounts.iter())
        .any(|target| {
            inner
                .pending_provider_keys
                .contains_key(target.upstream.as_str())
        });
    if route_uses_pending_credential {
        return Err(
            "Save the verified Provider credential before testing this Home route".to_owned(),
        );
    }
    if let Some(server) = reusable_model_test_server(inner, &config) {
        return Ok(ModelTestGatewaySource::Running {
            gateway: server.gateway(),
            running_revision: server.running_revision(),
            request: server
                .begin_gateway_request(Duration::from_mins(2), Duration::from_mins(2))
                .with_upstream_response_limit(MODEL_TEST_MAX_STREAM_BYTES as u64),
        });
    }
    Ok(ModelTestGatewaySource::Draft {
        config,
        upstream_epochs: inner.upstream_epochs.clone(),
        request: RequestContext::detached(Duration::from_mins(2), Duration::from_mins(2))
            .with_upstream_response_limit(MODEL_TEST_MAX_STREAM_BYTES as u64),
    })
}

#[derive(Clone, Default)]
pub(crate) struct ModelTestStreamState(
    pub(crate) Arc<Mutex<ModelTestStreamRegistry>>,
    pub(crate) Arc<Mutex<Option<CachedModelTestGateway>>>,
);

pub(crate) struct ModelTestStreamRegistration {
    pub(crate) registry: Arc<Mutex<ModelTestStreamRegistry>>,
    pub(crate) request_id: String,
}

impl Drop for ModelTestStreamRegistration {
    fn drop(&mut self) {
        let mut registry = self.registry.lock().unwrap();
        registry.active.remove(&self.request_id);
        registry.pending_cancellations.remove(&self.request_id);
    }
}

pub(crate) const MODEL_TEST_MAX_MESSAGES: usize = 20;
pub(crate) const MODEL_TEST_MAX_MESSAGE_BYTES: usize = 16_000;
pub(crate) const MODEL_TEST_MAX_TOTAL_BYTES: usize = 64_000;
pub(crate) const MODEL_TEST_MAX_REQUEST_ID_BYTES: usize = 64;
pub(crate) const MODEL_TEST_MAX_ACTIVE_STREAMS: usize = 4;
pub(crate) const MODEL_TEST_MAX_PENDING_CANCELLATIONS: usize = 32;
pub(crate) const MODEL_TEST_MAX_SSE_BUFFER_BYTES: usize = 1_048_576;
pub(crate) const MODEL_TEST_MAX_RESPONSE_BYTES: usize = 16_000;
pub(crate) const MODEL_TEST_MAX_STREAM_EVENTS: usize = 1_024;
pub(crate) const MODEL_TEST_MAX_STREAM_BYTES: usize = 4 * 1_048_576;
pub(crate) const MODEL_TEST_STREAM_EVENT: &str = "model-test-stream";

#[derive(Default)]
pub(crate) struct ModelTestOutputBudget {
    pub(crate) bytes: usize,
    pub(crate) events: usize,
    pub(crate) wire_bytes: usize,
}

impl ModelTestOutputBudget {
    pub(crate) fn accept_wire(&mut self, bytes: usize) -> Result<(), String> {
        let next_bytes = self.wire_bytes.saturating_add(bytes);
        if next_bytes > MODEL_TEST_MAX_STREAM_BYTES {
            return Err("The model stream exceeded the wire response limit".to_owned());
        }
        self.wire_bytes = next_bytes;
        Ok(())
    }

    pub(crate) fn accept(&mut self, delta: &str) -> Result<(), String> {
        let next_bytes = self.bytes.saturating_add(delta.len());
        if next_bytes > MODEL_TEST_MAX_RESPONSE_BYTES {
            return Err("The model output exceeded the response limit".to_owned());
        }
        if self.events >= MODEL_TEST_MAX_STREAM_EVENTS {
            return Err("The model output exceeded the stream event limit".to_owned());
        }
        self.bytes = next_bytes;
        self.events += 1;
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct ModelTestSseDecoder {
    pub(crate) buffer: Vec<u8>,
}

impl ModelTestSseDecoder {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, String> {
        if self.buffer.len().saturating_add(chunk.len()) > MODEL_TEST_MAX_SSE_BUFFER_BYTES {
            return Err("The model stream exceeded the response buffer limit".to_owned());
        }
        self.buffer.extend_from_slice(chunk);
        let mut frames = Vec::new();
        while let Some((boundary, delimiter_len)) = find_model_test_sse_boundary(&self.buffer) {
            let frame = self.buffer.drain(..boundary).collect::<Vec<_>>();
            self.buffer.drain(..delimiter_len);
            let frame = String::from_utf8(frame)
                .map_err(|_| "The model stream returned invalid UTF-8".to_owned())?;
            if !frame.trim().is_empty() {
                frames.push(frame);
            }
        }
        Ok(frames)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<String>, String> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            self.buffer.clear();
            return Ok(Vec::new());
        }
        let frame = String::from_utf8(std::mem::take(&mut self.buffer))
            .map_err(|_| "The model stream ended with invalid UTF-8".to_owned())?;
        Ok(vec![frame])
    }
}

pub(crate) fn find_model_test_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}

pub(crate) fn validate_model_test_request_id(request_id: &str) -> Result<(), String> {
    if request_id.is_empty()
        || request_id.len() > MODEL_TEST_MAX_REQUEST_ID_BYTES
        || !request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("The model test request ID is invalid".to_owned());
    }
    Ok(())
}

pub(crate) fn model_test_stream_delta(frame: &str) -> Result<Option<String>, String> {
    let data = frame
        .lines()
        .filter_map(|line| {
            let data = line.strip_prefix("data:")?;
            Some(data.strip_prefix(' ').unwrap_or(data))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let data = data.trim();
    if data.is_empty() || data == "[DONE]" {
        return Ok(None);
    }
    let value: Value = serde_json::from_str(data)
        .map_err(|_| "The model stream returned invalid JSON".to_owned())?;
    if value.get("error").is_some() {
        return Err("The Provider returned a stream error".to_owned());
    }
    let content = value.pointer("/choices/0/delta/content");
    if let Some(text) = content
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
    {
        return Ok(Some(text.to_owned()));
    }
    if let Some(parts) = content.and_then(Value::as_array) {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<String>();
        if !text.is_empty() {
            return Ok(Some(text));
        }
    }
    Ok(value
        .pointer("/choices/0/text")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned))
}

pub(crate) fn emit_model_test_frames<R: Runtime>(
    app: &AppHandle<R>,
    request_id: &str,
    started: Instant,
    frames: Vec<String>,
    content: &mut String,
    first_token_ms: &mut Option<u64>,
    output_budget: &mut ModelTestOutputBudget,
) -> Result<(), String> {
    for frame in frames {
        let Some(delta) = model_test_stream_delta(&frame)? else {
            continue;
        };
        output_budget.accept(&delta)?;
        if first_token_ms.is_none() {
            *first_token_ms =
                Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
        }
        content.push_str(&delta);
        app.emit(
            MODEL_TEST_STREAM_EVENT,
            ModelTestStreamEvent {
                request_id: request_id.to_owned(),
                delta,
                first_token_ms: *first_token_ms,
            },
        )
        .map_err(|_| "The model stream could not reach the test console".to_owned())?;
    }
    Ok(())
}

pub(crate) fn validate_model_test_messages(messages: &[ModelTestMessage]) -> Result<(), String> {
    if messages.is_empty() {
        return Err("Enter a message before sending".to_owned());
    }
    if messages.len() > MODEL_TEST_MAX_MESSAGES {
        return Err(format!(
            "A model test supports at most {MODEL_TEST_MAX_MESSAGES} messages"
        ));
    }

    let mut total_bytes = 0usize;
    for (index, message) in messages.iter().enumerate() {
        if !matches!(message.role.as_str(), "user" | "assistant") {
            return Err("Model test messages support only user and assistant roles".to_owned());
        }
        let expected_role = if index % 2 == 0 { "user" } else { "assistant" };
        if message.role != expected_role {
            return Err("Model test messages must alternate user and assistant roles".to_owned());
        }
        let message_bytes = message.content.len();
        if message_bytes == 0 || message.content.trim().is_empty() {
            return Err("Model test messages cannot be empty".to_owned());
        }
        if message_bytes > MODEL_TEST_MAX_MESSAGE_BYTES {
            return Err(format!(
                "One model test message exceeds {MODEL_TEST_MAX_MESSAGE_BYTES} bytes"
            ));
        }
        total_bytes = total_bytes.saturating_add(message_bytes);
        if total_bytes > MODEL_TEST_MAX_TOTAL_BYTES {
            return Err(format!(
                "The model test conversation exceeds {MODEL_TEST_MAX_TOTAL_BYTES} bytes"
            ));
        }
    }
    if messages.last().map(|message| message.role.as_str()) != Some("user") {
        return Err("The last model test message must be from the user".to_owned());
    }
    Ok(())
}

/// Pay-per-token providers report an exhausted wallet as HTTP 429 rather than
/// 402, where the retry advice of the rate-limit summary is wrong: waiting
/// never helps an empty account. Classify from the error body's stable fields
/// without ever echoing its text into the summary the console shows.
pub(crate) fn model_test_error_is_exhausted_balance(body: &Value) -> bool {
    let error = body.get("error").unwrap_or(body);
    if error.get("type").and_then(Value::as_str) == Some("insufficient_quota") {
        return true;
    }
    if ["code", "internal_code"].iter().any(|field| {
        error
            .get(*field)
            .and_then(Value::as_str)
            .is_some_and(|code| {
                code.eq_ignore_ascii_case("credit_balance_exhausted")
                    || code.eq_ignore_ascii_case("platform_balance_insufficient")
            })
    }) {
        return true;
    }
    error
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|message| {
            let lowered = message.to_lowercase();
            lowered.contains("insufficient balance")
                || lowered.contains("balance is insufficient")
                || message.contains("余额不足")
        })
}

pub(crate) fn model_test_error_is_local_concurrency(body: &Value) -> bool {
    body.get("error")
        .unwrap_or(body)
        .get("code")
        .and_then(Value::as_str)
        == Some("concurrency_limit")
}

pub(crate) fn model_test_http_error(status: u16, body: &Value) -> String {
    let summary = if status == 429 && model_test_error_is_local_concurrency(body) {
        "The Token Station concurrency limit is active. Try again shortly"
    } else if status == 429 && model_test_error_is_exhausted_balance(body) {
        "The Provider account has no available balance"
    } else {
        match status {
            400 => "The model rejected the request. Check the prompt and model limits",
            401 | 403 => "Provider authentication failed. Check this Provider credential",
            402 => "The Provider account has no available balance",
            404 => "The selected model or endpoint is unavailable",
            408 | 504 => "The model request timed out",
            409 => "The Provider rejected the current request state",
            429 => "The Provider rate limit is active. Try again later",
            500..=599 => "The Provider is temporarily unavailable",
            _ => "The model request failed",
        }
    };
    format!("{summary} (HTTP {status})")
}

pub(crate) fn model_test_assistant_content(body: &Value) -> Option<String> {
    let content = body.pointer("/choices/0/message/content")?;
    if let Some(text) = content.as_str().filter(|text| !text.trim().is_empty()) {
        return Some(text.to_owned());
    }
    let parts = content
        .as_array()?
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!parts.trim().is_empty()).then_some(parts)
}

pub(crate) fn extract_model_test_reply(status: u16, body: &str) -> Result<String, String> {
    if body.len() > MODEL_TEST_MAX_STREAM_BYTES {
        return Err("The model response exceeded the wire response limit".to_owned());
    }
    let value: Value = serde_json::from_str(body)
        .map_err(|_| format!("The model returned invalid JSON (HTTP {status})"))?;
    if !(200..300).contains(&status) {
        return Err(model_test_http_error(status, &value));
    }
    let content = model_test_assistant_content(&value)
        .ok_or_else(|| "The model returned no assistant text".to_owned())?;
    if content.len() > MODEL_TEST_MAX_RESPONSE_BYTES {
        return Err("The model output exceeded the response limit".to_owned());
    }
    Ok(content)
}

#[tauri::command]
pub(crate) async fn test_model_chat_stream(
    app: AppHandle,
    state: State<'_, AppStateManaged>,
    stream_state: State<'_, ModelTestStreamState>,
    messages: Vec<ModelTestMessage>,
    request_id: String,
) -> Result<ModelTestReply, String> {
    run_model_test_chat(
        app,
        state.inner(),
        stream_state.inner(),
        messages,
        request_id,
    )
    .await
}

pub(crate) async fn run_model_test_chat<R: Runtime>(
    app: AppHandle<R>,
    state: &AppStateManaged,
    stream_state: &ModelTestStreamState,
    messages: Vec<ModelTestMessage>,
    request_id: String,
) -> Result<ModelTestReply, String> {
    validate_model_test_messages(&messages)?;
    validate_model_test_request_id(&request_id)?;
    let gateway_source = model_test_gateway_source(&state.0.lock().unwrap())?;

    let registry = Arc::clone(&stream_state.0);
    {
        let mut streams = registry.lock().unwrap();
        streams.register(&request_id, gateway_source.request_context().token())?;
    }
    let registration = ModelTestStreamRegistration {
        registry,
        request_id: request_id.clone(),
    };
    let gateway_cache = Arc::clone(&stream_state.1);

    let provider_runtime = tokio::runtime::Handle::current();
    tauri::async_runtime::spawn_blocking(move || {
        let _registration = registration;
        let (gateway, running_revision, request_owner) = match gateway_source {
            ModelTestGatewaySource::Running {
                gateway,
                running_revision,
                request,
            } => (
                gateway,
                Some(running_revision),
                ModelTestRequestOwner::Running(request),
            ),
            ModelTestGatewaySource::Draft {
                config,
                upstream_epochs,
                request,
            } => {
                let secret_store_fingerprint = model_test_secret_store_fingerprint(&config)?;
                let plugin_identity_fingerprint = model_test_plugin_identity_fingerprint(&config)?;
                let mut cached = gateway_cache.lock().unwrap();
                let gateway = if let Some(cached) = cached.as_ref().filter(|cached| {
                    home_gateway_identities_match(
                        &cached.config,
                        &cached.upstream_epochs,
                        &config,
                        &upstream_epochs,
                    ) && cached.secret_store_fingerprint == secret_store_fingerprint
                        && cached.plugin_identity_fingerprint == plugin_identity_fingerprint
                }) {
                    let gateway = Arc::clone(&cached.gateway);
                    ensure_model_test_plugin_identity_unchanged(
                        &config,
                        plugin_identity_fingerprint,
                    )?;
                    gateway
                } else {
                    let recorder = Arc::new(token_station_cli::filelog::Recorders(Vec::new()));
                    let gateway = Arc::new(Gateway::new_with_provider_runtime(
                        &config,
                        recorder,
                        provider_runtime,
                    )?);
                    ensure_model_test_plugin_identity_unchanged(
                        &config,
                        plugin_identity_fingerprint,
                    )?;
                    *cached = Some(CachedModelTestGateway {
                        config,
                        upstream_epochs,
                        secret_store_fingerprint,
                        plugin_identity_fingerprint,
                        gateway: Arc::clone(&gateway),
                    });
                    gateway
                };
                (gateway, None, ModelTestRequestOwner::Draft(request))
            }
        };
        let request_context = request_owner.context();
        let body = serde_json::to_vec(&json!({
            "model": "auto",
            "messages": messages,
            "stream": true,
            "max_tokens": 1024
        }))
        .map_err(|_| "Failed to encode the model test request".to_owned())?;
        let started = Instant::now();
        let mut json_response = None;
        let mut decoder = ModelTestSseDecoder::default();
        let mut content = String::new();
        let mut first_token_ms = None;
        let mut output_budget = ModelTestOutputBudget::default();
        let mut stream_error = None;
        gateway.chat_scoped_without_body_log(
            request_context,
            None,
            running_revision,
            "POST",
            "/v1/chat/completions",
            &[("content-type".to_owned(), "application/json".to_owned())],
            &body,
            &mut |reply| {
                if request_context.is_cancelled() {
                    return false;
                }
                match reply {
                    Reply::BeginJson(reply) => {
                        json_response = Some((reply.status, reply.body));
                    }
                    Reply::BeginStream => {}
                    Reply::Chunk(chunk) => match output_budget
                        .accept_wire(chunk.len())
                        .and_then(|()| decoder.push(chunk.as_bytes()))
                        .and_then(|frames| {
                            emit_model_test_frames(
                                &app,
                                &request_id,
                                started,
                                frames,
                                &mut content,
                                &mut first_token_ms,
                                &mut output_budget,
                            )
                        }) {
                        Ok(()) => {}
                        Err(error) => {
                            stream_error = Some(error);
                            request_context.cancel();
                            return false;
                        }
                    },
                }
                true
            },
        );
        let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        if let Some(error) = stream_error {
            return Err(error);
        }
        if let Some(reason) = request_context.cancel_reason() {
            return Err(match reason {
                CancelReason::Deadline => "The model request timed out".to_owned(),
                CancelReason::ClientDisconnect | CancelReason::ServerDrain => {
                    "Model test cancelled".to_owned()
                }
            });
        }
        if let Some((status, body)) = json_response {
            let content = extract_model_test_reply(status, &body)?;
            return Ok(ModelTestReply {
                content,
                first_token_ms: latency_ms,
                latency_ms,
            });
        }
        let frames = decoder.finish()?;
        emit_model_test_frames(
            &app,
            &request_id,
            started,
            frames,
            &mut content,
            &mut first_token_ms,
            &mut output_budget,
        )?;
        if content.trim().is_empty() {
            return Err("The model returned no assistant text".to_owned());
        }
        Ok(ModelTestReply {
            content,
            first_token_ms: first_token_ms.unwrap_or(latency_ms),
            latency_ms,
        })
    })
    .await
    .map_err(|error| format!("Model test task stopped unexpectedly: {error}"))?
}

#[tauri::command]
pub(crate) fn cancel_model_test_chat(
    stream_state: State<'_, ModelTestStreamState>,
    request_id: String,
) -> Result<(), String> {
    validate_model_test_request_id(&request_id)?;
    let mut streams = stream_state.0.lock().unwrap();
    streams.cancel(request_id);
    Ok(())
}
