//! Explicit, bounded image checks against one configured Provider offering.
use crate::*;
use base64::Engine;
use sha2::{Digest, Sha256};

const SOURCE: &str = "x-token-station-vision-source";
const TESTED_AT: &str = "x-token-station-vision-tested-at-ms";
const COLORS: [(&str, [u8; 3]); 4] = [
    ("red", [240, 20, 20]),
    ("green", [20, 180, 20]),
    ("blue", [20, 20, 240]),
    ("yellow", [240, 240, 20]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VisionOutcome {
    Verified,
    Unsupported,
    Blocked,
    Inconclusive,
}

#[derive(Serialize)]
pub(crate) struct VisionVerificationView {
    #[serde(flatten)]
    report: VisionReport,
    state: StateView,
}

struct Challenge {
    url: String,
    expected: Vec<&'static str>,
}

fn challenge(random: [u8; 9]) -> Result<Challenge, String> {
    let cells = random.map(|byte| usize::from(byte % 4));
    let mut pixels = Vec::with_capacity(192 * 192 * 3);
    for y in 0..192 {
        for x in 0..192 {
            pixels.extend_from_slice(&COLORS[cells[(y / 64) * 3 + x / 64]].1);
        }
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, 192, 192);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|_| "Cannot encode the test image")?;
        writer
            .write_image_data(&pixels)
            .map_err(|_| "Cannot encode the test image")?;
    }
    Ok(Challenge {
        url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ),
        expected: cells.iter().map(|index| COLORS[*index].0).collect(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VisionFailureReason {
    Authorization,
    RateLimit,
    Timeout,
    ModelUnavailable,
    InvalidRequest,
    ServiceUnavailable,
    Protocol,
    InvalidResponse,
    NoResponse,
    RequestFailed,
}

#[derive(Debug, Serialize)]
struct VisionReport {
    outcome: VisionOutcome,
    reason: Option<VisionFailureReason>,
    http_status: Option<u16>,
    detail: String,
}

impl VisionReport {
    fn new(
        outcome: VisionOutcome,
        reason: Option<VisionFailureReason>,
        http_status: Option<u16>,
        detail: &str,
    ) -> Self {
        Self {
            outcome,
            reason,
            http_status,
            detail: detail.into(),
        }
    }
}

// Display only bounded error fields, never arbitrary reply bodies or request content.
fn safe_error_detail(value: &Value, secrets: &[&str]) -> String {
    let error = value.get("error").unwrap_or(value);
    let fields: Vec<_> = ["code", "type", "message"]
        .iter()
        .filter_map(|key| error.get(key).and_then(Value::as_str))
        .collect();
    let mut distinct = Vec::new();
    for field in fields {
        if !distinct.contains(&field) {
            distinct.push(field);
        }
    }
    let mut detail = distinct.join(" · ");
    for secret in secrets.iter().filter(|secret| !secret.is_empty()) {
        detail = detail.replace(secret, "[REDACTED]");
    }
    static PRIVATE_CONTENT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?i)(?:https?://|data:)[^\s"<>]+|[a-z0-9+/=_-]{80,}"#).unwrap()
    });
    detail = PRIVATE_CONTENT
        .replace_all(&detail, "[REDACTED]")
        .into_owned();
    crate::recovery::redact_and_bound(&detail, 600)
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

#[cfg(test)]
fn evaluate(status: u16, body: &str, expected: &[&str]) -> VisionReport {
    evaluate_with_secrets(status, body, expected, &[])
}

fn evaluate_with_secrets(
    status: u16,
    body: &str,
    expected: &[&str],
    secrets: &[&str],
) -> VisionReport {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if status == 400
        && matches!(
            value["error"]["code"].as_str(),
            Some("capability" | "unsupported_capability")
        )
        && value["error"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("channel rejected image input"))
    {
        return VisionReport::new(VisionOutcome::Unsupported, None, Some(status), "");
    }
    if status != 200 || !value["error"].is_null() {
        let detail = safe_error_detail(&value, secrets);
        let lower = detail.to_ascii_lowercase();
        let reason = match status {
            401 | 403 => VisionFailureReason::Authorization,
            429 => VisionFailureReason::RateLimit,
            408 | 504 => VisionFailureReason::Timeout,
            404 => VisionFailureReason::ModelUnavailable,
            _ if lower.contains("model_not_found")
                || lower.contains("invalidmodel")
                || lower.contains("model does not exist") =>
            {
                VisionFailureReason::ModelUnavailable
            }
            _ if lower.contains("bounded chat attempt")
                || lower.contains("protocol")
                || lower.contains("api dialect") =>
            {
                VisionFailureReason::Protocol
            }
            400 | 413 | 415 | 422 => VisionFailureReason::InvalidRequest,
            500..=599 => VisionFailureReason::ServiceUnavailable,
            _ => VisionFailureReason::RequestFailed,
        };
        return VisionReport::new(VisionOutcome::Blocked, Some(reason), Some(status), &detail);
    }
    if value["choices"][0]["message"]["content"].as_str().is_none()
        && !value["output"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["type"] == "message"
                    && item["role"] == "assistant"
                    && item["content"].as_array().is_some_and(|parts| {
                        parts
                            .iter()
                            .any(|part| part["type"] == "output_text" && part["text"].is_string())
                    })
            })
        })
    {
        return VisionReport::new(
            VisionOutcome::Blocked,
            Some(VisionFailureReason::InvalidResponse),
            Some(status),
            "",
        );
    }
    let answer = value["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            value["output"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| item["type"] == "message" && item["role"] == "assistant")
                .flat_map(|item| item["content"].as_array().into_iter().flatten())
                .filter(|item| item["type"] == "output_text")
                .filter_map(|item| item["text"].as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .to_ascii_lowercase();
    let actual: Vec<_> = answer
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|word| !word.is_empty())
        .collect();
    if !expected.is_empty() && actual == expected {
        VisionReport::new(VisionOutcome::Verified, None, Some(status), "")
    } else {
        VisionReport::new(VisionOutcome::Inconclusive, None, Some(status), "")
    }
}

