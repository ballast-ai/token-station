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

#[test]
fn automatic_catalog_failure_then_disconnect_stays_disconnected_until_manual_reconnect() {
    let mut f = Fixture::new();
    f.auth();
    let original = fs::read(f.manager.home.join("auth.json")).unwrap();
    let account = f.manager.connect_automatically().unwrap();
    let s = server(&mut f, 503, json!({"error":"fixture unavailable"}), || {});
    assert!(f.manager.models(&account.id).is_err());
    s.join().unwrap();
    f.manager.disconnect(&account.id).unwrap();
    assert!(!f.manager.auto_connect_allowed(&account.id).unwrap());
    assert!(f.manager.connect_automatically().is_err());
    assert!(f.manager.list().unwrap().is_empty());
    assert_eq!(
        original,
        fs::read(f.manager.home.join("auth.json")).unwrap()
    );
    let reconnected = f.manager.connect().unwrap();
    assert_eq!(reconnected.id, account.id);
    assert!(f.manager.auto_connect_allowed(&account.id).unwrap());
    assert_eq!(f.manager.connect_automatically().unwrap().id, account.id);
}

#[test]
fn disconnect_serializes_after_inflight_automatic_registration() {
    use std::sync::mpsc;
    use std::time::Duration;
    let mut f = Fixture::new();
    f.auth();
    let id = f.manager.discover().unwrap().id;
    expire(&f);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let s = server(&mut f, 200, refreshed(), move || {
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    let automatic = f.manager.clone();
    let connect = std::thread::spawn(move || automatic.connect_automatically());
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let disconnecting = f.manager.clone();
    let disconnected_id = id.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let disconnect = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        disconnecting.disconnect(&disconnected_id)
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    release_tx.send(()).unwrap();
    connect.join().unwrap().unwrap();
    disconnect.join().unwrap().unwrap();
    s.join().unwrap();
    assert!(f.manager.list().unwrap().is_empty());
    assert!(!f.manager.auto_connect_allowed(&id).unwrap());
    assert!(f.manager.connect_automatically().is_err());
    assert_eq!(f.manager.discover().unwrap().state, "ready");
}

#[test]
fn automatic_registration_checks_disconnect_before_refresh_or_source_registration() {
    let f = Fixture::new();
    f.auth();
    let id = f.manager.discover().unwrap().id;
    f.manager.disconnect(&id).unwrap();
    expire(&f);
    let original = fs::read(f.manager.home.join("auth.json")).unwrap();
    let mut automatic = f.manager.clone();
    automatic.endpoints.token = "https://attacker.invalid/never-call".into();
    let error = std::thread::spawn(move || automatic.connect_automatically())
        .join()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error,
        "Automatic Codex connection is disabled for this account."
    );
    assert!(!f.manager.path(&id).unwrap().exists());
    assert_eq!(
        original,
        fs::read(f.manager.home.join("auth.json")).unwrap()
    );
    assert!(f.manager.auto_connect_allowed("../invalid").is_err());
}

#[test]
fn automatic_attachment_does_not_commit_after_disconnect() {
    let fixture = Fixture::new();
    fixture.auth();
    let account = fixture.manager.connect_automatically().unwrap();
    fixture.manager.disconnect(&account.id).unwrap();
    let committed = fixture.dir.join("attached-provider");
    let result = fixture.manager.with_automatic_account(&account.id, |_| {
        fs::write(&committed, b"attached").map_err(local_error)?;
        Ok(true)
    });
    assert_eq!(result, Ok(None));
    assert!(!committed.exists());
    assert!(fixture.manager.list().unwrap().is_empty());
}

#[test]
fn automatic_attachment_holds_the_reference_lock_until_commit_finishes() {
    let fixture = Fixture::new();
    fixture.auth();
    let account = fixture.manager.connect_automatically().unwrap();
    let result = fixture.manager.with_automatic_account(&account.id, |_| {
        let concurrent = OpenOptions::new()
            .read(true)
            .write(true)
            .open(fixture.manager.root.join("references.lock"))
            .map_err(local_error)?;
        assert!(matches!(
            concurrent.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        Ok(true)
    });
    assert_eq!(result, Ok(Some(true)));
}

#[test]
fn automatic_attachment_skips_a_busy_catalog_without_waiting_for_network() {
    use std::sync::mpsc;
    use std::time::Duration;
    let mut fixture = Fixture::new();
    fixture.auth();
    let account = fixture.manager.connect_automatically().unwrap();
    let (entered_send, entered_receive) = mpsc::channel();
    let (release_send, release_receive) = mpsc::channel();
    let catalog = server(
        &mut fixture,
        200,
        json!({"models":[{"slug":"fixture-model","display_name":"Fixture model"}]}),
        move || {
            entered_send.send(()).unwrap();
            release_receive
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        },
    );
    let catalog_manager = fixture.manager.clone();
    let account_id = account.id.clone();
    let pending_catalog = std::thread::spawn(move || catalog_manager.models(&account_id));
    entered_receive
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let automatic_manager = fixture.manager.clone();
    let (completed_send, completed_receive) = mpsc::channel();
    let pending_attachment = std::thread::spawn(move || {
        let result = automatic_manager.with_automatic_account(&account.id, |_| Ok(true));
        completed_send.send(result).unwrap();
    });
    let early_result = completed_receive.recv_timeout(Duration::from_secs(2));
    release_send.send(()).unwrap();
    pending_catalog.join().unwrap().unwrap();
    pending_attachment.join().unwrap();
    catalog.join().unwrap();
    assert_eq!(
        early_result.expect("Automatic attachment waited for the catalog HTTP response"),
        Ok(None)
    );
}

#[cfg(unix)]
#[test]
fn automatic_attachment_reports_unsafe_lock_and_source_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.auth();
    let account = fixture.manager.connect_automatically().unwrap();
    let lock = fixture.manager.root.join("references.lock");
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        fixture
            .manager
            .with_automatic_account(&account.id, |_| Ok(true))
            .is_err()
    );
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    let source = fixture.manager.home.join("auth.json");
    fs::set_permissions(source, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        fixture
            .manager
            .with_automatic_account(&account.id, |_| Ok(true))
            .is_err()
    );
}

#[test]
fn automatic_attachment_commits_the_current_bound_account_without_source_changes() {
    let fixture = Fixture::new();
    fixture.auth();
    let original = fs::read(fixture.manager.home.join("auth.json")).unwrap();
    let account = fixture.manager.connect_automatically().unwrap();
    let committed = fixture.dir.join("attached-provider");
    let result = fixture
        .manager
        .with_automatic_account(&account.id, |current| {
            assert_eq!(current.id, account.id);
            assert_eq!(current.account_id, "synthetic-account");
            fs::write(&committed, &current.id).map_err(local_error)?;
            Ok(current.id.clone())
        });
    assert_eq!(result, Ok(Some(account.id.clone())));
    assert_eq!(fs::read_to_string(&committed).unwrap(), account.id);
    assert_eq!(
        original,
        fs::read(fixture.manager.home.join("auth.json")).unwrap()
    );
}

#[test]
fn automatic_attachment_rejects_a_replaced_native_account() {
    let fixture = Fixture::new();
    fixture.auth();
    let account = fixture.manager.connect_automatically().unwrap();
    rewrite(&fixture, |document| {
        document["tokens"]["account_id"] = "other-account".into();
        document["tokens"]["id_token"] = jwt("other-account", "other-user", 4_000_000_000).into();
        document["tokens"]["access_token"] = document["tokens"]["id_token"].clone();
    });
    let result = fixture.manager.with_automatic_account(&account.id, |_| {
        panic!("A replaced native account cannot authorize automatic attachment")
    });
    assert_eq!(result, Ok(None::<()>));
}

#[cfg(unix)]
#[test]
fn automatic_suppression_rejects_unsafe_files_without_following_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = Fixture::new();
    f.auth();
    let id = f.manager.discover().unwrap().id;
    f.manager.disconnect(&id).unwrap();
    let marker = f.manager.root.join(format!("{id}.auto-disabled"));
    assert_eq!(fs::read_to_string(&marker).unwrap(), "1\n");
    fs::set_permissions(&marker, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(f.manager.auto_connect_allowed(&id).is_err());
    fs::remove_file(&marker).unwrap();
    let outside = f.dir.join("outside");
    fs::write(&outside, "preserve").unwrap();
    symlink(&outside, &marker).unwrap();
    assert!(f.manager.auto_connect_allowed(&id).is_err());
    assert!(f.manager.connect().is_err());
    assert!(f.manager.disconnect(&id).is_err());
    assert_eq!(fs::read_to_string(outside).unwrap(), "preserve");
}
