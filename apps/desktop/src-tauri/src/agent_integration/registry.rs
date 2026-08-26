use std::collections::{BTreeSet, HashSet};

use serde::Deserialize;

use super::connectors::find_connector;
use super::types::{
    AdmissionStatus, AgentConnectorCapabilityView, AgentDescriptor, AgentUiMetadata, ConfigFormat,
    EnvValueKind, ProbeRuntime, RuntimeResolutionSource, VersionOutputMatcher,
};

pub const BUILTIN_REGISTRY_JSON: &str = include_str!("../../agent-registry/builtin-agents.json");

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryDocument {
    schema_version: u32,
    agents: Vec<AgentDescriptor>,
}

const REGISTRY_SCHEMA_VERSION: u32 = 1;
const DESCRIPTOR_SCHEMA_VERSION: u32 = 1;
const MAX_AGENTS: usize = 64;
const MAX_TIMEOUT_MS: u64 = 5_000;
const MAX_OUTPUT_BYTES: usize = 65_536;
const ALLOWED_PATH_VARIABLES: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "LOCALAPPDATA",
    "APPDATA",
    "USERPROFILE",
];
const ALLOWED_CONFIG_ENV: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GROK_HOME",
    "KIMI_CODE_HOME",
    "DSH_HOME",
    "OPENCODE_CONFIG",
    "OPENCODE_CONFIG_DIR",
    "OPENCLAW_CONFIG_PATH",
    "OPENCLAW_STATE_DIR",
    "OPENCLAW_HOME",
    "HERMES_HOME",
];
const ALLOWED_FINGERPRINT_RULES: &[&str] = &[
    "json-shape-v1",
    "toml-shape-v1",
    "yaml-shape-v1",
    "dotenv-shape-v1",
];

#[derive(Clone, Debug)]
pub struct AgentRegistry {
    descriptors: Vec<AgentDescriptor>,
}

impl AgentRegistry {
    pub fn from_json(input: &str) -> Result<Self, String> {
        let mut document: RegistryDocument = serde_json::from_str(input)
            .map_err(|error| format!("invalid Agent Registry JSON: {error}"))?;
        if document.schema_version != REGISTRY_SCHEMA_VERSION {
            return Err(format!(
                "unsupported Registry schema_version {}; expected {REGISTRY_SCHEMA_VERSION}",
                document.schema_version
            ));
        }
        if document.agents.is_empty() || document.agents.len() > MAX_AGENTS {
            return Err(format!(
                "Registry agents must contain 1..={MAX_AGENTS} descriptors"
            ));
        }

        let mut agent_ids = HashSet::new();
        let mut legacy_kinds = HashSet::new();
        let mut ui_orders = HashSet::new();
        for descriptor in &document.agents {
            validate_identifier("agent_id", &descriptor.agent_id)?;
            if !agent_ids.insert(descriptor.agent_id.as_str()) {
                return Err(format!(
                    "duplicate agent_id '{}' in Registry",
                    descriptor.agent_id
                ));
            }
            if let Some(kind) = &descriptor.legacy_kind {
                validate_identifier("legacy_kind", kind)?;
                if !legacy_kinds.insert(kind.as_str()) {
                    return Err(format!("duplicate legacy_kind '{kind}' in Registry"));
                }
            }
            if !ui_orders.insert(descriptor.ui_order) {
                return Err(format!(
                    "duplicate ui_order '{}' in Registry",
                    descriptor.ui_order
                ));
            }
        }
        for descriptor in &document.agents {
            validate_descriptor(descriptor)?;
        }

        document.agents.sort_by(|left, right| {
            left.ui_order
                .cmp(&right.ui_order)
                .then_with(|| left.agent_id.cmp(&right.agent_id))
        });
        Ok(Self {
            descriptors: document.agents,
        })
    }

    pub fn builtin() -> Result<Self, String> {
        Self::from_json(BUILTIN_REGISTRY_JSON)
    }

    #[must_use]
    pub fn descriptors(&self) -> &[AgentDescriptor] {
        &self.descriptors
    }

    #[must_use]
    pub fn ui_metadata(&self) -> Vec<AgentUiMetadata> {
        self.descriptors
            .iter()
            .map(|descriptor| {
                let mut metadata = AgentUiMetadata::from(descriptor);
                metadata.connector_capabilities = descriptor
                    .local_connector_ids
                    .iter()
                    .filter_map(|connector_id| find_connector(connector_id))
                    .map(|connector| {
                        let capability = connector.capabilities();
                        AgentConnectorCapabilityView {
                            connector_id: capability.connector_id.to_string(),
                            adapter_id: capability.adapter_id.to_string(),
                            base_url_shape: capability.base_url_shape,
                            platforms: capability.platforms.to_vec(),
                            config_format: document_format_name(capability.config_format)
                                .to_string(),
                            config_path_template: capability.config_path_template.to_string(),
                            owned_fields: capability
                                .owned_fields
                                .iter()
                                .map(|field| (*field).to_string())
                                .collect(),
                            requires_virtual_key: capability.requires_virtual_key,
                            restart_required: capability.restart_required,
                        }
                    })
                    .collect();
                metadata
            })
            .collect()
    }
}

