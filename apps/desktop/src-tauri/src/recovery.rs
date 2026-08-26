use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};
use token_station_cli::store::{self, SchemaCompatibility};
use token_station_metrics::SCHEMA_VERSION;

const MESSAGE_FIELD_LIMIT: usize = 2 * 1024;
pub(crate) const STACK_FIELD_LIMIT: usize = 8 * 1024;
const COMPONENT_FIELD_LIMIT: usize = 4 * 1024;
const MAX_LOG_BYTES: u64 = 256 * 1024;
const MAX_EVENTS: usize = 50;
static LOG_LOCK: Mutex<()> = Mutex::new(());

static SECRET_PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (
            Regex::new(r"(?i)\bbearer\s+[a-z0-9._~+/-]{3,}").expect("valid bearer regex"),
            "Bearer [REDACTED]",
        ),
        (
            Regex::new(
                r#"(?i)((?:api[_-]?key|access[_-]?token|token|password|secret|authorization)\s*[:=]\s*[\"']?)[^\"'\s,;}]{3,}"#,
            )
            .expect("valid named secret regex"),
            "$1[REDACTED]",
        ),
        (
            Regex::new(r"(?i)\b(?:sk|pk)[-_][a-z0-9_-]{4,}")
                .expect("valid prefixed secret regex"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"\bAIza[0-9A-Za-z_-]{12,}").expect("valid Google key regex"),
            "[REDACTED]",
        ),
        (
            Regex::new(r"(?i)(https?://[^:/\s]+:)[^@\s/]+@")
                .expect("valid URL credential regex"),
            "$1[REDACTED]@",
        ),
        (
            Regex::new(
                r#"(?is)(["']?(?:request[_-]?body|body|prompt|content|input|tool[_-]?input|toolinput|arguments|query|search[_-]?(?:term|query))["']?\s*[:=]\s*).*"#,
            )
            .expect("valid content-field regex"),
            "$1[REDACTED]",
        ),
    ]
});

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryMode {
    Normal,
    Safe,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RecoveryState {
    pub mode: RecoveryMode,
    pub reason_code: Option<String>,
    pub message: Option<String>,
    pub found_schema: Option<u32>,
    pub supported_schema: Option<u32>,
    pub metrics_path: String,
    pub backup_dir: String,
    pub local_only: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrontendDiagnosticInput {
    pub kind: String,
    pub message: String,
    pub stack: Option<String>,
    pub component_stack: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct FrontendDiagnosticRecord {
    pub timestamp_ms: u64,
    pub kind: String,
    pub message: String,
    pub stack: Option<String>,
    pub component_stack: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DiagnosticPreview {
    pub recovery: RecoveryState,
    pub frontend_events: Vec<FrontendDiagnosticRecord>,
    pub export_includes: Vec<String>,
    pub local_only: bool,
    pub redacted: bool,
    pub auto_upload: bool,
}

pub(crate) fn inspect_recovery_state(data_dir: &Path) -> RecoveryState {
    let metrics_path = data_dir.join("metrics.sqlite");
    match store::inspect_schema(&metrics_path) {
        Ok(compatibility) => state_from_compatibility(&metrics_path, compatibility),
        Err(error) => RecoveryState {
            mode: RecoveryMode::Safe,
            reason_code: Some("metrics_unreadable".to_string()),
            message: Some(redact_and_bound(&error, MESSAGE_FIELD_LIMIT)),
            found_schema: None,
            supported_schema: Some(SCHEMA_VERSION),
            metrics_path: metrics_path.display().to_string(),
            backup_dir: data_dir.display().to_string(),
            local_only: true,
        },
    }
}

fn state_from_compatibility(
    metrics_path: &Path,
    compatibility: SchemaCompatibility,
) -> RecoveryState {
    let data_dir = metrics_path.parent().unwrap_or_else(|| Path::new("."));
    let (mode, reason_code, message, found_schema, supported_schema) = match compatibility {
        SchemaCompatibility::Missing => (RecoveryMode::Normal, None, None, None, Some(SCHEMA_VERSION)),
        SchemaCompatibility::Current { version } => (
            RecoveryMode::Normal,
            None,
            None,
            Some(version),
            Some(version),
        ),
        SchemaCompatibility::Older { found, supported } => (
            RecoveryMode::Normal,
            None,
            None,
            Some(found),
            Some(supported),
        ),
        SchemaCompatibility::Newer { found, supported } => (
            RecoveryMode::Safe,
            Some("metrics_schema_newer".to_string()),
            Some(format!(
                "指标库 schema v{found} 高于当前程序支持的 v{supported}；已停止启动业务功能，未打开或迁移本地数据库。请更新应用后重试。"
            )),
            Some(found),
            Some(supported),
        ),
    };
    RecoveryState {
        mode,
        reason_code,
        message,
        found_schema,
        supported_schema,
        metrics_path: metrics_path.display().to_string(),
        backup_dir: data_dir.display().to_string(),
        local_only: true,
    }
}

pub(crate) fn diagnostic_preview(
    config_path: &Path,
    data_dir: &Path,
) -> Result<DiagnosticPreview, String> {
    let recovery = redact_recovery_for_diagnostics(inspect_recovery_state(data_dir), data_dir);
    let frontend_events = read_frontend_events(&diagnostic_log_path(data_dir))?;
    let mut export_includes = vec!["脱敏诊断清单".to_string()];
    if config_path.exists() {
        export_includes.push("原始本地配置（凭据应仅为 slot，但导出仍按敏感数据处理）".to_string());
    }
    if data_dir.join("metrics.sqlite").exists() {
        export_includes.push("原始本地指标库及 SQLite sidecar（如存在）".to_string());
    }
    Ok(DiagnosticPreview {
        recovery,
        frontend_events,
        export_includes,
        local_only: true,
        redacted: true,
        auto_upload: false,
    })
}

fn redact_recovery_for_diagnostics(mut recovery: RecoveryState, data_dir: &Path) -> RecoveryState {
    let raw_data_dir = data_dir.to_string_lossy();
    recovery.message = recovery.message.map(|message| {
        redact_and_bound(
            &message.replace(raw_data_dir.as_ref(), "$DATA_DIR"),
            MESSAGE_FIELD_LIMIT,
        )
    });
    recovery.metrics_path = "$DATA_DIR/metrics.sqlite".to_string();
    recovery.backup_dir = "$DATA_DIR".to_string();
    recovery
}

pub(crate) fn diagnostic_log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("diagnostics").join("frontend.jsonl")
}

pub(crate) fn append_frontend_event(
    log_path: &Path,
    input: FrontendDiagnosticInput,
) -> Result<FrontendDiagnosticRecord, String> {
    let _guard = LOG_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let record = FrontendDiagnosticRecord {
        timestamp_ms: now_ms(),
        kind: normalize_kind(&input.kind).to_string(),
        message: redact_and_bound(&input.message, MESSAGE_FIELD_LIMIT),
        stack: input
            .stack
            .as_deref()
            .map(|value| redact_and_bound(value, STACK_FIELD_LIMIT)),
        component_stack: input
            .component_stack
            .as_deref()
            .map(|value| redact_and_bound(value, COMPONENT_FIELD_LIMIT)),
    };
    let mut line = serde_json::to_vec(&record).map_err(|error| error.to_string())?;
    line.push(b'\n');
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        restrict_directory_permissions(parent)?;
    }
    if let Ok(metadata) = fs::symlink_metadata(log_path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("frontend diagnostic log is not a regular file".to_owned());
        }
    }
    let current_len = fs::metadata(log_path).map(|meta| meta.len()).unwrap_or(0);
    let truncate = current_len.saturating_add(line.len() as u64) > MAX_LOG_BYTES;
    let mut options = OpenOptions::new();
    options.create(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if truncate {
        options.truncate(true);
    } else {
        options.append(true);
    }
    let mut file = options
        .open(log_path)
        .map_err(|error| format!("{}: {error}", log_path.display()))?;
    file.write_all(&line)
        .map_err(|error| format!("{}: {error}", log_path.display()))?;
    restrict_file_permissions(log_path)?;
    Ok(record)
}

pub(crate) fn read_frontend_events(
    log_path: &Path,
) -> Result<Vec<FrontendDiagnosticRecord>, String> {
    if !log_path.exists() {
        return Ok(Vec::new());
    }
    let mut file =
        fs::File::open(log_path).map_err(|error| format!("{}: {error}", log_path.display()))?;
    let len = file.metadata().map_err(|error| error.to_string())?.len();
    let start = len.saturating_sub(MAX_LOG_BYTES);
    file.seek(SeekFrom::Start(start))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if start > 0 {
        if let Some(index) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=index);
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut events: Vec<FrontendDiagnosticRecord> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .map(second_redaction)
        .collect();
    if events.len() > MAX_EVENTS {
        events.drain(..events.len() - MAX_EVENTS);
    }
    Ok(events)
}

pub(crate) fn export_bundle(
    config_path: &Path,
    data_dir: &Path,
    confirmed: bool,
) -> Result<PathBuf, String> {
    if !confirmed {
        return Err("导出包含原始本地配置和指标数据，必须先明确确认".to_string());
    }
    let export_root = data_dir.join("recovery-exports");
    fs::create_dir_all(&export_root)
        .map_err(|error| format!("{}: {error}", export_root.display()))?;
    restrict_directory_permissions(&export_root)?;
    let mut export_dir = export_root.join(format!("recovery-export-{}", now_ms()));
    let mut suffix = 0_u16;
    while export_dir.exists() {
        suffix = suffix.saturating_add(1);
        export_dir = export_root.join(format!("recovery-export-{}-{suffix}", now_ms()));
    }
    fs::create_dir(&export_dir).map_err(|error| format!("{}: {error}", export_dir.display()))?;
    restrict_directory_permissions(&export_dir)?;

    let mut included = Vec::new();
    if config_path.exists() {
        copy_file(config_path, &export_dir.join("token-station.json"))?;
        included.push("token-station.json".to_string());
    }
    let metrics_source = data_dir.join("metrics.sqlite");
    if metrics_source.exists() {
        let metrics_target = export_dir.join("metrics.sqlite");
        if store::snapshot_database(&metrics_source, &metrics_target).is_ok() {
            restrict_file_permissions(&metrics_target)?;
            included.push("metrics.sqlite (SQLite online snapshot)".to_string());
        } else {
            // A corrupt or otherwise unopenable DB must still be recoverable
            // as raw bytes. Include the WAL pair so an offline repair tool has
            // the complete set.
            copy_file(&metrics_source, &metrics_target)?;
            included.push("metrics.sqlite (raw fallback)".to_string());
            for name in ["metrics.sqlite-wal", "metrics.sqlite-shm"] {
                let source = data_dir.join(name);
                if source.exists() {
                    copy_file(&source, &export_dir.join(name))?;
                    included.push(name.to_string());
                }
            }
        }
    }
    let events = read_frontend_events(&diagnostic_log_path(data_dir))?;
    let mut diagnostic_text = String::new();
    for event in events {
        diagnostic_text.push_str(&serde_json::to_string(&event).map_err(|e| e.to_string())?);
        diagnostic_text.push('\n');
    }
    fs::write(
        export_dir.join("frontend-diagnostics.jsonl"),
        diagnostic_text,
    )
    .map_err(|error| error.to_string())?;
    restrict_file_permissions(&export_dir.join("frontend-diagnostics.jsonl"))?;
    included.push("frontend-diagnostics.jsonl".to_string());
    let manifest = serde_json::json!({
        "version": 1,
        "created_at_ms": now_ms(),
        "local_only": true,
        "auto_upload": false,
        "contains_raw_local_data": true,
        "included": included,
    });
    fs::write(
        export_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    restrict_file_permissions(&export_dir.join("manifest.json"))?;
    Ok(export_dir)
}

fn second_redaction(mut record: FrontendDiagnosticRecord) -> FrontendDiagnosticRecord {
    record.kind = normalize_kind(&record.kind).to_string();
    record.message = redact_and_bound(&record.message, MESSAGE_FIELD_LIMIT);
    record.stack = record
        .stack
        .as_deref()
        .map(|value| redact_and_bound(value, STACK_FIELD_LIMIT));
    record.component_stack = record
        .component_stack
        .as_deref()
        .map(|value| redact_and_bound(value, COMPONENT_FIELD_LIMIT));
    record
}

fn normalize_kind(kind: &str) -> &'static str {
    match kind {
        "render_error" => "render_error",
        "window_error" => "window_error",
        "unhandled_rejection" => "unhandled_rejection",
        _ => "runtime_error",
    }
}

fn redact_and_bound(value: &str, limit: usize) -> String {
    let mut redacted = value.replace('\0', "�");
    if let Some(home) = std::env::var_os("HOME").and_then(|value| value.into_string().ok()) {
        if !home.is_empty() {
            redacted = redacted.replace(&home, "$HOME");
        }
    }
    for (pattern, replacement) in SECRET_PATTERNS.iter() {
        redacted = pattern.replace_all(&redacted, *replacement).into_owned();
    }
    truncate_utf8(&redacted, limit)
}

fn truncate_utf8(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_string();
    }
    let suffix = "…[truncated]";
    let content_limit = limit.saturating_sub(suffix.len());
    let mut end = content_limit.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}", &value[..end])
}

