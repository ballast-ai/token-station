use serde_json::Value;
use zeroize::Zeroizing;

use super::config_codec::{semantic_json, ConfigDocument};
use super::ownership::{compute_owned_value_macs, OwnershipRecord};
use super::types::{
    AgentDriftView, ConfigPath, DriftChange, DriftChangeKind, DriftScope, DriftStatus,
};

const MAX_CHANGES: usize = 200;
const MAX_PATH_SEGMENT_CHARS: usize = 128;

pub fn analyze_drift(
    ownership: &OwnershipRecord,
    baseline: &ConfigDocument,
    current: &ConfigDocument,
    current_hash: &str,
    key: &Zeroizing<[u8; 32]>,
    checked_at_ms: u64,
) -> Result<AgentDriftView, String> {
    if current_hash.len() != 64 {
        return Err("current 配置 hash 无效".to_string());
    }
    if current_hash == ownership.managed_after_hash {
        return Ok(view(
            ownership,
            DriftStatus::InSync,
            current_hash,
            checked_at_ms,
            Vec::new(),
            false,
            "当前磁盘配置与 Token Station 最后写入版本一致",
        ));
    }

    let baseline = semantic_json(baseline)?;
    let current_json = semantic_json(current)?;
    let current_macs = compute_owned_value_macs(current, &ownership.owned_paths, key)?;
    let mut changes = Vec::new();
    for path in &ownership.owned_paths {
        let path_text = path.to_string();
        let matches = current_macs.get(&path_text) == ownership.owned_value_macs.get(&path_text);
        if !matches {
            changes.push(DriftChange {
                path: sanitized_path(path),
                scope: DriftScope::Managed,
                kind: if value_at(&current_json, path).is_none() {
                    DriftChangeKind::Removed
                } else {
                    DriftChangeKind::Changed
                },
                current_matches_managed: Some(false),
            });
        }
    }
    let managed_changed = !changes.is_empty();
    let mut truncated = false;
    collect_unowned_changes(
        &baseline,
        &current_json,
        &mut Vec::new(),
        &ownership.owned_paths,
        &mut changes,
        &mut truncated,
    );
    let status = if managed_changed {
        DriftStatus::ManagedChanges
    } else {
        DriftStatus::UnownedChanges
    };
    let message = match (managed_changed, changes.is_empty()) {
        (true, _) => "外部修改触及 Token Station 受管字段",
        (false, true) => "文件字节或元数据已变化，但结构化字段语义未变化",
        (false, false) => "外部修改仅涉及非受管字段",
    };
    Ok(view(
        ownership,
        status,
        current_hash,
        checked_at_ms,
        changes,
        truncated,
        message,
    ))
}

fn view(
    ownership: &OwnershipRecord,
    status: DriftStatus,
    current_hash: &str,
    checked_at_ms: u64,
    changes: Vec<DriftChange>,
    truncated: bool,
    message: &str,
) -> AgentDriftView {
    AgentDriftView {
        agent_id: ownership.agent_id.clone(),
        installation_path: ownership.installation_path.clone(),
        target_config_path: ownership.target_config_path.clone(),
        connector_id: ownership.connector_id.clone(),
        status,
        baseline_hash: ownership.before_hash.clone(),
        managed_hash: ownership.managed_after_hash.clone(),
        current_hash: Some(current_hash.to_string()),
        checked_at_ms,
        changes,
        truncated,
        message: message.to_string(),
    }
}

