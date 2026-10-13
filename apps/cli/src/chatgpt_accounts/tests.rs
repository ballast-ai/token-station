//! Isolated OAuth tests. The checked-in RSA key is test material only.
use super::*;
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
fn scratch() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "ts-siwc-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn signed(claims: &Value) -> String {
    let mut h = Header::new(Algorithm::RS256);
    h.kid = Some("test-key".into());
    encode(
        &h,
        claims,
        &EncodingKey::from_rsa_pem(include_bytes!("fixtures/test-rsa.pem")).unwrap(),
    )
    .unwrap()
}
fn claims() -> Value {
    json!({"iss":"https://auth.openai.com","aud":"client-one","sub":"subject-one","nonce":"nonce-one","iat":now(),"exp":now()+3600,"email":"same@example.com"})
}
#[test]
fn signature_and_required_identity_claims_are_verified() {
    let jwks = serde_json::from_str(include_str!("fixtures/jwks.json")).unwrap();
    assert!(
        validate_id(
            &signed(&claims()),
            &jwks,
            ISSUER,
            "client-one",
            Some("nonce-one"),
            Some("subject-one")
        )
        .is_ok()
    );
    for key in ["iss", "aud", "sub", "exp", "iat", "nonce"] {
        let mut c = claims();
        c.as_object_mut().unwrap().remove(key);
        assert!(
            validate_id(
                &signed(&c),
                &jwks,
                ISSUER,
                "client-one",
                Some("nonce-one"),
                None
            )
            .is_err(),
            "missing {key}"
        );
    }
    for key in ["iss", "aud", "sub", "nonce"] {
        let mut c = claims();
        c[key] = json!("another");
        assert!(
            validate_id(
                &signed(&c),
                &jwks,
                ISSUER,
                "client-one",
                Some("nonce-one"),
                Some("subject-one")
            )
            .is_err(),
            "wrong {key}"
        );
    }
    let mut c = claims();
    c["exp"] = json!(now() - 120);
    assert!(
        validate_id(
            &signed(&c),
            &jwks,
            ISSUER,
            "client-one",
            Some("nonce-one"),
            None
        )
        .is_err()
    );
    let token = signed(&claims());
    let mut chunks: Vec<_> = token.split('.').map(str::to_owned).collect();
    chunks[1] = URL_SAFE_NO_PAD.encode(br#"{"sub":"forged"}"#);
    assert!(
        validate_id(
            &chunks.join("."),
            &jwks,
            ISSUER,
            "client-one",
            Some("nonce-one"),
            None
        )
        .is_err()
    );
}
#[test]
fn account_ids_reject_paths_and_same_email_does_not_merge_registrations() {
    let dir = scratch();
    let m = AccountManager::new(&dir);
    for id in ["../escape", "", "a/b", "a\\b"] {
        assert!(m.access_token(id).is_err());
    }
    for id in ["a".repeat(32), "b".repeat(32)] {
        let mut r = record(&id);
        r.email = Some("same@example.com".into());
        m.save(&r).unwrap();
    }
    assert_eq!(m.list().unwrap().len(), 2);
    fs::remove_dir_all(dir).unwrap();
}
fn record(id: &str) -> Record {
    Record {
        id: id.into(),
        client_id: "client-one".into(),
        subject: "subject-one".into(),
        email: None,
        host_id: "urn:uuid:test".into(),
        generation: 0,
        state: "ready".into(),
        access: Some("access-secret".into()),
        refresh: Some("refresh-secret".into()),
        id_token: None,
        scopes: vec![DIRECT_SCOPE.into()],
        expires_at: Some(now() + 3600),
        remote_revoked: None,
    }
}
#[test]
fn identity_only_never_provides_inference_credentials_and_views_are_redacted() {
    let dir = scratch();
    let m = AccountManager::new(&dir);
    let id = "c".repeat(32);
    let mut r = record(&id);
    r.scopes = vec!["openid".into()];
    r.state = "identity_only".into();
    m.save(&r).unwrap();
    assert!(m.access_token(&id).is_err());
    let view = serde_json::to_string(&m.list().unwrap()).unwrap();
    assert!(!view.contains("secret"));
    assert!(!view.contains("client-one"));
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn returning_authorization_uses_issued_client_and_cancel_is_terminal() {
    let dir = scratch();
    let m = AccountManager::new(&dir);
    let id = "d".repeat(32);
    m.save(&record(&id)).unwrap();
    let attempt = m.start_login(Some(&id)).unwrap();
    let url = url::Url::parse(&attempt.authorization_url).unwrap();
    let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(q["client_id"], "client-one");
    assert!(!q.contains_key("agent_name_hint"));
    m.cancel_login(&attempt.id).unwrap();
    assert_eq!(m.login_status(&attempt.id).unwrap().state, "cancelled");
    fs::remove_dir_all(dir).unwrap();
}

struct FakeServer {
    manager: AccountManager,
    stop: Arc<std::sync::atomic::AtomicBool>,
    refreshes: Arc<AtomicUsize>,
    revokes: Arc<AtomicUsize>,
    calls: Arc<Mutex<Vec<String>>>,
    nonce: Arc<Mutex<String>>,
    handle: Option<std::thread::JoinHandle<()>>,
}
impl FakeServer {
    fn new(dir: &Path, revoke_fails: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let manager = AccountManager {
            root: dir.join("chatgpt-accounts"),
            endpoints: Endpoints {
                issuer: origin.clone(),
                api: format!("{origin}/v1"),
            },
        };
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let refreshes = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&refreshes);
        let revokes = Arc::new(AtomicUsize::new(0));
        let revoke_count = Arc::clone(&revokes);
        let calls = Arc::new(Mutex::new(vec![]));
        let requests = Arc::clone(&calls);
        let nonce = Arc::new(Mutex::new(String::new()));
        let expected_nonce = Arc::clone(&nonce);
        let handle = std::thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                let Ok((mut s, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                s.set_nonblocking(false).unwrap();
                s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut data = Vec::new();
                let mut buf = [0u8; 1024];
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..end]);
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if data.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let raw = String::from_utf8(data).unwrap();
                requests.lock().unwrap().push(raw.clone());
                let path = raw.split_whitespace().nth(1).unwrap_or("");
                let (status,body)=match path {
                    "/.well-known/openid-configuration"=>(200,json!({"issuer":origin,"jwks_uri":format!("{origin}/jwks"),"revocation_endpoint":format!("{origin}/revoke")}).to_string()),
                    "/jwks"=>(200,include_str!("fixtures/jwks.json").into()),
                    "/api/accounts/oauth/token"=>{
                        let n=count.fetch_add(1,Ordering::SeqCst)+1;std::thread::sleep(Duration::from_millis(40));
                        if raw.contains("refresh_token=dead-token") { (400,json!({"error":"invalid_grant","error_description":"secret-must-not-escape"}).to_string()) } else {
                        let mut body=json!({"access_token":format!("access-{n}"),"refresh_token":format!("refresh-{n}"),"expires_in":3600,"token_type":"Bearer","scope":DIRECT_SCOPE});
                        if raw.contains("grant_type=authorization_code") { let mut c=claims(); c["iss"]=json!(origin); c["nonce"]=json!(*expected_nonce.lock().unwrap());body["id_token"]=json!(signed(&c)); }
                        (200,body.to_string())}},
                    "/revoke"=>{revoke_count.fetch_add(1,Ordering::SeqCst);(if revoke_fails{503}else{200},"{}".into())},
                    "/v1/models"=>(200,json!({"models":[{"slug":"visible","display_name":"Visible","visibility":"list","context_window":128_000,"input_modalities":["text","image"]},{"slug":"hidden","visibility":"hide"},{"slug":"unspecified"}]}).to_string()),
                    _=>(404,"{}".into()),
                };
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            manager,
            stop,
            refreshes,
            revokes,
            calls,
            nonce,
            handle: Some(handle),
        }
    }
}
impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            h.join().unwrap();
        }
    }
}
#[test]
fn simultaneous_refreshes_rotate_once_and_survive_manager_restart() {
    let dir = scratch();
    let fake = FakeServer::new(&dir, false);
    let id = "e".repeat(32);
    let mut r = record(&id);
    r.expires_at = Some(now() - 1);
    fake.manager.save(&r).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let mut workers = vec![];
    for _ in 0..8 {
        let m = fake.manager.clone();
        let id = id.clone();
        let b = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            b.wait();
            m.access_token(&id).unwrap()
        }));
    }
    for w in workers {
        assert_eq!(w.join().unwrap(), "access-1");
    }
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);
    let reloaded = fake.manager.load(&id).unwrap();
    assert_eq!(reloaded.refresh.as_deref(), Some("refresh-1"));
    let restarted = AccountManager {
        root: fake.manager.root.clone(),
        endpoints: fake.manager.endpoints.clone(),
    };
    assert_eq!(restarted.access_token(&id).unwrap(), "access-1");
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("refresh_token=refresh-secret")
                && r.contains("client_id=client-one"))
    );
    drop(fake);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn logout_waits_for_rotation_then_clears_the_rotated_session() {
    let dir = scratch();
    let fake = FakeServer::new(&dir, false);
    let id = "f".repeat(32);
    let mut r = record(&id);
    r.expires_at = Some(now() - 1);
    fake.manager.save(&r).unwrap();
    let m = fake.manager.clone();
    let key = id.clone();
    let worker = std::thread::spawn(move || m.access_token(&key));
    let started = Instant::now();
    while fake.refreshes.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(5));
    }
    let result = fake.manager.sign_out(&id).unwrap();
    assert!(result.remote_revoked);
    assert!(fake.manager.sign_out(&id).unwrap().remote_revoked);
    assert!(worker.join().unwrap().is_ok());
    let r = fake.manager.load(&id).unwrap();
    assert_eq!(r.state, "signed_out");
    assert!(r.refresh.is_none());
    assert!(r.access.is_none());
    assert_eq!(r.generation, 3);
    assert!(fake.manager.access_token(&id).is_err());
    assert!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.contains("token=refresh-1") && r.contains("/revoke"))
    );
    drop(fake);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn failed_revocation_is_reported_and_local_tokens_are_removed() {
    let dir = scratch();
    let fake = FakeServer::new(&dir, true);
    let id = "1".repeat(32);
    fake.manager.save(&record(&id)).unwrap();
    let out = fake.manager.sign_out(&id).unwrap();
    assert!(!out.remote_revoked);
    assert!(out.warning.unwrap().contains("unconfirmed"));
    assert_eq!(fake.revokes.load(Ordering::SeqCst), 1);
    assert!(!fake.manager.sign_out(&id).unwrap().remote_revoked);
    assert!(fake.manager.load(&id).unwrap().refresh.is_none());
    drop(fake);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn model_catalog_uses_the_account_token_and_only_explicitly_visible_models() {
    let dir = scratch();
    let fake = FakeServer::new(&dir, false);
    let id = "2".repeat(32);
    fake.manager.save(&record(&id)).unwrap();
    let models = fake.manager.models(&id).unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id, "visible");
    assert_eq!(models[0].supports_images, Some(true));
    assert!(fake.calls.lock().unwrap().iter().any(|r| {
        r.to_lowercase()
            .contains("authorization: bearer access-secret")
    }));
    drop(fake);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn browser_callback_checks_state_verifies_signature_and_completes_only_once() {
    let dir = scratch();
    let fake = FakeServer::new(&dir, false);
    let attempt = fake.manager.start_login(None).unwrap();
    let auth = url::Url::parse(&attempt.authorization_url).unwrap();
    let q: HashMap<_, _> = auth.query_pairs().into_owned().collect();
    assert_eq!(q["client_id"], "dynamic_agent_client");
    *fake.nonce.lock().unwrap() = q["nonce"].clone();
    let mut back = url::Url::parse(&q["redirect_uri"]).unwrap();
    back.query_pairs_mut()
        .append_pair("code", "auth-code")
        .append_pair("client_id", "client-one")
        .append_pair("state", "wrong");
    let _ = ureq::get(back.as_str()).call().unwrap();
    assert_eq!(
        fake.manager.login_status(&attempt.id).unwrap().state,
        "pending"
    );
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 0);
    back.set_query(None);
    back.query_pairs_mut()
        .append_pair("code", "auth-code")
        .append_pair("client_id", "client-one")
        .append_pair("state", &q["state"]);
    let _ = ureq::get(back.as_str()).call().unwrap();
    let status = fake.manager.login_status(&attempt.id).unwrap();
    assert_eq!(status.state, "complete");
    assert_eq!(
        fake.manager
            .access_token(status.account_id.as_ref().unwrap())
            .unwrap(),
        "access-1"
    );
    let _ = ureq::get(back.as_str()).call();
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);
    fake.manager.cancel_login(&attempt.id).unwrap();
    assert_eq!(
        fake.manager.login_status(&attempt.id).unwrap().state,
        "complete"
    );
    drop(fake);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn refresh_child_process() {
    let Some(dir) = std::env::var_os("TS_SIWC_TEST_DIR") else {
        return;
    };
    let origin = std::env::var("TS_SIWC_TEST_ORIGIN").unwrap();
    let manager = AccountManager {
        root: PathBuf::from(dir).join("chatgpt-accounts"),
        endpoints: Endpoints {
            issuer: origin.clone(),
            api: format!("{origin}/v1"),
        },
    };
    assert!(manager.access_token(&"3".repeat(32)).is_ok());
}
#[test]
fn independent_processes_share_one_refresh_rotation() {
    let dir = scratch();
    let fake = FakeServer::new(&dir, false);
    let id = "3".repeat(32);
    let mut r = record(&id);
    r.expires_at = Some(now() - 1);
    fake.manager.save(&r).unwrap();
    let mut children = vec![];
    for _ in 0..3 {
        children.push(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "chatgpt_accounts::tests::refresh_child_process"])
                .env("TS_SIWC_TEST_DIR", &dir)
                .env("TS_SIWC_TEST_ORIGIN", &fake.manager.endpoints.issuer)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap(),
        );
    }
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);
    drop(fake);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn permanently_refused_refresh_clears_credentials_without_leaking_endpoint_errors() {
    let dir = scratch();
    let fake = FakeServer::new(&dir, false);
    let id = "4".repeat(32);
    let mut r = record(&id);
    r.expires_at = Some(now() - 1);
    r.refresh = Some("dead-token".into());
    fake.manager.save(&r).unwrap();
    let error = fake.manager.access_token(&id).unwrap_err();
    assert!(!error.contains("secret-must-not-escape"));
    let r = fake.manager.load(&id).unwrap();
    assert_eq!(r.state, "reauthentication_required");
    assert!(r.refresh.is_none());
    assert!(r.access.is_none());
    drop(fake);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn logout_generation_prevents_an_older_login_from_restoring_the_session() {
    let dir = scratch();
    let fake = FakeServer::new(&dir, false);
    let id = "5".repeat(32);
    fake.manager.save(&record(&id)).unwrap();
    let previous = fake.manager.load(&id).unwrap();
    let pending = Arc::new(Mutex::new(AttemptState {
        status: LoginStatus {
            id: "pending".into(),
            state: "pending".into(),
            account_id: None,
            error: None,
        },
        created: Instant::now(),
    }));
    *fake.nonce.lock().unwrap() = "nonce-one".into();
    let input = LoginInput {
        previous: Some(previous),
        host_id: "host".into(),
        redirect: "http://127.0.0.1:1455/auth/callback".into(),
        state: "state".into(),
        nonce: "nonce-one".into(),
        verifier: "verifier".into(),
    };
    fake.manager.sign_out(&id).unwrap();
    let error = fake
        .manager
        .exchange(&input, "auth-code", Some("client-one"), &pending)
        .unwrap_err();
    assert!(error.contains("changed during sign-in"));
    assert_eq!(fake.manager.load(&id).unwrap().state, "signed_out");
    assert!(fake.manager.access_token(&id).is_err());
    drop(fake);
    fs::remove_dir_all(dir).unwrap();
}
#[test]
fn credential_endpoints_reject_other_origins_and_embedded_credentials() {
    let m = AccountManager::new("unused-test-data");
    assert!(m.allowed_url("https://auth.openai.com/jwks").is_ok());
    for endpoint in [
        "https://example.com/token",
        "http://auth.openai.com/token",
        "https://auth.openai.com.evil.example/token",
        "https://user:secret@auth.openai.com/token",
        "https://auth.openai.com/token#fragment",
    ] {
        assert!(m.allowed_url(endpoint).is_err());
    }
}
