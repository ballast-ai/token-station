//! Request finalization must retain local diagnostics even without an upstream attempt.

use super::{Gateway, RequestContext, jev_tests::Endpoint};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use token_station_metrics::{
    ClassifierInputDiagnostic, ClassifierInputHandling, ClassifierInputReason, ClassifierKind,
    Recorder, RequestRecord,
};

#[derive(Default)]
struct Receipts(Mutex<Vec<RequestRecord>>);

impl Recorder for Receipts {
    fn record(&self, record: &RequestRecord) {
        self.0.lock().unwrap().push(record.clone());
    }
}

#[test]
fn classifier_input_survives_upstream_failure_and_no_route_without_crossing_requests() {
    let endpoint = Endpoint::new(
        401,
        json!({"error":{"message":"synthetic refusal","type":"authentication_error"}}),
    );
    let directory =
        std::env::temp_dir().join(format!("ts-classifier-finalize-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let key = directory.join("provider-key");
    std::fs::write(&key, "synthetic-provider-key").unwrap();
    let config = serde_json::from_value(json!({
        "version":1, "server":{"listen":"127.0.0.1:0"},
        "data":{"dir":directory,"metrics":false},
        "plugins":{"dir":directory.join("unused-plugins"),"agents":["agent-openai"],
            "providers":{"openai-compatible":"provider-openai-compatible-v2"}},
        "upstreams":{"fixture":{"provider":"openai-compatible",
            "base_url":endpoint.url.trim_end_matches("/v1/systemone"),
            "auth":{"slot":"provider_api_key","file":key},
            "models":[{"model":"fixture-model","context_window":128_000}]}},
        "router":{"version":1,"honor_exact_model":true,
            "pools":{"main":[{"upstream":"fixture","model":"fixture-model"}]},"default_pool":"main"}
    }))
    .unwrap();
    let records = Arc::new(Receipts::default());
    let gateway = Gateway::new(&config, records.clone()).unwrap();
    let reduced = ClassifierInputDiagnostic {
        classifier: ClassifierKind::Jev,
        handling: ClassifierInputHandling::Reduced,
        reason: Some(ClassifierInputReason::ByteLimit),
    };
    let skipped = ClassifierInputDiagnostic {
        classifier: ClassifierKind::Scx,
        handling: ClassifierInputHandling::Skipped,
        reason: Some(ClassifierInputReason::TokenLimit),
    };
    for (model, diagnostic) in [
        ("absent-model", Some(skipped)),
        ("fixture-model", Some(reduced)),
        ("fixture-model", None),
    ] {
        let context = RequestContext::detached(Duration::from_secs(5), Duration::from_secs(2));
        if let Some(diagnostic) = diagnostic {
            context.set_classifier_input(diagnostic);
        }
        let body = json!({"model":model,"messages":[{"role":"user","content":"private-classifier-finalization-canary"}]}).to_string();
        gateway.chat_scoped(
            &context,
            None,
            None,
            "POST",
            "/v1/chat/completions",
            &[],
            body.as_bytes(),
            &mut |_| true,
        );
        let rows = records.0.lock().unwrap();
        let row = rows.last().unwrap();
        assert_eq!(row.classifier_input, diagnostic);
        assert!(row.error_code.is_some());
        if model == "absent-model" {
            assert_eq!(row.attempts, 0);
            assert!(row.decision.is_none());
        } else {
            assert_eq!(row.status, 401);
            assert_eq!(row.attempts, 1);
            assert_eq!(row.decision.as_ref().unwrap().model, "fixture-model");
        }
        assert!(
            !serde_json::to_string(row)
                .unwrap()
                .contains("private-classifier-finalization-canary")
        );
    }
    assert_eq!(records.0.lock().unwrap().len(), 3);
    drop(gateway);
    std::fs::remove_dir_all(directory).unwrap();
}
