use super::*;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tauri::Manager;

#[test]
fn desktop_update_runtime_support_is_macos_only() {
    #[cfg(target_os = "macos")]
    assert_eq!(desktop_update_platform_unsupported_message(), None);
    #[cfg(target_os = "windows")]
    assert_eq!(
        desktop_update_platform_unsupported_message(),
        Some(desktop_update::WINDOWS_FIRST_RELEASE_UNSUPPORTED_MESSAGE)
    );
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    assert_eq!(
        desktop_update_platform_unsupported_message(),
        Some(desktop_update::MACOS_ONLY_FIRST_RELEASE_UNSUPPORTED_MESSAGE)
    );
}

#[test]
fn south_selector_reports_each_static_ineligibility_without_exposing_secrets() {
    let eligible_draft = json!({
        "plugins": {"providers": {"openai-compatible": "provider-openai-compatible"}},
        "egress": {"mode": "direct"}
    });
    let eligible = json!({
        "provider": "openai-compatible",
        "base_url": "https://provider.example/v1",
        "auth": {"slot": "provider_api_key", "store": true}
    });
    assert_eq!(
        south_v1_unavailable_reason(&eligible_draft, &eligible, true),
        None
    );
    assert_eq!(
        south_v1_unavailable_reason(&eligible_draft, &eligible, false),
        Some("provider_package")
    );

    let cases = [
        (
            json!({"provider": "anthropic", "auth": {"store": true}}),
            eligible_draft.clone(),
            "provider_package",
        ),
        (
            json!({
                "provider": "openai-compatible",
                "api_dialect": "anthropic-native",
                "auth": {"store": true}
            }),
            eligible_draft.clone(),
            "api_dialect",
        ),
        (
            eligible.clone(),
            json!({
                "plugins": {"providers": {"openai-compatible": "provider-openai-compatible"}},
                "egress": {"mode": "proxy", "proxy": "http://secret.example"}
            }),
            "egress",
        ),
        (
            json!({
                "provider": "openai-compatible",
                "auth": {"slot": "provider_api_key", "file": "/private/key"}
            }),
            eligible_draft,
            "auth",
        ),
    ];

    for (upstream, draft, expected) in cases {
        assert_eq!(
            south_v1_unavailable_reason(&draft, &upstream, true),
            Some(expected)
        );
    }
}

#[test]
fn header_auth_selector_is_independent_from_the_legacy_south_modes() {
    let draft = json!({
        "plugins": {"providers": {"openai-compatible": "provider-openai-compatible"}},
        "egress": {"mode": "direct"}
    });
    let azure = json!({
        "provider": "azure-openai-v1",
        "base_url": "https://fixture.openai.azure.com/openai/v1",
        "auth": {"slot": "provider_api_key", "store": true}
    });

    assert_eq!(
        south_v1_unavailable_reason(&draft, &azure, true),
        Some("provider_package"),
        "the old South selector must remain Bearer-only"
    );
    assert_eq!(
        south_header_auth_v1_unavailable_reason(&draft, &azure, true),
        None,
        "the new cumulative selector accepts the exact Azure dialect"
    );

    let unknown = json!({
        "provider": "future-header-provider",
        "auth": {"slot": "provider_api_key", "store": true}
    });
    assert_eq!(
        south_header_auth_v1_unavailable_reason(&draft, &unknown, true),
        Some("provider_package")
    );
}

#[test]
fn prepare_desktop_draft_preserves_omitted_optional_maps() {
    let source = template(
        std::path::Path::new("/tmp/token-station-data"),
        std::path::Path::new("/tmp/plugins"),
    );
    assert_eq!(source["routing"]["mode"], json!("direct"));
    assert_eq!(source["router"]["routing_mode"], json!("tiered"));
    assert!(source["router"].get("direct_target").is_none());
    assert!(source.get("agent_routes").is_none());
    assert!(source.get("profiles").is_none());

    let prepared = prepare_desktop_draft(source, std::path::Path::new("/tmp"));

    assert!(prepared.get("agent_routes").is_none());
    assert!(prepared.get("profiles").is_none());
    serde_json::from_value::<ClientConfig>(prepared)
        .expect("desktop preparation must preserve the ClientConfig shape");
}

#[test]
fn free_provider_catalog_exposes_only_reviewed_free_models() {
    let presets = list_free_provider_presets();
    assert_eq!(presets.len(), 13);
    let nvidia = presets
        .iter()
        .find(|preset| preset.id == "nvidia")
        .expect("NVIDIA is included in the reviewed free catalog");
    assert_eq!(nvidia.upstream_name, "nvidia_free");
    assert_eq!(nvidia.base_url, "https://integrate.api.nvidia.com/v1");
    assert!(!nvidia.models.is_empty());
    assert!(presets
        .iter()
        .all(|preset| preset.upstream_name.ends_with("_free")));
    assert!(presets
        .iter()
        .flat_map(|preset| preset.models)
        .all(|model| {
            model.tool == CapabilityState::Unknown && model.json_schema == CapabilityState::Unknown
        }));
    assert!(["gemini", "hugging_face"].iter().all(|id| {
        presets
            .iter()
            .find(|preset| preset.id == *id)
            .is_some_and(|preset| {
                preset.overage_policy == free_provider_catalog::OveragePolicy::UserMustEnableGuard
            })
    }));
}

#[test]
fn provider_brand_id_uses_curated_identity_and_not_the_editable_upstream_name() {
    assert_eq!(
        provider_brand_id("renamed-account", "https://api.deepseek.com/v1", "paid"),
        Some("deepseek")
    );
    assert_eq!(
        provider_brand_id("nvidia_free", "https://integrate.api.nvidia.com/v1", "free"),
        Some("nvidia")
    );
    assert_eq!(
        provider_brand_id("deepseek", "https://proxy.example.test/v1", "paid"),
        None,
        "a custom endpoint must not inherit a logo from its editable name"
    );
    let root = scratch_home("provider-brand-view");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["renamed-account"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://api.deepseek.com/v1",
        "models": [{"model": "deepseek-chat"}]
    });
    let view = AppInner::new(root.join("token-station.json"), draft, None).snapshot();
    assert_eq!(view.providers[0].brand_id, Some("deepseek"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn free_provider_command_rejects_forged_models_before_network_or_keychain() {
    let root = scratch_home("free-provider-forged-model");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    )))));

    let error = match tauri::async_runtime::block_on(add_free_provider(
        app.state(),
        "nvidia".to_owned(),
        vec!["paid/model".to_owned()],
        "not-a-real-key".to_owned(),
        true,
    )) {
        Err(error) => error,
        Ok(_) => panic!("a model outside the backend allowlist is rejected"),
    };
    assert!(error.contains("免费目录"), "{error}");
    assert!(get_state(app.state()).providers.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn guarded_trial_provider_requires_explicit_quota_protection_confirmation() {
    let root = scratch_home("free-provider-guard");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    )))));

    let error = match tauri::async_runtime::block_on(add_free_provider(
        app.state(),
        "alibaba_model_studio".to_owned(),
        vec!["qwen-turbo".to_owned()],
        "not-a-real-key".to_owned(),
        false,
    )) {
        Err(error) => error,
        Ok(_) => panic!("trial providers cannot continue without the free-quota guard"),
    };
    assert!(error.contains("免费额度保护"), "{error}");
    assert!(get_state(app.state()).providers.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn catalog_managed_free_providers_reject_every_generic_mutator() {
    let root = scratch_home("free-provider-generic-mutation");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["nvidia_free"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://integrate.api.nvidia.com/v1",
        "access_tier": "free",
        "auth": { "slot": "provider_api_key", "store": true },
        "models": [{
            "model": "openai/gpt-oss-120b",
            "tool_state": "unknown",
            "vision_state": "unknown",
            "json_schema_state": "unknown",
            "context_window": 131072
        }]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    let edit_error = match edit_provider(
        app.state(),
        "nvidia_free".to_owned(),
        "https://attacker.invalid/v1".to_owned(),
        None,
    ) {
        Err(error) => error,
        Ok(_) => panic!("a generic edit cannot retarget a free credential"),
    };
    assert!(edit_error.contains("内置目录管理"), "{edit_error}");

    let state = app.state::<AppStateManaged>();
    let mut inner = state.0.lock().unwrap();
    let discovery_error = prepare_discovery_credential(
        &inner,
        "nvidia_free",
        "https://integrate.api.nvidia.com/v1",
        Some("renderer-supplied-key"),
    )
    .expect_err("generic discovery cannot use a free identity");
    assert!(
        discovery_error.contains("内置目录管理"),
        "{discovery_error}"
    );
    let models_error =
        replace_provider_models(&mut inner, "nvidia_free", vec!["paid/model".to_owned()])
            .expect_err("generic model replacement cannot change a free allowlist");
    assert!(models_error.contains("内置目录管理"), "{models_error}");
    let vision_error =
        replace_provider_model_vision(&mut inner, "nvidia_free", "openai/gpt-oss-120b", true)
            .expect_err("generic capability edits cannot change a free allowlist");
    assert!(vision_error.contains("内置目录管理"), "{vision_error}");
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn archived_free_provider_requires_catalog_revalidation_instead_of_restore() {
    let root = scratch_home("free-provider-restore");
    let draft = template_for_test(&root);
    let data_dir = root.join("token-station-data");
    provider_tombstones::archive(
        &data_dir,
        "nvidia_free",
        &json!({
            "provider": "openai-compatible",
            "base_url": "https://integrate.api.nvidia.com/v1",
            "access_tier": "free"
        }),
    )
    .unwrap();
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    let error = match restore_provider(app.state(), "nvidia_free".to_owned()) {
        Err(error) => error,
        Ok(_) => panic!("free provider tombstones cannot bypass catalog revalidation"),
    };
    assert!(error.contains("免费目录重新验证"), "{error}");
    assert!(provider_tombstones::contains(&data_dir, "nvidia_free").unwrap());
    assert!(get_state(app.state()).deleted_providers.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn prepare_desktop_draft_backfills_missing_builtin_agent_adapters() {
    // Legacy agents snapshots omit the later agent-gemini adapter.
    let draft = json!({
        "plugins": { "agents": ["agent-openai", "agent-anthropic", "agent-openai-responses"] }
    });
    let out = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));
    let agents: Vec<String> = out["plugins"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect();
    // Include every desktop_agents() built-in adapter, including agent-gemini, while preserving existing entries.
    assert!(agents.contains(&"agent-openai".to_string()));
    assert!(
        agents.contains(&"agent-gemini".to_string()),
        "agent-gemini 应被补齐,实际 ={agents:?}"
    );
    for adapter in desktop_agents() {
        assert!(
            agents.iter().any(|a| a == adapter),
            "缺 {adapter}:{agents:?}"
        );
    }
}

#[test]
fn prepare_desktop_draft_prunes_dangling_agent_route_and_profile_references() {
    // Only upstream `live` with model `keep` remains. agent_routes and profiles
    // still reference removed provider `gone` and removed model `dropped`.
    let draft = json!({
        "plugins": {"agents": desktop_agents()},
        "upstreams": { "live": { "models": [{ "model": "keep" }] } },
        "routing": {
            "mode": "direct",
            "direct_target": { "upstream": "live", "model": "dropped" }
        },
        "router": {
            "quota_accounts": [
                { "upstream": "gone", "model": "whatever" },
                { "upstream": "live", "model": "keep" },
                { "upstream": "live", "model": "dropped" }
            ]
        },
        "agent_routes": {
            "opencode": {
                "mode": "custom",
                "direct_target": { "upstream": "gone", "model": "whatever" },
                "custom_route": {
                    "high": { "upstream": "gone", "model": "whatever" },
                    "mid": { "upstream": "live", "model": "dropped" },
                    "low": { "upstream": "live", "model": "keep" }
                }
            }
        },
        "profiles": {
            "团队默认": {
                "high": { "upstream": "gone", "model": "x" },
                "mid": { "upstream": "live", "model": "keep" },
                "low": { "upstream": "live", "model": "dropped" }
            }
        }
    });

    let out = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));

    let route = &out["agent_routes"]["opencode"]["custom_route"];
    // A removed provider clears the entire tier.
    assert!(route["high"]["upstream"].is_null());
    assert!(route["high"]["model"].is_null());
    // A removed model clears only the model and keeps the provider.
    assert_eq!(route["mid"]["upstream"], json!("live"));
    assert!(route["mid"]["model"].is_null());
    // Preserve targets that remain valid.
    assert_eq!(route["low"]["upstream"], json!("live"));
    assert_eq!(route["low"]["model"], json!("keep"));

    let profile = &out["profiles"]["团队默认"];
    assert!(profile["high"]["upstream"].is_null());
    assert_eq!(profile["mid"]["model"], json!("keep"));
    assert_eq!(profile["low"]["upstream"], json!("live"));
    assert!(profile["low"]["model"].is_null());
    assert_eq!(out["routing"]["direct_target"]["upstream"], json!("live"));
    assert!(out["routing"]["direct_target"]["model"].is_null());
    assert!(out["router"].get("direct_target").is_none());
    assert!(out["agent_routes"]["opencode"]["direct_target"].is_object());
    assert!(out["agent_routes"]["opencode"]["direct_target"]["upstream"].is_null());
    assert!(out["agent_routes"]["opencode"]["direct_target"]["model"].is_null());
    assert_eq!(
        out["router"]["quota_accounts"],
        json!([{ "upstream": "live", "model": "keep" }])
    );
}

#[test]
fn known_context_window_reads_size_markers_then_family_defaults() {
    // Prefer explicit size markers supplied in model names.
    assert_eq!(known_context_window("glm-5.2[1m]"), 1_000_000);
    assert_eq!(known_context_window("moonshot-v1-128k"), 128_000);
    assert_eq!(known_context_window("qwen-turbo-1m"), 1_000_000);
    assert_eq!(known_context_window("gpt-4-32k"), 32_000);
    // Fall back to the family default when no marker exists.
    assert_eq!(known_context_window("gemini-2.5-pro"), 1_000_000);
    assert_eq!(known_context_window("claude-opus-4-8"), 200_000);
    // Unknown families and version numbers use the 128k fallback without false inference.
    assert_eq!(known_context_window("deepseek-v4-pro"), 128_000);
    assert_eq!(known_context_window("glm-4.6"), 128_000);
    assert_eq!(known_context_window("some-obscure-model"), 128_000);
}

#[test]
fn desktop_preparation_backfills_exact_kimi_models_from_builtin_limits() {
    let draft = json!({
        "upstreams": {
            "kimi": {
                "provider": "openai-compatible",
                "base_url": "https://api.moonshot.cn/v1/",
                "models": [
                    {"model": "kimi-k2.6", "context_window": 128000},
                    {"model": "kimi-k3", "context_window": 128000}
                ]
            }
        }
    });

    let prepared = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));
    let models = prepared["upstreams"]["kimi"]["models"].as_array().unwrap();
    assert_eq!(models[0]["context_window"], json!(262_144));
    assert_eq!(models[0]["max_output_tokens"], json!(262_144));
    assert_eq!(
        models[0]["x-token-station-context-window-source"],
        json!("builtin_preset")
    );
    assert_eq!(models[1]["context_window"], json!(1_048_576));
    assert_eq!(models[1]["max_output_tokens"], json!(131_072));
    assert_eq!(
        models[1]["x-token-station-max-output-tokens-source"],
        json!("builtin_preset")
    );
}

#[test]
fn builtin_limits_do_not_match_unofficial_endpoints_similar_ids_or_operator_values() {
    let draft = json!({
        "upstreams": {
            "gateway": {
                "provider": "openai-compatible",
                "base_url": "https://gateway.example/v1",
                "models": [{"model": "kimi-k3", "context_window": 128000}]
            },
            "kimi": {
                "provider": "openai-compatible",
                "base_url": "https://api.moonshot.cn/v1",
                "models": [
                    {"model": "kimi-k3-preview", "context_window": 128000},
                    {
                        "model": "kimi-k3",
                        "context_window": 64000,
                        "max_output_tokens": 8000,
                        "x-token-station-context-window-source": "operator",
                        "x-token-station-max-output-tokens-source": "operator"
                    }
                ]
            }
        }
    });

    let prepared = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));
    assert!(prepared["upstreams"]["gateway"]["models"][0]
        .get("max_output_tokens")
        .is_none());
    assert!(prepared["upstreams"]["kimi"]["models"][0]
        .get("max_output_tokens")
        .is_none());
    assert_eq!(
        prepared["upstreams"]["kimi"]["models"][1]["context_window"],
        json!(64_000)
    );
    assert_eq!(
        prepared["upstreams"]["kimi"]["models"][1]["max_output_tokens"],
        json!(8_000)
    );
}

#[test]
fn prepare_desktop_draft_keeps_free_capabilities_fail_closed() {
    let root = scratch_home("free-capability-migration");
    let draft = json!({
        "plugins": {"agents": desktop_agents(), "dir": root.join("plugins")},
        "data": {"dir": root.join("data")},
        "upstreams": {
            "nvidia_free": {
                "access_tier": "free",
                "models": [{
                    "model": "openai/gpt-oss-120b",
                    "tool": false,
                    "json_schema": false,
                    "tool_state": "unknown",
                    "json_schema_state": "unknown"
                }]
            }
        }
    });

    let migrated = prepare_desktop_draft(draft, &root);
    let model = &migrated["upstreams"]["nvidia_free"]["models"][0];
    assert_eq!(model["tool_state"], "unknown");
    assert_eq!(model["json_schema_state"], "unknown");
    assert_eq!(model["tool"], false);
    assert_eq!(model["json_schema"], false);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn prepare_desktop_draft_upgrades_unknown_tool_capability_but_keeps_unsupported() {
    let draft = json!({
        "upstreams": {
            "deepseek": { "models": [
                { "model": "a", "tool_state": "unknown", "json_schema_state": "unknown", "vision_state": "unknown" },
                { "model": "b", "tool_state": "unsupported", "json_schema_state": "verified", "vision_state": "unknown" },
            ]}
        }
    });
    let out = prepare_desktop_draft(draft, std::path::Path::new("/tmp"));
    let models = out["upstreams"]["deepseek"]["models"].as_array().unwrap();
    // Promote tools and structured output from unknown to declared while keeping vision unknown.
    assert_eq!(models[0]["tool_state"], json!("declared"));
    assert_eq!(models[0]["json_schema_state"], json!("declared"));
    assert_eq!(models[0]["vision_state"], json!("unknown"));
    // Do not overwrite explicit operator-set unsupported or verified states.
    assert_eq!(models[1]["tool_state"], json!("unsupported"));
    assert_eq!(models[1]["json_schema_state"], json!("verified"));
}

fn scratch_home(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "token-station-desktop-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).expect("scratch home is writable");
    path
}

fn template_for_test(root: &std::path::Path) -> Value {
    let mut draft = template(&root.join("token-station-data"), &root.join("plugins"));
    draft
        .as_object_mut()
        .expect("config fixture is an object")
        .remove("routing");
    draft
}

#[test]
fn state_snapshot_uses_cached_south_eligibility() {
    let root = scratch_home("cached-south-eligibility");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://api.example.test/v1",
        "auth": { "slot": "provider_api_key", "store": true },
        "models": [{ "model": "gpt-test" }],
        "provider_call": "south_v1_buffered"
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    inner
        .south_approved_dialects
        .insert("openai-compatible".to_owned());
    let invalid_package = root.join("plugins/broken");
    std::fs::create_dir_all(&invalid_package).expect("plugin fixture directory is writable");
    std::fs::write(invalid_package.join("manifest.json"), "not JSON")
        .expect("plugin fixture is writable");

    let view = inner.snapshot();

    assert_eq!(view.providers.len(), 1);
    assert!(view.providers[0].south_v1_available);
    assert_eq!(view.providers[0].south_v1_unavailable_reason, None);
    std::fs::remove_dir_all(root).ok();
}

fn gateway_template_for_test(root: &std::path::Path) -> Value {
    let plugins_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("plugins-dist");
    let mut draft = template(&root.join("token-station-data"), &plugins_dir);
    draft
        .as_object_mut()
        .expect("config fixture is an object")
        .remove("routing");
    draft
}

fn published_agent_route_fixture(root: &std::path::Path) -> (Value, RunningServer) {
    let mut draft = gateway_template_for_test(root);
    draft["server"]["listen"] = json!("127.0.0.1:0");
    draft["server"]["auth"] = json!(false);
    draft["data"]["metrics"] = json!(false);
    draft["upstreams"]["local"] = json!({
        "provider": "openai-compatible",
        "base_url": "http://127.0.0.1:11434/v1",
        "models": [{"model": "small"}]
    });
    draft["router"]["pools"] = json!({
        TIER_LOW: [{"upstream": "local", "model": "small"}]
    });
    draft["router"]["default_pool"] = json!(TIER_LOW);
    let config: ClientConfig = serde_json::from_value(draft.clone()).unwrap();
    let running = prepare_server(config)
        .unwrap()
        .bind()
        .unwrap()
        .publish(7)
        .unwrap();
    (draft, running)
}

fn manage_test_agent_state<R: Runtime>(app: &tauri::App<R>, root: &std::path::Path) {
    let paths = AgentIntegrationPaths {
        snapshot_root: root.join("agent-data/snapshots"),
        ownership_root: root.join("agent-data/ownership"),
    };
    assert!(app.manage(paths.clone()));
    assert!(app.manage(AgentCommandState::new(paths).expect("Agent command state initializes")));
}

fn serve_model_catalog(
    responses: Vec<(u16, &'static str)>,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("model catalog fixture binds");
    listener
        .set_nonblocking(true)
        .expect("model catalog fixture is nonblocking");
    let address = listener
        .local_addr()
        .expect("model catalog fixture has an address");
    let worker = std::thread::spawn(move || {
        for (status, body) in responses {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "model catalog discovery request did not arrive before deadline"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("model catalog fixture accept failed: {error}"),
                }
            };
            stream
                .set_nonblocking(false)
                .expect("accepted model catalog socket is blocking");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("model catalog fixture read is bounded");
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .expect("model catalog fixture write is bounded");
            let mut request = [0u8; 2048];
            let read = stream
                .read(&mut request)
                .expect("model catalog fixture reads the request");
            assert!(read > 0, "model catalog request must not be empty");
            let response = format!(
                "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("model catalog fixture responds");
        }
    });
    (format!("http://{address}"), worker)
}

fn serve_chat_completion(
    marker: &'static str,
    requests: usize,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("chat fixture binds");
    let address = listener.local_addr().expect("chat fixture has an address");
    let worker = std::thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().expect("chat request arrives");
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let read = stream.read(&mut chunk).expect("chat fixture reads request");
                assert!(read > 0, "chat request ended before its declared body");
                request.extend_from_slice(&chunk[..read]);
                let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if request.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            assert!(
                String::from_utf8_lossy(&request).contains("/v1/chat/completions"),
                "gateway must call the configured chat endpoint"
            );
            let streaming = String::from_utf8_lossy(&request).contains(r#""stream":true"#);
            let (content_type, body) = if streaming {
                (
                    "text/event-stream",
                    format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({
                            "id": format!("fixture-{marker}"),
                            "object": "chat.completion.chunk",
                            "created": 1,
                            "model": "small",
                            "choices": [{
                                "index": 0,
                                "delta": {"content": marker},
                                "finish_reason": null
                            }]
                        })
                    ),
                )
            } else {
                (
                    "application/json",
                    json!({
                        "id": format!("fixture-{marker}"),
                        "object": "chat.completion",
                        "created": 1,
                        "model": "small",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": marker},
                            "finish_reason": "stop"
                        }],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
                    })
                    .to_string(),
                )
            };
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("chat fixture responds");
        }
    });
    (format!("http://{address}/v1"), worker)
}