fn document_format_name(format: super::config_codec::DocumentFormat) -> &'static str {
    match format {
        super::config_codec::DocumentFormat::Json => "json",
        super::config_codec::DocumentFormat::Json5 => "json5",
        super::config_codec::DocumentFormat::Toml => "toml",
        super::config_codec::DocumentFormat::Yaml => "yaml",
        super::config_codec::DocumentFormat::Dotenv => "dotenv",
    }
}

fn validate_descriptor(descriptor: &AgentDescriptor) -> Result<(), String> {
    if descriptor.schema_version != DESCRIPTOR_SCHEMA_VERSION {
        return Err(format!(
            "{}: unsupported descriptor schema_version {}; expected {DESCRIPTOR_SCHEMA_VERSION}",
            descriptor.agent_id, descriptor.schema_version
        ));
    }
    validate_identifier("agent_id", &descriptor.agent_id)?;
    validate_text("display_name", &descriptor.display_name, 80)?;
    validate_identifier("icon_key", &descriptor.icon_key)?;
    validate_text("nav_mark", &descriptor.nav_mark, 4)?;

    if descriptor.executable_candidates.is_empty() || descriptor.executable_candidates.len() > 8 {
        return Err(format!(
            "{}: executable_candidates must contain 1..=8 names",
            descriptor.agent_id
        ));
    }
    ensure_unique(
        &descriptor.agent_id,
        "executable_candidates",
        &descriptor.executable_candidates,
    )?;
    for executable in &descriptor.executable_candidates {
        if executable.is_empty()
            || executable.len() > 80
            || !executable
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(format!(
                "{}: invalid executable candidate '{executable}'",
                descriptor.agent_id
            ));
        }
    }

    ensure_unique(
        &descriptor.agent_id,
        "discovery_fingerprint_rules",
        &descriptor.discovery_fingerprint_rules,
    )?;
    for rule in &descriptor.discovery_fingerprint_rules {
        if !ALLOWED_FINGERPRINT_RULES.contains(&rule.as_str()) {
            return Err(format!(
                "{}: discovery_fingerprint_rules contains an unsupported Task 4 scanner contract '{rule}'",
                descriptor.agent_id
            ));
        }
    }
    for format in descriptor
        .config_locations
        .iter()
        .map(|location| location.format)
    {
        let required = match format {
            ConfigFormat::Json | ConfigFormat::Jsonc | ConfigFormat::Json5 => "json-shape-v1",
            ConfigFormat::Toml => "toml-shape-v1",
            ConfigFormat::Yaml => "yaml-shape-v1",
            ConfigFormat::Dotenv => "dotenv-shape-v1",
        };
        if !descriptor
            .discovery_fingerprint_rules
            .iter()
            .any(|rule| rule == required)
        {
            return Err(format!(
                "{}: config format requires discovery fingerprint rule '{required}'",
                descriptor.agent_id
            ));
        }
    }

    validate_probe(descriptor)?;
    for paths in descriptor.known_install_locations.values() {
        ensure_unique(&descriptor.agent_id, "known install paths", paths)?;
        for path in paths {
            validate_path_template(&descriptor.agent_id, path)?;
        }
    }
    validate_config_locations(descriptor)?;

    match descriptor.admission {
        AdmissionStatus::DiscoveryOnly => {
            if descriptor.protocol_binding.is_some() || !descriptor.local_connector_ids.is_empty() {
                return Err(format!(
                    "{}: discovery_only descriptors cannot bind an Adapter or Connector",
                    descriptor.agent_id
                ));
            }
        }
        AdmissionStatus::Supported => validate_supported_capabilities(descriptor)?,
    }

    Ok(())
}

