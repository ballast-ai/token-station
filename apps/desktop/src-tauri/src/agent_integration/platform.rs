use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use super::types::{AgentDescriptor, ConfigFormat, DiscoverySource, EnvValueKind, Platform};

const ROOT_VARIABLES: &[&str] = &[
    "HOME",
    "XDG_CONFIG_HOME",
    "LOCALAPPDATA",
    "APPDATA",
    "USERPROFILE",
];

#[derive(Clone)]
pub struct ScanEnvironment {
    pub platform: Platform,
    pub variables: BTreeMap<String, String>,
    pub path_entries: Vec<PathBuf>,
    pub present_environment: BTreeSet<String>,
    pub child_environment: BTreeMap<String, String>,
}

impl ScanEnvironment {
    #[must_use]
    pub fn from_process(registry: &[AgentDescriptor]) -> Self {
        let platform = current_platform();
        let mut allowed = BTreeSet::from_iter(ROOT_VARIABLES.iter().copied());
        for descriptor in registry {
            for location in &descriptor.config_locations {
                if let Some(environment) = &location.env_override {
                    allowed.insert(environment.name.as_str());
                }
            }
        }
        if registry
            .iter()
            .any(|descriptor| descriptor.agent_id == "opencode")
        {
            allowed.insert("OPENCODE_CONFIG_CONTENT");
        }

        let mut variables = BTreeMap::new();
        let mut present_environment = BTreeSet::new();
        for name in allowed {
            if let Some(value) = std::env::var_os(name) {
                present_environment.insert(name.to_string());
                if let Some(value) = value.to_str().filter(|value| !value.is_empty()) {
                    variables.insert(name.to_string(), value.to_string());
                }
            }
        }
        let path_entries: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|value| {
                std::env::split_paths(&value)
                    .filter(|entry| entry.is_absolute())
                    .collect()
            })
            .unwrap_or_default();
        let mut child_environment = BTreeMap::new();
        if let Ok(path) = std::env::join_paths(&path_entries) {
            if let Some(path) = path.to_str() {
                child_environment.insert("PATH".to_string(), path.to_string());
            }
        }
        for name in ["SYSTEMROOT", "WINDIR"] {
            if let Ok(value) = std::env::var(name) {
                child_environment.insert(name.to_string(), value);
            }
        }
        Self {
            platform,
            variables,
            path_entries,
            present_environment,
            child_environment,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ExecutableCandidate {
    pub path: PathBuf,
    pub source: DiscoverySource,
    pub path_order: Option<usize>,
}

#[derive(Clone)]
pub(crate) struct ResolvedConfigCandidate {
    pub path: PathBuf,
    pub format: ConfigFormat,
}

pub(crate) struct ConfigResolution {
    pub candidates: Vec<ResolvedConfigCandidate>,
    pub invalid_environment_names: Vec<String>,
    pub inline_config_present: bool,
}

pub(crate) fn executable_candidates(
    descriptor: &AgentDescriptor,
    environment: &ScanEnvironment,
) -> Vec<ExecutableCandidate> {
    let mut candidates = Vec::new();
    if let Some(templates) = descriptor
        .known_install_locations
        .get(&environment.platform)
    {
        for template in templates {
            if let Some(path) = expand_template(template, environment) {
                candidates.push(ExecutableCandidate {
                    path,
                    source: DiscoverySource::KnownPath,
                    path_order: None,
                });
            }
        }
    }

    for (path_order, directory) in environment.path_entries.iter().enumerate() {
        for executable in &descriptor.executable_candidates {
            let names = path_executable_names(executable, environment.platform);
            candidates.extend(names.into_iter().map(|name| ExecutableCandidate {
                path: directory.join(name),
                source: DiscoverySource::Path,
                path_order: Some(path_order),
            }));
        }
    }
    candidates
}

fn path_executable_names(name: &str, platform: Platform) -> Vec<String> {
    if platform == Platform::Windows && PathBuf::from(name).extension().is_none() {
        ["exe", "com", "cmd", "bat", "ps1"]
            .into_iter()
            .map(|extension| format!("{name}.{extension}"))
            .collect()
    } else {
        vec![name.to_string()]
    }
}

pub(crate) fn config_candidates(
    descriptor: &AgentDescriptor,
    environment: &ScanEnvironment,
) -> ConfigResolution {
    let mut candidates = Vec::new();
    let mut invalid_environment_names = Vec::new();
    for location in &descriptor.config_locations {
        if let Some(override_) = &location.env_override {
            if let Some(value) = environment.variables.get(&override_.name) {
                if is_absolute_for(environment.platform, value) {
                    let path = match override_.value_kind {
                        EnvValueKind::File => PathBuf::from(normalize_separators(value)),
                        EnvValueKind::Directory => join_lexically(
                            value,
                            override_
                                .suffix
                                .as_deref()
                                .expect("validated Registry suffix"),
                        ),
                    };
                    candidates.push(ResolvedConfigCandidate {
                        path,
                        format: location.format,
                    });
                } else {
                    invalid_environment_names.push(override_.name.clone());
                }
            } else if environment.present_environment.contains(&override_.name) {
                invalid_environment_names.push(override_.name.clone());
            }
        }

        if let Some(templates) = location.platform_defaults.get(&environment.platform) {
            for template in templates {
                if let Some(path) = expand_template(template, environment) {
                    candidates.push(ResolvedConfigCandidate {
                        path,
                        format: location.format,
                    });
                }
            }
        }
    }

    let mut seen = BTreeSet::new();
    candidates
        .retain(|candidate| seen.insert(path_identity(&candidate.path, environment.platform)));
    ConfigResolution {
        candidates,
        invalid_environment_names,
        inline_config_present: descriptor.agent_id == "opencode"
            && environment
                .present_environment
                .contains("OPENCODE_CONFIG_CONTENT"),
    }
}

pub(crate) fn expand_template(template: &str, environment: &ScanEnvironment) -> Option<PathBuf> {
    if let Some(rest) = template.strip_prefix("${") {
        let close = rest.find('}')?;
        let name = &rest[..close];
        let root = environment.variables.get(name)?;
        if !is_absolute_for(environment.platform, root) {
            return None;
        }
        let suffix = rest[close + 1..].trim_start_matches(['/', '\\']);
        Some(join_lexically(root, suffix))
    } else if is_absolute_for(environment.platform, template) {
        Some(PathBuf::from(normalize_separators(template)))
    } else {
        None
    }
}

pub(crate) fn path_identity(path: &std::path::Path, platform: Platform) -> String {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if platform == Platform::Windows {
        normalized
            .strip_prefix("//?/")
            .unwrap_or(&normalized)
            .to_ascii_lowercase()
    } else {
        normalized
    }
}

fn normalize_separators(value: &str) -> String {
    value.replace('\\', "/")
}

fn join_lexically(root: &str, suffix: &str) -> PathBuf {
    let root = normalize_separators(root);
    let suffix = normalize_separators(suffix);
    PathBuf::from(format!(
        "{}/{}",
        root.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    ))
}

fn is_absolute_for(platform: Platform, value: &str) -> bool {
    let normalized = normalize_separators(value);
    match platform {
        Platform::Windows => {
            let bytes = normalized.as_bytes();
            bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'/'
        }
        Platform::Macos | Platform::Linux | Platform::Wsl => normalized.starts_with('/'),
    }
}

#[must_use]
pub fn current_platform() -> Platform {
    #[cfg(target_os = "windows")]
    {
        Platform::Windows
    }
    #[cfg(target_os = "macos")]
    {
        Platform::Macos
    }
    #[cfg(target_os = "linux")]
    {
        let kernel_mentions_wsl = std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .or_else(|_| std::fs::read_to_string("/proc/version"))
            .is_ok_and(|text| text.to_ascii_lowercase().contains("microsoft"));
        if kernel_mentions_wsl {
            Platform::Wsl
        } else {
            Platform::Linux
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Platform::Linux
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(platform: Platform) -> ScanEnvironment {
        ScanEnvironment {
            platform,
            variables: BTreeMap::from([
                ("HOME".to_string(), "/Users/tester".to_string()),
                ("USERPROFILE".to_string(), "C:\\Users\\tester".to_string()),
                (
                    "LOCALAPPDATA".to_string(),
                    "C:\\Users\\tester\\AppData\\Local".to_string(),
                ),
            ]),
            path_entries: Vec::new(),
            present_environment: BTreeSet::new(),
            child_environment: BTreeMap::new(),
        }
    }

    #[test]
    fn discovery_platform_templates_expand_without_touching_the_filesystem() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/discovery/platform-paths.json"
        ))
        .unwrap();
        for case in fixture.as_array().unwrap() {
            let platform = match case["platform"].as_str().unwrap() {
                "macos" => Platform::Macos,
                "linux" => Platform::Linux,
                "windows" => Platform::Windows,
                "wsl" => Platform::Wsl,
                other => panic!("unexpected fixture platform {other}"),
            };
            let context = environment(platform);
            assert_eq!(
                expand_template(case["template"].as_str().unwrap(), &context)
                    .unwrap()
                    .to_string_lossy(),
                case["expected"].as_str().unwrap()
            );
        }
    }

    #[test]
    fn discovery_rejects_relative_environment_roots_and_relative_path_entries() {
        let mut context = environment(Platform::Linux);
        context
            .variables
            .insert("HOME".to_string(), "relative/home".to_string());
        context.path_entries = vec![PathBuf::from("relative/bin")];

        assert!(expand_template("${HOME}/.claude/settings.json", &context).is_none());

        let process_context = ScanEnvironment::from_process(&[]);
        assert!(process_context
            .path_entries
            .iter()
            .all(|entry| entry.is_absolute()));
    }

    #[test]
    fn discovery_windows_path_enumerates_native_files_and_reports_shell_shims() {
        let mut context = environment(Platform::Windows);
        context.path_entries = vec![PathBuf::from("C:/Tools")];
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        let descriptor = &registry.descriptors()[0];

        let paths: Vec<_> = executable_candidates(descriptor, &context)
            .into_iter()
            .map(|candidate| candidate.path.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(paths.contains(&"C:/Tools/claude.exe".to_string()));
        assert!(paths.contains(&"C:/Tools/claude.cmd".to_string()));
        assert!(!paths.contains(&"C:/Tools/claude".to_string()));
    }
}