fn chat_through_proxy(listen: &str) -> String {
    let body = r#"{"model":"auto","messages":[{"role":"user","content":"ping"}]}"#;
    let mut stream = std::net::TcpStream::connect(listen).expect("proxy listener is reachable");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .unwrap();
    write!(
        stream,
        "POST /v1/chat/completions HTTP/1.1\r\nhost: {listen}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("proxy request writes");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("proxy response reads");
    response
}

fn wait_for_serve_phase<R: Runtime>(app: &tauri::App<R>, expected: ServePhase) -> StateView {
    wait_for_serve_phase_with_timeout(app, expected, Duration::from_secs(60))
}

fn wait_for_serve_phase_with_timeout<R: Runtime>(
    app: &tauri::App<R>,
    expected: ServePhase,
    timeout: Duration,
) -> StateView {
    let deadline = Instant::now() + timeout;
    loop {
        let state = get_state(app.state());
        if state.serve.phase == expected {
            return state;
        }
        assert!(
            expected == ServePhase::Error || state.serve.phase != ServePhase::Error,
            "serve phase entered Error before {expected:?}; error={:?}",
            state.serve.error
        );
        assert!(
            Instant::now() < deadline,
            "serve phase did not reach {expected:?}; current={:?}, error={:?}",
            state.serve.phase,
            state.serve.error
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_receipts(path: &std::path::Path, expected: usize) -> Vec<ReceiptView> {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let receipts = SqliteStore::recent_receipts(path, 5).expect("receipts read");
        if receipts.len() >= expected {
            return receipts;
        }
        assert!(
            Instant::now() < deadline,
            "receipt count did not reach {expected}; current={}",
            receipts.len()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn desktop_shell_distinguishes_live_application_from_listener_handoff() {
    assert_eq!(
        desktop_shell_applying_phase(true, true),
        desktop_shell::ProxyMenuPhase::Applying
    );
    assert_eq!(
        desktop_shell_applying_phase(true, false),
        desktop_shell::ProxyMenuPhase::Switching
    );
    assert_eq!(
        desktop_shell_applying_phase(false, true),
        desktop_shell::ProxyMenuPhase::Switching
    );
    assert_eq!(
        desktop_shell_applying_phase(false, false),
        desktop_shell::ProxyMenuPhase::Switching
    );
}

#[test]
fn status_menu_actions_require_the_same_generation_and_lifecycle_action() {
    assert!(menu_action_expectation_matches(
        7,
        7,
        desktop_shell::ProxyMenuAction::Start,
        desktop_shell::ProxyMenuAction::Start,
    ));
    assert!(menu_action_expectation_matches(
        7,
        7,
        desktop_shell::ProxyMenuAction::Stop,
        desktop_shell::ProxyMenuAction::Stop,
    ));
    assert!(!menu_action_expectation_matches(
        7,
        8,
        desktop_shell::ProxyMenuAction::Start,
        desktop_shell::ProxyMenuAction::Start,
    ));
    assert!(!menu_action_expectation_matches(
        7,
        7,
        desktop_shell::ProxyMenuAction::Start,
        desktop_shell::ProxyMenuAction::Stop,
    ));
}

#[test]
fn the_desktop_template_enables_every_supported_inbound_protocol() {
    let root = PathBuf::from("/tmp/token-station-desktop-test");
    let draft = template_for_test(&root);

    assert_eq!(draft["plugins"]["agents"], json!(desktop_agents()));
}

#[test]
fn desktop_paths_stay_inside_tauri_roots_and_create_writable_directories() {
    let root = scratch_home("tauri-paths");
    let config_root = root.join("config");
    let data_root = root.join("data");
    let paths = DesktopPaths::from_app_roots(config_root.clone(), data_root.clone());

    assert_eq!(paths.config_file, config_root.join("token-station.json"));
    assert_eq!(paths.data_dir, data_root.join("token-station-data"));
    assert_eq!(paths.plugins_dir, data_root.join("plugins"));
    assert_eq!(paths.agent_data_root, data_root.join("agent-integration"));

    std::fs::create_dir_all(&config_root).unwrap();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    std::fs::write(&paths.config_file, b"{}").unwrap();
    let legacy_cache = paths.data_dir.join("model-catalog-cache.json");
    std::fs::write(&legacy_cache, b"{}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&config_root, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&paths.data_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&paths.config_file, std::fs::Permissions::from_mode(0o644))
            .unwrap();
        std::fs::set_permissions(&legacy_cache, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    paths.create_writable_dirs().unwrap();
    assert!(config_root.is_dir());
    assert!(paths.data_dir.is_dir());
    assert!(paths.plugins_dir.is_dir());
    assert!(paths.agent_data_root.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&config_root)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&paths.data_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&paths.config_file)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&legacy_cache)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    let draft = template(&paths.data_dir, &paths.plugins_dir);
    assert_eq!(draft["data"]["dir"], json!(paths.data_dir));
    assert_eq!(draft["plugins"]["dir"], json!(paths.plugins_dir));
    std::fs::remove_dir_all(root).ok();
}

#[cfg(feature = "bundled-plugins")]
#[test]
fn desktop_bundled_plugins_load_without_an_external_plugin_directory() {
    let root = scratch_home("bundled-plugins");
    let output = root.join("installed-self-test.json");
    run_installed_self_test(&output).expect("the installed-artifact self-test passes");
    let report: Value =
        serde_json::from_slice(&std::fs::read(&output).expect("the self-test report was written"))
            .expect("the self-test report is JSON");
    assert_eq!(report["passed"], json!(true));
    // The set, not a count. This assertion used to read `Some(5)`, and it
    // went stale the moment `provider-anthropic` joined the bundle — the
    // desktop crate is excluded from the workspace, so
    // `cargo test --workspace` never ran it and only the desktop build
    // gate noticed. `official_package_set.rs` checks that every consumer
    // *names* each package; a bare integer is not a name, so it could not
    // see this one. Naming them puts this test back under that gate.
    let reported: Vec<&str> = report["plugins"]
        .as_array()
        .expect("the report lists the bundled plugins")
        .iter()
        .map(|plugin| plugin["id"].as_str().expect("a plugin id is a string"))
        .collect();
    assert_eq!(reported, token_station_cli::plugins::OFFICIAL_PACKAGE_IDS);
    assert_eq!(report["storage"]["credential_read"], json!(false));
    assert_eq!(report["gateway"]["loadable"], json!(true));
    assert!(report["gateway"]["provider_dialects"]
        .as_array()
        .is_some_and(|dialects| dialects.iter().any(|dialect| dialect == "azure-openai-v1")));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn enterprise_verification_never_persists_the_discovered_catalog() {
    const CATALOG: &str = r#"{"data":[{"id":"private-enterprise-model"}]}"#;
    let root = scratch_home("enterprise-verification-isolation");
    let data_dir = root.join("token-station-data");
    let (base_url, server) = serve_model_catalog(vec![(200, CATALOG)]);
    let mut draft = template_for_test(&root);
    draft["data"]["dir"] = json!(data_dir.clone());
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    let result = tauri::async_runtime::block_on(verify_enterprise_route(
        app.state(),
        "enterprise_main".to_owned(),
        base_url,
        "secret-key".to_owned(),
    ))
    .expect("live enterprise verification succeeds");

    assert_eq!(result.source, "live");
    assert_eq!(result.models, ["private-enterprise-model"]);
    assert!(!data_dir.join("model-catalog-cache.json").exists());
    assert!(get_state(app.state()).providers.is_empty());
    server.join().expect("model catalog fixture exits");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn repeated_model_discovery_only_updates_the_catalog_cache() {
    const CATALOG: &str = r#"{"data":[{"id":"model-b"},{"id":"model-a"}]}"#;

    let root = scratch_home("discovery-isolation");
    let data_dir = root.join("token-station-data");
    let config_path = root.join("token-station.json");
    let (base_url, server) = serve_model_catalog(vec![
        (200, CATALOG),
        (200, CATALOG),
        (200, CATALOG),
        (503, r#"{"error":"offline"}"#),
    ]);
    let mut draft = template_for_test(&root);
    draft["data"]["dir"] = json!(data_dir.clone());
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": base_url,
        "models": [{"model": "configured-model"}]
    });
    let expected_draft = draft.clone();
    // Compact JSON is intentionally different from the normal pretty save
    // format, so even a semantically identical rewrite fails this check.
    let expected_config =
        serde_json::to_vec(&expected_draft).expect("saved config fixture serializes");
    std::fs::write(&config_path, &expected_config).expect("saved config fixture writes");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        config_path.clone(),
        draft,
        None,
    )))));
    let agent_paths = AgentIntegrationPaths {
        snapshot_root: root.join("agent-data/snapshots"),
        ownership_root: root.join("agent-data/ownership"),
    };
    assert!(app.manage(agent_paths.clone()));
    assert!(
        app.manage(AgentCommandState::new(agent_paths).expect("Agent command state initializes"))
    );
    let initial_state = get_state(app.state());
    assert_eq!(initial_state.draft_revision, initial_state.saved_revision);
    assert!(!initial_state.config_dirty);

    for _ in 0..3 {
        let result = tauri::async_runtime::block_on(discover_provider_models(
            app.state(),
            "fixture".to_owned(),
            base_url.clone(),
            None,
        ))
        .expect("live discovery succeeds");
        assert_eq!(result.source, "live");
        assert_eq!(result.models, ["model-a", "model-b"]);
        assert_eq!(
            app.state::<AppStateManaged>()
                .inner()
                .0
                .lock()
                .unwrap()
                .draft,
            expected_draft
        );
        assert_eq!(
            std::fs::read(&config_path).expect("saved config remains readable"),
            expected_config
        );
        let state = get_state(app.state());
        assert_eq!(state.draft_revision, initial_state.draft_revision);
        assert_eq!(state.saved_revision, initial_state.saved_revision);
        assert!(!state.config_dirty);
    }

    let cached = tauri::async_runtime::block_on(discover_provider_models(
        app.state(),
        "fixture".to_owned(),
        base_url,
        None,
    ))
    .expect("offline discovery falls back to its cache");
    assert_eq!(cached.source, "cache");
    assert_eq!(cached.models, ["model-a", "model-b"]);
    assert_eq!(
        app.state::<AppStateManaged>()
            .inner()
            .0
            .lock()
            .unwrap()
            .draft,
        expected_draft
    );
    assert_eq!(
        std::fs::read(&config_path).expect("saved config remains readable"),
        expected_config
    );
    let cached_state = get_state(app.state());
    assert_eq!(cached_state.draft_revision, initial_state.draft_revision);
    assert_eq!(cached_state.saved_revision, initial_state.saved_revision);
    assert!(!cached_state.config_dirty);
    assert!(data_dir.join("model-catalog-cache.json").is_file());
    server.join().expect("model catalog fixture exits");

    {
        let managed = app.state::<AppStateManaged>();
        let mut inner = managed.0.lock().unwrap();
        inner.draft["upstreams"]["fixture"]["models"] = json!([{"model": "model-a"}]);
        inner.observe_draft().unwrap();
    }
    let explicitly_edited = get_state(app.state());
    assert!(explicitly_edited.draft_revision > initial_state.draft_revision);
    assert_eq!(
        explicitly_edited.saved_revision,
        initial_state.saved_revision
    );
    assert!(explicitly_edited.config_dirty);

    let warning_root = scratch_home("discovery-cache-warning");
    let warning_data = warning_root.join("data");
    std::fs::create_dir_all(warning_data.join("model-catalog-cache.json"))
        .expect("directory fixture blocks the cache rename");
    let warning_config = warning_root.join("token-station.json");
    let (warning_base, warning_server) = serve_model_catalog(vec![(200, CATALOG)]);
    let mut warning_draft = template_for_test(&warning_root);
    warning_draft["data"]["dir"] = json!(warning_data);
    warning_draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": warning_base,
        "models": [{"model": "configured-model"}]
    });
    let expected_warning_draft = warning_draft.clone();
    let expected_warning_config =
        serde_json::to_vec(&expected_warning_draft).expect("warning config fixture serializes");
    std::fs::write(&warning_config, &expected_warning_config)
        .expect("warning config fixture writes");
    let warning_app = tauri::test::mock_app();
    assert!(warning_app.manage(AppStateManaged(Mutex::new(AppInner::new(
        warning_config.clone(),
        warning_draft,
        None,
    )))));
    let warning_initial_state = get_state(warning_app.state());

    let warning = tauri::async_runtime::block_on(discover_provider_models(
        warning_app.state(),
        "fixture".to_owned(),
        warning_base,
        None,
    ))
    .expect("cache failure remains a live discovery result");
    assert_eq!(warning.source, "live");
    assert_eq!(warning.models, ["model-a", "model-b"]);
    assert!(warning
        .warning
        .as_deref()
        .is_some_and(|message| message.contains("保存模型缓存失败")));
    assert_eq!(
        warning_app
            .state::<AppStateManaged>()
            .inner()
            .0
            .lock()
            .unwrap()
            .draft,
        expected_warning_draft
    );
    assert_eq!(
        std::fs::read(&warning_config).expect("warning config remains readable"),
        expected_warning_config
    );
    let warning_state = get_state(warning_app.state());
    assert_eq!(
        warning_state.draft_revision,
        warning_initial_state.draft_revision
    );
    assert_eq!(
        warning_state.saved_revision,
        warning_initial_state.saved_revision
    );
    assert!(!warning_state.config_dirty);
    warning_server.join().expect("warning fixture exits");

    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(warning_root).ok();
}