fn collect_unowned_changes(
    baseline: &Value,
    current: &Value,
    path: &mut Vec<String>,
    owned_paths: &[ConfigPath],
    output: &mut Vec<DriftChange>,
    truncated: &mut bool,
) {
    if baseline == current || path_is_owned(path, owned_paths) {
        return;
    }
    if output.len() >= MAX_CHANGES {
        *truncated = true;
        return;
    }
    match (baseline, current) {
        (Value::Object(left), Value::Object(right)) => {
            let keys = left
                .keys()
                .chain(right.keys())
                .collect::<std::collections::BTreeSet<_>>();
            for key in keys {
                if output.len() >= MAX_CHANGES {
                    *truncated = true;
                    break;
                }
                path.push(key.to_string());
                if path_is_owned(path, owned_paths) {
                    path.pop();
                    continue;
                }
                match (left.get(key), right.get(key)) {
                    (Some(before), Some(after)) => {
                        collect_unowned_changes(before, after, path, owned_paths, output, truncated)
                    }
                    (None, Some(_)) => push_unowned(path, DriftChangeKind::Added, output),
                    (Some(_), None) => push_unowned(path, DriftChangeKind::Removed, output),
                    (None, None) => {}
                }
                path.pop();
            }
        }
        _ => push_unowned(path, DriftChangeKind::Changed, output),
    }
}

fn push_unowned(path: &[String], kind: DriftChangeKind, output: &mut Vec<DriftChange>) {
    output.push(DriftChange {
        path: ConfigPath {
            segments: path
                .iter()
                .map(|segment| sanitize_segment(segment))
                .collect(),
        },
        scope: DriftScope::Unowned,
        kind,
        current_matches_managed: None,
    });
}

fn path_is_owned(path: &[String], owned_paths: &[ConfigPath]) -> bool {
    owned_paths.iter().any(|owned| {
        path.len() >= owned.segments.len() && path[..owned.segments.len()] == owned.segments[..]
    })
}

