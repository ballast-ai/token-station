use super::*;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::json;
use std::fs;
struct Fixture {
    dir: PathBuf,
    manager: AccountManager,
}
impl Fixture {
    fn new() -> Self {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "ts-codex-accounts-{}",
            URL_SAFE_NO_PAD.encode(random)
        ));
        let home = dir.join("codex");
        ensure_private_dir(&home).unwrap();
        fs::write(
            home.join("models_cache.json"),
            r#"{"client_version":"0.162.0"}"#,
        )
        .unwrap();
        let mut manager = AccountManager::new(dir.join("station"));
        manager.home = home;
        Self { dir, manager }
    }
    fn auth(&self) {
        let claims = json!({"sub":"synthetic-user","email":"fixture@example.test","exp":4_000_000_000_u64,"https://api.openai.com/auth":{"chatgpt_account_id":"synthetic-account"}});
        let jwt = format!(
            "e30.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        fs::write(self.manager.home.join("auth.json"),json!({"auth_mode":"chatgpt","tokens":{"id_token":jwt,"access_token":jwt,"refresh_token":"synthetic-refresh-secret","account_id":"synthetic-account"},"unknown":{"keep":true}}).to_string()).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}
#[test]
fn discovers_existing_subscription_without_copying_credentials() {
    let f = Fixture::new();
    f.auth();
    let view = f.manager.discover().unwrap();
    assert_eq!(view.state, "ready");
    assert_eq!(view.account_id, "synthetic-account");
    assert!(
        !serde_json::to_string(&view)
            .unwrap()
            .contains("synthetic-refresh-secret")
    );
    assert!(!f.manager.root.exists());
}
#[test]
fn explicit_keyring_mode_does_not_reuse_a_stale_file() {
    let f = Fixture::new();
    f.auth();
    fs::write(
        f.manager.home.join("config.toml"),
        "cli_auth_credentials_store='keyring'\n",
    )
    .unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "unsupported_storage");
}
fn jwt(account: &str, subject: &str, expiry: u64) -> String {
    format!("e30.{}.signature",URL_SAFE_NO_PAD.encode(json!({"sub":subject,"exp":expiry,"https://api.openai.com/auth":{"chatgpt_account_id":account}}).to_string()))
}
fn rewrite(f: &Fixture, update: impl FnOnce(&mut serde_json::Value)) {
    let path = f.manager.home.join("auth.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    update(&mut value);
    fs::write(path, value.to_string()).unwrap();
}
fn expire(f: &Fixture) {
    rewrite(f, |v| {
        v["tokens"]["access_token"] = jwt("synthetic-account", "synthetic-user", 1).into();
    });
}
fn server(
    f: &mut Fixture,
    status: u16,
    body: serde_json::Value,
    action: impl FnOnce() + Send + 'static,
) -> std::thread::JoinHandle<String> {
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    f.manager.endpoints = transport::Endpoints {
        token: format!("{base}/token"),
        api: format!("{base}/codex"),
    };
    std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let n = socket.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let text = String::from_utf8_lossy(&bytes[..end]);
                let length = text
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        action();
        let body = body.to_string();
        let redirect = if status == 302 {
            "Location: http://127.0.0.1:1/never-forward-credentials\r\n"
        } else {
            ""
        };
        write!(socket,"HTTP/1.1 {status} Test\r\n{redirect}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        String::from_utf8(bytes).unwrap()
    })
}
fn refreshed() -> serde_json::Value {
    json!({"access_token":jwt("synthetic-account","synthetic-user",4_000_000_000),"refresh_token":"rotated-synthetic-refresh"})
}
#[test]
fn connection_is_idempotent_and_persists_only_bound_metadata() {
    let f = Fixture::new();
    f.auth();
    let before = fs::read(f.manager.home.join("auth.json")).unwrap();
    let a = f.manager.connect().unwrap();
    let b = f.manager.connect().unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(f.manager.list().unwrap().len(), 1);
    let stored = fs::read_to_string(f.manager.path(&a.id).unwrap()).unwrap();
    assert!(!stored.contains("access_token"));
    assert!(!stored.contains("refresh_token"));
    assert!(!stored.contains("signature"));
    assert_eq!(before, fs::read(f.manager.home.join("auth.json")).unwrap());
    f.manager.disconnect(&a.id).unwrap();
    assert!(f.manager.list().unwrap().is_empty());
    assert_eq!(before, fs::read(f.manager.home.join("auth.json")).unwrap());
}
#[test]
fn account_switch_never_substitutes_credentials() {
    let f = Fixture::new();
    f.auth();
    let a = f.manager.connect().unwrap();
    rewrite(&f, |v| {
        v["tokens"]["account_id"] = "other-account".into();
        v["tokens"]["id_token"] = jwt("other-account", "other-user", 4_000_000_000).into();
        v["tokens"]["access_token"] = v["tokens"]["id_token"].clone();
    });
    let state = f.manager.account(&a.id).unwrap();
    assert_eq!(state.state, "account_changed");
    assert_eq!(state.account_id, "synthetic-account");
    assert!(
        f.manager
            .access_token(&a.id)
            .unwrap_err()
            .contains("changed")
    );
    let other = f.manager.connect().unwrap();
    assert_ne!(other.id, a.id);
}
#[test]
fn unsupported_modes_missing_and_api_key_sources_have_safe_states() {
    let f = Fixture::new();
    assert_eq!(f.manager.discover().unwrap().state, "missing");
    f.auth();
    for mode in ["auto", "ephemeral", "keyring", "unexpected"] {
        fs::write(
            f.manager.home.join("config.toml"),
            format!("cli_auth_credentials_store='{mode}'"),
        )
        .unwrap();
        assert_eq!(f.manager.discover().unwrap().state, "unsupported_storage");
        assert!(f.manager.connect().is_err());
    }
    fs::remove_file(f.manager.home.join("config.toml")).unwrap();
    rewrite(&f, |v| {
        v["auth_mode"] = "apikey".into();
        v["OPENAI_API_KEY"] = "synthetic-api-key".into();
    });
    assert_eq!(f.manager.discover().unwrap().state, "api_key_only");
}
#[test]
fn expired_discovery_is_read_only_and_refresh_preserves_unknown_fields() {
    let mut f = Fixture::new();
    f.auth();
    expire(&f);
    let before = fs::read(f.manager.home.join("auth.json")).unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "expired");
    assert_eq!(before, fs::read(f.manager.home.join("auth.json")).unwrap());
    let server = server(&mut f, 200, refreshed(), || {});
    let a = f.manager.connect().unwrap();
    assert_eq!(a.state, "ready");
    let request = server.join().unwrap();
    assert!(request.starts_with("POST /token "));
    let body: serde_json::Value =
        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["client_id"], "app_EMoamEEZ73f0CkXaXp7hrann");
    assert_eq!(body["refresh_token"], "synthetic-refresh-secret");
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(f.manager.home.join("auth.json")).unwrap()).unwrap();
    assert_eq!(after["unknown"]["keep"], true);
    assert_eq!(
        after["tokens"]["refresh_token"],
        "rotated-synthetic-refresh"
    );
    assert!(after["last_refresh"].as_str().unwrap().ends_with('Z'));
    assert_eq!(
        f.manager.access_token(&a.id).unwrap(),
        refreshed()["access_token"].as_str().unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(f.manager.home.join("auth.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
#[test]
fn refresh_never_overwrites_external_change_or_deleted_source() {
    for delete in [false, true] {
        let mut f = Fixture::new();
        f.auth();
        let a = f.manager.connect().unwrap();
        expire(&f);
        let path = f.manager.home.join("auth.json");
        let original = fs::read(&path).unwrap();
        let changed = path.clone();
        let server = server(&mut f, 200, refreshed(), move || {
            if delete {
                fs::remove_file(changed).unwrap();
            } else {
                let mut v: serde_json::Value = serde_json::from_slice(&original).unwrap();
                v["external-change"] = true.into();
                fs::write(changed, v.to_string()).unwrap();
            }
        });
        assert!(f.manager.access_token(&a.id).is_err());
        server.join().unwrap();
        if delete {
            assert!(!path.exists());
        } else {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(value["external-change"], true);
            assert_eq!(value["tokens"]["refresh_token"], "synthetic-refresh-secret");
        }
    }
}
#[test]
fn refresh_identity_mismatch_and_upstream_error_are_redacted_without_mutation() {
    for (status, body) in [
        (
            200,
            json!({"access_token":jwt("other-account","other-user",4_000_000_000)}),
        ),
        (400, json!({"error":"synthetic-private-upstream-secret"})),
        (
            302,
            json!({"access_token":"synthetic-private-upstream-secret"}),
        ),
    ] {
        let mut f = Fixture::new();
        f.auth();
        let a = f.manager.connect().unwrap();
        expire(&f);
        let before = fs::read(f.manager.home.join("auth.json")).unwrap();
        let s = server(&mut f, status, body, || {});
        let error = f.manager.access_token(&a.id).unwrap_err();
        if status == 302 {
            assert_eq!(
                error,
                "Codex request was not accepted. Check the login in Codex."
            );
        }
        assert!(!error.contains("synthetic-private"));
        assert!(!error.contains("e30."));
        s.join().unwrap();
        assert_eq!(before, fs::read(f.manager.home.join("auth.json")).unwrap());
    }
}
#[test]
fn catalog_uses_bound_identity_version_and_actual_capabilities() {
    let mut f = Fixture::new();
    f.auth();
    let a = f.manager.connect().unwrap();
    let s = server(
        &mut f,
        200,
        json!({"models":[{"slug":"available-model","display_name":"Available","context_window":200_000,"input_modalities":["text","image"],"visibility":"list"},{"slug":"hidden-model","visibility":"hide"},{"slug":"unknown-capabilities"}]}),
        || {},
    );
    let models = f.manager.models(&a.id).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].context_window, Some(200_000));
    assert_eq!(models[0].supports_images, Some(true));
    assert_eq!(models[1].context_window, None);
    assert_eq!(models[1].supports_images, None);
    let request = s.join().unwrap().to_ascii_lowercase();
    assert!(request.starts_with("get /codex/models?client_version=0.162.0 "));
    assert!(request.contains("chatgpt-account-id: synthetic-account"));
    assert!(request.contains("originator: codex_cli_rs"));
}
#[test]
fn changed_source_path_is_not_silently_rebound() {
    let mut f = Fixture::new();
    f.auth();
    let a = f.manager.connect().unwrap();
    let other = f.dir.join("other");
    fs::create_dir(&other).unwrap();
    fs::copy(f.manager.home.join("auth.json"), other.join("auth.json")).unwrap();
    fs::copy(
        f.manager.home.join("models_cache.json"),
        other.join("models_cache.json"),
    )
    .unwrap();
    f.manager.home = other;
    assert_eq!(f.manager.account(&a.id).unwrap().state, "account_changed");
    assert!(f.manager.access_token(&a.id).is_err());
}
#[test]
fn fixed_endpoints_and_reference_identifiers_fail_closed() {
    let mut f = Fixture::new();
    f.auth();
    let a = f.manager.connect().unwrap();
    f.manager.endpoints.api = "https://attacker.invalid/codex".into();
    assert_eq!(
        f.manager.models(&a.id).unwrap_err(),
        "Untrusted Codex endpoint."
    );
    assert!(f.manager.account("../auth").is_err());
    assert!(f.manager.disconnect("../auth").is_err());
}
#[test]
fn timestamp_matches_unix_epoch_and_leap_day() {
    assert_eq!(transport::timestamp(0), "1970-01-01T00:00:00Z");
    assert_eq!(transport::timestamp(1_709_251_199), "2024-02-29T23:59:59Z");
}
#[test]
fn auth0_subject_is_valid_but_conflicting_access_subject_is_rejected() {
    let f = Fixture::new();
    f.auth();
    rewrite(&f, |v| {
        v["tokens"]["id_token"] =
            jwt("synthetic-account", "auth0|synthetic-user", 4_000_000_000).into();
        v["tokens"]["access_token"] = v["tokens"]["id_token"].clone();
    });
    assert_eq!(f.manager.discover().unwrap().state, "ready");
    rewrite(&f, |v| {
        v["tokens"]["access_token"] =
            jwt("synthetic-account", "other-subject", 4_000_000_000).into();
    });
    assert_eq!(f.manager.discover().unwrap().state, "invalid_source");
}
#[cfg(unix)]
#[test]
fn unsafe_permissions_symlinks_and_hardlinks_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    f.auth();
    let path = f.manager.home.join("auth.json");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "invalid_source");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "ready");
    let copy = f.dir.join("copy");
    fs::hard_link(&path, &copy).unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "invalid_source");
    fs::remove_file(&copy).unwrap();
    fs::rename(&path, &copy).unwrap();
    symlink(&copy, &path).unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "invalid_source");
}
#[cfg(unix)]
#[test]
fn writable_source_directory_and_dangling_storage_symlink_fail_closed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    f.auth();
    fs::set_permissions(&f.manager.home, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "invalid_source");
    fs::set_permissions(&f.manager.home, fs::Permissions::from_mode(0o755)).unwrap();
    symlink(
        f.dir.join("absent-config"),
        f.manager.home.join("config.toml"),
    )
    .unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "invalid_source");
}
#[test]
fn concurrent_managers_share_refresh_and_only_one_rotation_occurs() {
    let mut f = Fixture::new();
    f.auth();
    let a = f.manager.connect().unwrap();
    let mut two = f.manager.clone();
    two.root = f.dir.join("other-station").join("codex-accounts");
    assert_eq!(two.connect().unwrap().id, a.id);
    expire(&f);
    let s = server(&mut f, 200, refreshed(), || {
        std::thread::sleep(std::time::Duration::from_millis(60));
    });
    let one = f.manager.clone();
    two.endpoints = f.manager.endpoints.clone();
    let id = a.id.clone();
    let first = std::thread::spawn(move || one.access_token(&id));
    let second = std::thread::spawn(move || two.access_token(&a.id));
    assert_eq!(
        first.join().unwrap().unwrap(),
        second.join().unwrap().unwrap()
    );
    s.join().unwrap();
}
#[test]
fn malformed_oversize_and_expired_without_refresh_fail_without_secret_text() {
    let f = Fixture::new();
    fs::write(
        f.manager.home.join("auth.json"),
        "synthetic-private-malformed-source",
    )
    .unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "invalid_source");
    fs::write(
        f.manager.home.join("auth.json"),
        vec![b' '; usize::try_from(source::LIMIT).unwrap() + 1],
    )
    .unwrap();
    assert_eq!(f.manager.discover().unwrap().state, "invalid_source");
    f.auth();
    expire(&f);
    rewrite(&f, |v| {
        v["tokens"].as_object_mut().unwrap().remove("refresh_token");
    });
    assert_eq!(f.manager.discover().unwrap().state, "expired");
    assert_eq!(
        f.manager.connect().unwrap_err(),
        "Codex session expired. Sign in with Codex again."
    );
}