fn probe_config(draft: &Value, name: &str, model: &str) -> Result<ClientConfig, String> {
    let mut config = draft.clone();
    let mut upstream = config["upstreams"]
        .get(name)
        .cloned()
        .ok_or("Provider does not exist")?;
    let mut capability = upstream["models"]
        .as_array()
        .and_then(|models| {
            models
                .iter()
                .find(|item| item["model"].as_str() == Some(model))
        })
        .cloned()
        .ok_or("The model is not configured for this Provider")?;
    // The isolated check bypasses only the declaration gate. It does not save a claim.
    capability["vision"] = json!(true);
    capability["vision_state"] = json!("declared");
    upstream["models"] = json!([capability]);
    config["upstreams"] = json!({name: upstream});
    let target = json!({"upstream": name, "model": model});
    config["routing"] = json!({"mode": "direct", "direct_target": target});
    config["router"] = json!({"version":1,"routing_mode":"tiered","pools":{"probe":[target]},"rules":[],"hint_routes":[],"default_pool":"probe","assumed_context_window":8192});
    config["agent_routes"] = json!({});
    config["profiles"] = json!({});
    config["plugins"]["agents"] = if config["upstreams"][name]["api_dialect"] == "responses-native"
    {
        json!(["agent-openai-responses"])
    } else {
        json!(["agent-openai"])
    };
    config["data"]["metrics"] = json!(false);
    config["data"]["request_body_capture"] = json!(false);
    let config: ClientConfig = serde_json::from_value(config)
        .map_err(|_| "Cannot prepare the image check configuration")?;
    config
        .validate()
        .map_err(|_| "The image check configuration is invalid")?;
    Ok(config)
}

// Diagnostic copies come from the same immutable snapshot injected into Gateway.
struct ProbeCredentials {
    values: Vec<Zeroizing<String>>,
}

impl ProbeCredentials {
    fn capture(config: &ClientConfig, name: &str) -> Result<Self, String> {
        let store = secrets::SecretStore::from_config(config, &config.data.dir);
        Self::from_resolver(
            config,
            name,
            |owner, slot| store.resolve(owner, slot),
            |slot| store.resolve_egress(slot),
        )
    }

    fn from_snapshot(
        config: &ClientConfig,
        name: &str,
        snapshot: &secrets::SecretSnapshot,
    ) -> Result<Self, String> {
        Self::from_resolver(
            config,
            name,
            |owner, slot| snapshot.resolve(owner, slot),
            |slot| snapshot.resolve_egress(slot),
        )
    }

    fn from_resolver(
        config: &ClientConfig,
        name: &str,
        provider: impl FnOnce(&str, &str) -> Result<String, String>,
        egress: impl FnOnce(&str) -> Result<String, String>,
    ) -> Result<Self, String> {
        let mut values = Vec::new();
        if let Some(auth) = config.upstreams[name].auth.as_ref() {
            values.push(Zeroizing::new(
                provider(name, &auth.slot).map_err(|_| "Cannot read the Provider credential")?,
            ));
        }
        if let Some(auth) = &config.egress.auth {
            values.push(Zeroizing::new(
                egress(&auth.credential.slot).map_err(|_| "Cannot read the egress credential")?,
            ));
        }
        Ok(Self { values })
    }

    fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        for value in &self.values {
            hash.update((value.len() as u64).to_be_bytes());
            hash.update(value.as_bytes());
        }
        hash.finalize().into()
    }
}

fn credential_fingerprint(config: &ClientConfig, name: &str) -> Result<[u8; 32], String> {
    Ok(ProbeCredentials::capture(config, name)?.fingerprint())
}

fn probe_plugin_identity(config: &ClientConfig) -> Result<[u8; 32], String> {
    // The registry resolves the effective Provider and inbound packages. Its digest
    // includes package bytes and trust receipts, not only their configured paths.
    crate::model_test::model_test_plugin_identity_fingerprint(config)
        .map_err(|_| "Cannot read the image check plugin identity".to_owned())
}

struct ProbeIdentity {
    credentials: [u8; 32],
    plugins: [u8; 32],
}

impl ProbeIdentity {
    fn capture(config: &ClientConfig, credentials: &ProbeCredentials) -> Result<Self, String> {
        Ok(Self {
            credentials: credentials.fingerprint(),
            plugins: probe_plugin_identity(config)?,
        })
    }

    fn save_evidence(
        &self,
        inner: &mut AppInner,
        config: &ClientConfig,
        name: &str,
        model: &str,
        expected: &ProviderDiscoveryTarget,
        outcome: VisionOutcome,
    ) -> Result<(), String> {
        // Run under the configuration lock after the blocking task has returned.
        // Provider epochs in save_evidence also detect application-managed key changes.
        self.ensure_unchanged(config, name)?;
        save_evidence(inner, name, model, expected, outcome)
    }

    fn during<T>(
        &self,
        config: &ClientConfig,
        name: &str,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.ensure_unchanged(config, name)?;
        let result = operation()?;
        self.ensure_unchanged(config, name)?;
        Ok(result)
    }

    fn ensure_unchanged(&self, config: &ClientConfig, name: &str) -> Result<(), String> {
        if self.credentials != credential_fingerprint(config, name)? {
            return Err(
                "The credential changed during verification. Run verification again.".into(),
            );
        }
        if self.plugins != probe_plugin_identity(config)? {
            return Err("The plugin changed during verification. Run verification again.".into());
        }
        Ok(())
    }
}

fn run_probe(
    config: &ClientConfig,
    name: &str,
    runtime: tokio::runtime::Handle,
) -> Result<(VisionReport, ProbeIdentity), String> {
    run_probe_with_before_send(config, name, runtime, || {})
}

fn run_probe_with_before_send(
    config: &ClientConfig,
    name: &str,
    runtime: tokio::runtime::Handle,
    before_send: impl FnOnce(),
) -> Result<(VisionReport, ProbeIdentity), String> {
    // Retain these values until diagnostics have been redacted. A later read can
    // refer to a rotated key, even when the post-request identity check passed.
    let snapshot = secrets::SecretStore::snapshot_for_config(config, &config.data.dir)
        .map_err(|_| "Cannot capture the image check credentials".to_owned())?;
    let credentials = ProbeCredentials::from_snapshot(config, name, &snapshot)?;
    let identity = ProbeIdentity::capture(config, &credentials)?;
    let mut random = [0; 9];
    getrandom::fill(&mut random).map_err(|_| "Cannot generate an image challenge")?;
    let challenge = challenge(random)?;
    let gateway = identity.during(config, name, || {
        Gateway::new_with_provider_runtime(
            config,
            Arc::new(token_station_cli::filelog::Recorders(Vec::new())),
            runtime,
        )
        .map(|gateway| gateway.with_secret_snapshot(snapshot))
        .map_err(|_| {
            "Cannot start the image check. Check the Provider adapter and protocol.".to_owned()
        })
    })?;
    let prompt = "Identify the colors of this 3 by 3 grid. Read each row from left to right, top row first. Return exactly nine comma-separated lowercase color names. Use only red, green, blue, yellow. No explanation.";
    let (path, request) = if config.upstreams[name].api_dialect
        == token_station_cli::config::ApiDialect::ResponsesNative
    {
        (
            "/v1/responses",
            json!({"model":"auto","stream":false,"max_output_tokens":1024,
            "input":[{"type":"message","role":"user","content":[
                {"type":"input_text","text":prompt}, {"type":"input_image","image_url":challenge.url}
            ]}]}),
        )
    } else {
        (
            "/v1/chat/completions",
            json!({"model":"auto","stream":false,"max_tokens":1024,
            "messages":[{"role":"user","content":[
                {"type":"text","text":prompt}, {"type":"image_url","image_url":{"url":challenge.url}}
            ]}]}),
        )
    };
    let body = serde_json::to_vec(&request).map_err(|_| "Cannot encode the image check")?;
    let context = RequestContext::detached(Duration::from_secs(60), Duration::from_secs(45));
    context.enable_error_diagnostics();
    let mut response = None;
    before_send();
    gateway.chat_scoped_without_body_log(
        &context,
        None,
        None,
        "POST",
        path,
        &[("content-type".to_owned(), "application/json".to_owned())],
        &body,
        &mut |reply| {
            if let Reply::BeginJson(reply) = reply {
                response = Some((
                    reply.status,
                    if reply.body.len() <= 65536 {
                        reply.body
                    } else {
                        String::new()
                    },
                ));
            }
            true
        },
    );
    identity.ensure_unchanged(config, name)?;
    let report = finish_probe(
        config,
        name,
        &credentials,
        response,
        &context,
        &challenge.expected,
        || {},
    )?;
    Ok((report, identity))
}