fn value_at<'a>(root: &'a Value, path: &ConfigPath) -> Option<&'a Value> {
    let mut current = root;
    for segment in &path.segments {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

fn sanitized_path(path: &ConfigPath) -> ConfigPath {
    ConfigPath {
        segments: path
            .segments
            .iter()
            .map(|segment| sanitize_segment(segment))
            .collect(),
    }
}

fn sanitize_segment(segment: &str) -> String {
    let mut output: String = segment
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_PATH_SEGMENT_CHARS)
        .collect();
    if segment
        .chars()
        .filter(|character| !character.is_control())
        .count()
        > MAX_PATH_SEGMENT_CHARS
    {
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::{json, Map, Value};
    use zeroize::Zeroizing;

    use super::analyze_drift;
    use crate::agent_integration::config_codec::ConfigDocument;
    use crate::agent_integration::ownership::{compute_owned_value_macs, OwnershipRecord};
    use crate::agent_integration::types::{ConfigPath, DriftChangeKind, DriftScope, DriftStatus};

    fn path(segments: &[&str]) -> ConfigPath {
        ConfigPath {
            segments: segments.iter().map(|value| (*value).to_string()).collect(),
        }
    }

    fn ownership(
        owned_paths: Vec<ConfigPath>,
        owned_value_macs: BTreeMap<String, String>,
    ) -> OwnershipRecord {
        OwnershipRecord {
            schema_version: 1,
            revision: 1,
            agent_id: "claude-code".to_string(),
            installation_path: "/opt/claude".to_string(),
            target_config_path: "/tmp/settings.json".to_string(),
            connector_id: "claude-code-v1".to_string(),
            baseline_snapshot_id: "01".repeat(16),
            last_transaction_snapshot_id: "02".repeat(16),
            before_hash: "a".repeat(64),
            managed_after_hash: "b".repeat(64),
            owned_paths,
            owned_value_macs,
            companion_files: Vec::new(),
            acquired_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    #[test]
    fn drift_uses_the_managed_hash_as_a_fast_in_sync_fact() {
        let key = Zeroizing::new([6_u8; 32]);
        let document = ConfigDocument::Json(json!({"ignored":"value"}));
        let ownership = ownership(Vec::new(), BTreeMap::new());

        let view = analyze_drift(
            &ownership,
            &document,
            &document,
            &ownership.managed_after_hash,
            &key,
            3,
        )
        .unwrap();

        assert_eq!(view.status, DriftStatus::InSync);
        assert!(view.changes.is_empty());
        assert!(!view.truncated);
    }

    #[test]
    fn drift_caps_unowned_change_paths_at_two_hundred() {
        let key = Zeroizing::new([9_u8; 32]);
        let owned_paths = vec![path(&["managed"])];
        let mut baseline = Map::new();
        let mut current = Map::new();
        baseline.insert("managed".to_string(), Value::String("same".to_string()));
        current.insert("managed".to_string(), Value::String("same".to_string()));
        for index in 0..205 {
            baseline.insert(format!("field-{index:03}"), Value::from(0));
            current.insert(format!("field-{index:03}"), Value::from(1));
        }
        let baseline = ConfigDocument::Json(Value::Object(baseline));
        let current = ConfigDocument::Json(Value::Object(current));
        let ownership = ownership(
            owned_paths.clone(),
            compute_owned_value_macs(&current, &owned_paths, &key).unwrap(),
        );

        let view =
            analyze_drift(&ownership, &baseline, &current, &"c".repeat(64), &key, 3).unwrap();

        assert_eq!(view.status, DriftStatus::UnownedChanges);
        assert_eq!(view.changes.len(), 200);
        assert!(view.truncated);
    }

    #[test]
    fn drift_reports_an_unowned_key_without_exposing_any_configuration_value() {
        let key = Zeroizing::new([7_u8; 32]);
        let owned_paths = vec![path(&["env", "TOKEN_STATION_KEY"])];
        let baseline = ConfigDocument::Json(
            json!({"env":{"TOKEN_STATION_KEY":"before-secret"},"theme":"dark"}),
        );
        let managed = ConfigDocument::Json(
            json!({"env":{"TOKEN_STATION_KEY":"managed-secret"},"theme":"dark"}),
        );
        let current = ConfigDocument::Json(
            json!({"env":{"TOKEN_STATION_KEY":"managed-secret"},"theme":"light"}),
        );
        let ownership = ownership(
            owned_paths.clone(),
            compute_owned_value_macs(&managed, &owned_paths, &key).unwrap(),
        );

        let view =
            analyze_drift(&ownership, &baseline, &current, &"c".repeat(64), &key, 3).unwrap();

        assert_eq!(view.status, DriftStatus::UnownedChanges);
        assert_eq!(view.baseline_hash, "a".repeat(64));
        assert_eq!(view.managed_hash, "b".repeat(64));
        assert_eq!(view.current_hash.as_deref(), Some("c".repeat(64).as_str()));
        assert_eq!(
            view.changes,
            vec![crate::agent_integration::types::DriftChange {
                path: path(&["theme"]),
                scope: DriftScope::Unowned,
                kind: DriftChangeKind::Changed,
                current_matches_managed: None,
            }]
        );
        let serialized = serde_json::to_string(&view).unwrap();
        for secret in ["before-secret", "managed-secret", "dark", "light"] {
            assert!(!serialized.contains(secret));
        }
    }

    #[test]
    fn drift_identifies_a_changed_managed_key_by_hmac_without_exposing_values() {
        let key = Zeroizing::new([8_u8; 32]);
        let owned_paths = vec![path(&["env", "TOKEN_STATION_KEY"])];
        let baseline = ConfigDocument::Json(json!({"env":{},"theme":"dark"}));
        let managed = ConfigDocument::Json(
            json!({"env":{"TOKEN_STATION_KEY":"managed-secret"},"theme":"dark"}),
        );
        let current = ConfigDocument::Json(
            json!({"env":{"TOKEN_STATION_KEY":"external-secret"},"theme":"dark"}),
        );
        let ownership = ownership(
            owned_paths.clone(),
            compute_owned_value_macs(&managed, &owned_paths, &key).unwrap(),
        );

        let view =
            analyze_drift(&ownership, &baseline, &current, &"d".repeat(64), &key, 3).unwrap();

        assert_eq!(view.status, DriftStatus::ManagedChanges);
        assert_eq!(
            view.changes,
            vec![crate::agent_integration::types::DriftChange {
                path: path(&["env", "TOKEN_STATION_KEY"]),
                scope: DriftScope::Managed,
                kind: DriftChangeKind::Changed,
                current_matches_managed: Some(false),
            }]
        );
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(!serialized.contains("managed-secret"));
        assert!(!serialized.contains("external-secret"));
    }
}
