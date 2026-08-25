use crate::*;

#[derive(Serialize)]
pub(crate) struct InstalledPluginSelfTest {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) kind: String,
    pub(crate) source: &'static str,
    pub(crate) protocols: Vec<String>,
    pub(crate) agent_tools: Vec<String>,
    pub(crate) providers: Vec<String>,
    pub(crate) capabilities: Vec<String>,
    pub(crate) loadable: bool,
}

pub(crate) fn self_test_scratch_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    std::env::temp_dir().join(format!(
        "token-station-installed-self-test-{}-{nonce}",
        std::process::id()
    ))
}

pub(crate) fn collect_installed_self_test() -> Result<Value, String> {
    let scratch = self_test_scratch_dir();
    let data_dir = scratch.join("data");
    let permission_probe = data_dir.join("permission-probe");
    let missing_plugins = scratch.join("intentionally-missing-plugins");
    let result = (|| {
        token_station_private_fs::ensure_private_dir(&data_dir)
            .map_err(|error| format!("private data directory: {error}"))?;
        token_station_private_fs::verify_private_dir(&data_dir)
            .map_err(|error| format!("private data directory verification: {error}"))?;
        token_station_private_fs::write_atomic_private(&permission_probe, b"permission-probe")
            .map_err(|error| format!("private file: {error}"))?;
        token_station_private_fs::verify_private_file(&permission_probe)
            .map_err(|error| format!("private file verification: {error}"))?;

        let mut draft = template(&data_dir, &missing_plugins);
        draft["upstreams"]["installed_self_test"] = json!({
            "provider": "openai-compatible",
            "base_url": "http://127.0.0.1:1/v1",
            "models": [{
                "model": "installed-self-test",
                "tool": true,
                "vision": false,
                "json_schema": true,
                "tool_state": "declared",
                "vision_state": "unsupported",
                "json_schema_state": "declared",
                "context_window": 8192
            }]
        });
        draft["router"]["pools"]["installed_self_test"] = json!([{
            "upstream": "installed_self_test",
            "model": "installed-self-test"
        }]);
        draft["router"]["default_pool"] = json!("installed_self_test");
        draft["routing"]["direct_target"] = json!({
            "upstream": "installed_self_test",
            "model": "installed-self-test"
        });
        let config: ClientConfig = serde_json::from_value(draft)
            .map_err(|error| format!("self-test configuration: {error}"))?;
        config
            .validate()
            .map_err(|error| format!("self-test configuration: {error}"))?;

        let registry = PluginRegistry::for_config(&config)
            .map_err(|error| format!("builtin plugin registry: {error}"))?;
        let mut plugins =
            Vec::with_capacity(token_station_cli::plugins::OFFICIAL_PACKAGE_IDS.len());
        for &id in token_station_cli::plugins::OFFICIAL_PACKAGE_IDS {
            let package = registry
                .package(id)
                .ok_or_else(|| format!("builtin plugin `{id}` is missing"))?;
            if !matches!(
                package.source,
                token_station_cli::plugins::PackageSource::Builtin { .. }
            ) {
                return Err(format!(
                    "plugin `{id}` did not come from the signed builtin tier"
                ));
            }
            fn capability_names<T: serde::Serialize>(capabilities: &T) -> Vec<String> {
                serde_json::to_value(capabilities)
                    .ok()
                    .and_then(|value| value.as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .collect()
            }
            let (kind, protocols, agent_tools, providers, capabilities) = match &package.manifest {
                PackageManifest::Agent(agent) => (
                    "agent-adapter",
                    agent.agent_protocols.clone(),
                    agent.agent_tools.clone(),
                    Vec::new(),
                    capability_names(&agent.capabilities),
                ),
                PackageManifest::Provider(component) => (
                    "provider-component",
                    Vec::new(),
                    Vec::new(),
                    component.providers.clone(),
                    capability_names(&component.capabilities),
                ),
            };
            plugins.push(InstalledPluginSelfTest {
                id: package.manifest.name().to_owned(),
                version: package.manifest.version().to_owned(),
                kind: kind.to_owned(),
                source: "builtin",
                protocols,
                agent_tools,
                providers,
                capabilities,
                loadable: true,
            });
        }
        for dialect in ["openai-compatible", "azure-openai-v1", "anthropic"] {
            if registry.provider_binding(dialect).is_none() {
                return Err(format!("builtin provider dialect `{dialect}` is not bound"));
            }
        }

        let gateway = Gateway::new(&config, Arc::new(token_station_metrics::NoopRecorder))
            .map_err(|error| format!("gateway plugin load: {error}"))?;
        if !gateway.skipped_agents().is_empty() {
            let skipped = gateway
                .skipped_agents()
                .iter()
                .map(|(package, error)| format!("{package}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "one or more builtin agent plugins failed to load: {skipped}"
            ));
        }

        Ok(json!({
            "passed": true,
            "bundle": {
                "id": "com.tokenstation.desktop",
                "desktop_version": env!("CARGO_PKG_VERSION"),
                "core_version": upgrade::CURRENT_VERSION,
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH
            },
            "storage": {
                "isolated": true,
                "data_directory_private": true,
                "private_file_verified": true,
                "credential_read": false
            },
            "plugins": plugins,
            "gateway": {
                "loadable": true,
                "skipped_agents": [],
                "catalog_size": gateway.catalog_size(),
                "provider_dialects": registry.provider_dialects()
            }
        }))
    })();
    std::fs::remove_dir_all(&scratch).ok();
    result
}

/// Runs the final desktop executable's read-only, credential-free artifact
/// self-test and writes a private JSON report for release automation.
///
/// # Errors
///
/// Returns a closed failure reason when storage protection, builtin plugin
/// identity, WASM loading, or the complete gateway composition fails. The
/// report is still written with `passed: false` whenever the output path itself
/// is writable.
pub fn run_installed_self_test(output: &std::path::Path) -> Result<(), String> {
    let collected = collect_installed_self_test();
    let report = match &collected {
        Ok(report) => report.clone(),
        Err(error) => json!({
            "passed": false,
            "bundle": {
                "id": "com.tokenstation.desktop",
                "desktop_version": env!("CARGO_PKG_VERSION"),
                "core_version": upgrade::CURRENT_VERSION,
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH
            },
            "error": error
        }),
    };
    let mut rendered = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("self-test report serialization: {error}"))?;
    rendered.push(b'\n');
    token_station_private_fs::write_atomic_private(output, &rendered)
        .map_err(|error| format!("self-test report `{}`: {error}", output.display()))?;
    collected.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "bundled-plugins")]
    fn installed_artifact_self_test_exercises_storage_plugins_and_gateway_composition() {
        let report = collect_installed_self_test().expect("installed artifact self-test passes");
        assert_eq!(report["passed"], json!(true));
        assert_eq!(report["bundle"]["id"], json!("com.tokenstation.desktop"));
        assert_eq!(
            report["plugins"].as_array().map(Vec::len),
            Some(token_station_cli::plugins::OFFICIAL_PACKAGE_IDS.len())
        );
        assert!(
            report["gateway"]["catalog_size"]
                .as_u64()
                .is_some_and(|size| size > 0),
            "{report}"
        );
        assert!(
            report["gateway"]["provider_dialects"]
                .as_array()
                .is_some_and(|dialects| dialects.iter().any(|value| value == "anthropic")),
            "{report}"
        );

        let scratch = self_test_scratch_dir();
        let output = scratch.join("installed-self-test.json");
        run_installed_self_test(&output).expect("writes the release automation report");
        let persisted: Value = serde_json::from_slice(
            &std::fs::read(&output).expect("reads the persisted self-test report"),
        )
        .expect("persisted report is JSON");
        assert_eq!(persisted["passed"], json!(true));
        token_station_private_fs::verify_private_file(&output)
            .expect("persisted report remains private");
        std::fs::remove_dir_all(scratch).ok();
    }

    #[test]
    #[cfg(not(feature = "bundled-plugins"))]
    fn source_build_self_test_fails_closed_and_still_writes_a_private_report() {
        let scratch = self_test_scratch_dir();
        let output = scratch.join("source-self-test.json");
        let error = run_installed_self_test(&output).expect_err("source build has no builtins");
        assert!(error.contains("builtin plugin"), "{error}");
        let persisted: Value = serde_json::from_slice(
            &std::fs::read(&output).expect("reads the persisted failure report"),
        )
        .expect("persisted report is JSON");
        assert_eq!(persisted["passed"], json!(false));
        assert!(
            persisted["error"]
                .as_str()
                .is_some_and(|message| message.contains("builtin plugin")),
            "{persisted}"
        );
        token_station_private_fs::verify_private_file(&output)
            .expect("persisted failure report remains private");
        std::fs::remove_dir_all(scratch).ok();
    }
}