fn finish_probe(
    config: &ClientConfig,
    name: &str,
    credentials: &ProbeCredentials,
    response: Option<(u16, String)>,
    context: &RequestContext,
    expected: &[&str],
    after_credential_check: impl FnOnce(),
) -> Result<VisionReport, String> {
    if credentials.fingerprint() != credential_fingerprint(config, name)? {
        return Err("The credential changed during verification. Run verification again.".into());
    }
    after_credential_check();
    let secrets: Vec<&str> = credentials.values.iter().map(|key| key.as_str()).collect();
    let mut report = response.map_or_else(
        || {
            VisionReport::new(
                VisionOutcome::Blocked,
                Some(VisionFailureReason::NoResponse),
                None,
                "",
            )
        },
        |(status, body)| evaluate_with_secrets(status, &body, expected, &secrets),
    );
    if report.outcome == VisionOutcome::Blocked {
        if let Some((status, body)) = context.take_error_diagnostic() {
            let diagnostic =
                evaluate_with_secrets(status, &String::from_utf8_lossy(&body), &[], &secrets);
            if diagnostic.outcome == VisionOutcome::Blocked {
                report.http_status = diagnostic.http_status;
                if !diagnostic.detail.is_empty() {
                    report.reason = diagnostic.reason;
                    report.detail = diagnostic.detail;
                }
            }
        }
    }
    Ok(report)
}

pub(crate) fn invalidate_probe_evidence(upstream: &mut Value) {
    for model in upstream["models"].as_array_mut().into_iter().flatten() {
        if model[SOURCE].as_str() == Some("probe") {
            model["vision"] = json!(false);
            model["vision_state"] = json!("unknown");
            if let Some(object) = model.as_object_mut() {
                object.remove(SOURCE);
                object.remove(TESTED_AT);
            }
        }
    }
}

