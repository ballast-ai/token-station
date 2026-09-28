//! Identity and storage boundaries for the separately installed SCX App.

use std::path::{Component, Path};

use serde_json::Value;

pub(crate) const SCX_BUNDLE_ID: &str = "com.tokenstation.desktop.scx";
pub(crate) const STABLE_BUNDLE_ID: &str = "com.tokenstation.desktop";
pub(crate) const SCX_LISTEN: &str = "127.0.0.1:18787";
pub(crate) const FORCE_FORGET_DISABLED: &str =
    "SCX 实验版不支持强制清除接管记录，以免丢失接入前的连接。请使用正常断开或快照恢复。";
pub(crate) const CURSOR_TUNNEL_DISABLED: &str =
    "SCX 实验版暂不支持 Cursor 专用 HTTPS 隧道。请在原版中管理 Cursor；其他 Agent 可在实验版预览并接入。";
pub(crate) const UPDATES_DISABLED: &str = "SCX 实验版使用独立安装，不接收正式版自动更新。";

pub(crate) fn is_scx_experiment() -> bool {
    option_env!("TOKEN_STATION_SCX_EXPERIMENT") == Some("1")
}

pub(crate) fn bundle_id() -> &'static str {
    if is_scx_experiment() {
        SCX_BUNDLE_ID
    } else {
        STABLE_BUNDLE_ID
    }
}

pub(crate) const fn app_name() -> &'static str {
    "Token Station"
}

pub(crate) fn default_listen() -> &'static str {
    if is_scx_experiment() {
        SCX_LISTEN
    } else {
        "127.0.0.1:8787"
    }
}

pub(crate) fn require_force_forget_support() -> Result<(), String> {
    if is_scx_experiment() {
        Err(FORCE_FORGET_DISABLED.to_owned())
    } else {
        Ok(())
    }
}

pub(crate) fn require_cursor_tunnel_support() -> Result<(), String> {
    if is_scx_experiment() {
        Err(CURSOR_TUNNEL_DISABLED.to_owned())
    } else {
        Ok(())
    }
}

pub(crate) fn validate_identity(
    identifier: &str,
    config_root: &Path,
    data_root: &Path,
) -> Result<(), String> {
    validate_identity_for(is_scx_experiment(), identifier, config_root, data_root)
}

fn validate_identity_for(
    experimental: bool,
    identifier: &str,
    config_root: &Path,
    data_root: &Path,
) -> Result<(), String> {
    let expected = if experimental {
        SCX_BUNDLE_ID
    } else {
        STABLE_BUNDLE_ID
    };
    if identifier != expected {
        return Err("Desktop build identity does not match its isolation mode.".to_owned());
    }
    if experimental {
        for root in [config_root, data_root] {
            if root.file_name().and_then(|part| part.to_str()) != Some(SCX_BUNDLE_ID) {
                return Err("SCX storage must use its private application root.".to_owned());
            }
            reject_path_aliases(root)?;
        }
    }
    Ok(())
}

fn reject_path_aliases(path: &Path) -> Result<(), String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(
            "SCX paths must be absolute and must not contain traversal components.".to_owned(),
        );
    }
    for parent in path.ancestors() {
        if std::fs::symlink_metadata(parent).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return Err("SCX paths must not contain symbolic links.".to_owned());
        }
    }
    Ok(())
}

pub(crate) fn validate_draft(config_path: &Path, draft: &Value) -> Result<(), String> {
    if !is_scx_experiment() {
        return Ok(());
    }
    validate_scx_draft(config_path, draft)
}

fn validate_scx_draft(config_path: &Path, draft: &Value) -> Result<(), String> {
    let root = config_path
        .parent()
        .ok_or("SCX configuration root is missing.")?;
    if root.file_name().and_then(|part| part.to_str()) != Some(SCX_BUNDLE_ID) {
        return Err("SCX configuration must use its private application root.".to_owned());
    }
    reject_path_aliases(config_path)?;
    for (pointer, expected) in [
        ("/data/dir", root.join("token-station-data")),
        ("/plugins/dir", root.join("plugins")),
    ] {
        let actual = draft
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(Path::new);
        if actual != Some(expected.as_path()) {
            return Err(
                "SCX data and plugins must use private application directories.".to_owned(),
            );
        }
        reject_path_aliases(&expected)?;
    }
    if draft.pointer("/server/listen").and_then(Value::as_str) != Some(SCX_LISTEN)
        || draft.pointer("/server/auth").and_then(Value::as_bool) == Some(false)
    {
        return Err("SCX requires authenticated loopback port 18787.".to_owned());
    }
    let mut auth_sources = Vec::new();
    if let Some(upstreams) = draft.get("upstreams").and_then(Value::as_object) {
        auth_sources.extend(
            upstreams
                .values()
                .filter_map(|upstream| upstream.get("auth")),
        );
    }
    if let Some(auth) = draft.pointer("/egress/auth/credential") {
        auth_sources.push(auth);
    }
    for auth in auth_sources {
        if let Some(file) = auth.get("file").and_then(Value::as_str) {
            let path = Path::new(file);
            if !path.starts_with(root.join("token-station-data/credentials")) {
                return Err("SCX credential files must use private credential storage.".to_owned());
            }
            reject_path_aliases(path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (std::path::PathBuf, Value) {
        let root = Path::new("/scx-test").join(SCX_BUNDLE_ID);
        let draft = json!({
            "server": {"listen": SCX_LISTEN, "auth": true},
            "data": {"dir": root.join("token-station-data")},
            "plugins": {"dir": root.join("plugins")},
            "upstreams": {"fixture": {"auth": {"slot": "key", "store": true}}}
        });
        (root.join("token-station.json"), draft)
    }

    #[test]
    fn experiment_rejects_stable_identity_and_roots() {
        let root = Path::new("/scx-test").join(SCX_BUNDLE_ID);
        assert!(validate_identity_for(true, SCX_BUNDLE_ID, &root, &root).is_ok());
        assert!(validate_identity_for(true, STABLE_BUNDLE_ID, &root, &root).is_err());
        assert!(validate_identity_for(false, SCX_BUNDLE_ID, &root, &root).is_err());
        assert!(validate_identity_for(true, SCX_BUNDLE_ID, Path::new("/stable"), &root).is_err());
    }

    #[test]
    fn experiment_rejects_shared_data_credentials_and_ports() {
        let (config, draft) = fixture();
        assert!(validate_scx_draft(&config, &draft).is_ok());
        for (pointer, replacement) in [
            ("/server/listen", json!("127.0.0.1:8787")),
            ("/server/auth", json!(false)),
            ("/data/dir", json!("/stable/data")),
            ("/plugins/dir", json!("/stable/plugins")),
        ] {
            let mut invalid = draft.clone();
            *invalid.pointer_mut(pointer).unwrap() = replacement;
            assert!(validate_scx_draft(&config, &invalid).is_err(), "{pointer}");
        }
        let mut invalid = draft;
        invalid["upstreams"]["fixture"]["auth"] = json!({"slot": "key", "file": "/stable/secret"});
        assert!(validate_scx_draft(&config, &invalid).is_err());
    }

    #[test]
    fn experiment_rejects_credential_path_traversal() {
        let (config, mut draft) = fixture();
        draft["upstreams"]["fixture"]["auth"] = json!({
            "slot": "key", "file": config.parent().unwrap().join("token-station-data/credentials/../../../stable/key")
        });
        assert!(validate_scx_draft(&config, &draft).is_err());
    }
}