#[test]
fn remote_http_discovery_fails_before_network_access_even_without_credentials() {
    let root = scratch_home("credentialed-http-discovery");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    )))));

    let error = tauri::async_runtime::block_on(discover_provider_models(
        app.state(),
        "remote_http".to_owned(),
        "http://192.0.2.1/v1".to_owned(),
        None,
    ))
    .expect_err("a remote plaintext endpoint must fail before the request starts");

    assert!(error.contains("must use HTTPS"), "{error}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_legacy_chat_only_config_is_migrated_in_memory_with_absolute_runtime_paths() {
    let root = scratch_home("legacy");
    let mut draft = template_for_test(&root);
    draft["plugins"].as_object_mut().unwrap().remove("agents");
    draft["plugins"]["agent"] = json!("agent-openai");
    draft["plugins"]["dir"] = json!("plugins-dist");
    draft["data"]["dir"] = json!("token-station-data");

    let saved = draft.clone();
    let prepared = prepare_desktop_draft(draft, &root);

    assert_eq!(prepared["plugins"]["agents"], json!(desktop_agents()));
    assert!(prepared["plugins"].get("agent").is_none());
    assert_eq!(prepared["plugins"]["dir"], json!(root.join("plugins-dist")));
    assert_eq!(
        prepared["data"]["dir"],
        json!(root.join("token-station-data"))
    );
    let inner = AppInner::new_with_saved(root.join("token-station.json"), prepared, saved, None);
    assert!(inner.config_state.is_dirty());
    assert_ne!(
        inner.config_state.draft_revision(),
        inner.config_state.saved_revision()
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_desktop_v1_agent_list_is_migrated_to_every_supported_inbound_protocol() {
    let root = scratch_home("desktop-v1-agents");
    let mut draft = template_for_test(&root);
    draft["plugins"]["agents"] = json!(["agent-openai"]);

    let prepared = prepare_desktop_draft(draft, &root);

    assert_eq!(prepared["plugins"]["agents"], json!(desktop_agents()));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_broken_existing_config_enters_read_only_protection_without_overwrite() {
    let root = scratch_home("broken-config");
    let path = root.join("token-station.json");
    let original = b"{ definitely not json";
    std::fs::write(&path, original).unwrap();

    let (_draft, error) = load_draft(&path, &root);

    assert!(error.as_deref().is_some_and(|e| e.contains("只读保护")));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn dangling_direct_and_quota_references_load_as_an_editable_dirty_draft() {
    let root = scratch_home("dangling-direct-startup-repair");
    let path = root.join("token-station.json");
    let mut source = template_for_test(&root);
    source["upstreams"]["live"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "keep"}]
    });
    source["routing"] = json!({
        "mode": "direct",
        "direct_target": {"upstream": "live", "model": "dropped"}
    });
    source["router"]["quota_accounts"] = json!([
        {"upstream": "gone", "model": "missing"},
        {"upstream": "live", "model": "keep"},
        {"upstream": "live", "model": "dropped"}
    ]);
    source["agent_routes"] = json!({
        "codex": {
            "mode": "inherit",
            "routing_mode": "direct",
            "direct_target": {"upstream": "gone", "model": "missing"}
        }
    });
    // The recovery path must apply the exact same legacy load migration as
    // ClientConfig::load before auditing the known dangling references.
    source["concurrency"] = json!({
        "global": 0,
        "per_agent": 0,
        "per_provider": 0
    });
    let original = serde_json::to_vec(&source).expect("dangling fixture serializes");
    std::fs::write(&path, &original).expect("dangling fixture writes");

    let (draft, saved, load_error) = load_draft_state(
        &path,
        &root.join("token-station-data"),
        &root.join("plugins"),
    );

    assert_eq!(load_error, None);
    assert_eq!(draft["routing"]["mode"], json!("direct"));
    assert_eq!(draft["routing"]["direct_target"]["upstream"], json!("live"));
    assert!(draft["routing"]["direct_target"]["model"].is_null());
    assert!(draft["agent_routes"]["codex"]["direct_target"].is_object());
    assert!(draft["agent_routes"]["codex"]["direct_target"]["upstream"].is_null());
    assert!(draft["agent_routes"]["codex"]["direct_target"]["model"].is_null());
    assert_eq!(
        draft["router"]["quota_accounts"],
        json!([{"upstream": "live", "model": "keep"}])
    );
    for field in ["global", "per_agent", "per_provider"] {
        assert!(draft["concurrency"][field].as_u64().unwrap_or_default() > 0);
        assert!(saved["concurrency"][field].as_u64().unwrap_or_default() > 0);
    }
    assert_eq!(
        saved["routing"]["direct_target"],
        json!({"upstream": "live", "model": "dropped"})
    );
    assert_eq!(
        std::fs::read(&path).expect("startup repair never rewrites source"),
        original
    );

    let inner = AppInner::new_with_saved(path, draft, saved, load_error);
    assert!(inner.ensure_editable().is_ok());
    assert!(inner.config_state.is_dirty());
    assert!(inner.materialize().is_err());
    let wire = serde_json::to_value(inner.snapshot()).expect("StateView serializes");
    assert_eq!(
        wire["direct_target"],
        json!({"upstream": "live", "model": null})
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn dangling_agent_direct_target_never_silently_inherits_a_valid_home_target() {
    let root = scratch_home("dangling-agent-direct-does-not-inherit-home");
    let path = root.join("token-station.json");
    let mut source = template_for_test(&root);
    source["upstreams"]["live"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "keep"}]
    });
    source["routing"] = json!({
        "mode": "direct",
        "direct_target": {"upstream": "live", "model": "keep"}
    });
    source["agent_routes"] = json!({
        "codex": {
            "mode": "inherit",
            "routing_mode": "direct",
            "direct_target": {"upstream": "gone", "model": "missing"}
        }
    });
    let original = serde_json::to_vec(&source).expect("dangling fixture serializes");
    std::fs::write(&path, &original).expect("dangling fixture writes");

    let (draft, saved, load_error) = load_draft_state(
        &path,
        &root.join("token-station-data"),
        &root.join("plugins"),
    );

    assert_eq!(load_error, None);
    assert!(draft["agent_routes"]["codex"]["direct_target"].is_object());
    assert!(draft["agent_routes"]["codex"]["direct_target"]["upstream"].is_null());
    assert!(draft["agent_routes"]["codex"]["direct_target"]["model"].is_null());
    let inner = AppInner::new_with_saved(path.clone(), draft, saved, load_error);
    let view = inner.snapshot();
    assert_eq!(
        view.direct_target.as_ref().unwrap().model.as_deref(),
        Some("keep")
    );
    assert!(view.agent_routes["codex"].direct_target.is_none());
    assert!(view.agent_routes["codex"].config_error.is_some());
    let wire = serde_json::to_value(&view).expect("StateView serializes");
    assert_eq!(
        wire["direct_target"],
        json!({"upstream": "live", "model": "keep"})
    );
    assert!(wire["agent_routes"]["codex"]["direct_target"].is_null());
    assert_eq!(
        std::fs::read(&path).expect("startup repair remains zero-write"),
        original
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn dangling_direct_recovery_keeps_unrelated_semantic_damage_read_only() {
    let root = scratch_home("dangling-direct-with-unrelated-damage");
    let path = root.join("token-station.json");
    let mut source = template_for_test(&root);
    source["server"]["listen"] = json!("0.0.0.0:8787");
    source["upstreams"]["live"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "keep"}]
    });
    source["routing"] = json!({
        "mode": "direct",
        "direct_target": {"upstream": "live", "model": "dropped"}
    });
    let original = serde_json::to_vec(&source).expect("damaged fixture serializes");
    std::fs::write(&path, &original).expect("damaged fixture writes");

    let (_draft, _saved, load_error) = load_draft_state(
        &path,
        &root.join("token-station-data"),
        &root.join("plugins"),
    );

    let error = load_error.expect("unrelated semantic damage remains protected");
    assert!(error.contains("只读保护"), "{error}");
    assert_eq!(
        std::fs::read(&path).expect("damaged source remains untouched"),
        original
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn loading_config_without_agent_routes_or_profiles_stays_materializable() {
    let root = scratch_home("omitted-routing-maps");
    let path = root.join("token-station.json");
    let source = template_for_test(&root);
    assert!(source.get("agent_routes").is_none());
    assert!(source.get("profiles").is_none());
    let original = serde_json::to_vec(&source).expect("config fixture serializes");
    std::fs::write(&path, &original).expect("config fixture writes");

    let (draft, saved, error) = load_draft_state(
        &path,
        &root.join("token-station-data"),
        &root.join("plugins"),
    );

    assert_eq!(error, None);
    assert!(saved.get("agent_routes").is_none());
    assert!(saved.get("profiles").is_none());
    assert!(draft.get("agent_routes").is_none());
    assert!(draft.get("profiles").is_none());
    serde_json::from_value::<ClientConfig>(draft)
        .expect("loaded desktop draft remains structurally valid");
    assert_eq!(
        std::fs::read(&path).expect("source config remains readable"),
        original
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn loading_legacy_null_optional_maps_recovers_without_read_only_lockout() {
    let root = scratch_home("null-optional-routing-maps");
    let path = root.join("token-station.json");
    let mut source = template_for_test(&root);
    source["agent_routes"] = Value::Null;
    source["profiles"] = Value::Null;
    source["agent_budgets"] = Value::Null;
    std::fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();

    let (draft, saved, error) = load_draft_state(
        &path,
        &root.join("token-station-data"),
        &root.join("plugins"),
    );

    assert_eq!(error, None);
    assert!(saved.get("agent_routes").is_none());
    assert!(saved.get("profiles").is_none());
    assert!(saved.get("agent_budgets").is_none());
    serde_json::from_value::<ClientConfig>(draft)
        .expect("legacy null optional maps recover to empty maps");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_desktop_legacy_zero_concurrency_config_loads_writable_without_rewriting_source() {
    let root = scratch_home("legacy-zero-concurrency");
    let path = root.join("token-station.json");
    let mut legacy = template_for_test(&root);
    legacy["server"]["auth"] = json!(false);
    legacy["concurrency"] = json!({
        "global": 0,
        "per_agent": 0,
        "per_provider": 0
    });
    let original = serde_json::to_vec(&legacy).expect("legacy fixture serializes");
    std::fs::write(&path, &original).expect("legacy fixture writes");

    let (draft, error) = load_draft(&path, &root);

    assert_eq!(error, None);
    assert_eq!(draft["concurrency"]["global"], json!(64));
    assert_eq!(draft["concurrency"]["per_agent"], json!(16));
    assert_eq!(draft["concurrency"]["per_provider"], json!(16));
    assert_eq!(draft["server"]["auth"], json!(false));
    assert_eq!(
        std::fs::read(&path).expect("legacy source remains readable"),
        original
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_model_vision_declaration_updates_the_public_state() {
    let root = scratch_home("model-vision");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{
            "model": "vision-model",
            "vision": false,
            "vision_state": "unknown",
            "context_window": 128000
        }]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    let declared = set_provider_model_vision(
        app.state(),
        "provider".to_owned(),
        "vision-model".to_owned(),
        true,
    )
    .expect("a configured model can be declared vision-capable");
    let model = &declared.providers[0].model_capabilities[0];
    assert_eq!(model.vision, CapabilityState::Declared);

    let unsupported = set_provider_model_vision(
        app.state(),
        "provider".to_owned(),
        "vision-model".to_owned(),
        false,
    )
    .expect("an operator can explicitly disable vision routing");
    let model = &unsupported.providers[0].model_capabilities[0];
    assert_eq!(model.vision, CapabilityState::Unsupported);

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("token-station.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved["upstreams"]["provider"]["models"][0]["vision_state"],
        json!("unsupported")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_model_limits_require_a_positive_output_within_context_and_persist_atomically() {
    let root = scratch_home("model-limits");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{
            "model": "bounded-model",
            "tool": true,
            "context_window": 128000
        }]
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    let before = inner.draft.clone();

    let error = replace_provider_model_limits(&mut inner, "provider", "bounded-model", 128_000, 0)
        .expect_err("a missing maximum output remains unproven");
    assert!(error.contains("大于 0"), "{error}");
    assert_eq!(inner.draft, before);

    let error =
        replace_provider_model_limits(&mut inner, "provider", "bounded-model", 128_000, 128_001)
            .expect_err("output cannot exceed the context window");
    assert!(error.contains("不能大于"), "{error}");
    assert_eq!(inner.draft, before);

    replace_provider_model_limits(&mut inner, "provider", "bounded-model", 128_000, 32_768)
        .expect("operator-confirmed limits persist");
    let model = &inner.draft["upstreams"]["provider"]["models"][0];
    assert_eq!(model["context_window"], json!(128_000));
    assert_eq!(model["max_output_tokens"], json!(32_768));
    assert_eq!(
        model[CONTEXT_WINDOW_SOURCE_KEY],
        json!(LIMIT_SOURCE_OPERATOR)
    );
    assert_eq!(
        model[MAX_OUTPUT_TOKENS_SOURCE_KEY],
        json!(LIMIT_SOURCE_OPERATOR)
    );
    assert_eq!(model["tool"], json!(true));

    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("token-station.json")).unwrap())
            .unwrap();
    assert_eq!(
        saved["upstreams"]["provider"]["models"][0]["max_output_tokens"],
        json!(32_768)
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn trusted_catalog_vision_facts_update_configured_models() {
    let root = scratch_home("catalog-vision");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["openrouter"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://openrouter.ai/api/v1",
        "models": [
            {"model": "vision-model", "vision": false, "context_window": 128000},
            {"model": "text-model", "vision": true, "vision_state": "declared", "context_window": 128000}
        ]
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    let catalog = vec![
        model_catalog::CatalogModelView {
            model: "vision-model".to_owned(),
            tool: CapabilityState::Unknown,
            vision: CapabilityState::Verified,
            json_schema: CapabilityState::Unknown,
            context_window: Some(257_550),
            max_output_tokens: Some(32_768),
            cost: Some(model_catalog::CatalogCostView {
                input: Some(0.2),
                output: Some(0.6),
                cache_read: Some(0.04),
                cache_write: None,
            }),
            source: model_catalog::CatalogSource::Live,
            last_seen_ms: Some(42),
            catalog_state: model_catalog::CatalogState::Active,
        },
        model_catalog::CatalogModelView {
            model: "text-model".to_owned(),
            tool: CapabilityState::Unknown,
            vision: CapabilityState::Unsupported,
            json_schema: CapabilityState::Unknown,
            context_window: None,
            max_output_tokens: None,
            cost: None,
            source: model_catalog::CatalogSource::Live,
            last_seen_ms: Some(42),
            catalog_state: model_catalog::CatalogState::Active,
        },
    ];

    assert!(
        apply_discovered_model_capabilities(&mut inner, "openrouter", &catalog)
            .expect("trusted catalog facts apply")
    );

    let models = inner.draft["upstreams"]["openrouter"]["models"]
        .as_array()
        .unwrap();
    assert_eq!(models[0]["vision"], json!(true));
    assert_eq!(models[0]["vision_state"], json!("verified"));
    assert_eq!(
        models[0]["context_window"],
        json!(128000),
        "a non-zero legacy value without a default source remains operator-owned"
    );
    assert_eq!(models[0]["max_output_tokens"], json!(32768));
    assert_eq!(
        models[0]["catalog_cost"],
        json!({"input": 0.2, "output": 0.6, "cache_read": 0.04})
    );
    assert_eq!(models[1]["vision"], json!(false));
    assert_eq!(models[1]["vision_state"], json!("unsupported"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn catalog_output_larger_than_the_existing_context_is_ignored() {
    let root = scratch_home("catalog-invalid-output");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "bounded", "context_window": 32000}]
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    let catalog = vec![model_catalog::CatalogModelView {
        model: "bounded".to_owned(),
        tool: CapabilityState::Unknown,
        vision: CapabilityState::Unknown,
        json_schema: CapabilityState::Unknown,
        context_window: None,
        max_output_tokens: Some(64_000),
        cost: None,
        source: model_catalog::CatalogSource::Live,
        last_seen_ms: Some(42),
        catalog_state: model_catalog::CatalogState::Active,
    }];

    assert!(!apply_discovered_model_capabilities(&mut inner, "provider", &catalog).unwrap());
    assert!(inner.draft["upstreams"]["provider"]["models"][0]
        .get("max_output_tokens")
        .is_none());
    inner
        .materialize()
        .expect("catalog refresh keeps the draft valid");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_context_replaces_preset_and_clears_an_incompatible_preset_output() {
    let root = scratch_home("catalog-provider-over-preset");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["kimi"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://api.moonshot.cn/v1",
        "models": [{
            "model": "kimi-k3",
            "context_window": 1048576,
            "max_output_tokens": 131072,
            "x-token-station-context-window-source": "builtin_preset",
            "x-token-station-max-output-tokens-source": "builtin_preset"
        }]
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    let catalog = vec![model_catalog::CatalogModelView {
        model: "kimi-k3".to_owned(),
        tool: CapabilityState::Unknown,
        vision: CapabilityState::Unknown,
        json_schema: CapabilityState::Unknown,
        context_window: Some(64_000),
        max_output_tokens: None,
        cost: None,
        source: model_catalog::CatalogSource::Live,
        last_seen_ms: Some(42),
        catalog_state: model_catalog::CatalogState::Active,
    }];

    assert!(apply_discovered_model_capabilities(&mut inner, "kimi", &catalog).unwrap());
    let model = &inner.draft["upstreams"]["kimi"]["models"][0];
    assert_eq!(model["context_window"], json!(64_000));
    assert_eq!(
        model["x-token-station-context-window-source"],
        json!("provider")
    );
    assert!(model.get("max_output_tokens").is_none());
    assert!(model
        .get("x-token-station-max-output-tokens-source")
        .is_none());
    inner
        .materialize()
        .expect("conflicting metadata remains valid");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn catalog_refresh_fills_unknown_metadata_without_overwriting_operator_values() {
    let root = scratch_home("catalog-metadata-ownership");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [
            {"model": "operator-owned", "context_window": 64000, "max_output_tokens": 8000},
            {"model": "unknown", "context_window": 0}
        ]
    });
    draft["pricing"] = json!({
        "version": 4,
        "models": {
            "provider/operator-owned": {
                "input_per_mtok": 900000,
                "output_per_mtok": 1800000
            }
        }
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    let catalog_cost = model_catalog::CatalogCostView {
        input: Some(0.2),
        output: Some(0.6),
        cache_read: Some(0.02),
        cache_write: Some(0.2),
    };
    let catalog_price = catalog_cost_to_model_price(&catalog_cost).unwrap();
    let catalog = ["operator-owned", "unknown"]
        .into_iter()
        .map(|model| model_catalog::CatalogModelView {
            model: model.to_owned(),
            tool: CapabilityState::Unknown,
            vision: CapabilityState::Unknown,
            json_schema: CapabilityState::Unknown,
            context_window: Some(128_000),
            max_output_tokens: Some(32_000),
            cost: Some(catalog_cost.clone()),
            source: model_catalog::CatalogSource::Live,
            last_seen_ms: Some(42),
            catalog_state: model_catalog::CatalogState::Active,
        })
        .collect::<Vec<_>>();

    assert!(apply_discovered_model_capabilities(&mut inner, "provider", &catalog).unwrap());

    let models = inner.draft["upstreams"]["provider"]["models"]
        .as_array()
        .unwrap();
    assert_eq!(models[0]["context_window"], json!(64_000));
    assert_eq!(models[0]["max_output_tokens"], json!(8_000));
    assert_eq!(models[1]["context_window"], json!(128_000));
    assert_eq!(models[1]["max_output_tokens"], json!(32_000));
    let pricing = draft_price_table(&inner).unwrap();
    assert_eq!(
        pricing.version, 5,
        "only the previously unknown price is added"
    );
    assert_eq!(
        pricing.models["provider/operator-owned"].input_per_mtok,
        900_000
    );
    assert_eq!(pricing.models["provider/unknown"], catalog_price);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn catalog_cost_requires_every_billed_token_class() {
    let partial = model_catalog::CatalogCostView {
        input: Some(0.2),
        output: Some(0.6),
        cache_read: Some(0.02),
        cache_write: None,
    };
    assert!(catalog_cost_to_model_price(&partial).is_none());
}

#[test]
fn provider_model_updates_preserve_metadata_and_protect_routing_references() {
    let root = scratch_home("model-update");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["moonshot"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://api.moonshot.cn/v1",
        "models": [
            {
                "model": "moonshot-v1-8k",
                "tool": false,
                "context_window": 8192
            }
        ]
    });
    draft["router"]["pools"][TIER_LOW] =
        json!([{ "upstream": "moonshot", "model": "moonshot-v1-8k" }]);
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    inner.rebuild_routing();

    let error = replace_provider_models(&mut inner, "moonshot", vec!["kimi-k2.6".to_owned()])
        .expect_err("the routed model cannot be removed");
    assert!(error.contains("下档"), "{error}");

    replace_provider_models(
        &mut inner,
        "moonshot",
        vec![
            "moonshot-v1-8k".to_owned(),
            "kimi-k2.6".to_owned(),
            "kimi-k2.6".to_owned(),
        ],
    )
    .expect("retaining the routed model is valid");
    let models = inner.draft["upstreams"]["moonshot"]["models"]
        .as_array()
        .unwrap();
    assert_eq!(models.len(), 2);
    let retained = models
        .iter()
        .find(|model| model["model"] == json!("moonshot-v1-8k"))
        .unwrap();
    assert_eq!(retained["tool"], json!(false));
    assert_eq!(retained["context_window"], json!(8192));
    let preset = models
        .iter()
        .find(|model| model["model"] == json!("kimi-k2.6"))
        .unwrap();
    assert_eq!(preset["context_window"], json!(262_144));
    assert_eq!(preset["max_output_tokens"], json!(262_144));
    assert_eq!(
        preset[MAX_OUTPUT_TOKENS_SOURCE_KEY],
        json!(LIMIT_SOURCE_BUILTIN_PRESET)
    );
    assert!(std::fs::read_to_string(&inner.config_path)
        .unwrap()
        .contains("kimi-k2.6"));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_mutations_protect_home_agent_direct_and_quota_references() {
    let root = scratch_home("provider-direct-quota-references");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [
            {"model": "home-direct"},
            {"model": "agent-direct"},
            {"model": "quota"},
            {"model": "keep"}
        ]
    });
    draft["routing"] = json!({
        "mode": "direct",
        "direct_target": {"upstream": "provider", "model": "home-direct"}
    });
    draft["router"]["quota_accounts"] = json!([{"upstream": "provider", "model": "quota"}]);
    draft["agent_routes"]["codex"] = json!({
        "mode": "inherit",
        "direct_target": {"upstream": "provider", "model": "agent-direct"}
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);

    let references = provider_references(&inner, "provider");
    assert!(
        references.contains(&"主页/单独路由".to_owned()),
        "{references:?}"
    );
    assert!(
        references.contains(&"Agent/codex/单独路由".to_owned()),
        "{references:?}"
    );
    assert!(
        references.contains(&"主页/额度优先#1".to_owned()),
        "{references:?}"
    );

    let before = inner.draft.clone();
    let error = replace_provider_models(&mut inner, "provider", vec!["keep".to_owned()])
        .expect_err("every direct and quota target protects its model");
    assert!(
        error.contains("单独路由") && error.contains("额度优先"),
        "{error}"
    );
    assert_eq!(inner.draft, before);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_model_updates_respect_broken_config_read_only_protection() {
    let root = scratch_home("model-update-read-only");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "keep"}]
    });
    let before = draft.clone();
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        draft,
        Some("只读保护".to_owned()),
    );

    let error = replace_provider_models(&mut inner, "provider", vec!["replacement".to_owned()])
        .expect_err("read-only protection blocks model writes");
    assert!(error.contains("只读保护"), "{error}");
    assert_eq!(inner.draft, before);
    assert!(!inner.config_path.exists());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_model_updates_protect_inactive_agent_route_drafts() {
    let root = scratch_home("model-update-agent-route");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "home"}, {"model": "agent"}]
    });
    inner
        .set_tier_value(TIER_LOW, Some("provider".into()), Some("home".into()))
        .unwrap();
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    for slot in ["high", "mid", "low"] {
        set_agent_tier(
            app.state(),
            "codex".to_owned(),
            slot.to_owned(),
            Some("provider".to_owned()),
            Some("agent".to_owned()),
        )
        .unwrap();
    }
    save_agent_routes(app.state()).unwrap();
    set_agent_route_mode(app.state(), "codex".to_owned(), "inherit".to_owned()).unwrap();

    let error =
        match update_provider_models(app.state(), "provider".to_owned(), vec!["home".to_owned()]) {
            Ok(_) => panic!("inactive custom drafts still protect their model references"),
            Err(error) => error,
        };
    assert!(error.contains("codex/high"), "{error}");
    let state = app.state::<AppStateManaged>();
    let inner = state.0.lock().unwrap();
    assert_eq!(
        inner.draft["upstreams"]["provider"]["models"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_model_updates_protect_unsaved_agent_route_editors() {
    let root = scratch_home("model-update-agent-editor");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "home"}, {"model": "agent"}]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    set_agent_tier(
        app.state(),
        "codex".to_owned(),
        "high".to_owned(),
        Some("provider".to_owned()),
        Some("agent".to_owned()),
    )
    .unwrap();

    let error =
        match update_provider_models(app.state(), "provider".to_owned(), vec!["home".to_owned()]) {
            Ok(_) => panic!("an unsaved Agent editor must protect its selected model"),
            Err(error) => error,
        };

    assert!(error.contains("codex/high"), "{error}");
    let state = app.state::<AppStateManaged>();
    let inner = state.0.lock().unwrap();
    assert_eq!(
        inner.draft["upstreams"]["provider"]["models"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        inner.agent_route_drafts["codex"]["high"].model.as_deref(),
        Some("agent")
    );
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_removal_reports_unsaved_agent_route_editor_references() {
    let root = scratch_home("provider-removal-agent-editor");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "agent"}]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    set_agent_tier(
        app.state(),
        "codex".to_owned(),
        "high".to_owned(),
        Some("provider".to_owned()),
        Some("agent".to_owned()),
    )
    .unwrap();

    let preview = preview_provider_removal(app.state(), "provider".to_owned()).unwrap();
    assert!(!preview.can_remove);
    assert_eq!(preview.references, ["Agent/codex/high"]);
    let error = match remove_provider(app.state(), "provider".to_owned()) {
        Ok(_) => panic!("an editor reference must block Provider removal"),
        Err(error) => error,
    };
    assert!(error.contains("Agent/codex/high"), "{error}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tier_keywords_write_valid_rules_dedupe_and_require_a_configured_pool() {
    let root = scratch_home("tier-keywords");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "m"}]
    });

    // Unconfigured tiers cannot accept keywords because the rule would target an empty pool.
    let error = inner
        .add_tier_keyword("low", "提交git")
        .expect_err("adding to an unconfigured tier is refused");
    assert!(error.contains("先"), "{error}");

    inner
        .set_tier_value(TIER_LOW, Some("provider".into()), Some("m".into()))
        .unwrap();

    inner.add_tier_keyword("low", "提交git").unwrap();
    // Deduplicate case-insensitively.
    let dup = inner
        .add_tier_keyword("low", "提交GIT")
        .expect_err("case-insensitive duplicate is refused");
    assert!(dup.contains("已在"), "{dup}");

    // The keyword enters the low-tier rule targeting tier_low, and the full config validates.
    let keywords = inner.home_keywords();
    assert_eq!(keywords["low"], vec!["提交git".to_string()]);
    let config = inner
        .materialize()
        .expect("keyword rule keeps config valid");
    let rule = config
        .router
        .rules
        .iter()
        .find(|rule| rule.id == KW_RULE_LOW)
        .expect("low keyword rule exists");
    assert_eq!(rule.route_to, TIER_LOW);
    assert_eq!(rule.matcher.keywords_any, vec!["提交git".to_string()]);

    // Remove case-insensitively and delete the rule when its list empties instead of leaving empty keywords_any.
    inner.remove_tier_keyword("low", "提交GIT").unwrap();
    assert!(inner.home_keywords()["low"].is_empty());
    assert!(inner.materialize().unwrap().router.rules.is_empty());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn clearing_a_tier_drops_its_keyword_rule_so_the_config_stays_valid() {
    let root = scratch_home("tier-keywords-clear");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "m"}]
    });
    // Keep another fallback tier so clearing the only tier does not empty pools.
    inner
        .set_tier_value(TIER_HIGH, Some("provider".into()), Some("m".into()))
        .unwrap();
    inner
        .set_tier_value(TIER_LOW, Some("provider".into()), Some("m".into()))
        .unwrap();
    inner.add_tier_keyword("low", "翻译").unwrap();
    assert!(inner
        .materialize()
        .unwrap()
        .router
        .rules
        .iter()
        .any(|rule| rule.id == KW_RULE_LOW));

    // Clearing the low tier must also remove its keyword rule or route_to would target an empty pool.
    inner.set_tier_value(TIER_LOW, None, None).unwrap();
    let config = inner
        .materialize()
        .expect("clearing a tier leaves a valid config, not a dangling rule");
    assert!(config
        .router
        .rules
        .iter()
        .all(|rule| rule.id != KW_RULE_LOW));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn stored_discovery_credentials_cannot_be_redirected_to_another_base_url() {
    let root = scratch_home("model-discovery-url-binding");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://trusted.example/v1",
        "auth": {"slot": "provider_api_key", "store": true},
        "models": [{"model": "model"}]
    });
    let inner = AppInner::new(root.join("token-station.json"), draft, None);

    let error =
        prepare_discovery_credential(&inner, "provider", "https://attacker.example/v1", None)
            .expect_err("a stored credential is bound to its configured URL");
    assert!(error.contains("Base URL 必须与供应商配置一致"), "{error}");

    let one_time = prepare_discovery_credential(
        &inner,
        "new-provider",
        "https://new.example/v1",
        Some("one-time-secret"),
    )
    .expect("an explicit one-time key is accepted");
    assert_eq!(
        one_time,
        DiscoveryCredential::Explicit(Some("one-time-secret".to_owned()))
    );

    let stored =
        prepare_discovery_credential(&inner, "provider", "https://trusted.example/v1", None)
            .expect("stored credentials are prepared without resolving the keyring");
    assert_eq!(
        stored,
        DiscoveryCredential::Stored {
            provider: "provider".to_owned(),
            slot: "provider_api_key".to_owned(),
        }
    );

    let openrouter =
        prepare_discovery_credential(&inner, "openrouter", "https://openrouter.ai/api/v1", None)
            .expect("OpenRouter's public catalog needs no stored credential");
    assert_eq!(openrouter, DiscoveryCredential::Explicit(None));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn azure_model_discovery_fails_before_resolving_any_credential() {
    let root = scratch_home("azure-model-discovery");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["azure"] = json!({
        "provider": "azure-openai-v1",
        "base_url": "https://fixture.openai.azure.com/openai/v1",
        "auth": {"slot": "provider_api_key", "store": true},
        "models": [{"model": "deployment-fixture"}]
    });
    let inner = AppInner::new(root.join("token-station.json"), draft, None);

    for api_key in [None, Some("one-time-secret")] {
        let error = prepare_discovery_credential(
            &inner,
            "azure",
            "https://fixture.openai.azure.com/openai/v1",
            api_key,
        )
        .expect_err("Azure deployments are configured manually, never fetched with Bearer");
        assert_eq!(error, "model_catalog_azure_deployment_manual");
    }

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tier_updates_refuse_unknown_provider_model_and_partial_values() {
    let root = scratch_home("tiers-invalid");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["deepseek"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://api.deepseek.com",
        "models": [{"model": "deepseek-chat"}]
    });

    assert!(inner
        .set_tier_value(TIER_HIGH, Some("missing".into()), Some("model".into()))
        .unwrap_err()
        .contains("未知供应商"));
    assert!(inner
        .set_tier_value(
            TIER_HIGH,
            Some("deepseek".into()),
            Some("missing-model".into())
        )
        .unwrap_err()
        .contains("未配置模型"));
    assert!(inner
        .set_tier_value(TIER_HIGH, Some("deepseek".into()), None)
        .unwrap_err()
        .contains("同时提供"));
    assert!(inner.draft["router"]["pools"]
        .as_object()
        .unwrap()
        .is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn entering_an_incomplete_agent_route_keeps_the_global_config_valid_and_clean() {
    let root = scratch_home("agent-route-editor-isolation");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    )))));
    let before = get_state(app.state());

    let editing =
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();

    assert_eq!(editing.agent_routes["codex"].mode, "custom");
    assert_eq!(
        editing.agent_routes["codex"].config_error.as_deref(),
        Some("Agent `codex` 的 high 档缺少供应商和模型")
    );
    assert_eq!(editing.config_error, None);
    assert_eq!(editing.draft_revision, before.draft_revision);
    assert_eq!(editing.saved_revision, before.saved_revision);
    assert_eq!(editing.config_dirty, before.config_dirty);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn rejecting_an_agent_route_target_keeps_editor_and_global_state_unchanged() {
    let root = scratch_home("agent-route-target-rollback");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    )))));
    let before = get_state(app.state());

    let error = match set_agent_tier(
        app.state(),
        "codex".to_owned(),
        "high".to_owned(),
        Some("missing-provider".to_owned()),
        Some("missing-model".to_owned()),
    ) {
        Ok(_) => panic!("an unknown provider must be rejected"),
        Err(error) => error,
    };

    assert_eq!(error, "未知供应商 `missing-provider`");
    let after = get_state(app.state());
    assert_eq!(after.agent_routes["codex"].mode, "inherit");
    assert_eq!(after.agent_routes["codex"].config_error, None);
    assert_eq!(after.config_error, None);
    assert_eq!(after.draft_revision, before.draft_revision);
    assert_eq!(after.saved_revision, before.saved_revision);
    assert_eq!(after.config_dirty, before.config_dirty);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn incomplete_agent_editor_does_not_block_configuring_and_saving_home() {
    let root = scratch_home("agent-route-home-save");
    let config_path = root.join("token-station.json");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        config_path.clone(),
        template_for_test(&root),
        None,
    )))));

    set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    add_provider(
        app.state(),
        "provider".to_owned(),
        "https://example.com/v1".to_owned(),
        vec!["model".to_owned()],
        None,
        false,
    )
    .unwrap();
    for slot in ["high", "mid", "low"] {
        set_tier(
            app.state(),
            slot.to_owned(),
            Some("provider".to_owned()),
            Some("model".to_owned()),
        )
        .unwrap();
    }

    let saved = save_config(app.state()).unwrap();

    assert_eq!(saved.config_error, None);
    assert!(!saved.config_dirty);
    assert_eq!(saved.agent_routes["codex"].mode, "custom");
    assert_eq!(
        saved.agent_routes["codex"].config_error.as_deref(),
        Some("Agent `codex` 的 high 档缺少供应商和模型")
    );
    assert!(saved
        .tiers
        .values()
        .all(|tier| tier.upstream.as_deref() == Some("provider")
            && tier.model.as_deref() == Some("model")));
    let config = ClientConfig::load(&config_path).unwrap();
    assert!(
        !config.agent_routes.contains_key("codex"),
        "an incomplete editor must not enter the saved ClientConfig"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn saving_an_incomplete_agent_editor_fails_without_touching_config_state_or_disk() {
    let root = scratch_home("agent-route-incomplete-save");
    let config_path = root.join("token-station.json");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        config_path.clone(),
        template_for_test(&root),
        None,
    )))));
    let editing =
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();

    let error = match save_agent_routes(app.state()) {
        Ok(_) => panic!("an incomplete Agent editor cannot be saved"),
        Err(error) => error,
    };

    assert_eq!(error, "Agent `codex` 的 high 档缺少供应商和模型");
    let after = get_state(app.state());
    assert_eq!(after.agent_routes["codex"].mode, "custom");
    assert_eq!(after.config_error, None);
    assert_eq!(after.draft_revision, editing.draft_revision);
    assert_eq!(after.saved_revision, editing.saved_revision);
    assert_eq!(after.config_dirty, editing.config_dirty);
    assert!(!config_path.exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn completing_and_saving_an_agent_editor_commits_one_valid_custom_route() {
    let root = scratch_home("agent-route-complete-save");
    let config_path = root.join("token-station.json");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        config_path.clone(),
        template_for_test(&root),
        None,
    )))));
    add_provider(
        app.state(),
        "provider".to_owned(),
        "https://example.com/v1".to_owned(),
        vec!["model".to_owned()],
        None,
        false,
    )
    .unwrap();
    let before =
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    for slot in ["high", "mid", "low"] {
        let editing = set_agent_tier(
            app.state(),
            "codex".to_owned(),
            slot.to_owned(),
            Some("provider".to_owned()),
            Some("model".to_owned()),
        )
        .unwrap();
        assert_eq!(editing.config_error, None);
        assert_eq!(editing.draft_revision, before.draft_revision);
    }

    let saved = save_agent_routes(app.state()).unwrap();

    assert_eq!(saved.agent_routes["codex"].mode, "custom");
    assert_eq!(saved.agent_routes["codex"].config_error, None);
    assert!(!saved.config_dirty);
    assert!(saved.draft_revision > before.draft_revision);
    assert_eq!(saved.draft_revision, saved.saved_revision);
    let config = ClientConfig::load(&config_path).unwrap();
    let route = config.agent_routes["codex"].custom_route.as_ref().unwrap();
    for target in [&route.high, &route.mid, &route.low] {
        assert_eq!(target.upstream.as_str(), "provider");
        assert_eq!(target.model, "model");
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn agent_route_drafts_seed_from_home_validate_targets_and_preserve_complete_profiles() {
    let root = scratch_home("agent-route-draft");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "home"}, {"model": "agent"}]
    });
    for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
        inner
            .set_tier_value(pool, Some("provider".into()), Some("home".into()))
            .unwrap();
    }
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    let editing =
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    assert_eq!(
        editing.agent_routes["codex"].tiers["high"].model.as_deref(),
        Some("home")
    );
    set_agent_tier(
        app.state(),
        "codex".to_owned(),
        "high".to_owned(),
        Some("provider".to_owned()),
        Some("agent".to_owned()),
    )
    .unwrap();
    let unknown = match set_agent_tier(
        app.state(),
        "future-agent".to_owned(),
        "high".to_owned(),
        Some("provider".to_owned()),
        Some("agent".to_owned()),
    ) {
        Ok(_) => panic!("an unknown Agent must be rejected"),
        Err(error) => error,
    };
    assert!(unknown.contains("未知 Agent"), "{unknown}");
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert!(
            !inner
                .materialize()
                .unwrap()
                .agent_routes
                .contains_key("codex"),
            "editor state stays outside ClientConfig until save"
        );
    }
    save_agent_routes(app.state()).unwrap();
    let config = ClientConfig::load(&root.join("token-station.json")).unwrap();
    assert_eq!(
        config.agent_routes["codex"]
            .custom_route
            .as_ref()
            .unwrap()
            .high
            .model,
        "agent"
    );

    let inherited =
        set_agent_route_mode(app.state(), "codex".to_owned(), "inherit".to_owned()).unwrap();
    assert_eq!(inherited.agent_routes["codex"].mode, "inherit");
    let state = app.state::<AppStateManaged>();
    let inner = state.0.lock().unwrap();
    assert!(inner.draft["agent_routes"]["codex"]["custom_route"].is_object());
    assert!(inner.materialize().is_ok());
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn restoring_one_agent_to_home_clears_every_routing_override() {
    let root = scratch_home("agent-restore-clears-routing-overrides");
    let config_path = root.join("token-station.json");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "home"}, {"model": "agent"}]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        config_path.clone(),
        draft,
        None,
    )))));
    set_direct_route(app.state(), "provider".to_owned(), "home".to_owned(), None).unwrap();
    set_direct_route(
        app.state(),
        "provider".to_owned(),
        "agent".to_owned(),
        Some("codex".to_owned()),
    )
    .unwrap();
    set_routing_mode(app.state(), "direct".to_owned(), Some("codex".to_owned())).unwrap();

    let restored =
        set_agent_route_mode(app.state(), "codex".to_owned(), "inherit".to_owned()).unwrap();

    assert_eq!(restored.agent_routes["codex"].routing_mode, "tiered");
    assert_eq!(
        restored.agent_routes["codex"]
            .direct_target
            .as_ref()
            .unwrap()
            .model
            .as_deref(),
        Some("home")
    );
    save_agent_routes(app.state()).unwrap();
    let saved = ClientConfig::load(&config_path).unwrap();
    assert!(saved.agent_routes["codex"].routing_mode.is_none());
    assert!(saved.agent_routes["codex"].direct_target.is_none());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn returning_an_incomplete_agent_draft_to_inherit_cannot_poison_home_config() {
    let root = scratch_home("agent-route-incomplete");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "model"}]
    });
    inner
        .set_tier_value(TIER_LOW, Some("provider".into()), Some("model".into()))
        .unwrap();
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    let editing =
        set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    assert!(editing.agent_routes["codex"].config_error.is_some());
    assert_eq!(editing.config_error, None);
    let inherited =
        set_agent_route_mode(app.state(), "codex".to_owned(), "inherit".to_owned()).unwrap();
    assert_eq!(inherited.agent_routes["codex"].mode, "inherit");
    assert_eq!(inherited.config_error, None);
    let state = app.state::<AppStateManaged>();
    let inner = state.0.lock().unwrap();
    assert!(inner.draft["agent_routes"]["codex"]["custom_route"].is_null());
    assert!(inner.materialize().is_ok());
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn applying_direct_agent_route_ignores_and_preserves_incomplete_tier_draft() {
    let root = scratch_home("agent-direct-with-incomplete-tier-draft");
    let config_path = root.join("token-station.json");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "direct"}, {"model": "tier"}]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        config_path.clone(),
        draft,
        None,
    )))));
    manage_test_agent_state(&app, &root);

    set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    set_agent_tier(
        app.state(),
        "codex".to_owned(),
        "high".to_owned(),
        Some("provider".to_owned()),
        Some("tier".to_owned()),
    )
    .unwrap();
    set_direct_route(
        app.state(),
        "provider".to_owned(),
        "direct".to_owned(),
        Some("codex".to_owned()),
    )
    .unwrap();
    set_routing_mode(app.state(), "direct".to_owned(), Some("codex".to_owned())).unwrap();

    let applied = restart_agent_route(app.state(), app.state(), "codex".to_owned())
        .expect("Direct apply must not promote an unrelated incomplete tier draft");

    assert_eq!(applied.agent_routes["codex"].routing_mode, "direct");
    assert_eq!(
        applied.agent_routes["codex"]
            .direct_target
            .as_ref()
            .unwrap()
            .model
            .as_deref(),
        Some("direct")
    );
    let saved = ClientConfig::load(&config_path).expect("applied Direct route persists");
    let saved_route = &saved.agent_routes["codex"];
    assert_eq!(saved_route.routing_mode, Some(HostRoutingMode::Direct));
    assert_eq!(saved_route.direct_target.as_ref().unwrap().model, "direct");
    assert!(saved_route.custom_route.is_none());
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        let tier_draft = &inner.agent_route_drafts["codex"];
        assert_eq!(tier_draft["high"].model.as_deref(), Some("tier"));
        assert!(tier_draft["mid"].model.is_none());
        assert!(tier_draft["low"].model.is_none());
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn restarting_one_agent_route_rejects_an_apply_already_in_progress_before_saving() {
    let root = scratch_home("agent-route-during-apply");
    let config_path = root.join("token-station.json");
    let mut draft = gateway_template_for_test(&root);
    draft["server"]["listen"] = json!("127.0.0.1:0");
    draft["server"]["auth"] = json!(false);
    draft["data"]["metrics"] = json!(false);
    draft["upstreams"]["local"] = json!({
        "provider": "openai-compatible",
        "base_url": "http://127.0.0.1:11434/v1",
        "models": [{"model": "small"}]
    });
    draft["router"]["pools"] = json!({
        TIER_LOW: [{"upstream": "local", "model": "small"}]
    });
    draft["router"]["default_pool"] = json!(TIER_LOW);
    let config: ClientConfig = serde_json::from_value(draft.clone()).unwrap();
    let running = prepare_server(config)
        .unwrap()
        .bind()
        .unwrap()
        .publish(7)
        .unwrap();
    let mut inner = AppInner::new(config_path.clone(), draft, None);
    inner.server = ServerLifecycle::Applying {
        generation: 8,
        revision: 2,
        old: running,
    };
    let saved_before = inner.config_state.saved_revision();
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    manage_test_agent_state(&app, &root);

    let error = match restart_agent_route(app.state(), app.state(), "opencode".to_owned()) {
        Ok(_) => panic!("an already frozen apply cannot accept another Agent route revision"),
        Err(error) => error,
    };

    assert!(error.contains("apply_in_progress"), "{error}");
    let state = app.state::<AppStateManaged>();
    let mut inner = state.0.lock().unwrap();
    assert_eq!(inner.config_state.saved_revision(), saved_before);
    let lifecycle = std::mem::replace(
        &mut inner.server,
        ServerLifecycle::Stopped { generation: 9 },
    );
    drop(inner);
    let ServerLifecycle::Applying { old, .. } = lifecycle else {
        panic!("the rejected command must leave the applying runtime in place");
    };
    old.drain_and_shutdown();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn restarting_one_agent_route_rejects_draft_only_targets_before_saving() {
    let root = scratch_home("agent-route-draft-only-target");
    let config_path = root.join("token-station.json");
    let (serving_draft, running) = published_agent_route_fixture(&root);
    let serving_config: ClientConfig = serde_json::from_value(serving_draft.clone()).unwrap();
    serving_config.save(&config_path).unwrap();
    let persisted_before = std::fs::read(&config_path).unwrap();
    let mut latest_draft = serving_draft.clone();
    latest_draft["upstreams"]["draft_only"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://draft-only.example/v1",
        "models": [{"model": "new-model"}]
    });
    latest_draft["agent_routes"]["opencode"] = json!({
        "mode": "inherit",
        "routing_mode": "direct",
        "direct_target": {"upstream": "draft_only", "model": "new-model"}
    });
    let mut inner =
        AppInner::new_with_saved(config_path.clone(), latest_draft, serving_draft, None);
    inner.server = ServerLifecycle::Running {
        generation: 7,
        server: running,
        apply_error: None,
    };
    let saved_before = inner.config_state.saved_revision();
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    manage_test_agent_state(&app, &root);

    let outcome = restart_agent_route(app.state(), app.state(), "opencode".to_owned());
    let state = app.state::<AppStateManaged>();
    let mut inner = state.0.lock().unwrap();
    let saved_after = inner.config_state.saved_revision();
    let override_after = match &inner.server {
        ServerLifecycle::Running { server, .. } => server
            .agent_router_override("opencode")
            .map(|router| router.cloned()),
        _ => panic!("a rejected Agent route must leave the proxy running"),
    };
    let lifecycle = std::mem::replace(
        &mut inner.server,
        ServerLifecycle::Stopped { generation: 8 },
    );
    drop(inner);
    let ServerLifecycle::Running { server, .. } = lifecycle else {
        unreachable!()
    };
    server.drain_and_shutdown();

    let error = match outcome {
        Err(error) => error,
        Ok(_) => panic!("a draft-only target cannot be installed into the old Gateway"),
    };
    assert!(error.contains("draft_only/new-model"), "{error}");
    assert!(error.contains("全量应用"), "{error}");
    assert_eq!(saved_after, saved_before);
    assert_eq!(std::fs::read(&config_path).unwrap(), persisted_before);
    assert_eq!(override_after, None);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn restarting_one_agent_route_prepares_the_router_before_saving() {
    let root = scratch_home("agent-route-invalid-router");
    let config_path = root.join("token-station.json");
    let (serving_draft, running) = published_agent_route_fixture(&root);
    let serving_config: ClientConfig = serde_json::from_value(serving_draft.clone()).unwrap();
    serving_config.save(&config_path).unwrap();
    let persisted_before = std::fs::read(&config_path).unwrap();
    let mut latest_draft = serving_draft.clone();
    latest_draft["router"]["assumed_context_window"] = json!(0);
    latest_draft["agent_routes"]["opencode"] = json!({
        "mode": "inherit",
        "routing_mode": "direct",
        "direct_target": {"upstream": "local", "model": "small"}
    });
    let mut inner =
        AppInner::new_with_saved(config_path.clone(), latest_draft, serving_draft, None);
    inner.server = ServerLifecycle::Running {
        generation: 7,
        server: running,
        apply_error: None,
    };
    let saved_before = inner.config_state.saved_revision();
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    manage_test_agent_state(&app, &root);

    let outcome = restart_agent_route(app.state(), app.state(), "opencode".to_owned());
    let state = app.state::<AppStateManaged>();
    let mut inner = state.0.lock().unwrap();
    let saved_after = inner.config_state.saved_revision();
    let override_after = match &inner.server {
        ServerLifecycle::Running { server, .. } => server
            .agent_router_override("opencode")
            .map(|router| router.cloned()),
        _ => panic!("an invalid Agent router must leave the proxy running"),
    };
    let lifecycle = std::mem::replace(
        &mut inner.server,
        ServerLifecycle::Stopped { generation: 8 },
    );
    drop(inner);
    let ServerLifecycle::Running { server, .. } = lifecycle else {
        unreachable!()
    };
    server.drain_and_shutdown();

    let error = match outcome {
        Err(error) => error,
        Ok(_) => panic!("an invalid router must fail during the prepare phase"),
    };
    assert!(error.contains("assumed_context_window"), "{error}");
    assert_eq!(saved_after, saved_before);
    assert_eq!(std::fs::read(&config_path).unwrap(), persisted_before);
    assert_eq!(override_after, None);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn restarting_one_agent_route_commits_and_installs_one_prevalidated_plan() {
    let root = scratch_home("agent-route-prevalidated-success");
    let config_path = root.join("token-station.json");
    let (serving_draft, running) = published_agent_route_fixture(&root);
    let serving_config: ClientConfig = serde_json::from_value(serving_draft.clone()).unwrap();
    serving_config.save(&config_path).unwrap();
    let mut latest_draft = serving_draft.clone();
    latest_draft["agent_routes"]["opencode"] = json!({
        "mode": "inherit",
        "routing_mode": "direct",
        "direct_target": {"upstream": "local", "model": "small"}
    });
    let mut inner =
        AppInner::new_with_saved(config_path.clone(), latest_draft, serving_draft, None);
    inner.server = ServerLifecycle::Running {
        generation: 7,
        server: running,
        apply_error: None,
    };
    let saved_before = inner.config_state.saved_revision();
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    manage_test_agent_state(&app, &root);

    let applied = restart_agent_route(app.state(), app.state(), "opencode".to_owned())
        .expect("a target in the serving snapshot can be prepared, saved, and installed");
    let state = app.state::<AppStateManaged>();
    let mut inner = state.0.lock().unwrap();
    assert!(inner.config_state.saved_revision() > saved_before);
    let installed = match &inner.server {
        ServerLifecycle::Running { server, .. } => server
            .agent_router_override("opencode")
            .and_then(|router| router)
            .cloned()
            .expect("the committed router is recorded on the running instance"),
        _ => panic!("the successful Agent reload must leave the proxy running"),
    };
    let persisted = ClientConfig::load(&config_path).unwrap();
    assert_eq!(
        Some(installed),
        persisted.custom_router_for_agent("opencode").unwrap()
    );
    assert_eq!(
        applied.agent_routes["opencode"]
            .direct_target
            .as_ref()
            .and_then(|target| target.model.as_deref()),
        Some("small")
    );
    let lifecycle = std::mem::replace(
        &mut inner.server,
        ServerLifecycle::Stopped { generation: 8 },
    );
    drop(inner);
    let ServerLifecycle::Running { server, .. } = lifecycle else {
        unreachable!()
    };
    server.drain_and_shutdown();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn agent_route_commands_save_one_profile_and_apply_home_without_deleting_its_draft() {
    let root = scratch_home("agent-route-commands");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "model"}]
    });
    for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
        inner
            .set_tier_value(pool, Some("provider".into()), Some("model".into()))
            .unwrap();
    }
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    manage_test_agent_state(&app, &root);

    let custom =
        set_agent_route_mode(app.state(), "codex".to_string(), "custom".to_string()).unwrap();
    assert_eq!(custom.agent_routes["codex"].mode, "custom");
    save_agent_routes(app.state()).unwrap();
    for agent_id in ["codex", "opencode"] {
        set_direct_route(
            app.state(),
            "provider".to_owned(),
            "model".to_owned(),
            Some(agent_id.to_owned()),
        )
        .unwrap();
        set_routing_mode(app.state(), "direct".to_owned(), Some(agent_id.to_owned())).unwrap();
    }
    let inherited = apply_home_route_to_all_agents(app.state(), app.state()).unwrap();
    assert!(inherited
        .agent_routes
        .values()
        .all(|profile| profile.mode == "inherit"));
    assert!(inherited
        .agent_routes
        .values()
        .all(|route| route.routing_mode == "tiered" && route.direct_target.is_none()));
    let saved = ClientConfig::load(&root.join("token-station.json")).unwrap();
    assert!(saved.agent_routes["codex"].custom_route.is_some());
    for agent_id in ["codex", "opencode"] {
        assert!(saved.agent_routes[agent_id].routing_mode.is_none());
        assert!(saved.agent_routes[agent_id].direct_target.is_none());
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn applying_home_routes_replaces_running_agent_overrides() {
    let root = scratch_home("agent-routes-apply-home-running");
    let config_path = root.join("token-station.json");
    let (serving_draft, mut running) = published_agent_route_fixture(&root);
    let mut custom_draft = serving_draft.clone();
    custom_draft["agent_routes"]["opencode"] = json!({
        "mode": "inherit",
        "routing_mode": "direct",
        "direct_target": {"upstream": "local", "model": "small"}
    });
    let custom_config: ClientConfig = serde_json::from_value(custom_draft.clone()).unwrap();
    custom_config.save(&config_path).unwrap();
    let custom_router = custom_config
        .custom_router_for_agent("opencode")
        .unwrap()
        .expect("the fixture has one custom Agent router");
    let prepared = running
        .prepare_agent_router_reload("opencode", Some(custom_router))
        .unwrap();
    running.install_prevalidated_agent_router(prepared);

    let mut inner =
        AppInner::new_with_saved(config_path.clone(), custom_draft, serving_draft, None);
    inner.server = ServerLifecycle::Running {
        generation: 7,
        server: running,
        apply_error: None,
    };
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    manage_test_agent_state(&app, &root);

    apply_home_route_to_all_agents(app.state(), app.state()).unwrap();

    let state = app.state::<AppStateManaged>();
    let mut inner = state.0.lock().unwrap();
    let override_after = match &inner.server {
        ServerLifecycle::Running { server, .. } => server
            .agent_router_override("opencode")
            .map(|router| router.cloned()),
        _ => panic!("applying Home routes must leave the proxy running"),
    };
    assert_eq!(override_after, Some(None));
    let persisted = ClientConfig::load(&config_path).unwrap();
    assert_eq!(persisted.custom_router_for_agent("opencode").unwrap(), None);
    let lifecycle = std::mem::replace(
        &mut inner.server,
        ServerLifecycle::Stopped { generation: 8 },
    );
    drop(inner);
    let ServerLifecycle::Running { server, .. } = lifecycle else {
        unreachable!()
    };
    server.drain_and_shutdown();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn named_profiles_are_draft_only_until_saved_and_can_be_shared_by_agents() {
    let root = scratch_home("named-agent-profile");
    let config_path = root.join("token-station.json");
    let mut inner = AppInner::new(config_path.clone(), template_for_test(&root), None);
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "shared"}]
    });
    for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
        inner
            .set_tier_value(pool, Some("provider".into()), Some("shared".into()))
            .unwrap();
    }
    inner.observe_draft().unwrap();
    let before_revision = inner.config_state.draft_revision();
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    let missing = match mount_agent_profile(app.state(), "codex".to_string(), "missing".to_string())
    {
        Ok(_) => panic!("an unknown profile cannot be mounted"),
        Err(error) => error,
    };
    assert!(missing.contains("不存在"), "{missing}");

    let profiled = save_home_route_as_profile(app.state(), "daily".to_string()).unwrap();
    assert_eq!(profiled.profiles, vec!["daily"]);
    assert!(profiled.config_dirty);
    assert!(profiled.draft_revision > before_revision);
    assert!(
        !config_path.exists(),
        "creating a profile must not bypass save"
    );

    for agent_id in ["codex", "opencode"] {
        let mounted =
            mount_agent_profile(app.state(), agent_id.to_string(), "daily".to_string()).unwrap();
        assert_eq!(mounted.agent_routes[agent_id].mode, "profile");
        assert_eq!(
            mounted.agent_routes[agent_id].profile.as_deref(),
            Some("daily")
        );
        assert_eq!(
            mounted.agent_routes[agent_id].tiers["high"]
                .model
                .as_deref(),
            Some("shared")
        );
    }

    let error = match delete_profile(app.state(), "daily".to_string()) {
        Ok(_) => panic!("mounted profiles cannot be deleted"),
        Err(error) => error,
    };
    assert!(
        error.contains("codex") && error.contains("opencode"),
        "{error}"
    );

    {
        let managed = app.state::<AppStateManaged>();
        let mut inner = managed.0.lock().unwrap();
        inner.draft["upstreams"]["provider"]["models"] =
            json!([{"model": "shared"}, {"model": "updated"}]);
        for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
            inner
                .set_tier_value(pool, Some("provider".into()), Some("updated".into()))
                .unwrap();
        }
        inner.observe_draft().unwrap();
    }
    let updated = save_home_route_as_profile(app.state(), "daily".to_string()).unwrap();
    assert_eq!(updated.profiles, vec!["daily"]);
    assert_eq!(
        updated.agent_routes["codex"].tiers["high"].model.as_deref(),
        Some("updated")
    );

    save_agent_routes(app.state()).unwrap();
    let saved = ClientConfig::load(&config_path).unwrap();
    assert!(saved.profiles.contains_key("daily"));
    for agent_id in ["codex", "opencode"] {
        let router = saved
            .custom_router_for_agent(agent_id)
            .unwrap()
            .expect("mounted profile materializes");
        assert_eq!(router.pools[TIER_HIGH][0].model, "updated");
    }

    for agent_id in ["codex", "opencode"] {
        set_agent_route_mode(app.state(), agent_id.to_string(), "inherit".to_string()).unwrap();
    }
    let deleted = delete_profile(app.state(), "daily".to_string()).unwrap();
    assert!(deleted.profiles.is_empty());
    assert!(deleted
        .agent_routes
        .values()
        .all(|route| route.profile.is_none()));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn one_two_and_three_tiers_always_end_with_a_zero_score_fallback() {
    let root = scratch_home("tiers-valid");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [
            {"model": "high"},
            {"model": "mid"},
            {"model": "low"}
        ]
    });

    for (pool, model) in [(TIER_HIGH, "high"), (TIER_MID, "mid"), (TIER_LOW, "low")] {
        inner
            .set_tier_value(pool, Some("provider".into()), Some(model.into()))
            .unwrap();
        let bands = inner.draft["router"]["heuristic"]["bands"]
            .as_array()
            .unwrap();
        assert_eq!(bands.last().unwrap()["at_least"], json!(0));
    }
    assert_eq!(inner.draft["router"]["default_pool"], json!(TIER_LOW));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn ensure_serve_running_starts_a_stopped_proxy_and_waits_until_reachable() {
    let root = scratch_home("ensure-stopped");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        gateway_template_for_test(&root),
        None,
    );
    inner.draft["server"]["listen"] = json!("127.0.0.1:0");
    inner.draft["server"]["auth"] = json!(false);
    inner.draft["data"]["metrics"] = json!(false);
    inner.draft["upstreams"]["local"] = json!({
        "provider": "openai-compatible",
        "base_url": "http://127.0.0.1:11434/v1",
        "models": [{"model": "small"}]
    });
    inner
        .set_tier_value(TIER_LOW, Some("local".into()), Some("small".into()))
        .unwrap();
    // Build dependency-heavy runtime state before the bounded lifecycle
    // window. Coverage instrumentation can make this preparation exceed
    // the production readiness timeout on a loaded runner.
    let first_prepared = prepare_server(inner.materialize().unwrap()).unwrap();
    let recovered_prepared = prepare_server(inner.materialize().unwrap()).unwrap();
    let app = tauri::test::mock_app();
    manage_test_agent_state(&app, &root);
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    let ready = tauri::async_runtime::block_on(ensure_serve_running_with(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        move |_config| Ok(first_prepared),
        Duration::from_secs(30),
    ))
    .unwrap();

    assert_eq!(ready.serve.phase, ServePhase::Running);
    assert_eq!(ready.serve.app_runtime, AppRuntime::Running);
    assert!(ready.serve.listener_reachable);
    let instance_id = ready.serve.instance_id.clone();
    let running_revision = ready.serve.running_revision;
    let prepare_calls = Arc::new(AtomicUsize::new(0));
    let calls_in_prepare = Arc::clone(&prepare_calls);
    let idempotent = tauri::async_runtime::block_on(ensure_serve_running_with(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        move |_config| {
            calls_in_prepare.fetch_add(1, Ordering::SeqCst);
            Err(StartFailure::new("duplicate", "must not restart"))
        },
        Duration::from_secs(1),
    ))
    .unwrap();
    assert_eq!(idempotent.serve.instance_id, instance_id);
    assert_eq!(idempotent.serve.running_revision, running_revision);
    assert_eq!(prepare_calls.load(Ordering::SeqCst), 0);
    begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
    wait_for_serve_phase(&app, ServePhase::Stopped);

    {
        let state = app.state::<AppStateManaged>();
        state.0.lock().unwrap().server = ServerLifecycle::Failed {
            generation: 9,
            listen: "127.0.0.1:0".to_owned(),
            error: "previous fixture failure".to_owned(),
        };
    }
    let recovered = tauri::async_runtime::block_on(ensure_serve_running_with(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        move |_config| Ok(recovered_prepared),
        Duration::from_secs(30),
    ))
    .unwrap();
    assert!(recovered.serve.listener_reachable);
    begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
    wait_for_serve_phase(&app, ServePhase::Stopped);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn ensure_serve_running_joins_starting_and_rejects_a_failed_apply() {
    let root = scratch_home("ensure-join");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        gateway_template_for_test(&root),
        None,
    );
    inner.draft["server"]["listen"] = json!("127.0.0.1:0");
    inner.draft["server"]["auth"] = json!(false);
    inner.draft["data"]["metrics"] = json!(false);
    inner.draft["upstreams"]["local"] = json!({
        "provider": "openai-compatible",
        "base_url": "http://127.0.0.1:11434/v1",
        "models": [{"model": "small"}]
    });
    inner
        .set_tier_value(TIER_LOW, Some("local".into()), Some("small".into()))
        .unwrap();
    // This test owns the Starting/Applying join contract. Build the real
    // Gateway before its bounded lifecycle window so parallel Wasm cold
    // starts cannot turn a join assertion into a machine-speed benchmark.
    let prepared = prepare_server(inner.materialize().unwrap()).unwrap();
    let app = tauri::test::mock_app();
    manage_test_agent_state(&app, &root);
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        move |_config| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(prepared)
        },
    )
    .unwrap();
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fixture is Starting");
    let (joined, ()) = tauri::async_runtime::block_on(async {
        tokio::join! {
            biased;
            ensure_serve_running_with(
                app.handle().clone(),
                app.state::<AppStateManaged>().inner(),
                |_config| panic!("joining Starting must not prepare another runtime"),
                Duration::from_secs(30),
            ),
            async move { release_tx.send(()).unwrap() },
        }
    });
    let joined = joined.unwrap();
    assert!(joined.serve.listener_reachable);

    let (apply_started_tx, apply_started_rx) = mpsc::channel();
    let (apply_release_tx, apply_release_rx) = mpsc::channel();
    begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        move |_config| {
            apply_started_tx.send(()).unwrap();
            apply_release_rx.recv().unwrap();
            Err(StartFailure::new("apply_fixture", "candidate rejected"))
        },
    )
    .unwrap();
    apply_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("fixture is Applying");
    let (outcome, ()) = tauri::async_runtime::block_on(async {
        tokio::join! {
            biased;
            ensure_serve_running_with(
                app.handle().clone(),
                app.state::<AppStateManaged>().inner(),
                |_config| panic!("joining Applying must not prepare another runtime"),
                Duration::from_secs(2),
            ),
            async move { apply_release_tx.send(()).unwrap() },
        }
    });
    let error = match outcome {
        Ok(_) => {
            panic!("a failed apply must not authorize Agent connection through the old runtime")
        }
        Err(error) => error,
    };
    assert!(
        error.contains("ensure_serve_running_start_failed"),
        "{error}"
    );
    assert!(error.contains("apply_fixture"), "{error}");

    begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
    wait_for_serve_phase(&app, ServePhase::Stopped);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn ensure_serve_running_fails_closed_for_stopping_timeout_failure_and_generation_change() {
    let root = scratch_home("ensure-failures");
    let app = tauri::test::mock_app();
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.server = ServerLifecycle::Stopping {
        generation: 4,
        listen: "127.0.0.1:8787".to_owned(),
        draining: true,
    };
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    let stopping = match tauri::async_runtime::block_on(ensure_serve_running_with(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        |_config| panic!("Stopping must fail before preparation"),
        Duration::from_millis(50),
    )) {
        Ok(_) => panic!("Stopping is not automatically reversed"),
        Err(error) => error,
    };
    assert!(
        stopping.contains("ensure_serve_running_stopping"),
        "{stopping}"
    );

    {
        let state = app.state::<AppStateManaged>();
        state.0.lock().unwrap().server = ServerLifecycle::Starting {
            generation: 5,
            listen: "127.0.0.1:8787".to_owned(),
            revision: 1,
        };
    }
    let timeout = match tauri::async_runtime::block_on(ensure_serve_running_with(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        |_config| panic!("joining Starting must not prepare"),
        Duration::from_millis(30),
    )) {
        Ok(_) => panic!("an unfinished generation is bounded"),
        Err(error) => error,
    };
    assert!(
        timeout.contains("ensure_serve_running_timeout"),
        "{timeout}"
    );

    {
        let state = app.state::<AppStateManaged>();
        state.0.lock().unwrap().server = ServerLifecycle::Starting {
            generation: 6,
            listen: "127.0.0.1:8787".to_owned(),
            revision: 2,
        };
    }
    let fail_app = app.handle().clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        fail_app.state::<AppStateManaged>().0.lock().unwrap().server = ServerLifecycle::Failed {
            generation: 6,
            listen: "127.0.0.1:8787".to_owned(),
            error: "fixture failed".to_owned(),
        };
    });
    let failed = match tauri::async_runtime::block_on(ensure_serve_running_with(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        |_config| panic!("joining Starting must not prepare"),
        Duration::from_secs(1),
    )) {
        Ok(_) => panic!("the lifecycle failure is returned"),
        Err(error) => error,
    };
    assert!(failed.contains("fixture failed"), "{failed}");

    {
        let state = app.state::<AppStateManaged>();
        state.0.lock().unwrap().server = ServerLifecycle::Starting {
            generation: 7,
            listen: "127.0.0.1:8787".to_owned(),
            revision: 3,
        };
    }
    let replace_app = app.handle().clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        replace_app
            .state::<AppStateManaged>()
            .0
            .lock()
            .unwrap()
            .server = ServerLifecycle::Stopped { generation: 8 };
    });
    let interrupted = match tauri::async_runtime::block_on(ensure_serve_running_with(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        |_config| panic!("joining Starting must not prepare"),
        Duration::from_secs(1),
    )) {
        Ok(_) => panic!("a replacement generation invalidates the wait"),
        Err(error) => error,
    };
    assert!(
        interrupted.contains("ensure_serve_running_interrupted"),
        "{interrupted}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn startup_preparation_is_single_flight_lock_free_and_cancellable() {
    let root = scratch_home("nonblocking-start");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        gateway_template_for_test(&root),
        None,
    );
    inner.draft["data"]["dir"] = json!(root.join("data"));
    inner.draft["server"]["listen"] = json!("127.0.0.1:0");
    inner.draft["server"]["auth"] = json!(false);
    inner.draft["data"]["metrics"] = json!(false);
    inner.draft["upstreams"]["local"] = json!({
        "provider": "openai-compatible",
        "base_url": "http://127.0.0.1:11434/v1",
        "models": [{"model": "small"}]
    });
    inner
        .set_tier_value(TIER_LOW, Some("local".into()), Some("small".into()))
        .unwrap();

    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let prepare_calls = Arc::new(AtomicUsize::new(0));
    let calls_in_task = Arc::clone(&prepare_calls);

    let starting = begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        move |_config| {
            calls_in_task.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Err(StartFailure::new("test_gate", "cancelled fixture"))
        },
    )
    .unwrap();
    assert_eq!(starting.serve.phase, ServePhase::Starting);
    started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("preparer starts in the background");

    // get_state acquires the same AppInner mutex while preparation is blocked.
    let visible = get_state(app.state());
    assert_eq!(visible.serve.phase, ServePhase::Starting);

    let duplicate_calls = Arc::new(AtomicUsize::new(0));
    let duplicate_calls_in_task = Arc::clone(&duplicate_calls);
    let duplicate = begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        move |_config| {
            duplicate_calls_in_task.fetch_add(1, Ordering::SeqCst);
            Err(StartFailure::new("duplicate", "must not run"))
        },
    )
    .err()
    .expect("a concurrent apply is rejected explicitly");
    assert!(duplicate.contains("apply_in_progress"));
    assert_eq!(duplicate_calls.load(Ordering::SeqCst), 0);
    assert_eq!(prepare_calls.load(Ordering::SeqCst), 1);

    let stopping = begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
    assert_eq!(stopping.serve.phase, ServePhase::Stopping);
    release_tx.send(()).unwrap();
    let stopped = wait_for_serve_phase(&app, ServePhase::Stopped);
    assert_eq!(stopped.serve.app_runtime, AppRuntime::Stopped);
    assert!(stopped.serve.error.is_none());

    let retrying = begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        |_config| Err(StartFailure::new("gateway_init", "fixture failure")),
    )
    .unwrap();
    assert_eq!(retrying.serve.phase, ServePhase::Starting);
    let failed = wait_for_serve_phase(&app, ServePhase::Error);
    assert!(failed
        .serve
        .error
        .as_deref()
        .is_some_and(|error| error.contains("gateway_init: fixture failure")));

    let panicking = begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        |_config| panic!("fixture preparation panic"),
    )
    .unwrap();
    assert_eq!(panicking.serve.phase, ServePhase::Starting);
    let panicked = wait_for_serve_phase(&app, ServePhase::Error);
    assert!(panicked
        .serve
        .error
        .as_deref()
        .is_some_and(|error| error.contains("startup_task: 后台启动任务异常退出")));

    let (retry_started_tx, retry_started_rx) = mpsc::channel();
    let (retry_release_tx, retry_release_rx) = mpsc::channel();
    let retry = begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        move |_config| {
            retry_started_tx.send(()).unwrap();
            retry_release_rx.recv().unwrap();
            Err(StartFailure::new("test_gate", "retry cancelled"))
        },
    )
    .unwrap();
    assert_eq!(retry.serve.phase, ServePhase::Starting);
    retry_started_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("failed lifecycle can start a fresh generation");
    let retry_stopping =
        begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
    assert_eq!(retry_stopping.serve.phase, ServePhase::Stopping);
    retry_release_tx.send(()).unwrap();
    wait_for_serve_phase(&app, ServePhase::Stopped);

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn desktop_update_recovery_waits_for_the_restart_result() {
    let root = scratch_home("desktop-update-recovery");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        gateway_template_for_test(&root),
        None,
    );
    inner.draft["data"]["dir"] = json!(root.join("data"));
    inner.draft["server"]["listen"] = json!("127.0.0.1:0");
    inner.draft["server"]["auth"] = json!(false);
    inner.draft["data"]["metrics"] = json!(false);
    inner.draft["upstreams"]["local"] = json!({
        "provider": "openai-compatible",
        "base_url": "http://127.0.0.1:11434/v1",
        "models": [{"model": "small"}]
    });
    inner
        .set_tier_value(TIER_LOW, Some("local".into()), Some("small".into()))
        .unwrap();
    inner.server = ServerLifecycle::Failed {
        generation: 7,
        listen: "127.0.0.1:0".to_owned(),
        error: "previous stop failure".to_owned(),
    };

    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    let error = tauri::async_runtime::block_on(restore_gateway_after_failed_update_with(
        app.handle().clone(),
        true,
        |_config| Err(StartFailure::new("restore_fixture", "restart failed")),
    ))
    .expect_err("an asynchronous restart failure must be returned to the updater");

    assert!(error.contains("update_gateway_restore_start_failed"));
    assert!(error.contains("restore_fixture: restart failed"));
    let failed = get_state(app.state());
    assert_eq!(failed.serve.phase, ServePhase::Error);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn save_and_apply_hands_new_requests_to_the_new_revision() {
    let root = scratch_home("live-apply");
    let listen = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().to_string()
    };
    let (upstream_a, fixture_a) = serve_chat_completion("revision-a", 1);
    let (upstream_b, fixture_b) = serve_chat_completion("revision-b", 2);
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        gateway_template_for_test(&root),
        None,
    );
    inner.draft["server"]["listen"] = json!(listen.clone());
    inner.draft["server"]["auth"] = json!(false);
    inner.draft["data"]["metrics"] = json!(true);
    inner.draft["data"]["dir"] = json!(root.join("data"));
    inner.draft["pricing"] = json!({
        "version": 1,
        "models": {
            "small": { "input_per_mtok": 1_000_000, "output_per_mtok": 2_000_000 }
        }
    });
    let metrics_path = root.join("data/metrics.sqlite");
    inner.draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": upstream_a,
        "models": [{"model": "small"}]
    });
    for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
        inner
            .set_tier_value(pool, Some("fixture".into()), Some("small".into()))
            .unwrap();
    }
    inner.observe_draft().unwrap();
    let app = tauri::test::mock_app();
    manage_test_agent_state(&app, &root);
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        prepare_server,
    )
    .unwrap();
    let first =
        wait_for_serve_phase_with_timeout(&app, ServePhase::Running, Duration::from_secs(180));
    let revision_a = first.serve.running_revision.unwrap();
    let instance_a = first.serve.instance_id.clone().unwrap();
    assert_eq!(revision_a, first.saved_revision);
    assert!(chat_through_proxy(&listen).contains("revision-a"));
    let first_receipts = wait_for_receipts(&metrics_path, 1);
    assert_eq!(first_receipts[0].running_revision, Some(revision_a));
    assert_eq!(first_receipts[0].cost_micros, Some(3));
    assert_eq!(first_receipts[0].price_version, Some(1));
    fixture_a.join().unwrap();

    save_home_route_as_profile(app.state(), "shared".to_string()).unwrap();
    let mounted =
        mount_agent_profile(app.state(), "codex".to_string(), "shared".to_string()).unwrap();
    assert!(mounted.config_dirty);
    assert_eq!(mounted.serve.running_revision, Some(revision_a));

    let price_v2 = set_model_price(
        app.state(),
        "small".to_string(),
        2_000_000,
        4_000_000,
        0,
        0,
        None,
        1,
    )
    .unwrap();
    assert_eq!(price_v2.version, 2);

    edit_provider(app.state(), "fixture".to_owned(), upstream_b, None).unwrap();
    update_provider_models(
        app.state(),
        "fixture".to_owned(),
        vec!["small".to_owned(), "extra".to_owned()],
    )
    .unwrap();
    let applying = begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        prepare_server,
    )
    .unwrap();
    assert_eq!(applying.serve.app_runtime, AppRuntime::Running);
    assert_eq!(applying.serve.running_revision, Some(revision_a));
    let second =
        wait_for_serve_phase_with_timeout(&app, ServePhase::Running, Duration::from_secs(180));
    assert!(second.serve.running_revision.unwrap() > revision_a);
    assert_eq!(second.serve.running_revision, Some(second.saved_revision));
    assert_ne!(
        second.serve.instance_id.as_deref(),
        Some(instance_a.as_str())
    );
    assert!(chat_through_proxy(&listen).contains("revision-b"));
    let second_revision = second.serve.running_revision.unwrap();
    let second_receipts = wait_for_receipts(&metrics_path, 2);
    assert_eq!(second_receipts[0].running_revision, Some(second_revision));
    assert_eq!(second_receipts[0].cost_micros, Some(6));
    assert_eq!(second_receipts[0].price_version, Some(2));
    assert_eq!(second_receipts[1].cost_micros, Some(3));
    assert_eq!(second_receipts[1].price_version, Some(1));
    let ipc_receipts = get_recent_receipts(app.state(), 5).expect("receipt IPC reads");
    assert_eq!(
        ipc_receipts, second_receipts,
        "IPC uses the fixed store view"
    );

    edit_provider(
        app.state(),
        "fixture".to_owned(),
        "http://127.0.0.1:1/v1".to_owned(),
        None,
    )
    .unwrap();
    save_home_route_as_profile(app.state(), "candidate".to_string()).unwrap();
    let candidate =
        mount_agent_profile(app.state(), "opencode".to_string(), "candidate".to_string()).unwrap();
    assert_eq!(candidate.serve.running_revision, Some(second_revision));
    begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        |_config| {
            Err(StartFailure::new(
                "gateway_init",
                "preflight fixture failure",
            ))
        },
    )
    .unwrap();
    let failed_apply = wait_for_serve_phase(&app, ServePhase::Running);
    assert_eq!(
        failed_apply.serve.running_revision,
        second.serve.running_revision
    );
    assert!(failed_apply.saved_revision > failed_apply.serve.running_revision.unwrap());
    assert!(failed_apply
        .serve
        .error
        .as_deref()
        .is_some_and(|error| error.contains("已保存尚未应用")));
    assert!(chat_through_proxy(&listen).contains("revision-b"));
    let failed_apply_receipts = wait_for_receipts(&metrics_path, 3);
    assert_eq!(
        failed_apply_receipts[0].running_revision,
        Some(second_revision),
        "a failed apply keeps serving and receipting the published revision"
    );
    fixture_b.join().unwrap();

    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        let ServerLifecycle::Running { server, .. } = &inner.server else {
            panic!("fixture server must still be published");
        };
        server.abort_task();
    }
    let exited = wait_for_serve_phase_with_timeout(&app, ServePhase::Error, Duration::from_secs(1));
    assert_eq!(exited.serve.app_runtime, AppRuntime::Stopped);
    assert!(!exited.serve.listener_reachable);
    assert_eq!(exited.serve.running_revision, None);
    assert_eq!(exited.serve.instance_id, None);

    begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
    wait_for_serve_phase(&app, ServePhase::Stopped);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_endpoint_preview_uses_the_protocol_resolver() {
    for input in [
        "https://api.example.com",
        "https://api.example.com/v1",
        "https://api.example.com/v1/chat/completions",
    ] {
        let preview = preview_provider_endpoints(input.to_owned()).unwrap();
        assert_eq!(preview.chat, "https://api.example.com/v1/chat/completions");
        assert_eq!(preview.responses, "https://api.example.com/v1/responses");
        assert_eq!(preview.messages, "https://api.example.com/v1/messages");
        assert!(!preview.loopback);
    }

    let local = preview_provider_endpoints("http://127.0.0.1:11434/v1".to_owned()).unwrap();
    assert!(local.loopback);
}