fn copy_file(source: &Path, target: &Path) -> Result<(), String> {
    fs::copy(source, target)
        .map_err(|error| {
            format!(
                "copy `{}` -> `{}`: {error}",
                source.display(),
                target.display()
            )
        })
        .and_then(|_| restrict_file_permissions(target))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use token_station_cli::store::SchemaCompatibility;

    fn scratch(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "token-station-recovery-{}-{name}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn a_future_schema_selects_the_db_independent_safe_shell() {
        let root = scratch("future-state");
        let state = state_from_compatibility(
            &root.join("metrics.sqlite"),
            SchemaCompatibility::Newer {
                found: 12,
                supported: 4,
            },
        );
        assert_eq!(state.mode, RecoveryMode::Safe);
        assert_eq!(state.reason_code.as_deref(), Some("metrics_schema_newer"));
        assert_eq!(state.found_schema, Some(12));
        assert_eq!(state.supported_schema, Some(4));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn diagnostics_are_bounded_and_redacted_on_write_and_read() {
        let root = scratch("redaction");
        let log = root.join("frontend.jsonl");
        let record = append_frontend_event(
            &log,
            FrontendDiagnosticInput {
                kind: "window_error".to_string(),
                message: "Authorization: Bearer secret-token sk-live-abcdef; body=private-body-text; tool_input=private-tool-value; search_term=private-search-value".to_string(),
                stack: Some(format!("api_key=very-secret {}", "x".repeat(20_000))),
                component_stack: None,
            },
        )
        .unwrap();
        assert!(!record.message.contains("secret-token"));
        assert!(!record.message.contains("sk-live"));
        assert!(!record.message.contains("private-body-text"));
        assert!(!record.message.contains("private-tool-value"));
        assert!(!record.message.contains("private-search-value"));
        assert!(record.stack.as_deref().unwrap().len() <= STACK_FIELD_LIMIT);

        std::fs::write(
            &log,
            "{\"timestamp_ms\":1,\"kind\":\"window_error\",\"message\":\"password=raw-secret\",\"stack\":null,\"component_stack\":null}\n",
        )
        .unwrap();
        let read = read_frontend_events(&log).unwrap();
        assert_eq!(read.len(), 1);
        assert!(!read[0].message.contains("raw-secret"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn diagnostics_redact_tool_input_across_semicolons_and_line_breaks() {
        let root = scratch("multiline-redaction");
        let log = root.join("frontend.jsonl");
        let record = append_frontend_event(
            &log,
            FrontendDiagnosticInput {
                kind: "window_error".to_string(),
                message:
                    "arguments={\"command\":\"echo private-a; curl private-b\nnext private-c\"}"
                        .to_string(),
                stack: None,
                component_stack: None,
            },
        )
        .unwrap();

        assert!(!record.message.contains("private-a"));
        assert!(!record.message.contains("private-b"));
        assert!(!record.message.contains("private-c"));
        let persisted = std::fs::read_to_string(&log).unwrap();
        assert!(!persisted.contains("private-a"));
        assert!(!persisted.contains("private-b"));
        assert!(!persisted.contains("private-c"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn diagnostic_preview_never_exposes_absolute_local_paths() {
        let root = scratch("preview-path-redaction");
        let data = root.join("private-user-data");
        std::fs::create_dir_all(&data).unwrap();

        let preview = diagnostic_preview(&root.join("token-station.json"), &data).unwrap();
        let serialized = serde_json::to_string(&preview).unwrap();

        assert!(!serialized.contains(root.to_string_lossy().as_ref()));
        assert_eq!(preview.recovery.metrics_path, "$DATA_DIR/metrics.sqlite");
        assert_eq!(preview.recovery.backup_dir, "$DATA_DIR");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_local_raw_export_requires_confirmation_and_never_changes_sources() {
        let root = scratch("export");
        let config = root.join("token-station.json");
        let data = root.join("data");
        std::fs::create_dir_all(data.join("diagnostics")).unwrap();
        std::fs::write(&config, b"config-bytes").unwrap();
        std::fs::write(data.join("metrics.sqlite"), b"future-db-bytes").unwrap();
        std::fs::write(
            data.join("diagnostics/frontend.jsonl"),
            "{\"timestamp_ms\":1,\"kind\":\"render_error\",\"message\":\"token=raw-secret\",\"stack\":null,\"component_stack\":null}\n",
        )
        .unwrap();

        assert!(export_bundle(&config, &data, false).is_err());
        let exported = export_bundle(&config, &data, true).unwrap();
        assert_eq!(std::fs::read(&config).unwrap(), b"config-bytes");
        assert_eq!(
            std::fs::read(data.join("metrics.sqlite")).unwrap(),
            b"future-db-bytes"
        );
        assert_eq!(
            std::fs::read(exported.join("metrics.sqlite")).unwrap(),
            b"future-db-bytes"
        );
        let diagnostics =
            std::fs::read_to_string(exported.join("frontend-diagnostics.jsonl")).unwrap();
        assert!(!diagnostics.contains("raw-secret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                std::fs::metadata(exported.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&exported).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for name in [
                "token-station.json",
                "metrics.sqlite",
                "frontend-diagnostics.jsonl",
                "manifest.json",
            ] {
                assert_eq!(
                    std::fs::metadata(exported.join(name))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600,
                    "{name} must remain private",
                );
            }
        }
        std::fs::remove_dir_all(root).ok();
    }
}
