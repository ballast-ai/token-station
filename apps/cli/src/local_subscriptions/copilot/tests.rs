use super::*;
fn manager() -> AccountManager {
    let root = std::env::temp_dir().join(format!(
        "ts-copilot-test-{}-{}",
        std::process::id(),
        now_nanos()
    ));
    ensure_private_dir(&root).unwrap();
    let mut manager = AccountManager::new(root.join("data"));
    manager.config = root.join("config");
    manager.cli_home = root.join("cli");
    manager
}
fn now_nanos() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}
fn editor(manager: &AccountManager, user: &str, token: &str) {
    let dir = manager.config.join("github-copilot");
    ensure_private_dir(&dir).unwrap();
    write_atomic_private(
        &dir.join("apps.json"),
        serde_json::to_string(&json!({"github.com:Iv1.test":{"user":user,"oauth_token":token}}))
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
}
#[test]
fn discovers_editor_identity_without_persisting_credentials() {
    let m = manager();
    editor(&m, "alice", "synthetic-token");
    let view = m.discover().unwrap();
    assert_eq!(view.label, "alice");
    assert_eq!(view.state, "ready");
    assert!(!m.root.exists());
    fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
}
#[test]
fn reference_binds_source_account_and_allows_same_account_token_rotation() {
    let m = manager();
    editor(&m, "alice", "synthetic-token");
    let source = m.snapshot().unwrap();
    let reference = source.reference(API_BASE.into());
    editor(&m, "alice", "rotated-token");
    assert!(m.bound(&reference).is_ok());
    editor(&m, "bob", "different-token");
    assert!(m.bound(&reference).is_err());
    let serialized = serde_json::to_string(&reference).unwrap();
    assert!(!serialized.contains("token"));
    assert!(!serialized.contains("synthetic"));
    fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
}
#[test]
fn catalog_keeps_real_endpoints_and_omits_disabled_auto_and_unknown_protocols() {
    let raw = json!({"data":[
        {"id":"gpt-real","name":"Real","model_picker_enabled":true,"capabilities":{"type":"chat","limits":{"max_context_window_tokens":128_000},"supports":{"vision":true,"tool_calls":true}},"supported_endpoints":["/responses"],"policy":{"state":"enabled"}},
        {"id":"legacy","model_picker_enabled":true,"capabilities":{"type":"chat"}},
        {"id":"disabled","model_picker_enabled":true,"capabilities":{"type":"chat"},"policy":{"state":"disabled"}},
        {"id":"auto","model_picker_enabled":true,"capabilities":{"type":"chat"}},
        {"id":"websocket","model_picker_enabled":true,"capabilities":{"type":"chat"},"supported_endpoints":["/responses/ws"]}
    ]});
    let (models, endpoints) = catalog(&raw).unwrap();
    assert_eq!(models.len(), 2);
    assert_eq!(models[0].context_window, Some(128_000));
    assert_eq!(endpoints["gpt-real"], json!(["/responses"]));
    assert_eq!(endpoints["legacy"], json!(["/chat/completions"]));
}
#[test]
fn fixed_destination_rejects_ports_paths_credentials_and_suffix_attacks() {
    for url in [
        "https://api.githubcopilot.com",
        "https://api.individual.githubcopilot.com",
    ] {
        assert!(valid_api(url));
    }
    for url in [
        "http://api.githubcopilot.com",
        "https://api.githubcopilot.com.evil.test",
        "https://api.githubcopilot.com:443",
        "https://api.githubcopilot.com/",
        "https://user@api.githubcopilot.com",
        "https://api.githubcopilot.com?token=secret",
    ] {
        assert!(!valid_api(url));
    }
}
#[test]
fn disconnect_removes_only_reference_and_rejects_path_traversal() {
    let m = manager();
    editor(&m, "alice", "synthetic-token");
    let r = m.snapshot().unwrap().reference(API_BASE.into());
    let lock = m.lock().unwrap();
    write_atomic_private(&m.path(&r.id).unwrap(), &serde_json::to_vec(&r).unwrap()).unwrap();
    drop(lock);
    m.disconnect(&r.id).unwrap();
    assert!(m.list().unwrap().is_empty());
    assert_eq!(m.discover().unwrap().state, "ready");
    assert!(m.disconnect("../apps").is_err());
    fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
}
fn fixture(replies: Vec<(&'static str, Value)>) -> (String, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let handle = std::thread::spawn(move || {
        for (path, reply) in replies {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut b = [0; 1024];
                let n = socket.read(&mut b).unwrap();
                bytes.extend_from_slice(&b[..n]);
                if bytes.windows(4).any(|p| p == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(bytes).unwrap();
            assert!(
                request.starts_with(&format!("GET {path} ")),
                "Unexpected path: {:?}",
                request.lines().next()
            );
            assert!(request.to_ascii_lowercase().contains("authorization: "));
            let body = reply.to_string();
            write!(socket,"HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        }
    });
    (base, handle)
}
fn enabled_catalog() -> Value {
    json!({"data":[{"id":"fixture-model","name":"Fixture Model","model_picker_enabled":true,"capabilities":{"type":"chat","supports":{"tool_calls":true}},"supported_endpoints":["/responses"],"policy":{"state":"enabled"}}]})
}
#[test]
fn verifies_identity_exchanges_editor_token_and_discovers_real_catalog() {
    let mut m = manager();
    editor(&m, "alice", "synthetic-editor-secret");
    let (base, handle) = fixture(vec![
        ("/user", json!({"login":"alice"})),
        (
            "/copilot_internal/v2/token",
            json!({"token":"synthetic-session-secret","expires_at":now()+600,"endpoints":{"api":API_BASE}}),
        ),
        ("/models", enabled_catalog()),
        ("/models", enabled_catalog()),
    ]);
    m.fixture = Some(base);
    let view = m.connect().unwrap();
    assert_eq!(
        m.access_token(&view.id).unwrap(),
        "synthetic-session-secret"
    );
    let connection = m.connection(&view.id).unwrap();
    assert_eq!(connection.models[0].id, "fixture-model");
    assert_eq!(
        connection.metadata["copilot_model_endpoints"]["fixture-model"],
        json!(["/responses"])
    );
    let reference = fs::read_to_string(m.path(&view.id).unwrap()).unwrap();
    assert!(!reference.contains("secret"));
    handle.join().unwrap();
    m.disconnect(&view.id).unwrap();
    assert_eq!(m.discover().unwrap().state, "ready");
    fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
}
#[test]
fn refuses_token_identity_mismatch_and_expired_sessions_without_registration() {
    for (user, expires) in [("bob", now() + 600), ("alice", now() - 1)] {
        let mut m = manager();
        editor(&m, "alice", "synthetic-secret");
        let mut replies = vec![("/user", json!({"login":user}))];
        if user == "alice" {
            replies.push((
                "/copilot_internal/v2/token",
                json!({"token":"expired","expires_at":expires,"endpoints":{"api":API_BASE}}),
            ));
        }
        let (base, handle) = fixture(replies);
        m.fixture = Some(base);
        assert!(m.connect().is_err());
        assert!(m.list().unwrap().is_empty());
        handle.join().unwrap();
        fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
    }
}
#[test]
fn cli_token_uses_direct_transport_without_editor_exchange() {
    let mut m = manager();
    ensure_private_dir(&m.cli_home).unwrap();
    let source = m.cli_home.join("config.json");
    write_atomic_private(&source,serde_json::to_string(&json!({"lastLoggedInUser":{"host":"https://github.com","login":"alice"},"copilotTokens":{"https://github.com:alice":"synthetic-cli-secret"}})).unwrap().as_bytes()).unwrap();
    let (base, handle) = fixture(vec![
        ("/user", json!({"login":"alice"})),
        ("/models", enabled_catalog()),
    ]);
    m.fixture = Some(base);
    let view = m.connect().unwrap();
    assert_eq!(m.access_token(&view.id).unwrap(), "synthetic-cli-secret");
    handle.join().unwrap();
    fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
}
#[cfg(unix)]
#[test]
fn rejects_symlink_and_shared_readable_credentials() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let m = manager();
    editor(&m, "alice", "synthetic-secret");
    let p = m.config.join("github-copilot/apps.json");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(m.discover().unwrap().state, "invalid_source");
    fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
    let moved = p.with_extension("original");
    fs::rename(&p, &moved).unwrap();
    symlink(&moved, &p).unwrap();
    assert_eq!(m.discover().unwrap().state, "invalid_source");
    fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
}
#[test]
fn rejects_a_replaced_token_even_when_local_login_metadata_is_unchanged() {
    let mut m = manager();
    editor(&m, "alice", "initial-secret");
    let (base, handle) = fixture(vec![
        ("/user", json!({"login":"alice"})),
        (
            "/copilot_internal/v2/token",
            json!({"token":"session","expires_at":now()+600,"endpoints":{"api":API_BASE}}),
        ),
        ("/models", enabled_catalog()),
        ("/user", json!({"login":"bob"})),
    ]);
    m.fixture = Some(base);
    let view = m.connect().unwrap();
    editor(&m, "alice", "replaced-secret");
    assert!(m.access_token(&view.id).is_err());
    handle.join().unwrap();
    fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
}
#[test]
fn rejects_an_untrusted_exchanged_endpoint_before_model_credentials_are_sent() {
    let mut m = manager();
    editor(&m, "alice", "initial-secret");
    let (base, handle) = fixture(vec![
        ("/user", json!({"login":"alice"})),
        (
            "/copilot_internal/v2/token",
            json!({"token":"session","expires_at":now()+600,"endpoints":{"api":"https://evil.test"}}),
        ),
    ]);
    m.fixture = Some(base);
    assert!(m.connect().is_err());
    assert!(m.list().unwrap().is_empty());
    handle.join().unwrap();
    fs::remove_dir_all(m.config.parent().unwrap()).unwrap();
}