#[test]
fn invalid_proxy_settings_are_transactional_and_field_scoped() {
    let root = scratch_home("settings-transaction");
    let config_path = root.join("token-station.json");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        config_path.clone(),
        template_for_test(&root),
        None,
    )))));
    add_provider(
        app.state(),
        "local".to_owned(),
        "http://127.0.0.1:11434/v1".to_owned(),
        vec!["local-model".to_owned()],
        None,
        true,
    )
    .expect("baseline provider is valid");
    set_tier(
        app.state(),
        "low".to_owned(),
        Some("local".to_owned()),
        Some("local-model".to_owned()),
    )
    .expect("baseline route is valid");
    let before = save_config(app.state()).expect("baseline config saves");
    let before_disk = std::fs::read(&config_path).expect("baseline config is on disk");

    let error = match set_settings(
        app.state(),
        false,
        false,
        "http".to_owned(),
        "ftp://invalid.example".to_owned(),
        vec!["localhost".to_owned()],
        String::new(),
        String::new(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("an unsupported proxy scheme is rejected"),
    };

    assert_eq!(error.field, "egress_proxy_url", "{}", error.message);
    assert_eq!(error.reason_code, "invalid_proxy_url");
    let after = get_state(app.state());
    assert_eq!(after.draft_revision, before.draft_revision);
    assert_eq!(after.saved_revision, before.saved_revision);
    assert_eq!(after.settings.auth, before.settings.auth);
    assert_eq!(after.settings.metrics, before.settings.metrics);
    assert_eq!(
        std::fs::read(&config_path).expect("saved config remains readable"),
        before_disk,
        "a rejected settings edit must not mutate the authoritative file"
    );

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn managed_enterprise_provider_and_direct_target_are_one_draft_mutation() {
    let root = scratch_home("managed-enterprise-route");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    )))));

    let view = add_provider_impl(
        app.state(),
        "enterprise_main".to_owned(),
        "https://enterprise.example.com/v1".to_owned(),
        vec!["auto".to_owned()],
        None,
        false,
        "env",
        Some("ENTERPRISE_API_KEY"),
        "openai-compatible",
        true,
    )
    .expect("the managed provider and Direct target are valid together");

    assert_eq!(view.routing_mode, "direct");
    let target = view.direct_target.expect("the Direct target is complete");
    assert_eq!(target.upstream, "enterprise_main");
    assert_eq!(target.model.as_deref(), Some("auto"));
    let provider = view
        .providers
        .iter()
        .find(|provider| provider.name == "enterprise_main")
        .expect("the managed provider is visible");
    assert_eq!(
        provider.model_capabilities[0].vision,
        CapabilityState::Declared
    );

    let managed = app.state::<AppStateManaged>();
    let inner = managed.0.lock().unwrap();
    let upstream = &inner.draft["upstreams"]["enterprise_main"];
    assert_eq!(upstream["managed_route"], json!(true));
    assert_eq!(
        upstream["models"][0]["supported_parameters"],
        json!(["reasoning_effort"])
    );
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn managed_enterprise_route_rollback_restores_present_and_absent_routing() {
    let previous_router = json!({ "routing_mode": "quota-first" });
    let expected_routing = json!({
        "mode": "quota-first",
        "direct_target": { "upstream": "original", "model": "stable" }
    });
    let previous_routing = Some(expected_routing.clone());
    let mut draft = json!({
        "router": { "routing_mode": "tiered" },
        "routing": { "mode": "direct" }
    });

    restore_managed_route_mutation(&mut draft, &previous_routing, &previous_router);
    assert_eq!(draft["router"], previous_router);
    assert_eq!(draft["routing"], expected_routing);

    let mut draft_without_previous_routing = json!({
        "router": { "routing_mode": "tiered" },
        "routing": { "mode": "direct" }
    });
    restore_managed_route_mutation(
        &mut draft_without_previous_routing,
        &None,
        &json!({ "routing_mode": "tiered" }),
    );
    assert!(draft_without_previous_routing.get("routing").is_none());
}