fn save_evidence(
    inner: &mut AppInner,
    name: &str,
    model: &str,
    expected: &ProviderDiscoveryTarget,
    outcome: VisionOutcome,
) -> Result<(), String> {
    ensure_provider_discovery_target_unchanged(inner, name, expected).map_err(|_| {
        "The Provider changed during verification. Run verification again.".to_owned()
    })?;
    ensure_capability_change_ready(inner)?;
    if !matches!(
        outcome,
        VisionOutcome::Verified | VisionOutcome::Unsupported
    ) {
        return Ok(());
    }
    let previous = inner.draft.clone();
    let previous_state = inner.config_state.clone();
    let capability = inner.draft["upstreams"][name]["models"]
        .as_array_mut()
        .and_then(|models| {
            models
                .iter_mut()
                .find(|item| item["model"].as_str() == Some(model))
        })
        .ok_or("The model was removed during verification")?;
    capability["vision"] = json!(outcome == VisionOutcome::Verified);
    capability["vision_state"] = json!(if outcome == VisionOutcome::Verified {
        "verified"
    } else {
        "unsupported"
    });
    capability[SOURCE] = json!("probe");
    capability[TESTED_AT] = json!(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64);
    if let Err(error) = inner.observe_draft().and_then(|()| inner.save_draft()) {
        inner.draft = previous;
        inner.config_state = previous_state;
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn verify_provider_model_vision<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppStateManaged>,
    name: String,
    model: String,
) -> Result<VisionVerificationView, String> {
    let name = name.trim().to_owned();
    let model = model.trim().to_owned();
    let (config, expected, transport) = {
        let mut inner = state.0.lock().unwrap();
        ensure_capability_change_ready(&inner)?;
        ensure_generic_provider_mutation_allowed(&inner, &name)?;
        if inner.pending_provider_discoveries.contains(&name)
            || inner.pending_provider_discoveries.len() >= 8
        {
            return Err("Wait for the current Provider check before starting another.".into());
        }
        if inner.pending_provider_keys.contains_key(&name) {
            return Err("Save the Provider before verifying vision.".into());
        }
        let config = probe_config(&inner.draft, &name, &model)?;
        let expected = begin_provider_discovery_target(&mut inner, &name);
        inner.pending_provider_discoveries.insert(name.clone());
        (
            config,
            expected,
            (
                inner.draft["egress"].clone(),
                inner.draft["plugins"].clone(),
            ),
        )
    };
    let _guard = ProviderDiscoveryGuard {
        inner: &state.0,
        provider: name.clone(),
    };
    let task_name = name.clone();
    let runtime = tokio::runtime::Handle::current();
    let task_config = config.clone();
    let (report, identity) =
        tauri::async_runtime::spawn_blocking(move || run_probe(&task_config, &task_name, runtime))
            .await
            .map_err(|_| "The image check stopped unexpectedly")??;
    {
        let mut inner = state.0.lock().unwrap();
        if transport
            != (
                inner.draft["egress"].clone(),
                inner.draft["plugins"].clone(),
            )
        {
            return Err(
                "The transport configuration changed during verification. Run verification again."
                    .into(),
            );
        }
        identity.save_evidence(
            &mut inner,
            &config,
            &name,
            &model,
            &expected,
            report.outcome,
        )?;
    }
    let snapshot = if matches!(
        report.outcome,
        VisionOutcome::Verified | VisionOutcome::Unsupported
    ) {
        apply_saved_capabilities(app, state.inner())?
    } else {
        state.0.lock().unwrap().snapshot()
    };
    Ok(VisionVerificationView {
        report,
        state: snapshot,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SecurityFixture {
        root: PathBuf,
        config: ClientConfig,
    }

    impl SecurityFixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "ts-vision-security-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::fs::create_dir_all(&root).unwrap();
            let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../plugins-dist");
            let plugins = root.join("plugins");
            // Use unreserved names so bundled adapters cannot shadow these file fixtures.
            for (source_package, package, wasm) in [
                ("agent-openai", "vision-security-agent", "adapter.wasm"),
                (
                    "provider-openai-compatible-v2",
                    "vision-security-provider",
                    "component.wasm",
                ),
            ] {
                let target = plugins.join(package);
                std::fs::create_dir_all(&target).unwrap();
                std::fs::copy(source.join(source_package).join(wasm), target.join(wasm)).unwrap();
                let mut manifest: Value = serde_json::from_slice(
                    &std::fs::read(source.join(source_package).join("manifest.json")).unwrap(),
                )
                .unwrap();
                manifest["name"] = json!(package);
                if wasm == "component.wasm" {
                    manifest["providers"] = json!(["vision-security-fixture"]);
                }
                std::fs::write(
                    target.join("manifest.json"),
                    serde_json::to_vec(&manifest).unwrap(),
                )
                .unwrap();
            }
            std::fs::write(root.join("credential"), "oldCredentialM4x9").unwrap();
            let mut draft = template(&root.join("data"), &plugins);
            draft["plugins"]["providers"] =
                json!({"vision-security-fixture": "vision-security-provider"});
            draft["upstreams"]["p"] = json!({
                "provider": "vision-security-fixture", "base_url": "https://example.test/v1",
                "auth": { "slot": "provider_api_key", "file": root.join("credential") },
                "models": [{"model": "m", "vision_state": "unknown"}]
            });
            let mut config = probe_config(&draft, "p", "m").unwrap();
            config.plugins.agents = vec!["vision-security-agent".to_owned()];
            Self { root, config }
        }

        fn replace_plugin_bytes(&self, package: &str, artifact: &str) {
            let path = self.config.plugins.dir.join(package).join(artifact);
            let mut bytes = std::fs::read(&path).unwrap();
            // Append a valid Wasm custom section while retaining the exact file path.
            bytes.extend_from_slice(b"\0\x02\x01x");
            std::fs::write(path, bytes).unwrap();
        }

        fn assert_evidence_rejected(&self, identity: &ProbeIdentity, reason: &str) {
            let mut draft = serde_json::to_value(&self.config).unwrap();
            draft["upstreams"]["p"]["models"][0]["vision"] = json!(false);
            draft["upstreams"]["p"]["models"][0]["vision_state"] = json!("unknown");
            let mut inner = AppInner::new(self.root.join("config.json"), draft, None);
            let expected = begin_provider_discovery_target(&mut inner, "p");
            let error = identity
                .save_evidence(
                    &mut inner,
                    &self.config,
                    "p",
                    "m",
                    &expected,
                    VisionOutcome::Verified,
                )
                .unwrap_err();
            assert!(error.contains(reason));
            assert_eq!(
                inner.draft["upstreams"]["p"]["models"][0]["vision_state"],
                "unknown"
            );
            assert!(inner.draft["upstreams"]["p"]["models"][0]
                .get(SOURCE)
                .is_none());
        }

        fn rotate_credential(&self) {
            std::fs::write(self.root.join("credential"), "newCredentialV7z2").unwrap();
        }
    }

    impl Drop for SecurityFixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[test]
    fn security_rotation_after_validation_does_not_leak_request_credential() {
        let fixture = SecurityFixture::new();
        let credentials = ProbeCredentials::capture(&fixture.config, "p").unwrap();
        let context = RequestContext::detached(Duration::from_secs(60), Duration::from_secs(45));
        let report = finish_probe(
            &fixture.config,
            "p",
            &credentials,
            Some((
                400,
                json!({"error": {"message": "Rejected oldCredentialM4x9"}}).to_string(),
            )),
            &context,
            &[],
            || fixture.rotate_credential(),
        )
        .unwrap();
        assert!(
            !report.detail.contains("oldCredentialM4x9"),
            "Request credential escaped redaction"
        );
        assert!(report.detail.contains("[REDACTED]"));
    }

    #[test]
    fn security_same_path_plugin_change_rejects_probe_evidence() {
        let fixture = SecurityFixture::new();
        let credentials = ProbeCredentials::capture(&fixture.config, "p").unwrap();
        let identity = ProbeIdentity::capture(&fixture.config, &credentials).unwrap();
        identity.ensure_unchanged(&fixture.config, "p").unwrap();
        fixture.replace_plugin_bytes("vision-security-provider", "component.wasm");
        fixture.assert_evidence_rejected(&identity, "plugin changed");
    }

    #[test]
    fn security_redaction_does_not_reread_a_removed_credential() {
        let fixture = SecurityFixture::new();
        let credentials = ProbeCredentials::capture(&fixture.config, "p").unwrap();
        let report = finish_probe(
            &fixture.config,
            "p",
            &credentials,
            Some((
                400,
                json!({"error": {"message": "Rejected oldCredentialM4x9"}}).to_string(),
            )),
            &RequestContext::detached(Duration::from_secs(60), Duration::from_secs(45)),
            &[],
            || {
                fixture.rotate_credential();
                // Deleting the source also proves redaction does not read it again.
                std::fs::remove_file(fixture.root.join("credential")).unwrap();
            },
        )
        .unwrap();
        assert_eq!(report.detail, "Rejected [REDACTED]");
    }

    #[test]
    fn security_credential_rotation_before_persist_rejects_evidence() {
        let fixture = SecurityFixture::new();
        let credentials = ProbeCredentials::capture(&fixture.config, "p").unwrap();
        let identity = ProbeIdentity::capture(&fixture.config, &credentials).unwrap();
        fixture.rotate_credential();
        fixture.assert_evidence_rejected(&identity, "credential changed");
    }

    #[test]
    fn security_plugin_change_during_gateway_preparation_rejects_evidence() {
        let fixture = SecurityFixture::new();
        let credentials = ProbeCredentials::capture(&fixture.config, "p").unwrap();
        let identity = ProbeIdentity::capture(&fixture.config, &credentials).unwrap();
        let result = identity.during(&fixture.config, "p", || {
            fixture.replace_plugin_bytes("vision-security-agent", "adapter.wasm");
            Ok(())
        });
        assert!(result.unwrap_err().contains("plugin changed"));
    }

    fn verify_wire_aba_uses_snapshot(responses: bool) {
        use std::io::{Read, Write};
        let mut fixture = SecurityFixture::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let plugins = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../plugins-dist");
        let mut draft = template(&fixture.root.join("data"), &plugins);
        draft["upstreams"]["p"] = json!({
            "provider": "openai-compatible", "base_url": format!("http://{address}/v1"),
            "auth": {"slot": "provider_api_key", "file": fixture.root.join("credential")},
            "models": [{"model": "m", "vision_state": "unknown"}]
        });
        if responses {
            draft["upstreams"]["p"]["api_dialect"] = json!("responses-native");
        }
        fixture.config = probe_config(&draft, "p", "m").unwrap();
        let credential_path = fixture.root.join("credential");
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => panic!("Probe did not reach the loopback fixture"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let auth = loop {
                let mut chunk = [0; 4096];
                let read = stream.read(&mut chunk).unwrap();
                assert!(read > 0 && bytes.len() + read <= 65536);
                bytes.extend_from_slice(&chunk[..read]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let head = std::str::from_utf8(&bytes[..end]).unwrap();
                    let len = head
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + len {
                        let request: Value =
                            serde_json::from_slice(&bytes[end + 4..end + 4 + len]).unwrap();
                        assert!(request.to_string().contains("data:image/png;base64,"));
                        break head
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("authorization").then(|| {
                                    value.trim().strip_prefix("Bearer ").unwrap().to_owned()
                                })
                            })
                            .unwrap();
                    }
                }
            };
            // Restore A before the reply, so the probe's post-request checks see A.
            std::fs::write(credential_path, "oldCredentialM4x9").unwrap();
            let body = json!({"error": {"message": format!("Rejected {auth}")}}).to_string();
            write!(stream, "HTTP/1.1 401 Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            auth == "oldCredentialM4x9"
        });
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result =
            run_probe_with_before_send(&fixture.config, "p", runtime.handle().clone(), || {
                fixture.rotate_credential();
            });
        let sent_snapshot = server.join().unwrap();
        let (report, identity) = result.unwrap();
        identity.ensure_unchanged(&fixture.config, "p").unwrap();
        assert!(
            sent_snapshot,
            "Wire request used rotated B instead of captured A"
        );
        assert_eq!(report.outcome, VisionOutcome::Blocked);
        assert!(report.detail.contains("[REDACTED]"));
        assert!(!report.detail.contains("oldCredentialM4x9"));
        assert!(!report.detail.contains("newCredentialV7z2"));
    }

    #[test]
    fn security_wire_chat_aba_sends_only_the_captured_credential() {
        verify_wire_aba_uses_snapshot(false);
    }

    #[test]
    fn security_wire_responses_aba_sends_only_the_captured_credential() {
        verify_wire_aba_uses_snapshot(true);
    }

    #[test]
    fn blocked_verification_retains_status_and_reason() {
        let report = evaluate(
            404,
            r#"{"error":{"code":"model_not_found","message":"The model does not exist."}}"#,
            &[],
        );
        assert_eq!(report.outcome, VisionOutcome::Blocked);
        assert_eq!(report.http_status, Some(404));
        let detail = report.detail;
        assert!(
            detail.contains("model_not_found"),
            "Provider code was lost: {detail}"
        );
        assert!(detail.contains("The model does not exist"));
    }

    #[test]
    fn request_failures_do_not_prove_missing_vision() {
        for (status, body, reason) in [
            (401, "{}", VisionFailureReason::Authorization),
            (403, "{}", VisionFailureReason::Authorization),
            (429, "{}", VisionFailureReason::RateLimit),
            (504, "{}", VisionFailureReason::Timeout),
            (503, "{}", VisionFailureReason::ServiceUnavailable),
            (
                400,
                r#"{"error":{"message":"Invalid model: model does not exist"}}"#,
                VisionFailureReason::ModelUnavailable,
            ),
            (
                400,
                r#"{"error":{"message":"Cannot construct a bounded Chat attempt"}}"#,
                VisionFailureReason::Protocol,
            ),
            (
                400,
                r#"{"error":{"message":"Image format is invalid"}}"#,
                VisionFailureReason::InvalidRequest,
            ),
            (
                200,
                "<html>error</html>",
                VisionFailureReason::InvalidResponse,
            ),
            (
                200,
                r#"{"error":{"message":"Internal failure"}}"#,
                VisionFailureReason::RequestFailed,
            ),
        ] {
            let report = evaluate(status, body, &["red"]);
            assert_eq!(report.outcome, VisionOutcome::Blocked);
            assert_eq!(report.reason, Some(reason));
            assert_eq!(report.http_status, Some(status));
        }
    }

    #[test]
    fn error_details_remove_credentials_content_and_control_characters() {
        let secret = "unusual credential value";
        let value = json!({"error": {"code": "invalid_parameter", "message": format!("bad argument {secret} Bearer abcdef sk-example-secret https://user:pass@host/path?key=123 data:image/png;base64,abcdef \u{202e}\ncontent: private image"), "body": "private response"}});
        let detail = safe_error_detail(&value, &[secret]);
        assert!(detail.contains("invalid_parameter"));
        for hidden in [
            secret,
            "abcdef",
            "example-secret",
            "user:pass",
            "private image",
            "private response",
            "\u{202e}",
            "\n",
        ] {
            assert!(!detail.contains(hidden), "Unsafe diagnostic: {detail}");
        }
        let huge = json!({"message": "错误 ".repeat(1000)});
        assert!(safe_error_detail(&huge, &[]).len() <= 600);
        assert!(safe_error_detail(&json!({"body":"never show bodies"}), &[]).is_empty());
    }

    #[test]
    fn verification_requires_every_random_image_cell() {
        let check = challenge([0, 1, 2, 3, 0, 2, 1, 3, 2]).unwrap();
        let answer = |text| json!({"choices":[{"message":{"content":text}}]}).to_string();
        assert_eq!(
            evaluate(
                200,
                &answer("red,green,blue,yellow,red,blue,green,yellow,blue"),
                &check.expected
            )
            .outcome,
            VisionOutcome::Verified
        );
        for text in [
            "I can see the image",
            "red",
            "red,green,blue,yellow,red,blue,green,yellow,red",
        ] {
            assert_eq!(
                evaluate(200, &answer(text), &check.expected).outcome,
                VisionOutcome::Inconclusive
            );
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(check.url.split(',').nth(1).unwrap())
            .unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(decoded))
            .read_info()
            .unwrap();
        let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut bytes).unwrap();
        assert_eq!(&bytes[0..3], &COLORS[0].1);
        assert_eq!(&bytes[64 * 3..64 * 3 + 3], &COLORS[1].1);
        assert_ne!(check.url, challenge([3; 9]).unwrap().url);
    }

    #[test]
    fn only_explicit_channel_image_rejection_proves_unsupported() {
        let rejection = json!({"error":{"code":"capability","message":"The selected Provider channel rejected image input."}}).to_string();
        assert_eq!(
            evaluate(400, &rejection, &[]).outcome,
            VisionOutcome::Unsupported
        );
        for status in [401, 403, 429, 500, 504] {
            assert_eq!(
                evaluate(status, &rejection, &[]).outcome,
                VisionOutcome::Blocked
            );
        }
        assert_eq!(
            evaluate(400, r#"{"error":{"message":"Invalid model"}}"#, &[]).outcome,
            VisionOutcome::Blocked
        );
    }

    #[test]
    fn stale_evidence_cannot_change_a_reconfigured_provider() {
        let root = std::env::temp_dir().join(format!(
            "ts-vision-stale-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut draft = template(&root.join("data"), &root.join("plugins"));
        draft["upstreams"]["p"] = json!({"provider":"openai-compatible","base_url":"https://original.example/v1","models":[{"model":"m","vision_state":"unknown"}]});
        let mut inner = AppInner::new(root.join("config.json"), draft, None);
        let expected = begin_provider_discovery_target(&mut inner, "p");
        inner.draft["upstreams"]["p"]["base_url"] = json!("https://changed.example/v1");
        assert!(
            save_evidence(&mut inner, "p", "m", &expected, VisionOutcome::Verified)
                .unwrap_err()
                .contains("changed")
        );
        assert_eq!(
            inner.draft["upstreams"]["p"]["models"][0]["vision_state"],
            "unknown"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn probe_configuration_selects_only_the_requested_offering() {
        let mut draft = template(
            Path::new("/tmp/ts-probe-config"),
            Path::new("/tmp/ts-probe-plugins"),
        );
        draft["upstreams"]["p"] = json!({"provider":"openai-compatible","base_url":"https://p.example/v1","models":[{"model":"m","vision_state":"unknown"},{"model":"other"}]});
        draft["upstreams"]["other"] = draft["upstreams"]["p"].clone();
        let original = draft.clone();
        let config = probe_config(&draft, "p", "m").unwrap();
        assert_eq!(config.upstreams.len(), 1);
        assert_eq!(config.upstreams["p"].models.len(), 1);
        assert_eq!(config.upstreams["p"].models[0].model, "m");
        assert_eq!(
            draft, original,
            "a check must not pre-declare real capabilities"
        );
        assert!(config.agent_routes.is_empty());
        assert!(!config.data.request_body_capture);
    }

    #[test]
    fn changed_channel_invalidates_only_local_probe_evidence() {
        let mut upstream = json!({"models":[
            {"model":"a","vision":true,"vision_state":"verified",SOURCE:"probe",TESTED_AT:1},
            {"model":"b","vision":true,"vision_state":"declared",SOURCE:"operator"}
        ]});
        invalidate_probe_evidence(&mut upstream);
        assert_eq!(upstream["models"][0]["vision_state"], "unknown");
        assert!(upstream["models"][0].get(TESTED_AT).is_none());
        assert_eq!(upstream["models"][1]["vision_state"], "declared");
    }
}
