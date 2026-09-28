//! Opt-in production contract check. Default test runs never call `TypeSafe`.
use std::path::PathBuf;
use std::time::Duration;

use token_station_cli::config::EgressConfig;
use token_station_cli::jev::{JevController, Outcome};
use token_station_cli::request_context::RequestContext;
use token_station_protocol::{ChatRequest, Message, Role};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "requires TOKEN_STATION_JEV_LIVE_KEY_FILE and makes four billed synthetic API calls"]
fn live_cloud_client_classifies_synthetic_tasks_without_exposing_credentials() {
    let key_path = std::env::var_os("TOKEN_STATION_JEV_LIVE_KEY_FILE")
        .expect("Set TOKEN_STATION_JEV_LIVE_KEY_FILE to a private credential file.");
    let key = std::fs::read_to_string(key_path).expect("Read the private credential file.");
    let root = Scratch(std::env::temp_dir().join(format!(
        "token-station-jev-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )));
    std::fs::create_dir_all(&root.0).unwrap();
    let controller = JevController::shared(&root.0);
    controller.save_key(key.trim()).unwrap();
    let egress = EgressConfig::default();
    let probe = controller.test_connection(&egress).unwrap();
    assert_eq!(probe.last_outcome, Some(Outcome::Ready));
    assert!(
        !probe.enabled,
        "A connection test must not activate routing."
    );
    controller.set_enabled(true).unwrap();

    let mut accepted = 0;
    for (case, prompt) in [
        ("simple", "Return the result of 2 + 2."),
        (
            "routine",
            "Write a Python function that deduplicates a list while preserving order, and explain its time complexity.",
        ),
        (
            "complex",
            "Design a distributed database migration protocol that preserves serializable transactions during concurrent regional failover. Analyze partition failures, prove safety, and explain the recovery invariants.",
        ),
    ] {
        let request = ChatRequest::new("auto", vec![Message::text(Role::User, prompt)]);
        let context = RequestContext::detached(Duration::from_secs(5), Duration::from_secs(5));
        if let Some(suggestion) = controller.classify(&request, &context, &egress) {
            assert!(controller.is_current(&suggestion));
            controller.finish(suggestion, true);
            accepted += 1;
        }
        let status = controller.status();
        let safe = serde_json::to_string(&status).unwrap();
        assert!(!safe.contains(key.trim()));
        assert!(!safe.contains(prompt));
        println!("live Jev case={case} status={safe}");
        assert!(matches!(
            status.last_outcome,
            Some(Outcome::Applied | Outcome::LowConfidence | Outcome::Timeout)
        ));
    }
    assert!(
        accepted > 0,
        "At least one real classification must complete."
    );
    let removed = controller.clear_key().unwrap();
    assert!(!removed.enabled);
    assert!(!removed.has_key);
}