#[test]
fn provider_credentials_default_to_store_and_advanced_sources_save_only_references() {
    let root = scratch_home("credential-sources");
    let config_path = root.join("token-station.json");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        config_path.clone(),
        template_for_test(&root),
        None,
    )))));

    let env_view = add_provider_with_credential(
        app.state(),
        "deepseek_env".to_owned(),
        "https://api.deepseek.com/v1".to_owned(),
        vec!["deepseek-chat".to_owned()],
        None,
        false,
        "env".to_owned(),
        Some("DEEPSEEK_API_KEY".to_owned()),
        None,
    )
    .expect("an environment credential reference is accepted");
    let env_provider = env_view
        .providers
        .iter()
        .find(|provider| provider.name == "deepseek_env")
        .expect("environment provider is visible");
    assert_eq!(env_provider.credential_source, "env");
    assert_eq!(env_provider.credential_reference, "DEEPSEEK_API_KEY");

    let credential_file = root.join("credentials").join("deepseek.key");
    let file_view = add_provider_with_credential(
        app.state(),
        "deepseek_file".to_owned(),
        "https://api.deepseek.com/v1".to_owned(),
        vec!["deepseek-reasoner".to_owned()],
        None,
        false,
        "file".to_owned(),
        Some(credential_file.to_string_lossy().into_owned()),
        None,
    )
    .expect("an absolute credential file reference is accepted");
    let file_provider = file_view
        .providers
        .iter()
        .find(|provider| provider.name == "deepseek_file")
        .expect("file provider is visible");
    assert_eq!(file_provider.credential_source, "file");
    assert_eq!(
        file_provider.credential_reference,
        credential_file.to_string_lossy()
    );

    let plaintext_error = match add_provider_with_credential(
        app.state(),
        "forbidden_plaintext".to_owned(),
        "https://api.example.com/v1".to_owned(),
        vec!["model".to_owned()],
        Some("must-not-be-saved".to_owned()),
        false,
        "env".to_owned(),
        Some("EXAMPLE_API_KEY".to_owned()),
        None,
    ) {
        Err(error) => error,
        Ok(_) => panic!("env/file sources cannot accept plaintext API keys"),
    };
    assert!(plaintext_error.contains("不能同时提交 API Key 明文"));
    let invalid_env = match add_provider_with_credential(
        app.state(),
        "bad_env".to_owned(),
        "https://api.example.com/v1".to_owned(),
        vec!["model".to_owned()],
        None,
        false,
        "env".to_owned(),
        Some("1INVALID".to_owned()),
        None,
    ) {
        Err(error) => error,
        Ok(_) => panic!("invalid environment names are rejected"),
    };
    assert!(invalid_env.contains("不能以数字开头"));
    let invalid_file = match add_provider_with_credential(
        app.state(),
        "bad_file".to_owned(),
        "https://api.example.com/v1".to_owned(),
        vec!["model".to_owned()],
        None,
        false,
        "file".to_owned(),
        Some("relative.key".to_owned()),
        None,
    ) {
        Err(error) => error,
        Ok(_) => panic!("relative credential files are rejected"),
    };
    assert!(invalid_file.contains("绝对路径"));

    set_tier(
        app.state(),
        "low".to_owned(),
        Some("deepseek_env".to_owned()),
        Some("deepseek-chat".to_owned()),
    )
    .expect("credential test has a valid route");
    save_config(app.state()).expect("credential references save");
    let saved = std::fs::read_to_string(config_path).expect("saved config is readable");
    assert!(saved.contains("DEEPSEEK_API_KEY"));
    assert!(saved.contains(&credential_file.to_string_lossy().replace('\\', "\\\\")));
    assert!(!saved.contains("must-not-be-saved"));
    assert!(!root
        .join("token-station-data")
        .join(secrets::SECRETS_FILE)
        .exists());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_creation_persists_only_the_closed_dialect_catalog() {
    let root = scratch_home("provider-dialect");
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    )))));

    let view = add_provider_with_credential(
        app.state(),
        "azure".to_owned(),
        "https://fixture.openai.azure.com/openai/v1".to_owned(),
        vec!["deployment-fixture".to_owned()],
        None,
        false,
        "env".to_owned(),
        Some("AZURE_OPENAI_API_KEY".to_owned()),
        Some("azure-openai-v1".to_owned()),
    )
    .expect("the Azure OpenAI v1 dialect is accepted");
    let provider = view
        .providers
        .iter()
        .find(|provider| provider.name == "azure")
        .expect("the Azure provider is visible");
    assert_eq!(provider.provider, "azure-openai-v1");

    let before = get_state(app.state()).draft_revision;
    let error = match add_provider_with_credential(
        app.state(),
        "azure_wrong_base".to_owned(),
        "https://fixture.openai.azure.com/v1".to_owned(),
        vec!["deployment-fixture".to_owned()],
        None,
        false,
        "env".to_owned(),
        Some("AZURE_OPENAI_API_KEY".to_owned()),
        Some("azure-openai-v1".to_owned()),
    ) {
        Err(error) => error,
        Ok(_) => panic!("Azure OpenAI v1 requires the exact /openai/v1 API root"),
    };
    assert!(error.contains("/openai/v1"), "{error}");
    let after_wrong_base = get_state(app.state());
    assert_eq!(after_wrong_base.draft_revision, before);
    assert!(after_wrong_base
        .providers
        .iter()
        .all(|provider| provider.name != "azure_wrong_base"));

    let error = match edit_provider_with_credential(
        app.state(),
        "azure".to_owned(),
        "https://fixture.openai.azure.com/v1".to_owned(),
        None,
        "env".to_owned(),
        Some("AZURE_OPENAI_API_KEY".to_owned()),
        Some("legacy".to_owned()),
    ) {
        Err(error) => error,
        Ok(_) => panic!("editing Azure must preserve the exact /openai/v1 API root"),
    };
    assert!(error.contains("/openai/v1"), "{error}");
    let after_wrong_edit = get_state(app.state());
    assert_eq!(after_wrong_edit.draft_revision, before);
    assert_eq!(
        after_wrong_edit
            .providers
            .iter()
            .find(|provider| provider.name == "azure")
            .map(|provider| provider.base_url.as_str()),
        Some("https://fixture.openai.azure.com/openai/v1")
    );

    let error = match add_provider_with_credential(
        app.state(),
        "unknown".to_owned(),
        "https://provider.example/v1".to_owned(),
        vec!["model".to_owned()],
        None,
        false,
        "env".to_owned(),
        Some("UNKNOWN_API_KEY".to_owned()),
        Some("future-header-provider".to_owned()),
    ) {
        Err(error) => error,
        Ok(_) => panic!("unknown provider dialects must fail closed"),
    };
    assert!(error.contains("Provider dialect"), "{error}");
    let after = get_state(app.state());
    assert_eq!(after.draft_revision, before);
    assert!(after
        .providers
        .iter()
        .all(|provider| provider.name != "unknown"));

    let error = match add_provider_with_credential(
        app.state(),
        "remote_http".to_owned(),
        "http://192.0.2.1/v1".to_owned(),
        vec!["model".to_owned()],
        None,
        false,
        "env".to_owned(),
        Some("REMOTE_HTTP_API_KEY".to_owned()),
        None,
    ) {
        Err(error) => error,
        Ok(_) => panic!("desktop creation must reject credentialed remote HTTP"),
    };
    assert!(error.contains("must use HTTPS"), "{error}");
    let after_http = get_state(app.state());
    assert_eq!(after_http.draft_revision, before);
    assert!(after_http
        .providers
        .iter()
        .all(|provider| provider.name != "remote_http"));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn changing_provider_identity_clears_only_that_providers_scoped_prices() {
    let root = scratch_home("provider-identity-pricing");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://old.example/v1",
        "models": [{"model": "shared", "context_window": 128000}]
    });
    draft["pricing"] = json!({
        "version": 7,
        "models": {
            "fixture/shared": {"input_per_mtok": 200000, "output_per_mtok": 600000},
            "other/shared": {"input_per_mtok": 900000, "output_per_mtok": 1200000},
            "shared": {"input_per_mtok": 100000, "output_per_mtok": 300000}
        }
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    edit_provider(
        app.state(),
        "fixture".to_owned(),
        "https://new.example/v1".to_owned(),
        None,
    )
    .expect("a provider identity may be edited");

    let state = app.state::<AppStateManaged>();
    let inner = state.0.lock().unwrap();
    let pricing = draft_price_table(&inner).unwrap();
    assert_eq!(pricing.version, 8);
    assert!(!pricing.models.contains_key("fixture/shared"));
    assert!(pricing.models.contains_key("other/shared"));
    assert!(pricing.models.contains_key("shared"));
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn saving_unchanged_provider_credentials_preserves_scoped_price() {
    let root = scratch_home("provider-identity-noop");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://same.example/v1",
        "auth": {"slot": "provider_api_key", "env": "FIXTURE_API_KEY"},
        "models": [{"model": "shared", "context_window": 128000}]
    });
    draft["pricing"] = json!({
        "version": 7,
        "models": {
            "fixture/shared": {"input_per_mtok": 200000, "output_per_mtok": 600000}
        }
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));
    edit_provider_with_credential(
        app.state(),
        "fixture".to_owned(),
        "https://same.example/v1".to_owned(),
        None,
        "env".to_owned(),
        Some("FIXTURE_API_KEY".to_owned()),
        Some("south_v1_buffered_streaming".to_owned()),
    )
    .expect("submitting unchanged provider details is a no-op identity update");

    let state = app.state::<AppStateManaged>();
    let inner = state.0.lock().unwrap();
    let pricing = draft_price_table(&inner).unwrap();
    assert_eq!(pricing.version, 7);
    assert!(pricing.models.contains_key("fixture/shared"));
    assert_eq!(
        inner.draft["upstreams"]["fixture"]["provider_call"],
        json!("south_v1_buffered_streaming")
    );
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[cfg(unix)]
#[test]
fn a_failed_identity_cleanup_does_not_apply_the_provider_call_engine() {
    let root = scratch_home("provider-engine-rollback");
    let data_dir = root.join("data");
    std::fs::create_dir_all(&data_dir).expect("data directory exists");
    let cache_target = root.join("catalog-target.json");
    std::fs::write(
        &cache_target,
        r#"{
                "version": 3,
                "providers": {
                    "fixture": {
                        "base_url": "https://old.example/v1",
                        "revision": 1,
                        "models": [],
                        "fetched_at_ms": 0
                    }
                }
            }"#,
    )
    .expect("catalog target writes");
    std::os::unix::fs::symlink(&cache_target, data_dir.join("model-catalog-cache.json"))
        .expect("catalog symlink writes");

    let mut draft = template_for_test(&root);
    draft["data"]["dir"] = json!(data_dir);
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://old.example/v1",
        "auth": {"slot": "provider_api_key", "env": "FIXTURE_API_KEY"},
        "models": [{"model": "shared", "context_window": 128000}]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    let error = match edit_provider_with_credential(
        app.state(),
        "fixture".to_owned(),
        "https://new.example/v1".to_owned(),
        None,
        "env".to_owned(),
        Some("FIXTURE_API_KEY".to_owned()),
        Some("south_v1_buffered".to_owned()),
    ) {
        Err(error) => error,
        Ok(_) => panic!("a catalog symlink makes identity cleanup fail closed"),
    };
    assert!(error.contains("保存模型缓存失败"), "{error}");

    let state = app.state::<AppStateManaged>();
    let inner = state.0.lock().unwrap();
    assert!(
        inner.draft["upstreams"]["fixture"]
            .get("provider_call")
            .is_none(),
        "a failed save must preserve the previous engine"
    );
    assert_eq!(
        inner.draft["upstreams"]["fixture"]["base_url"],
        json!("https://old.example/v1")
    );
    drop(inner);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn desktop_commands_cover_provider_routing_settings_server_and_read_only_views() {
    let root = scratch_home("command-lifecycle");
    let mut draft = gateway_template_for_test(&root);
    draft["data"]["dir"] = json!(root.join("data"));
    draft["server"]["listen"] = json!("127.0.0.1:0");
    let app = tauri::test::mock_app();
    manage_test_agent_state(&app, &root);
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    let initial = get_state(app.state());
    assert_eq!(initial.serve.app_runtime, AppRuntime::Stopped);
    assert!(initial.providers.is_empty());
    assert_eq!(initial.settings.listen, "127.0.0.1:0");

    for (name, url, models) in [
        ("local", "http://127.0.0.1:11434/v1", vec!["small", "large"]),
        ("spare", "http://127.0.0.1:11435/v1", vec!["backup"]),
    ] {
        let view = add_provider(
            app.state(),
            name.to_string(),
            url.to_string(),
            models.into_iter().map(str::to_string).collect(),
            None,
            name == "local",
        )
        .unwrap();
        let provider = view
            .providers
            .iter()
            .find(|provider| provider.name == name)
            .expect("the added provider is visible");
        // OpenAI-compatible chat declares tools and structured output by default; keep vision Unknown.
        assert_eq!(
            provider.model_capabilities[0].tool,
            CapabilityState::Declared
        );
        assert_eq!(
            provider.model_capabilities[0].vision,
            CapabilityState::Unknown
        );
        assert_eq!(
            provider.model_capabilities[0].json_schema,
            CapabilityState::Declared
        );
    }
    let duplicate = add_provider(
        app.state(),
        "local".to_owned(),
        "http://127.0.0.1:9999/v1".to_owned(),
        vec!["replacement".to_owned()],
        None,
        false,
    )
    .err()
    .expect("重复名称不能绕过 Provider 编辑流程");
    assert!(duplicate.contains("已存在"));
    let unchanged = get_state(app.state());
    let local = unchanged
        .providers
        .iter()
        .find(|provider| provider.name == "local")
        .unwrap();
    assert_eq!(local.base_url, "http://127.0.0.1:11434/v1");
    assert_eq!(local.models, ["small", "large"]);
    assert!(add_provider(
        app.state(),
        " ".to_string(),
        "http://127.0.0.1/v1".to_string(),
        vec!["m".to_string()],
        None,
        false,
    )
    .err()
    .expect("blank provider is rejected")
    .contains("不能为空"));
    assert!(add_provider(
        app.state(),
        "empty".to_string(),
        "http://127.0.0.1/v1".to_string(),
        vec![" ".to_string()],
        None,
        false,
    )
    .err()
    .expect("blank model set is rejected")
    .contains("至少填一个"));
    let provider_count = get_state(app.state()).providers.len();
    let invalid_name = match add_provider(
        app.state(),
        "minimax-cn".to_string(),
        "https://api.minimaxi.com/v1".to_string(),
        vec!["MiniMax-M3".to_string()],
        None,
        false,
    ) {
        Err(error) => error,
        Ok(_) => panic!("invalid upstream reference names must be rejected before mutation"),
    };
    assert!(invalid_name.contains("upstream reference name"));
    assert_eq!(get_state(app.state()).providers.len(), provider_count);

    set_tier(
        app.state(),
        "high".to_string(),
        Some("local".to_string()),
        Some("large".to_string()),
    )
    .unwrap();
    set_tier(
        app.state(),
        "low".to_string(),
        Some("local".to_string()),
        Some("small".to_string()),
    )
    .unwrap();
    assert!(set_tier(app.state(), "invalid".to_string(), None, None)
        .err()
        .expect("invalid tier is rejected")
        .contains("未知档位"));

    let saved = save_config(app.state()).unwrap();
    assert!(saved.config_error.is_none());
    assert!(root.join("token-station.json").is_file());
    let router = get_router_table(app.state());
    assert_eq!(router.default_pool, TIER_LOW);
    assert_eq!(router.threshold, Some(CUT_MID));
    assert_eq!(router.bands.len(), 2);
    assert_eq!(router.pools.len(), 2);
    assert_eq!(router.bands[0].upstream.as_deref(), Some("local"));

    update_provider_models(
        app.state(),
        "local".to_string(),
        vec![
            "large".to_string(),
            "small".to_string(),
            "extra".to_string(),
        ],
    )
    .unwrap();
    let configured = set_settings(
        app.state(),
        false,
        false,
        "direct".to_string(),
        String::new(),
        Vec::new(),
        String::new(),
        String::new(),
    )
    .unwrap();
    assert!(!configured.settings.auth);
    assert!(!configured.settings.metrics);

    let plugins = get_plugins(app.state()).unwrap();
    assert!(plugins.agent.contains("agent-openai"));
    assert!(plugins
        .dialects
        .iter()
        .any(|dialect| dialect == "openai-compatible"));
    assert!(plugins.listing.contains("provider-openai-compatible"));

    let empty_stats =
        get_stats(app.state(), "all".to_string(), None, None, None, None, None).unwrap();
    assert!(empty_stats.empty);
    assert_eq!(empty_stats.total.requests, 0);

    let started = begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        prepare_server,
    )
    .unwrap();
    assert_eq!(started.serve.phase, ServePhase::Starting);
    let duplicate = begin_serve_start(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        prepare_server,
    )
    .err()
    .expect("a concurrent apply is rejected explicitly");
    assert!(duplicate.contains("apply_in_progress"));
    // Coverage instrumentation makes Wasmtime's first compilation much
    // slower on a cold Linux runner; this remains a bounded integration test.
    let running =
        wait_for_serve_phase_with_timeout(&app, ServePhase::Running, Duration::from_secs(180));
    assert_eq!(running.serve.app_runtime, AppRuntime::Running);
    assert!(running.serve.listener_reachable);
    assert!(running.serve.virtual_key.is_none());
    assert!(root.join("data").join("requests.log").exists());
    let stopping = begin_serve_stop(app.handle().clone(), app.state::<AppStateManaged>().inner());
    assert_eq!(stopping.serve.phase, ServePhase::Stopping);
    let stopped = wait_for_serve_phase(&app, ServePhase::Stopped);
    assert_eq!(stopped.serve.app_runtime, AppRuntime::Stopped);

    let impact = preview_provider_removal(app.state(), "local".to_string()).unwrap();
    assert!(!impact.can_remove);
    assert!(impact
        .references
        .iter()
        .any(|item| item.contains("主页/上档")));
    assert!(remove_provider(app.state(), "local".to_string())
        .err()
        .expect("被引用的 Provider 必须拒绝删除")
        .contains("仍被引用"));
    set_tier(app.state(), "high".to_string(), None, None).unwrap();
    set_tier(app.state(), "low".to_string(), None, None).unwrap();
    assert!(
        preview_provider_removal(app.state(), "local".to_string())
            .unwrap()
            .can_remove
    );

    let catalog_path = root.join("data").join("model-catalog-cache.json");
    crate::agent_integration::safe_fs::write_atomic_private(
        &catalog_path,
        &serde_json::to_vec_pretty(&json!({
            "version": 3,
            "providers": {
                "local": {
                    "base_url": "http://127.0.0.1:11434/v1",
                    "revision": 7,
                    "models": [{
                        "model": "old-account-private-model",
                        "tool": "unknown",
                        "vision": "unknown",
                        "json_schema": "unknown",
                        "source": "live",
                        "last_seen_ms": 1,
                        "catalog_state": "active"
                    }],
                    "fetched_at_ms": 1
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let removed = remove_provider(app.state(), "local".to_string()).unwrap();
    assert_eq!(removed.providers.len(), 1);
    assert_eq!(removed.deleted_providers, ["local"]);
    assert!(removed.tiers.values().all(|tier| tier.upstream.is_none()));
    assert!(
        !std::fs::read_to_string(&catalog_path)
            .unwrap()
            .contains("old-account-private-model"),
        "deletion invalidates the old Provider identity's trusted catalog"
    );
    let Err(readd_error) = add_provider(
        app.state(),
        "local".to_owned(),
        "http://127.0.0.1:11434/v1".to_owned(),
        vec!["replacement".to_owned()],
        None,
        false,
    ) else {
        panic!("a tombstoned Provider name must be restored, never silently replaced")
    };
    assert!(readd_error.contains("请先恢复"), "{readd_error}");
    let restored = restore_provider(app.state(), "local".to_string()).unwrap();
    assert_eq!(restored.providers.len(), 2);
    assert!(restored.deleted_providers.is_empty());
    let restored_local = restored
        .providers
        .iter()
        .find(|provider| provider.name == "local")
        .unwrap();
    assert_eq!(restored_local.catalog_revision, 0);
    assert!(restored_local
        .catalog
        .iter()
        .all(|model| model.source == model_catalog::CatalogSource::Configured));
    assert!(save_config(app.state())
        .err()
        .expect("empty routing config is rejected")
        .contains("至少配置一档"));

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn agent_budget_commands_persist_display_only_thresholds_and_report_zero_without_a_store() {
    let root = scratch_home("agent-budget-commands");
    let inner = AppInner::new(
        root.join("token-station.json"),
        gateway_template_for_test(&root),
        None,
    );
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    let statuses = set_agent_budget(
        app.state(),
        "codex".to_string(),
        1_000_000,
        80,
        Some(1_000),
        Some(2_000),
        7,
    )
    .unwrap();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].agent_id, "codex");
    assert_eq!(statuses[0].used_micros, 0);
    assert!(!statuses[0].routing_affected);
    let saved = ClientConfig::load(&root.join("token-station.json")).unwrap();
    assert_eq!(saved.agent_budgets["codex"].limit_micros, 1_000_000);

    assert!(set_agent_budget(
        app.state(),
        "unknown-agent".to_string(),
        1,
        80,
        None,
        None,
        7,
    )
    .is_err());
    assert!(remove_agent_budget(app.state(), "codex".to_string())
        .unwrap()
        .is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn model_price_edits_append_versions_and_never_revalue_historical_receipts() {
    use token_station_metrics::{CostKind, Recorder, RequestRecord};

    let root = scratch_home("model-price-editor");
    let mut draft = gateway_template_for_test(&root);
    draft["pricing"] = json!({ "version": 0, "models": {} });
    let data_dir = PathBuf::from(draft["data"]["dir"].as_str().unwrap());
    std::fs::create_dir_all(&data_dir).unwrap();
    let store = SqliteStore::open(&data_dir.join("metrics.sqlite")).unwrap();
    let mut historical = RequestRecord::begin(1, "openai-responses");
    historical.request_id = "historical-v7".to_string();
    historical.requested_model = "model-a".to_string();
    historical.status = 200;
    historical.cost_kind = CostKind::Estimated;
    historical.cost_micros = Some(111);
    historical.price_version = Some(7);
    store.record(&historical);
    drop(store);

    let inner = AppInner::new(root.join("token-station.json"), draft, None);
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    assert_eq!(get_price_table(app.state()).unwrap().version, 0);
    let v1 = set_model_price(
        app.state(),
        "model-a".to_string(),
        1_000_000,
        2_000_000,
        300_000,
        4_000_000,
        Some(5_000_000),
        0,
    )
    .unwrap();
    assert_eq!(v1.version, 1);
    assert_eq!(v1.models["model-a"].reasoning_per_mtok, Some(5_000_000));
    assert!(
        set_model_price(app.state(), "model-a".to_string(), 9, 9, 9, 9, None, 0,)
            .unwrap_err()
            .contains("版本冲突")
    );

    let v2 = set_model_price(
        app.state(),
        "model-a".to_string(),
        2_000_000,
        3_000_000,
        300_000,
        4_000_000,
        None,
        1,
    )
    .unwrap();
    assert_eq!(v2.version, 2);
    let v3 = remove_model_price(app.state(), "model-a".to_string(), 2).unwrap();
    assert_eq!(v3.version, 3);
    assert!(v3.models.is_empty());

    let saved = ClientConfig::load(&root.join("token-station.json")).unwrap();
    assert_eq!(saved.pricing.version, 3);
    assert!(saved.pricing.models.is_empty());
    let receipts = SqliteStore::recent_receipts(&data_dir.join("metrics.sqlite"), 5).unwrap();
    assert_eq!(receipts[0].cost_micros, Some(111));
    assert_eq!(receipts[0].price_version, Some(7));
    let source_filtered = get_stats(
        app.state(),
        "all".to_string(),
        None,
        None,
        Some("openai-responses".to_string()),
        None,
        None,
    )
    .unwrap();
    assert_eq!(source_filtered.total.requests, 1);
    let agent_filtered = get_stats(
        app.state(),
        "all".to_string(),
        None,
        Some("codex".to_string()),
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(agent_filtered.total.requests, 0);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn public_price_batch_scopes_models_preserves_manual_values_and_bumps_once() {
    use token_station_metrics::{
        CostKind, RecordedDecidedBy, Recorder, RequestRecord, RoutingRecord,
    };
    use token_station_protocol::Usage;
    use token_station_router_core::RequestFeatures;

    let root = scratch_home("public-price-batch");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://api.example.test/v1",
        "models": [{"model": "model-a"}, {"model": "model-b"}, {"model": "missing"}]
    });
    draft["pricing"] = json!({
        "version": 4,
        "models": {
            "fixture/model-a": {
                "input_per_mtok": 99,
                "output_per_mtok": 199
            }
        }
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    let suggestion = |requested_model_id: &str, input_per_mtok| RequestedModelPriceSuggestion {
        requested_model_id: requested_model_id.to_owned(),
        suggestion: ModelPriceSuggestionView {
            model_id: requested_model_id.to_owned(),
            display_name: requested_model_id.to_owned(),
            provider_id: "fixture-catalog".to_owned(),
            provider_name: "Fixture".to_owned(),
            source: "models.dev".to_owned(),
            catalog_source: "cache".to_owned(),
            fetched_at_ms: 1,
            input_per_mtok,
            output_per_mtok: input_per_mtok * 2,
            cache_read_per_mtok: 0,
            cache_write_per_mtok: 0,
            reasoning_per_mtok: None,
        },
    };
    let requested = BTreeSet::from([
        "missing".to_owned(),
        "model-a".to_owned(),
        "model-b".to_owned(),
    ]);

    let mut stale = vec![suggestion("model-b", 2)];
    stale[0].suggestion.catalog_source = "stale_cache".to_owned();
    assert!(ensure_automatic_price_suggestions_fresh(&stale)
        .unwrap_err()
        .contains("cached prices are advisory only"));

    let result = apply_public_model_prices(
        &mut inner,
        "fixture",
        &requested,
        vec![suggestion("model-a", 1), suggestion("model-b", 2)],
    )
    .unwrap();

    assert_eq!(result, (1, 1, vec!["missing".to_owned()], 5));
    let pricing = draft_price_table(&inner).unwrap();
    assert_eq!(pricing.version, 5);
    assert_eq!(pricing.models["fixture/model-a"].input_per_mtok, 99);
    assert_eq!(pricing.models["fixture/model-b"].input_per_mtok, 2);
    assert!(!pricing.models.contains_key("model-b"));

    let data_dir = inner.data_dir();
    std::fs::create_dir_all(&data_dir).unwrap();
    let db = data_dir.join("metrics.sqlite");
    let store = SqliteStore::open(&db).unwrap();
    let mut unknown = RequestRecord::begin(1, "openai-responses");
    unknown.request_id = "auto-price-backfill".to_owned();
    unknown.requested_model = "model-b".to_owned();
    unknown.status = 200;
    unknown.routing = Some(RoutingRecord {
        upstream: "fixture".to_owned(),
        model: "model-b".to_owned(),
        pool: "main".to_owned(),
        decided_by: RecordedDecidedBy::Default,
        fallbacks: 0,
        features: RequestFeatures::default(),
    });
    unknown.usage = Some(Usage {
        input_tokens: 1_000_000,
        ..Usage::default()
    });
    unknown.cost_kind = CostKind::Unknown;
    store.record(&unknown);
    drop(store);

    inner
        .save_draft()
        .expect("saving an automatically imported price also backfills receipts");
    let receipts = SqliteStore::recent_receipts(&db, 5).unwrap();
    assert_eq!(receipts[0].cost_kind, CostKind::Estimated);
    assert_eq!(receipts[0].cost_micros, Some(2));
    assert_eq!(receipts[0].price_version, Some(5));

    let unconfigured = BTreeSet::from(["model-outside-provider".to_owned()]);
    let error = apply_public_model_prices(
        &mut inner,
        "fixture",
        &unconfigured,
        vec![suggestion("model-outside-provider", 3)],
    )
    .unwrap_err();
    assert!(error.contains("is not configured for Provider"), "{error}");
    assert_eq!(draft_price_table(&inner).unwrap().version, 5);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn public_price_import_rejects_a_changed_target_snapshot() {
    let root = scratch_home("public-price-stale-target");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://old.example/v1",
        "models": [{"model": "model-a"}]
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);
    let target = capture_price_import_target(&inner, "fixture").unwrap();
    let original = inner.draft["upstreams"]["fixture"].clone();

    inner.draft["upstreams"]["fixture"]["base_url"] = json!("https://new.example/v1");
    inner.observe_draft().unwrap();
    inner.draft["upstreams"]["fixture"] = original;
    inner.observe_draft().unwrap();

    let restored = capture_price_import_target(&inner, "fixture").unwrap();
    assert_eq!(restored.upstream, target.upstream);
    assert_eq!(restored.price_version, target.price_version);
    assert!(restored.upstream_epoch > target.upstream_epoch);

    let error = ensure_price_import_target_unchanged(&inner, "fixture", &target)
        .expect_err("an ABA edit must still invalidate the old Provider identity");
    assert!(
        error.contains("changed while public prices were loading"),
        "{error}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn public_price_import_rejects_a_secret_only_identity_rotation() {
    let root = scratch_home("public-price-secret-rotation");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://same.example/v1",
        "auth": {"slot": "provider_api_key", "store": true},
        "models": [{"model": "model-a"}]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));
    let target = {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        capture_price_import_target(&inner, "fixture").unwrap()
    };

    edit_provider(
        app.state(),
        "fixture".to_owned(),
        "https://same.example/v1".to_owned(),
        Some("rotated-secret".to_owned()),
    )
    .expect("a stored credential may be rotated without changing its descriptor");

    let state = app.state::<AppStateManaged>();
    let inner = state.0.lock().unwrap();
    let rotated = capture_price_import_target(&inner, "fixture").unwrap();
    assert_eq!(rotated.upstream, target.upstream);
    assert_eq!(rotated.price_version, target.price_version);
    assert!(rotated.upstream_epoch > target.upstream_epoch);
    ensure_price_import_target_unchanged(&inner, "fixture", &target)
        .expect_err("a key rotation must invalidate an in-flight price import");
    drop(inner);
    secrets::store_remove(&root, "fixture", "provider_api_key").ok();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_discovery_targets_reject_identity_aba_absent_add_and_older_generations() {
    let root = scratch_home("provider-discovery-targets");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://a.example/v1",
        "models": [{"model": "model-a"}]
    });
    let mut inner = AppInner::new(root.join("token-station.json"), draft, None);

    let original = inner.draft["upstreams"]["fixture"].clone();
    let aba_target = begin_provider_discovery_target(&mut inner, "fixture");
    inner.draft["upstreams"]["fixture"]["base_url"] = json!("https://b.example/v1");
    inner.observe_draft().unwrap();
    inner.draft["upstreams"]["fixture"] = original;
    inner.observe_draft().unwrap();
    ensure_provider_discovery_target_unchanged(&inner, "fixture", &aba_target)
        .expect_err("an A-B-A edit must invalidate discovery");

    let older = begin_provider_discovery_target(&mut inner, "fixture");
    let latest = begin_provider_discovery_target(&mut inner, "fixture");
    ensure_provider_discovery_target_unchanged(&inner, "fixture", &older)
        .expect_err("only the latest same-identity discovery may commit");
    ensure_provider_discovery_target_unchanged(&inner, "fixture", &latest)
        .expect("the latest same-identity discovery remains current");

    let absent = begin_provider_discovery_target(&mut inner, "new-provider");
    inner.draft["upstreams"]["new-provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://new.example/v1",
        "models": [{"model": "model-a"}]
    });
    inner.observe_draft().unwrap();
    ensure_provider_discovery_target_unchanged(&inner, "new-provider", &absent)
        .expect_err("adding a same-name Provider must invalidate prior discovery");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn provider_model_ids_are_bounded_normalized_and_unique() {
    assert_eq!(
        normalize_provider_model_ids(vec![
            " model-b ".to_owned(),
            "model-a".to_owned(),
            "model-b".to_owned(),
        ])
        .unwrap(),
        vec!["model-b".to_owned(), "model-a".to_owned()]
    );
    assert!(normalize_provider_model_ids(vec!["bad\nmodel".to_owned()]).is_err());
    assert!(
        normalize_provider_model_ids(vec!["x".repeat(model_catalog::MAX_MODEL_ID_BYTES + 1)])
            .is_err()
    );
    assert!(normalize_provider_model_ids(vec![
        "same".to_owned();
        model_catalog::MAX_MODELS_PER_PROVIDER + 1
    ])
    .is_err());
}

#[test]
fn provider_transport_rejects_remote_http_and_proxied_loopback_credentials() {
    let direct = EgressConfig::default();
    let loopback = ProviderEndpoint::try_new("http://127.0.0.1:11434/v1").unwrap();
    ensure_credential_transport(&loopback, &direct)
        .expect("direct loopback credentials stay on the device");

    let proxied: EgressConfig = serde_json::from_value(json!({
        "mode": "http",
        "proxy_url": "http://proxy.example.test:8080"
    }))
    .unwrap();
    assert!(ensure_credential_transport(&loopback, &proxied)
        .unwrap_err()
        .contains("must bypass"));

    let bypassed: EgressConfig = serde_json::from_value(json!({
        "mode": "http",
        "proxy_url": "http://proxy.example.test:8080",
        "no_proxy": ["127.0.0.1"]
    }))
    .unwrap();
    ensure_credential_transport(&loopback, &bypassed)
        .expect("an exact proxy bypass keeps loopback credentials local");

    let remote = ProviderEndpoint::try_new("http://192.0.2.1/v1").unwrap();
    assert!(ensure_credential_transport(&remote, &direct)
        .unwrap_err()
        .contains("must use HTTPS"));
}

#[test]
fn provider_health_uses_the_configured_production_engine() {
    let draft = json!({
        "plugins": {"providers": {
            "openai-compatible": "provider-openai-compatible",
            "azure-openai-v1": "provider-openai-compatible"
        }},
        "egress": {"mode": "direct"}
    });
    let eligible = json!({
        "provider": "openai-compatible",
        "provider_call": "south_v1_buffered_streaming",
        "auth": {"env": "PROVIDER_API_KEY"}
    });
    assert!(provider_health_uses_south(&draft, &eligible, true));

    // No engine named: the South default applies, as it does for traffic.
    let defaulted = json!({
        "provider": "openai-compatible",
        "auth": {"env": "PROVIDER_API_KEY"}
    });
    assert!(provider_health_uses_south(&draft, &defaulted, true));
    let explicit_legacy = json!({
        "provider": "openai-compatible",
        "provider_call": "legacy",
        "auth": {"env": "PROVIDER_API_KEY"}
    });
    assert!(!provider_health_uses_south(&draft, &explicit_legacy, true));

    let mut proxied = draft.clone();
    proxied["egress"] = json!({
        "mode": "http",
        "proxy_url": "http://proxy.example.test:8080"
    });
    assert!(!provider_health_uses_south(&proxied, &eligible, true));

    let mut native = eligible.clone();
    native["api_dialect"] = json!("anthropic-native");
    assert!(!provider_health_uses_south(&draft, &native, true));
    assert!(!provider_health_uses_south(&draft, &eligible, false));

    let azure_header = json!({
        "provider": "azure-openai-v1",
        "provider_call": "south_v1_buffered_streaming_header_auth",
        "auth": {"store": true}
    });
    assert!(provider_health_uses_south(&draft, &azure_header, true));
    let mut azure_legacy_south = azure_header;
    azure_legacy_south["provider_call"] = json!("south_v1_buffered_streaming");
    assert!(!provider_health_uses_south(
        &draft,
        &azure_legacy_south,
        true
    ));
}

#[test]
fn public_price_import_derives_its_catalog_namespace_from_provider_identity() {
    let root = scratch_home("public-price-provider-mapping");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["glm_cn"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://open.bigmodel.cn/api/paas/v4",
        "models": [{"model": "glm-5.2"}]
    });
    draft["upstreams"]["custom"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://custom.example/v1",
        "models": [{"model": "custom-model"}]
    });
    let inner = AppInner::new(root.join("token-station.json"), draft, None);

    assert_eq!(
        configured_public_price_provider_id(&inner, "glm_cn").unwrap(),
        "zhipuai"
    );
    assert!(configured_public_price_provider_id(&inner, "custom")
        .unwrap_err()
        .contains("no authoritative"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn legacy_empty_price_table_receives_builtin_catalog_once() {
    let mut draft = json!({ "pricing": { "version": 0, "models": {} } });

    assert!(seed_builtin_pricing(&mut draft).unwrap());
    let table: PriceTable = serde_json::from_value(draft["pricing"].clone()).unwrap();
    assert_eq!(table.version, 1);
    assert!(table.models.contains_key("deepseek-v4-pro"));
    assert!(!seed_builtin_pricing(&mut draft).unwrap());
}

#[test]
fn local_only_routing_flags_local_providers_and_toggles_the_switch() {
    let root = scratch_home("local-only-routing");
    let inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    // One local provider marked local and one cloud provider.
    add_provider(
        app.state(),
        "ollama".to_owned(),
        "http://127.0.0.1:11434/v1".to_owned(),
        vec!["llama3".to_owned()],
        None,
        true,
    )
    .unwrap();
    let view = add_provider(
        app.state(),
        "openai".to_owned(),
        "https://api.openai.com/v1".to_owned(),
        vec!["gpt-5".to_owned()],
        None,
        false,
    )
    .unwrap();

    let ollama = view.providers.iter().find(|p| p.name == "ollama").unwrap();
    assert!(
        ollama.local,
        "the local provider is flagged for local_only routing"
    );
    let openai = view.providers.iter().find(|p| p.name == "openai").unwrap();
    assert!(!openai.local, "an ordinary cloud provider is not flagged");
    assert!(!view.local_only, "local_only is off until asked for");
    assert!(!view.allow_cloud_fallback);

    // Enable local-only routing with cloud fallback.
    let on = set_local_routing(app.state(), true, true).unwrap();
    assert!(on.local_only);
    assert!(on.allow_cloud_fallback);

    // Disabling clears both keys and returns the config to clean default-equivalent state.
    let off = set_local_routing(app.state(), false, false).unwrap();
    assert!(!off.local_only);
    assert!(!off.allow_cloud_fallback);
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert!(inner.draft["router"].get("local_only").is_none());
        assert!(inner.draft["router"].get("allow_cloud_fallback").is_none());
    }

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn historical_home_mode_without_top_level_routing_remains_visible() {
    let root = scratch_home("historical-home-routing-mode");
    let mut draft = template_for_test(&root);
    draft["router"]["routing_mode"] = json!("quota_first");

    let view = AppInner::new(root.join("token-station.json"), draft, None).snapshot();

    assert_eq!(view.routing_mode, "quota_first");
    assert!(view
        .agent_routes
        .values()
        .all(|route| route.routing_mode == "quota_first"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn routing_mode_switches_per_agent_without_touching_home_or_siblings() {
    let root = scratch_home("per-agent-routing-mode");
    let inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    // Home default is tiered, and every Agent inherits it.
    let view = get_state(app.state());
    assert_eq!(view.routing_mode, "tiered");
    assert!(
        view.agent_routes
            .values()
            .all(|r| r.routing_mode == "tiered"),
        "every Agent inherits the tiered home default"
    );

    // Flip Home to quota-first: the Home view flips, and Agents that never
    // overrode follow it.
    let view = set_routing_mode(app.state(), "quota_first".to_owned(), None).unwrap();
    assert_eq!(view.routing_mode, "quota_first");
    assert!(
        view.agent_routes
            .values()
            .all(|r| r.routing_mode == "quota_first"),
        "un-overridden Agents follow the new Home default"
    );
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(inner.draft["routing"]["mode"], json!("quota_first"));
        assert_eq!(inner.draft["router"]["routing_mode"], json!("quota_first"));
    }

    // Pin one Agent back to tiered while Home stays quota-first. Only that
    // Agent changes; Home and its siblings keep quota-first.
    let view =
        set_routing_mode(app.state(), "tiered".to_owned(), Some("codex".to_owned())).unwrap();
    assert_eq!(view.routing_mode, "quota_first", "Home is untouched");
    assert_eq!(view.agent_routes["codex"].routing_mode, "tiered");
    assert!(
        view.agent_routes
            .iter()
            .filter(|(id, _)| id.as_str() != "codex")
            .all(|(_, r)| r.routing_mode == "quota_first"),
        "sibling Agents are untouched by the per-Agent switch"
    );
    // The Agent's mode is written explicitly (not cleared), so it stays
    // pinned independent of Home — the whole point of a per-Agent switch.
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(
            inner.draft["agent_routes"]["codex"]["routing_mode"].as_str(),
            Some("tiered")
        );
    }

    // Flipping Home back to tiered leaves the pinned Agent alone (still an
    // explicit "tiered" override), and un-pinned siblings follow Home.
    let view = set_routing_mode(app.state(), "tiered".to_owned(), None).unwrap();
    assert_eq!(view.routing_mode, "tiered");
    assert_eq!(view.agent_routes["codex"].routing_mode, "tiered");
    assert!(
        view.agent_routes
            .iter()
            .filter(|(id, _)| id.as_str() != "codex")
            .all(|(_, r)| r.routing_mode == "tiered"),
        "un-pinned siblings track the tiered Home again"
    );
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(inner.draft["routing"]["mode"], json!("tiered"));
        assert_eq!(inner.draft["router"]["routing_mode"], json!("tiered"));
    }

    // Switching the pinned Agent to quota-first writes the explicit value.
    let view = set_routing_mode(
        app.state(),
        "quota_first".to_owned(),
        Some("codex".to_owned()),
    )
    .unwrap();
    assert_eq!(view.agent_routes["codex"].routing_mode, "quota_first");
    assert_eq!(view.routing_mode, "tiered", "Home stays tiered");

    let direct = set_routing_mode(app.state(), "direct".to_owned(), None).unwrap();
    assert_eq!(direct.routing_mode, "direct");
    assert_eq!(direct.agent_routes["codex"].routing_mode, "quota_first");
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(inner.draft["routing"]["mode"], json!("direct"));
        assert_eq!(inner.draft["router"]["routing_mode"], json!("tiered"));
    }

    // Unknown Agent is rejected.
    assert!(set_routing_mode(
        app.state(),
        "quota_first".to_owned(),
        Some("nope".to_owned())
    )
    .is_err());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn direct_route_targets_are_validated_and_isolated_between_home_and_agents() {
    let root = scratch_home("direct-route-targets");
    let mut draft = template_for_test(&root);
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "home"}, {"model": "agent"}]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    let home =
        set_direct_route(app.state(), "provider".to_owned(), "home".to_owned(), None).unwrap();
    let home_target = home.direct_target.expect("Home target is public state");
    assert_eq!(
        (home_target.upstream.as_str(), home_target.model.as_deref()),
        ("provider", Some("home"))
    );
    assert!(home.agent_routes.values().all(|route| route
        .direct_target
        .as_ref()
        .is_some_and(|target| target.model.as_deref() == Some("home"))));
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(inner.draft["routing"]["mode"], json!("tiered"));
        assert_eq!(
            inner.draft["routing"]["direct_target"],
            json!({"upstream": "provider", "model": "home"})
        );
        assert!(inner.draft["router"].get("direct_target").is_none());
    }

    let codex = set_direct_route(
        app.state(),
        "provider".to_owned(),
        "agent".to_owned(),
        Some("codex".to_owned()),
    )
    .unwrap();
    assert_eq!(
        codex.direct_target.as_ref().unwrap().model.as_deref(),
        Some("home")
    );
    assert_eq!(
        codex.agent_routes["codex"]
            .direct_target
            .as_ref()
            .unwrap()
            .model
            .as_deref(),
        Some("agent")
    );
    assert_eq!(
        codex.agent_routes["opencode"]
            .direct_target
            .as_ref()
            .unwrap()
            .model
            .as_deref(),
        Some("home")
    );
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(
            inner.draft["agent_routes"]["codex"]["direct_target"],
            json!({"upstream": "provider", "model": "agent"})
        );
        assert_eq!(inner.draft["routing"]["direct_target"]["model"], "home");
    }

    let before_revision = codex.draft_revision;
    let error = match set_direct_route(
        app.state(),
        "provider".to_owned(),
        "missing".to_owned(),
        Some("codex".to_owned()),
    ) {
        Ok(_) => panic!("an unmanaged model is rejected transactionally"),
        Err(error) => error,
    };
    assert!(error.contains("未配置模型"), "{error}");
    let unchanged = get_state(app.state());
    assert_eq!(unchanged.draft_revision, before_revision);
    assert_eq!(
        unchanged.agent_routes["codex"]
            .direct_target
            .as_ref()
            .unwrap()
            .model
            .as_deref(),
        Some("agent")
    );
    assert!(!root.join("token-station.json").exists());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn an_explicit_incomplete_agent_direct_target_does_not_inherit_home() {
    let root = scratch_home("agent-incomplete-direct-target");
    let mut draft = template_for_test(&root);
    draft["routing"] = json!({
        "mode": "direct",
        "direct_target": {"upstream": "provider", "model": "home"}
    });
    draft["agent_routes"]["codex"] = json!({
        "mode": "inherit",
        "direct_target": {"upstream": "provider", "model": null}
    });

    let view = AppInner::new(root.join("token-station.json"), draft, None).snapshot();

    assert_eq!(
        view.direct_target.as_ref().unwrap().model.as_deref(),
        Some("home")
    );
    let wire = serde_json::to_value(&view).expect("StateView serializes");
    assert_eq!(
        wire["agent_routes"]["codex"]["direct_target"],
        json!({"upstream": "provider", "model": null}),
        "a known Agent provider must remain selected while its model is incomplete"
    );
    assert!(view.agent_routes["codex"].config_error.is_some());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn direct_config_saves_without_a_dummy_tier_pool() {
    let root = scratch_home("direct-save");
    let mut draft = template_for_test(&root);
    draft["routing"] = json!({"mode": "direct"});
    draft["router"]["routing_mode"] = json!("tiered");
    draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "selected"}]
    });
    draft["routing"]["direct_target"] = json!({"upstream": "provider", "model": "selected"});
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));

    let saved = save_config(app.state()).expect("direct mode needs no synthetic tier pool");

    assert_eq!(saved.routing_mode, "direct");
    assert_eq!(
        saved.direct_target.as_ref().unwrap().model.as_deref(),
        Some("selected")
    );
    assert!(root.join("token-station.json").exists());
    let persisted: Value = serde_json::from_slice(
        &std::fs::read(root.join("token-station.json")).expect("saved config is readable"),
    )
    .expect("saved config remains JSON");
    assert_eq!(persisted["routing"]["mode"], json!("direct"));
    assert_eq!(persisted["router"]["routing_mode"], json!("tiered"));
    assert!(persisted["router"].get("direct_target").is_none());
    let config_path = root.join("token-station.json");
    let (reloaded_draft, reloaded_saved, load_error) = load_draft_state(
        &config_path,
        &root.join("token-station-data"),
        &root.join("plugins"),
    );
    assert!(load_error.is_none());
    assert_eq!(reloaded_draft["routing"], persisted["routing"]);
    let reloaded =
        AppInner::new_with_saved(config_path, reloaded_draft, reloaded_saved, load_error)
            .snapshot();
    assert!(!reloaded.config_dirty);
    assert_eq!(reloaded.routing_mode, "direct");
    assert_eq!(
        reloaded.direct_target.as_ref().unwrap().model.as_deref(),
        Some("selected")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn saving_agent_tier_edits_preserves_its_direct_target_and_routing_mode() {
    let root = scratch_home("agent-direct-preserved");
    let mut inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    inner.draft["upstreams"]["provider"] = json!({
        "provider": "openai-compatible",
        "base_url": "https://example.com/v1",
        "models": [{"model": "selected"}]
    });
    for pool in [TIER_HIGH, TIER_MID, TIER_LOW] {
        inner
            .set_tier_value(
                pool,
                Some("provider".to_owned()),
                Some("selected".to_owned()),
            )
            .unwrap();
    }
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    set_direct_route(
        app.state(),
        "provider".to_owned(),
        "selected".to_owned(),
        Some("codex".to_owned()),
    )
    .unwrap();
    set_routing_mode(app.state(), "direct".to_owned(), Some("codex".to_owned())).unwrap();
    set_agent_route_mode(app.state(), "codex".to_owned(), "custom".to_owned()).unwrap();
    for slot in ["high", "mid", "low"] {
        set_agent_tier(
            app.state(),
            "codex".to_owned(),
            slot.to_owned(),
            Some("provider".to_owned()),
            Some("selected".to_owned()),
        )
        .unwrap();
    }

    let saved = save_agent_routes(app.state()).unwrap();

    assert_eq!(saved.agent_routes["codex"].routing_mode, "direct");
    assert_eq!(
        saved.agent_routes["codex"]
            .direct_target
            .as_ref()
            .unwrap()
            .model
            .as_deref(),
        Some("selected")
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn quota_accounts_persist_validate_dedupe_and_reject_invalid_input() {
    let root = scratch_home("quota-accounts");
    let inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));

    add_provider(
        app.state(),
        "deepseek".to_owned(),
        "https://api.deepseek.com/v1".to_owned(),
        vec!["deepseek-v4-flash".to_owned(), "deepseek-v4-pro".to_owned()],
        None,
        false,
    )
    .unwrap();
    add_provider(
        app.state(),
        "ollama".to_owned(),
        "http://127.0.0.1:11434/v1".to_owned(),
        vec!["qwen2.5".to_owned()],
        None,
        true,
    )
    .unwrap();

    // Two valid picks, then an exact duplicate of the first (→ collapsed).
    // Order is preserved as the operator's priority.
    let arg = |upstream: &str, model: &str| QuotaAccountArg {
        upstream: upstream.to_owned(),
        model: model.to_owned(),
    };
    let view = set_quota_accounts(
        app.state(),
        vec![
            arg("deepseek", "deepseek-v4-flash"),
            arg("ollama", "qwen2.5"),
            arg("deepseek", "deepseek-v4-flash"),
        ],
    )
    .unwrap();
    assert_eq!(view.quota_accounts.len(), 2);
    assert_eq!(view.quota_accounts[0].upstream, "deepseek");
    assert_eq!(view.quota_accounts[0].model, "deepseek-v4-flash");
    assert_eq!(view.quota_accounts[1].upstream, "ollama");
    assert_eq!(view.quota_accounts[1].model, "qwen2.5");

    // The list lands verbatim under router.quota_accounts (the router-core
    // key that drives quota routing).
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        let stored = inner.draft["router"]["quota_accounts"].as_array().unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0]["upstream"], json!("deepseek"));
        assert_eq!(stored[0]["model"], json!("deepseek-v4-flash"));
    }

    // An incomplete row is rejected as a whole; the command must not
    // silently reinterpret the visible editor state or touch prior state.
    assert!(set_quota_accounts(app.state(), vec![arg("deepseek", "")]).is_err());
    assert_eq!(get_state(app.state()).quota_accounts.len(), 2);

    // An account referencing a model the provider never declared is rejected,
    // and the previously saved list is left intact.
    assert!(set_quota_accounts(app.state(), vec![arg("deepseek", "ghost-model")]).is_err());
    assert_eq!(get_state(app.state()).quota_accounts.len(), 2);

    // Empty selection is not a valid quota route and leaves prior state intact.
    assert!(set_quota_accounts(app.state(), vec![]).is_err());
    assert_eq!(get_state(app.state()).quota_accounts.len(), 2);

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn quota_plan_declares_a_window_validates_and_clears() {
    let root = scratch_home("quota-plan");
    let inner = AppInner::new(
        root.join("token-station.json"),
        template_for_test(&root),
        None,
    );
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(inner))));
    add_provider(
        app.state(),
        "deepseek".to_owned(),
        "https://api.deepseek.com/v1".to_owned(),
        vec!["deepseek-v4-flash".to_owned()],
        None,
        false,
    )
    .unwrap();

    // Declare a 5h / 1,000,000-token plan with a 60/min rate limit.
    let view = set_quota_plan(
        app.state(),
        "deepseek".to_owned(),
        18_000_000,
        1_000_000,
        "tokens".to_owned(),
        Some(60),
    )
    .unwrap();
    let plan = view
        .providers
        .iter()
        .find(|p| p.name == "deepseek")
        .unwrap()
        .quota_plan
        .as_ref()
        .unwrap();
    assert_eq!(plan.len_ms, 18_000_000);
    assert_eq!(plan.limit, 1_000_000);
    assert_eq!(plan.unit, "tokens");
    assert_eq!(plan.rate_limit_per_min, Some(60));
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert_eq!(
            inner.draft["upstreams"]["deepseek"]["quota_plan"]["windows"][0]["limit"],
            json!(1_000_000)
        );
    }

    // Unknown provider and unknown unit are rejected.
    assert!(set_quota_plan(
        app.state(),
        "nope".to_owned(),
        1,
        1,
        "tokens".to_owned(),
        None
    )
    .is_err());
    assert!(set_quota_plan(
        app.state(),
        "deepseek".to_owned(),
        1,
        1,
        "credits".to_owned(),
        None
    )
    .is_err());

    // A zero limit clears the plan entirely.
    let cleared = set_quota_plan(
        app.state(),
        "deepseek".to_owned(),
        18_000_000,
        0,
        "tokens".to_owned(),
        None,
    )
    .unwrap();
    assert!(cleared
        .providers
        .iter()
        .find(|p| p.name == "deepseek")
        .unwrap()
        .quota_plan
        .is_none());
    {
        let state = app.state::<AppStateManaged>();
        let inner = state.0.lock().unwrap();
        assert!(inner.draft["upstreams"]["deepseek"]
            .get("quota_plan")
            .is_none());
    }

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn desktop_helpers_cover_empty_absolute_and_legacy_display_shapes() {
    let root = scratch_home("helper-shapes");
    let missing = root.join("missing.json");
    let (draft, error) = load_draft(&missing, &root);
    assert!(error.is_none());
    assert_eq!(draft["server"]["auth"], json!(true));

    let absolute = root.join("already-absolute");
    let mut shapes = template_for_test(&root);
    shapes["plugins"]["dir"] = json!(absolute.clone());
    shapes["data"]["dir"] = json!(42);
    let shapes = prepare_desktop_draft(shapes, &root);
    assert_eq!(shapes["plugins"]["dir"], json!(absolute));
    assert_eq!(shapes["data"]["dir"], json!(42));

    assert_eq!(agents_display(&json!({"agent": "legacy"})), "legacy");
    assert_eq!(agents_display(&json!({"agents": [1, null]})), "");
    assert_eq!(pool_key("high").unwrap(), TIER_HIGH);
    assert_eq!(pool_key("mid").unwrap(), TIER_MID);
    assert_eq!(pool_key("low").unwrap(), TIER_LOW);

    let mut inner = AppInner::new(
        root.join("token-station.json"),
        json!({
            "server": {}, "data": {}, "plugins": {}, "upstreams": [],
            "router": {"pools": [], "rules": null, "hint_routes": null}
        }),
        None,
    );
    assert!(inner.upstreams().is_empty());
    assert_eq!(inner.pool_member("missing"), (None, None));
    inner.rebuild_routing();
    assert!(inner.draft["router"]["heuristic"].is_null());
    assert_eq!(inner.serve_view().listen, "127.0.0.1:8787");
    assert!(inner.config_error().is_some());

    std::fs::remove_dir_all(root).ok();
}