fn validate_probe(descriptor: &AgentDescriptor) -> Result<(), String> {
    let probe = &descriptor.version_probe;
    let passive_file = matches!(probe.runtime, Some(ProbeRuntime::PassiveFile));
    if passive_file {
        if !probe.argv.is_empty() || probe.output_matcher != VersionOutputMatcher::SuccessOnly {
            return Err(format!(
                "{}: passive_file version probe requires empty argv and SUCCESS_ONLY output",
                descriptor.agent_id
            ));
        }
    } else if probe.argv.is_empty() || probe.argv.len() > 8 {
        return Err(format!(
            "{}: version_probe.argv must contain 1..=8 arguments",
            descriptor.agent_id
        ));
    }
    for argument in &probe.argv {
        if argument.is_empty()
            || argument.len() > 128
            || argument.trim() != argument
            || argument.chars().any(char::is_whitespace)
        {
            return Err(format!(
                "{}: version_probe.argv contains an invalid argument",
                descriptor.agent_id
            ));
        }
        if argument.contains("$(")
            || argument
                .chars()
                .any(|character| matches!(character, ';' | '|' | '&' | '`' | '>' | '<' | '$'))
        {
            return Err(format!(
                "{}: version_probe.argv contains forbidden shell syntax",
                descriptor.agent_id
            ));
        }
    }
    if probe.timeout_ms == 0 || probe.timeout_ms > MAX_TIMEOUT_MS {
        return Err(format!(
            "{}: version_probe.timeout_ms must be 1..={MAX_TIMEOUT_MS}",
            descriptor.agent_id
        ));
    }
    if probe.max_output_bytes == 0 || probe.max_output_bytes > MAX_OUTPUT_BYTES {
        return Err(format!(
            "{}: version_probe.max_output_bytes must be 1..={MAX_OUTPUT_BYTES}",
            descriptor.agent_id
        ));
    }
    if let Some(runtime) = &probe.runtime {
        match runtime {
            ProbeRuntime::Direct | ProbeRuntime::PassiveFile => {}
            ProbeRuntime::EnvShebang {
                interpreter_candidates,
                resolution_sources,
                known_install_locations,
            }
            | ProbeRuntime::NodePackage {
                interpreter_candidates,
                resolution_sources,
                known_install_locations,
            } => {
                if interpreter_candidates.is_empty() || interpreter_candidates.len() > 8 {
                    return Err(format!(
                        "{}: interpreter_candidates must contain 1..=8 names",
                        descriptor.agent_id
                    ));
                }
                ensure_unique(
                    &descriptor.agent_id,
                    "interpreter_candidates",
                    interpreter_candidates,
                )?;
                for candidate in interpreter_candidates {
                    validate_executable_name(
                        &descriptor.agent_id,
                        "interpreter candidate",
                        candidate,
                    )?;
                }
                if resolution_sources.is_empty() || resolution_sources.len() > 3 {
                    return Err(format!(
                        "{}: runtime resolution_sources must contain 1..=3 values",
                        descriptor.agent_id
                    ));
                }
                let unique_sources = BTreeSet::from_iter(resolution_sources.iter().copied());
                if unique_sources.len() != resolution_sources.len() {
                    return Err(format!(
                        "{}: duplicate runtime resolution_sources",
                        descriptor.agent_id
                    ));
                }
                let uses_known_locations =
                    resolution_sources.contains(&RuntimeResolutionSource::KnownInstallLocations);
                let has_known_locations = known_install_locations
                    .values()
                    .any(|paths| !paths.is_empty());
                if uses_known_locations != has_known_locations {
                    return Err(format!(
                        "{}: known_install_locations must match runtime resolution_sources",
                        descriptor.agent_id
                    ));
                }
                for paths in known_install_locations.values() {
                    ensure_unique(&descriptor.agent_id, "runtime known install paths", paths)?;
                    for path in paths {
                        validate_path_template(&descriptor.agent_id, path)?;
                        let file_name = path
                            .replace('\\', "/")
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_string();
                        if !interpreter_candidates.contains(&file_name) {
                            return Err(format!(
                                "{}: runtime known install path must end in an interpreter candidate",
                                descriptor.agent_id
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn validate_executable_name(agent_id: &str, label: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(format!("{agent_id}: invalid {label} '{value}'"));
    }
    Ok(())
}

fn validate_config_locations(descriptor: &AgentDescriptor) -> Result<(), String> {
    let mut environment_names = BTreeSet::new();
    for location in &descriptor.config_locations {
        if let Some(environment) = &location.env_override {
            if !ALLOWED_CONFIG_ENV.contains(&environment.name.as_str()) {
                return Err(format!(
                    "{}: config env override '{}' is not allowlisted",
                    descriptor.agent_id, environment.name
                ));
            }
            if !environment_names.insert(environment.name.as_str()) {
                return Err(format!(
                    "{}: duplicate config env override '{}'",
                    descriptor.agent_id, environment.name
                ));
            }
            match environment.value_kind {
                EnvValueKind::File if environment.suffix.is_some() => {
                    return Err(format!(
                        "{}: file env override '{}' cannot have a suffix",
                        descriptor.agent_id, environment.name
                    ));
                }
                EnvValueKind::Directory => {
                    let suffix = environment.suffix.as_deref().ok_or_else(|| {
                        format!(
                            "{}: directory env override '{}' requires a suffix",
                            descriptor.agent_id, environment.name
                        )
                    })?;
                    validate_relative_suffix(&descriptor.agent_id, suffix)?;
                }
                EnvValueKind::File => {}
            }
        }
        for paths in location.platform_defaults.values() {
            ensure_unique(&descriptor.agent_id, "config paths", paths)?;
            for path in paths {
                validate_path_template(&descriptor.agent_id, path)?;
            }
        }
        for (platform, paths) in &location.installation_path_defaults {
            if !location.platform_defaults.contains_key(platform) {
                return Err(format!(
                    "{}: installation-scoped config has no defaults for {platform:?}",
                    descriptor.agent_id
                ));
            }
            ensure_unique(&descriptor.agent_id, "config installation paths", paths)?;
            for path in paths {
                validate_path_template(&descriptor.agent_id, path)?;
                if !descriptor
                    .known_install_locations
                    .get(platform)
                    .is_some_and(|known| known.contains(path))
                {
                    return Err(format!(
                        "{}: config installation path '{path}' is not a known installation",
                        descriptor.agent_id
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_supported_capabilities(descriptor: &AgentDescriptor) -> Result<(), String> {
    let binding = descriptor.protocol_binding.as_ref().ok_or_else(|| {
        format!(
            "{}: supported descriptor requires protocol_binding",
            descriptor.agent_id
        )
    })?;
    if descriptor.local_connector_ids.is_empty() {
        return Err(format!(
            "{}: supported descriptor requires a local Connector",
            descriptor.agent_id
        ));
    }
    ensure_unique(
        &descriptor.agent_id,
        "local_connector_ids",
        &descriptor.local_connector_ids,
    )?;
    for connector_id in &descriptor.local_connector_ids {
        let connector = find_connector(connector_id).ok_or_else(|| {
            format!(
                "{}: Connector '{connector_id}' is not locally available",
                descriptor.agent_id
            )
        })?;
        let capability = connector.capabilities();
        if capability.agent_id != descriptor.agent_id
            || capability.adapter_id != binding.adapter_id
            || capability.base_url_shape != binding.base_url_shape
        {
            return Err(format!(
                "{}: Connector '{connector_id}' does not match this Agent/Adapter binding",
                descriptor.agent_id
            ));
        }
    }
    Ok(())
}

fn validate_identifier(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!(
            "invalid {label} '{value}'; expected lower-kebab ASCII"
        ));
    }
    Ok(())
}

fn validate_text(label: &str, value: &str, max_len: usize) -> Result<(), String> {
    if value.is_empty()
        || value.len() > max_len
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

fn validate_path_template(agent_id: &str, template: &str) -> Result<(), String> {
    if template.is_empty()
        || template.len() > 512
        || !template.is_ascii()
        || template.chars().any(char::is_control)
    {
        return Err(format!("{agent_id}: invalid path template"));
    }
    let normalized = template.replace('\\', "/");
    if normalized.starts_with("//") {
        return Err(format!("{agent_id}: UNC path templates are not allowed"));
    }
    if normalized
        .split('/')
        .any(|component| matches!(component, "." | ".."))
    {
        return Err(format!("{agent_id}: path traversal is not allowed"));
    }

    let mut remainder = template;
    let mut variable_count = 0;
    while let Some(position) = remainder.find('$') {
        let candidate = &remainder[position..];
        if !candidate.starts_with("${") {
            return Err(format!("{agent_id}: malformed path template variable"));
        }
        let close = candidate
            .find('}')
            .ok_or_else(|| format!("{agent_id}: unterminated path template variable"))?;
        let variable = &candidate[2..close];
        if !ALLOWED_PATH_VARIABLES.contains(&variable) {
            return Err(format!(
                "{agent_id}: path variable '{variable}' is not allowlisted"
            ));
        }
        variable_count += 1;
        if position != 0 || variable_count > 1 {
            return Err(format!(
                "{agent_id}: a path variable is allowed only once at the template root"
            ));
        }
        let following = &candidate[close + 1..];
        if !following.is_empty() && !following.starts_with(['/', '\\']) {
            return Err(format!(
                "{agent_id}: path variable must end the root segment"
            ));
        }
        remainder = &candidate[close + 1..];
    }

    let anchored = normalized.starts_with('/')
        || normalized.starts_with("${")
        || (normalized.len() >= 3
            && normalized.as_bytes()[0].is_ascii_alphabetic()
            && normalized.as_bytes()[1] == b':'
            && normalized.as_bytes()[2] == b'/');
    if !anchored {
        return Err(format!(
            "{agent_id}: path template must be absolute or anchored"
        ));
    }
    Ok(())
}

fn validate_relative_suffix(agent_id: &str, suffix: &str) -> Result<(), String> {
    if suffix.is_empty()
        || suffix.len() > 200
        || !suffix.is_ascii()
        || suffix.starts_with(['/', '\\'])
        || suffix.contains(['$', ':'])
        || suffix
            .split(['/', '\\'])
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return Err(format!("{agent_id}: invalid config path suffix '{suffix}'"));
    }
    Ok(())
}

fn ensure_unique<T>(agent_id: &str, label: &str, values: &[T]) -> Result<(), String>
where
    T: AsRef<str>,
{
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value.as_ref()) {
            return Err(format!(
                "{agent_id}: duplicate value '{}' in {label}",
                value.as_ref()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(BUILTIN_REGISTRY_JSON).unwrap()
    }

    fn validate(value: &Value) -> Result<AgentRegistry, String> {
        AgentRegistry::from_json(&serde_json::to_string(value).unwrap())
    }

    #[test]
    fn duplicate_agent_ids_are_rejected() {
        let mut value = fixture();
        value["agents"][1]["agent_id"] = value["agents"][0]["agent_id"].clone();

        let error = validate(&value).unwrap_err();

        assert!(error.contains("duplicate agent_id"), "{error}");
    }

    #[test]
    fn unknown_registry_and_descriptor_schema_versions_are_rejected() {
        let mut registry = fixture();
        registry["schema_version"] = json!(2);
        assert!(validate(&registry).unwrap_err().contains("schema_version"));

        let mut descriptor = fixture();
        descriptor["agents"][0]["schema_version"] = json!(2);
        assert!(validate(&descriptor)
            .unwrap_err()
            .contains("schema_version"));
    }

    #[test]
    fn mismatched_adapter_and_unknown_connector_references_are_rejected() {
        let mut adapter = fixture();
        adapter["agents"][0]["protocol_binding"]["adapter_id"] = json!("missing-adapter");
        assert!(validate(&adapter)
            .unwrap_err()
            .contains("does not match this Agent/Adapter binding"));

        let mut connector = fixture();
        connector["agents"][0]["local_connector_ids"] = json!(["missing-connector"]);
        assert!(validate(&connector)
            .unwrap_err()
            .contains("missing-connector"));
    }

    #[test]
    fn version_probe_must_be_nonempty_argv_without_shell_syntax() {
        let mut empty = fixture();
        empty["agents"][0]["version_probe"]["argv"] = json!([]);
        assert!(validate(&empty).unwrap_err().contains("version_probe.argv"));

        let mut shell = fixture();
        shell["agents"][0]["version_probe"]["argv"] =
            json!(["${EXECUTABLE}", "--version; curl example.invalid"]);
        assert!(validate(&shell).unwrap_err().contains("shell syntax"));

        let mut joined_arguments = fixture();
        joined_arguments["agents"][0]["version_probe"]["argv"] = json!(["--version --json"]);
        assert!(validate(&joined_arguments)
            .unwrap_err()
            .contains("invalid argument"));
    }

    #[test]
    fn version_probe_runtime_is_declarative_and_fail_closed() {
        let runtime = json!({
            "kind": "env_shebang",
            "interpreter_candidates": ["node"],
            "resolution_sources": ["observed_entry_sibling", "known_install_locations"],
            "known_install_locations": {
                "macos": ["${HOME}/.local/bin/node"],
                "linux": ["${HOME}/.local/bin/node"],
                "wsl": ["${HOME}/.local/bin/node"]
            }
        });

        let mut valid = fixture();
        valid["agents"][3]["version_probe"]["runtime"] = runtime.clone();
        assert!(validate(&valid).is_ok());

        let mut shell_candidate = fixture();
        shell_candidate["agents"][3]["version_probe"]["runtime"] = runtime.clone();
        shell_candidate["agents"][3]["version_probe"]["runtime"]["interpreter_candidates"] =
            json!(["../node"]);
        assert!(validate(&shell_candidate)
            .unwrap_err()
            .contains("interpreter candidate"));

        let mut traversal = fixture();
        traversal["agents"][3]["version_probe"]["runtime"] = runtime.clone();
        traversal["agents"][3]["version_probe"]["runtime"]["known_install_locations"]["macos"] =
            json!(["${HOME}/../node"]);
        assert!(validate(&traversal).unwrap_err().contains("traversal"));

        let mut duplicate_source = fixture();
        duplicate_source["agents"][3]["version_probe"]["runtime"] = runtime;
        duplicate_source["agents"][3]["version_probe"]["runtime"]["resolution_sources"] =
            json!(["observed_entry_sibling", "observed_entry_sibling"]);
        assert!(validate(&duplicate_source)
            .unwrap_err()
            .contains("resolution_sources"));
    }

    #[test]
    fn passive_file_probe_requires_empty_argv_and_cursor_uses_it() {
        let mut passive = fixture();
        passive["agents"][0]["version_probe"]["runtime"] = json!({ "kind": "passive_file" });
        assert!(validate(&passive).unwrap_err().contains("passive_file"));

        passive["agents"][0]["version_probe"]["argv"] = json!([]);
        passive["agents"][0]["version_probe"]["output_matcher"] = json!("SUCCESS_ONLY");
        assert!(validate(&passive).is_ok());

        let cursor = AgentRegistry::builtin()
            .unwrap()
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "cursor")
            .unwrap()
            .clone();
        assert!(cursor.version_probe.argv.is_empty());
        assert_eq!(
            cursor.version_probe.output_matcher,
            super::super::types::VersionOutputMatcher::SuccessOnly
        );
        assert!(matches!(
            cursor.version_probe.runtime,
            Some(ProbeRuntime::PassiveFile)
        ));
    }

    #[test]
    fn cursor_dedicated_setup_is_macos_only_until_a_native_backend_exists() {
        use super::super::types::Platform;

        let registry = AgentRegistry::builtin().unwrap();
        let cursor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "cursor")
            .unwrap();

        assert!(cursor
            .known_install_locations
            .contains_key(&Platform::Macos));
        assert!(!cursor
            .known_install_locations
            .contains_key(&Platform::Windows));
    }

    #[test]
    fn kimi_windows_npm_install_uses_the_validated_node_package_probe() {
        let registry = AgentRegistry::builtin().unwrap();
        let kimi = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "kimi-code")
            .unwrap();

        assert!(matches!(
            kimi.version_probe.runtime,
            Some(ProbeRuntime::NodePackage { .. })
        ));
    }

    #[test]
    fn linux_node_package_agents_cover_common_user_managers() {
        use super::super::types::Platform;

        let registry = AgentRegistry::builtin().unwrap();
        for descriptor in registry.descriptors().iter().filter(|descriptor| {
            matches!(
                descriptor.version_probe.runtime,
                Some(ProbeRuntime::NodePackage { .. })
            )
        }) {
            let executable = &descriptor.executable_candidates[0];
            for platform in [Platform::Linux, Platform::Wsl] {
                let locations = &descriptor.known_install_locations[&platform];
                for expected in [
                    format!("${{HOME}}/.npm-global/bin/{executable}"),
                    format!("${{HOME}}/.local/share/pnpm/{executable}"),
                    format!("${{HOME}}/.bun/bin/{executable}"),
                    format!("${{HOME}}/.volta/bin/{executable}"),
                ] {
                    assert!(
                        locations.contains(&expected),
                        "{} is missing {expected} on {platform:?}",
                        descriptor.agent_id
                    );
                }
            }
        }
    }

    #[test]
    fn path_templates_reject_traversal_and_unknown_variables() {
        let mut traversal = fixture();
        traversal["agents"][0]["known_install_locations"]["macos"] =
            json!(["${HOME}/../other/claude"]);
        assert!(validate(&traversal).unwrap_err().contains("traversal"));

        let mut variable = fixture();
        variable["agents"][0]["known_install_locations"]["macos"] =
            json!(["${UNTRUSTED_ROOT}/claude"]);
        assert!(validate(&variable).unwrap_err().contains("UNTRUSTED_ROOT"));
    }

    #[test]
    fn unknown_fields_and_probe_resource_overrides_fail_closed() {
        let mut unknown = fixture();
        unknown["agents"][0]["install_script"] = json!("curl example.invalid | sh");
        assert!(validate(&unknown).unwrap_err().contains("install_script"));

        let mut timeout = fixture();
        timeout["agents"][0]["version_probe"]["timeout_ms"] = json!(0);
        assert!(validate(&timeout).unwrap_err().contains("timeout_ms"));

        let mut output = fixture();
        output["agents"][0]["version_probe"]["max_output_bytes"] = json!(1048577);
        assert!(validate(&output).unwrap_err().contains("max_output_bytes"));

        let mut fingerprint = fixture();
        fingerprint["agents"][0]["discovery_fingerprint_rules"] = json!(["script:any"]);
        assert!(validate(&fingerprint)
            .unwrap_err()
            .contains("Task 4 scanner contract"));
    }

    #[test]
    fn admission_is_an_explicit_write_capability_boundary() {
        let mut discovery_only = fixture();
        discovery_only["agents"][2]["admission"] = json!("discovery_only");
        assert!(validate(&discovery_only)
            .unwrap_err()
            .contains("discovery_only"));

        let mut supported = fixture();
        supported["agents"][0]["protocol_binding"] = Value::Null;
        assert!(validate(&supported).unwrap_err().contains("supported"));
    }

    #[test]
    fn builtin_registry_is_stably_ui_ordered_and_exposes_ui_metadata() {
        let registry = AgentRegistry::builtin().unwrap();
        let ids: Vec<_> = registry
            .descriptors()
            .iter()
            .map(|descriptor| descriptor.agent_id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "claude-code",
                "claude-desktop",
                "codex",
                "gemini-cli",
                "grok-build",
                "kimi-code",
                "deepseek-harness",
                "opencode",
                "cursor",
                "openclaw",
                "workbuddy",
                "nous-hermes-agent",
            ]
        );

        let labels: Vec<_> = registry
            .ui_metadata()
            .into_iter()
            .map(|metadata| (metadata.agent_id, metadata.display_name))
            .collect();
        assert_eq!(labels.len(), 12);
        assert_eq!(labels[0].1, "Claude Code");
        assert_eq!(labels[4].1, "Grok Build");
        assert_eq!(labels[5].1, "Kimi Code");
        assert_eq!(labels[6].1, "DeepSeek Harness");
        assert_eq!(labels[10].1, "WorkBuddy");
        assert_eq!(labels[11].1, "Hermes Agent");

        let claude_desktop = &registry.descriptors()[1];
        assert!(claude_desktop.version_probe.argv.is_empty());
        assert_eq!(
            claude_desktop.version_probe.output_matcher,
            super::super::types::VersionOutputMatcher::SuccessOnly
        );
        assert!(matches!(
            claude_desktop.version_probe.runtime,
            Some(ProbeRuntime::PassiveFile)
        ));

        let gemini = &registry.descriptors()[3];
        assert_eq!(gemini.agent_id, "gemini-cli");
        assert_eq!(gemini.local_connector_ids, ["gemini-cli-v1"]);
        assert_eq!(
            registry.ui_metadata()[3].connector_capabilities[0].config_format,
            "dotenv"
        );
        let hermes = &registry.descriptors()[11];
        assert_eq!(hermes.admission, AdmissionStatus::Supported);
        assert_eq!(hermes.local_connector_ids, ["hermes-v1"]);
        assert_eq!(hermes.version_probe.argv, ["--help"]);
        assert_eq!(
            hermes.version_probe.output_matcher,
            super::super::types::VersionOutputMatcher::SuccessOnly
        );
        assert!(!hermes.version_probe.retry_on_timeout);
        let openclaw = &registry.descriptors()[9];
        assert_eq!(openclaw.admission, AdmissionStatus::Supported);
        assert_eq!(openclaw.local_connector_ids, ["openclaw-v1"]);
        assert!(matches!(
            openclaw.version_probe.runtime,
            Some(ProbeRuntime::NodePackage { .. })
        ));
        // Cover both Unix `env node` and the Windows npm shim for Gemini CLI.
        assert!(matches!(
            registry.descriptors()[3].version_probe.runtime,
            Some(ProbeRuntime::NodePackage { .. })
        ));
        assert!(registry
            .descriptors()
            .iter()
            .filter(|descriptor| matches!(
                descriptor.agent_id.as_str(),
                "claude-code"
                    | "codex"
                    | "gemini-cli"
                    | "deepseek-harness"
                    | "opencode"
                    | "openclaw"
            ))
            .all(|descriptor| matches!(
                descriptor.version_probe.runtime,
                Some(ProbeRuntime::NodePackage { .. })
            )));
        assert!(registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "workbuddy")
            .is_some_and(|descriptor| matches!(
                descriptor.version_probe.runtime,
                Some(ProbeRuntime::EnvShebang { .. })
            )));
        let workbuddy = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "workbuddy")
            .unwrap();
        assert_eq!(
            workbuddy.known_install_locations[&super::super::types::Platform::Macos],
            [
                "/Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy",
                "/Applications/WorkBuddy AI.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy",
            ]
        );
        assert_eq!(workbuddy.config_locations.len(), 2);
        assert_eq!(
            workbuddy.config_locations[1].installation_path_defaults
                [&super::super::types::Platform::Macos],
            [
                "/Applications/WorkBuddy AI.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy"
            ]
        );
        assert!(registry
            .descriptors()
            .iter()
            .all(|descriptor| !descriptor.version_probe.retry_on_timeout));
        assert_eq!(
            registry.descriptors()[0].known_install_locations
                [&super::super::types::Platform::Macos],
            [
                "${HOME}/.local/bin/claude",
                "${HOME}/.npm-global/bin/claude",
                "${HOME}/Library/pnpm/claude",
                "${HOME}/.local/share/pnpm/claude",
                "${HOME}/.bun/bin/claude",
                "${HOME}/.volta/bin/claude",
            ]
        );
        assert!(registry
            .descriptors()
            .iter()
            .all(|descriptor| { !descriptor.discovery_fingerprint_rules.is_empty() }));

        let ui_json = serde_json::to_value(registry.ui_metadata()).unwrap();
        assert_eq!(ui_json[0]["agent_id"], "claude-code");
        assert_eq!(ui_json[7]["icon_key"], "opencode");

        let mut shuffled = fixture();
        shuffled["agents"].as_array_mut().unwrap().reverse();
        let shuffled = validate(&shuffled).unwrap();
        assert_eq!(registry.descriptors(), shuffled.descriptors());
    }

    #[test]
    fn builtin_macos_registry_covers_common_user_cli_locations() {
        use super::super::types::Platform;

        let registry = AgentRegistry::builtin().unwrap();
        let expected = [
            ("claude-code", "${HOME}/.npm-global/bin/claude"),
            ("gemini-cli", "${HOME}/Library/pnpm/gemini"),
            ("opencode", "${HOME}/.opencode/bin/opencode"),
            ("openclaw", "${HOME}/.local/share/pnpm/openclaw"),
            ("nous-hermes-agent", "${HOME}/.local/bin/hermes"),
            ("grok-build", "${HOME}/.grok/bin/grok"),
            ("kimi-code", "${HOME}/.local/bin/kimi"),
            ("deepseek-harness", "${HOME}/.npm-global/bin/dsh"),
        ];

        for (agent_id, path) in expected {
            let descriptor = registry
                .descriptors()
                .iter()
                .find(|descriptor| descriptor.agent_id == agent_id)
                .unwrap();
            assert!(
                descriptor.known_install_locations[&Platform::Macos]
                    .iter()
                    .any(|candidate| candidate == path),
                "{agent_id} is missing {path}"
            );
        }
    }

    #[test]
    fn installation_scoped_config_must_name_a_known_installation_on_the_same_platform() {
        let mut unknown = fixture();
        let workbuddy = unknown["agents"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|agent| agent["agent_id"] == "workbuddy")
            .unwrap();
        workbuddy["config_locations"][1]["installation_path_defaults"]["macos"] =
            json!(["/Applications/Other.app/Contents/MacOS/other"]);
        assert!(validate(&unknown)
            .unwrap_err()
            .contains("is not a known installation"));

        let mut wrong_platform = fixture();
        let workbuddy = wrong_platform["agents"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|agent| agent["agent_id"] == "workbuddy")
            .unwrap();
        workbuddy["config_locations"][1]["installation_path_defaults"]["windows"] =
            json!([r"C:\\Program Files\\WorkBuddy AI\\codebuddy.exe"]);
        assert!(validate(&wrong_platform)
            .unwrap_err()
            .contains("has no defaults for Windows"));
    }

    #[test]
    fn every_cross_platform_agent_has_linux_install_and_config_locations() {
        use super::super::types::Platform;
        let registry = AgentRegistry::builtin().unwrap();
        for descriptor in registry.descriptors() {
            // Claude Desktop and the verified WorkBuddy 5.3.8 integration do not
            // claim a Linux installation until that product path is tested.
            if matches!(
                descriptor.agent_id.as_str(),
                "claude-desktop" | "workbuddy" | "cursor"
            ) {
                continue;
            }
            assert!(
                descriptor
                    .known_install_locations
                    .get(&Platform::Linux)
                    .is_some_and(|paths| !paths.is_empty()),
                "{} has no Linux install locations",
                descriptor.agent_id
            );
            let has_linux_config = descriptor.config_locations.iter().any(|location| {
                location
                    .platform_defaults
                    .get(&Platform::Linux)
                    .is_some_and(|paths| !paths.is_empty())
            });
            assert!(
                has_linux_config,
                "{} has no Linux config path",
                descriptor.agent_id
            );
        }
    }

    #[test]
    fn every_supported_agent_has_windows_install_and_config_locations() {
        use super::super::types::Platform;
        let registry = AgentRegistry::builtin().unwrap();
        for descriptor in registry.descriptors() {
            // WorkBuddy is intentionally macOS-only until its Windows install and
            // config paths have been verified against a real installation.
            if matches!(descriptor.agent_id.as_str(), "workbuddy" | "cursor") {
                continue;
            }
            assert!(
                descriptor
                    .known_install_locations
                    .get(&Platform::Windows)
                    .is_some_and(|paths| !paths.is_empty()),
                "{} has no Windows install locations",
                descriptor.agent_id
            );
            let has_windows_config = descriptor.config_locations.iter().any(|location| {
                location
                    .platform_defaults
                    .get(&Platform::Windows)
                    .is_some_and(|paths| !paths.is_empty())
            });
            assert!(
                has_windows_config,
                "{} has no Windows config path",
                descriptor.agent_id
            );
        }
    }

    #[test]
    fn registry_boundary_matrix_rejects_ambiguous_metadata_paths_and_capabilities() {
        for path in [
            "",
            "//server/share/agent",
            "relative/agent",
            "$HOME/agent",
            "${HOME",
            "prefix/${HOME}/agent",
            "${HOME}suffix/agent",
            "${HOME}/${APPDATA}/agent",
        ] {
            assert!(validate_path_template("fixture", path).is_err(), "{path}");
        }
        for suffix in ["", "/absolute", "../escape", "a//b", "a:$HOME"] {
            assert!(
                validate_relative_suffix("fixture", suffix).is_err(),
                "{suffix}"
            );
        }
        assert!(validate_identifier("agent_id", "-bad").is_err());
        assert!(validate_text("display_name", " bad ", 80).is_err());
        assert!(ensure_unique("fixture", "values", &["same", "same"]).is_err());

        let mut empty = fixture();
        empty["agents"] = json!([]);
        assert!(validate(&empty).unwrap_err().contains("1..=64"));

        let mut legacy = fixture();
        legacy["agents"][1]["legacy_kind"] = legacy["agents"][0]["legacy_kind"].clone();
        assert!(validate(&legacy)
            .unwrap_err()
            .contains("duplicate legacy_kind"));

        let mut executable = fixture();
        executable["agents"][0]["executable_candidates"] = json!(["bad/name"]);
        assert!(validate(&executable)
            .unwrap_err()
            .contains("executable candidate"));

        let mut missing_shape = fixture();
        missing_shape["agents"][0]["discovery_fingerprint_rules"] = json!(["toml-shape-v1"]);
        assert!(validate(&missing_shape)
            .unwrap_err()
            .contains("json-shape-v1"));

        let mut connector_mismatch = fixture();
        connector_mismatch["agents"][0]["local_connector_ids"] = json!(["codex-v1"]);
        assert!(validate(&connector_mismatch)
            .unwrap_err()
            .contains("does not match"));
    }
}