#[test]
fn model_test_messages_enforce_roles_order_and_size_bounds() {
    let valid = vec![
        ModelTestMessage {
            role: "user".to_owned(),
            content: "hello".to_owned(),
        },
        ModelTestMessage {
            role: "assistant".to_owned(),
            content: "hi".to_owned(),
        },
        ModelTestMessage {
            role: "user".to_owned(),
            content: "status".to_owned(),
        },
    ];
    assert!(validate_model_test_messages(&valid).is_ok());

    let wrong_role = [ModelTestMessage {
        role: "system".to_owned(),
        content: "hidden".to_owned(),
    }];
    assert!(validate_model_test_messages(&wrong_role).is_err());

    let assistant_last = [ModelTestMessage {
        role: "assistant".to_owned(),
        content: "done".to_owned(),
    }];
    assert!(validate_model_test_messages(&assistant_last).is_err());

    let assistant_first = [
        ModelTestMessage {
            role: "assistant".to_owned(),
            content: "orphan".to_owned(),
        },
        ModelTestMessage {
            role: "user".to_owned(),
            content: "question".to_owned(),
        },
    ];
    assert!(validate_model_test_messages(&assistant_first).is_err());

    let consecutive_users = [
        ModelTestMessage {
            role: "user".to_owned(),
            content: "first".to_owned(),
        },
        ModelTestMessage {
            role: "user".to_owned(),
            content: "second".to_owned(),
        },
    ];
    assert!(validate_model_test_messages(&consecutive_users).is_err());

    let oversized = [ModelTestMessage {
        role: "user".to_owned(),
        content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES + 1),
    }];
    assert!(validate_model_test_messages(&oversized).is_err());

    let too_many = (0..=MODEL_TEST_MAX_MESSAGES)
        .map(|_| ModelTestMessage {
            role: "user".to_owned(),
            content: "x".to_owned(),
        })
        .collect::<Vec<_>>();
    assert!(validate_model_test_messages(&too_many).is_err());

    let empty = [ModelTestMessage {
        role: "user".to_owned(),
        content: "  ".to_owned(),
    }];
    assert!(validate_model_test_messages(&empty).is_err());

    let total_too_large = [
        ModelTestMessage {
            role: "user".to_owned(),
            content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES),
        },
        ModelTestMessage {
            role: "assistant".to_owned(),
            content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES),
        },
        ModelTestMessage {
            role: "user".to_owned(),
            content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES),
        },
        ModelTestMessage {
            role: "assistant".to_owned(),
            content: "x".repeat(MODEL_TEST_MAX_MESSAGE_BYTES),
        },
        ModelTestMessage {
            role: "user".to_owned(),
            content: "x".to_owned(),
        },
    ];
    assert!(validate_model_test_messages(&total_too_large).is_err());
}

#[test]
fn model_test_reply_extracts_text_and_keeps_provider_errors_value_free() {
    let reply =
        extract_model_test_reply(200, r#"{"choices":[{"message":{"content":"connected"}}]}"#)
            .unwrap();
    assert_eq!(reply, "connected");

    let multipart = extract_model_test_reply(
        200,
        r#"{"choices":[{"message":{"content":[{"text":"part "},{"text":"two"}]}}]}"#,
    )
    .unwrap();
    assert_eq!(multipart, "part two");

    let error = extract_model_test_reply(
        401,
        r#"{"error":{"code":"invalid_api_key","message":"prompt and secret must not escape"}}"#,
    )
    .unwrap_err();
    assert!(error.contains("authentication failed"));
    assert!(!error.contains("invalid_api_key"));
    assert!(!error.contains("prompt"));
    assert!(!error.contains("secret"));

    let oversized = format!(
        r#"{{"choices":[{{"message":{{"content":"{}"}}}}]}}"#,
        "x".repeat(MODEL_TEST_MAX_RESPONSE_BYTES + 1)
    );
    assert!(extract_model_test_reply(200, &oversized)
        .unwrap_err()
        .contains("response limit"));

    let oversized_envelope = format!(
        r#"{{"id":"{}","choices":[{{"message":{{"content":"ok"}}}}]}}"#,
        "x".repeat(MODEL_TEST_MAX_STREAM_BYTES)
    );
    assert!(extract_model_test_reply(200, &oversized_envelope)
        .unwrap_err()
        .contains("wire response limit"));
}

/// Pay-per-token providers report an exhausted wallet as HTTP 429 rather
/// than 402 (observed live: wecoding's `credit_balance_exhausted` and
/// z.ai's code 1113 "Insufficient balance"). The console must tell the
/// user to recharge, not to retry — while still never echoing body text.
#[test]
fn model_test_reply_reports_a_429_exhausted_wallet_as_no_balance() {
    let wecoding = extract_model_test_reply(
        429,
        r#"{"error":{"code":"credit_balance_exhausted","internal_code":"PLATFORM_BALANCE_INSUFFICIENT","message":"wallet balance is insufficient","type":"insufficient_quota"}}"#,
    )
    .unwrap_err();
    assert!(wecoding.contains("no available balance"), "{wecoding}");
    assert!(wecoding.contains("HTTP 429"), "{wecoding}");
    assert!(!wecoding.contains("wallet"), "{wecoding}");

    let zai = extract_model_test_reply(
        429,
        r#"{"error":{"code":"1113","message":"Insufficient balance or no resource package. Please recharge."}}"#,
    )
    .unwrap_err();
    assert!(zai.contains("no available balance"), "{zai}");
    assert!(!zai.contains("recharge"), "{zai}");

    let zai_chinese = extract_model_test_reply(
        429,
        r#"{"error":{"code":"1113","message":"余额不足或无可用资源包,请充值。"}}"#,
    )
    .unwrap_err();
    assert!(
        zai_chinese.contains("no available balance"),
        "{zai_chinese}"
    );
    assert!(!zai_chinese.contains("余额"), "{zai_chinese}");

    let real_rate_limit = extract_model_test_reply(
        429,
        r#"{"error":{"code":"rate_limit_exceeded","message":"Rate limit reached, retry after 20s"}}"#,
    )
    .unwrap_err();
    assert!(real_rate_limit.contains("rate limit"), "{real_rate_limit}");
    assert!(!real_rate_limit.contains("20s"), "{real_rate_limit}");
}

#[test]
fn model_test_sse_decoder_handles_split_frames_and_utf8_boundaries() {
    let wire = "data: {\"choices\":[{\"delta\":{\"content\":\"你\"}}]}\n\n".as_bytes();
    let chinese = "你".as_bytes();
    let utf8_start = wire
        .windows(chinese.len())
        .position(|window| window == chinese)
        .expect("fixture contains a multibyte delta");
    let mut decoder = ModelTestSseDecoder::default();

    assert!(decoder.push(&wire[..utf8_start + 1]).unwrap().is_empty());
    let frames = decoder.push(&wire[utf8_start + 1..wire.len() - 1]).unwrap();
    assert!(
        frames.is_empty(),
        "an incomplete SSE delimiter must remain buffered"
    );
    let frames = decoder.push(&wire[wire.len() - 1..]).unwrap();

    assert_eq!(frames.len(), 1);
    assert_eq!(
        model_test_stream_delta(&frames[0]).unwrap(),
        Some("你".to_owned())
    );
    assert!(decoder.finish().unwrap().is_empty());

    let mut trailing = ModelTestSseDecoder::default();
    assert!(trailing.push(b"data: tail").unwrap().is_empty());
    assert_eq!(trailing.finish().unwrap(), ["data: tail"]);

    assert_eq!(
        find_model_test_sse_boundary(b"a\n\nb\r\n\r\n"),
        Some((1, 2))
    );
    assert_eq!(
        find_model_test_sse_boundary(b"a\r\n\r\nb\n\n"),
        Some((1, 4))
    );
    assert_eq!(find_model_test_sse_boundary(b"a\r\n\r\n"), Some((1, 4)));
}

#[test]
fn model_test_sse_delta_ignores_metadata_and_recognizes_completion() {
    assert_eq!(
        model_test_stream_delta(
            "event: message\ndata: {\"choices\":[{\"delta\":{\"content\":\"hel\"}}]}"
        )
        .unwrap(),
        Some("hel".to_owned())
    );
    assert_eq!(model_test_stream_delta("data: [DONE]").unwrap(), None);
    assert_eq!(model_test_stream_delta(": keepalive").unwrap(), None);

    assert_eq!(
        model_test_stream_delta(
            "data: {\"choices\":[{\"delta\":{\"content\":[{\"text\":\"part \"},{\"text\":\"two\"}]}}]}"
        )
        .unwrap(),
        Some("part two".to_owned())
    );
    assert_eq!(
        model_test_stream_delta("data: {\"choices\":[{\"text\":\"legacy\"}]}").unwrap(),
        Some("legacy".to_owned())
    );

    let coded_error = model_test_stream_delta(
        "data: {\"error\":{\"code\":\"stream_rejected\",\"message\":\"secret\"}}",
    )
    .unwrap_err();
    assert!(!coded_error.contains("stream_rejected"));
    assert!(!coded_error.contains("secret"));
    assert_eq!(
        model_test_stream_delta("data: {\"error\":{\"message\":\"secret\"}}").unwrap_err(),
        "The Provider returned a stream error"
    );
}

#[test]
fn model_test_output_budget_rejects_many_individually_valid_deltas() {
    let mut budget = ModelTestOutputBudget::default();
    for _ in 0..MODEL_TEST_MAX_STREAM_EVENTS {
        budget.accept("x").unwrap();
    }
    assert!(budget.accept("x").unwrap_err().contains("event limit"));

    let mut byte_budget = ModelTestOutputBudget::default();
    byte_budget
        .accept(&"x".repeat(MODEL_TEST_MAX_RESPONSE_BYTES))
        .unwrap();
    assert!(byte_budget
        .accept("x")
        .unwrap_err()
        .contains("response limit"));

    let mut wire_budget = ModelTestOutputBudget::default();
    wire_budget
        .accept_wire(MODEL_TEST_MAX_STREAM_BYTES)
        .unwrap();
    assert!(wire_budget
        .accept_wire(1)
        .unwrap_err()
        .contains("wire response limit"));
}

#[test]
fn model_test_command_uses_an_exact_in_memory_route_and_cleans_registration() {
    let root = scratch_home("model-test-command");
    let (upstream, fixture) = serve_chat_completion("model-test-ok", 1);
    let mut draft = gateway_template_for_test(&root);
    draft["upstreams"]["fixture"] = json!({
        "provider": "openai-compatible",
        "base_url": upstream,
        "models": [{"model": "small"}]
    });
    let app = tauri::test::mock_app();
    assert!(app.manage(AppStateManaged(Mutex::new(AppInner::new(
        root.join("token-station.json"),
        draft,
        None,
    )))));
    assert!(app.manage(ModelTestStreamState::default()));

    let reply = tauri::async_runtime::block_on(run_model_test_chat(
        app.handle().clone(),
        app.state::<AppStateManaged>().inner(),
        app.state::<ModelTestStreamState>().inner(),
        "fixture".to_owned(),
        "small".to_owned(),
        vec![ModelTestMessage {
            role: "user".to_owned(),
            content: "ping".to_owned(),
        }],
        "model-test-command".to_owned(),
    ))
    .unwrap();

    assert_eq!(reply.content, "model-test-ok");
    assert!(app
        .state::<ModelTestStreamState>()
        .0
        .lock()
        .unwrap()
        .active
        .is_empty());
    fixture.join().unwrap();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn model_test_request_ids_are_bounded_correlation_values() {
    assert!(validate_model_test_request_id("model-test-1729-aBc_0").is_ok());
    assert!(validate_model_test_request_id("").is_err());
    assert!(validate_model_test_request_id("contains spaces").is_err());
    assert!(validate_model_test_request_id(&"a".repeat(65)).is_err());
}

#[test]
fn model_test_cancel_before_registration_still_stops_the_request() {
    let mut registry = ModelTestStreamRegistry::default();
    registry.cancel("model-test-race".to_owned());

    let error = registry
        .register("model-test-race", CancelToken::root())
        .unwrap_err();

    assert_eq!(error, "Model test cancelled");
    assert!(!registry.active.contains_key("model-test-race"));
    assert!(!registry.pending_cancellations.contains("model-test-race"));
}
