use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

use super::types::{
    AgentDescriptor, ConfigFormat, DiscoverySource, EnvValueKind, Platform, ProbeRuntime,
};

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
        // Version probes run with env_clear() so credentials and unrelated
        // process state cannot leak into discovered tools. Several Python CLIs,
        // including Hermes on Windows, still need the ordinary user-directory
        // variables just to initialize pathlib/platformdirs before printing
        // --help. These roots were already allowlisted and validated above.
        for name in ROOT_VARIABLES {
            if let Some(value) = variables.get(*name) {
                child_environment.insert((*name).to_string(), value.clone());
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
    if descriptor.agent_id == "codex" && environment.platform == Platform::Windows {
        candidates.extend(
            windows_codex_cli_candidates(environment)
                .into_iter()
                .map(|path| ExecutableCandidate {
                    path,
                    source: DiscoverySource::PackageManager,
                    path_order: None,
                }),
        );
    }
    if descriptor.agent_id == "workbuddy" && environment.platform == Platform::Macos {
        candidates.extend(
            macos_workbuddy_app_candidates(environment)
                .into_iter()
                .map(|path| ExecutableCandidate {
                    path,
                    source: DiscoverySource::KnownPath,
                    path_order: None,
                }),
        );
    }
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

    if matches!(
        environment.platform,
        Platform::Macos | Platform::Linux | Platform::Wsl
    ) && matches!(
        descriptor.version_probe.runtime.as_ref(),
        Some(ProbeRuntime::NodePackage { .. })
    ) {
        if let Some(prefix) = user_npm_prefix(environment) {
            for executable in &descriptor.executable_candidates {
                candidates.push(ExecutableCandidate {
                    path: prefix.join("bin").join(executable),
                    source: DiscoverySource::KnownPath,
                    path_order: None,
                });
            }
        }
        if matches!(environment.platform, Platform::Linux | Platform::Wsl) {
            candidates.extend(linux_node_manager_candidates(
                environment,
                &descriptor.executable_candidates,
            ));
        }
    }

    if environment.platform == Platform::Macos && descriptor.agent_id == "nous-hermes-agent" {
        candidates.extend(macos_python_user_candidates(environment, "hermes"));
    }

    if descriptor.agent_id == "deepseek-harness" {
        candidates.extend(npm_npx_cache_candidates(
            environment,
            &descriptor.executable_candidates,
        ));
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

const WORKBUDDY_CLI_RELATIVE_PATH: &str = "Contents/Resources/app.asar.unpacked/cli/bin/codebuddy";
const MAX_APP_SCAN_ENTRIES: usize = 1_024;

fn direct_app_bundles(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .take(MAX_APP_SCAN_ENTRIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            (metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && path.extension().is_some_and(|extension| extension == "app"))
            .then_some(path)
        })
        .collect()
}

fn bounded_macos_app_candidates(
    applications: &Path,
    user_applications: Option<&Path>,
    validate: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut apps = direct_app_bundles(applications);
    if let Some(root) = user_applications {
        apps.extend(direct_app_bundles(root));
    }
    apps.into_iter()
        .filter(|app| validate(app))
        .map(|app| app.join(WORKBUDDY_CLI_RELATIVE_PATH))
        .collect()
}

#[cfg(target_os = "macos")]
fn verified_workbuddy_bundle(app: &Path) -> bool {
    let cli = app.join(WORKBUDDY_CLI_RELATIVE_PATH);
    if !std::fs::symlink_metadata(&cli)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
    {
        return false;
    }
    let verified = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(app)
        .output()
        .is_ok_and(|output| output.status.success());
    if !verified {
        return false;
    }
    let Ok(output) = std::process::Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2"])
        .arg(app)
        .output()
    else {
        return false;
    };
    if !output.status.success() || output.stderr.len() > MAX_USER_CONFIG_BYTES as usize {
        return false;
    }
    workbuddy_signing_identity_is_allowed(&output.stderr)
}

#[cfg(any(target_os = "macos", test))]
fn workbuddy_signing_identity_is_allowed(bytes: &[u8]) -> bool {
    let metadata = String::from_utf8_lossy(bytes);
    let identifier = metadata
        .lines()
        .find_map(|line| line.strip_prefix("Identifier="));
    let team = metadata
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="));
    matches!(
        identifier,
        Some("com.workbuddy.workbuddy" | "com.workbuddy.workbuddy-ai")
    ) && team == Some("FN2V63AD2J")
}

#[cfg(not(target_os = "macos"))]
fn verified_workbuddy_bundle(_app: &Path) -> bool {
    false
}

fn macos_workbuddy_app_candidates(environment: &ScanEnvironment) -> Vec<PathBuf> {
    let user_applications = environment
        .variables
        .get("HOME")
        .filter(|home| is_absolute_for(Platform::Macos, home))
        .map(|home| PathBuf::from(home).join("Applications"));
    bounded_macos_app_candidates(
        Path::new("/Applications"),
        user_applications.as_deref(),
        verified_workbuddy_bundle,
    )
}

const MAX_USER_CONFIG_BYTES: u64 = 65_536;
const MAX_PYTHON_USER_VERSIONS: usize = 32;
const MAX_NPX_CACHE_ENTRIES: usize = 256;

fn npm_npx_cache_root(environment: &ScanEnvironment) -> Option<PathBuf> {
    match environment.platform {
        Platform::Macos | Platform::Linux | Platform::Wsl => environment
            .variables
            .get("HOME")
            .filter(|home| is_absolute_for(environment.platform, home))
            .map(|home| PathBuf::from(home).join(".npm/_npx")),
        Platform::Windows => environment
            .variables
            .get("LOCALAPPDATA")
            .filter(|root| is_absolute_for(Platform::Windows, root))
            .map(|root| PathBuf::from(normalize_separators(root)).join("npm-cache/_npx")),
    }
}

fn npm_npx_cache_candidates(
    environment: &ScanEnvironment,
    executables: &[String],
) -> Vec<ExecutableCandidate> {
    let Some(root) = npm_npx_cache_root(environment) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut cache_entries = entries
        .take(MAX_NPX_CACHE_ENTRIES)
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return false;
            };
            (8..=64).contains(&name.len())
                && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                && entry
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    cache_entries.sort();
    cache_entries
        .into_iter()
        .flat_map(|cache| {
            executables
                .iter()
                .map(move |executable| ExecutableCandidate {
                    path: cache.join("node_modules/.bin").join(executable),
                    source: DiscoverySource::KnownPath,
                    path_order: None,
                })
        })
        .collect()
}

fn user_npm_prefix(environment: &ScanEnvironment) -> Option<PathBuf> {
    let home = environment.variables.get("HOME")?;
    if !is_absolute_for(environment.platform, home) {
        return None;
    }
    let mut file = std::fs::File::open(PathBuf::from(home).join(".npmrc")).ok()?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_USER_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_USER_CONFIG_BYTES {
        return None;
    }
    let text = std::str::from_utf8(&bytes).ok()?;
    let mut prefix = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case("prefix") {
            let value = value
                .trim()
                .trim_matches(|character| matches!(character, '\'' | '"'));
            prefix = safe_absolute_user_prefix(value, environment.platform);
        }
    }
    prefix
}

fn safe_absolute_user_prefix(value: &str, platform: Platform) -> Option<PathBuf> {
    if value.is_empty()
        || value.contains(['\0', '$', '`'])
        || value.contains("://")
        || !is_absolute_for(platform, value)
    {
        return None;
    }
    let path = PathBuf::from(value);
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        return None;
    }
    Some(path)
}

const MAX_NODE_MANAGER_VERSIONS: usize = 64;

fn linux_node_manager_candidates(
    environment: &ScanEnvironment,
    executables: &[String],
) -> Vec<ExecutableCandidate> {
    let Some(home) = environment.variables.get("HOME") else {
        return Vec::new();
    };
    if !is_absolute_for(environment.platform, home) {
        return Vec::new();
    }
    let home = PathBuf::from(home);
    let mut version_bins = bounded_node_manager_bins(&home.join(".nvm/versions/node"), "bin");
    version_bins.extend(bounded_node_manager_bins(
        &home.join(".local/share/fnm/node-versions"),
        "installation/bin",
    ));
    version_bins.sort();

    version_bins
        .into_iter()
        .flat_map(|bin| {
            executables
                .iter()
                .map(move |executable| ExecutableCandidate {
                    path: bin.join(executable),
                    source: DiscoverySource::KnownPath,
                    path_order: None,
                })
        })
        .collect()
}

fn bounded_node_manager_bins(root: &Path, bin_suffix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .take(MAX_NODE_MANAGER_VERSIONS)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            (metadata.is_dir() && !metadata.file_type().is_symlink()).then(|| path.join(bin_suffix))
        })
        .collect()
}

fn macos_python_user_candidates(
    environment: &ScanEnvironment,
    executable: &str,
) -> Vec<ExecutableCandidate> {
    let Some(home) = environment.variables.get("HOME") else {
        return Vec::new();
    };
    if !is_absolute_for(Platform::Macos, home) {
        return Vec::new();
    }
    let root = PathBuf::from(home).join("Library/Python");
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name();
            name.to_str()
                .is_some_and(|name| {
                    !name.is_empty()
                        && name
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || byte == b'.')
                })
                .then(|| entry.path())
        })
        .collect();
    versions.sort();
    versions.truncate(MAX_PYTHON_USER_VERSIONS);
    versions
        .into_iter()
        .map(|version| ExecutableCandidate {
            path: version.join("bin").join(executable),
            source: DiscoverySource::KnownPath,
            path_order: None,
        })
        .collect()
}

const MAX_CODEX_CLI_DIRECTORIES: usize = 64;

fn windows_codex_cli_candidates(environment: &ScanEnvironment) -> Vec<PathBuf> {
    let Some(local_app_data) = environment.variables.get("LOCALAPPDATA") else {
        return Vec::new();
    };
    if !is_absolute_for(Platform::Windows, local_app_data) {
        return Vec::new();
    }
    newest_codex_cli(Path::new(local_app_data).join("OpenAI/Codex/bin"))
        .into_iter()
        .collect()
}

/// Codex desktop stores the runnable CLI below the user's app-data directory.
/// The similarly named executable in the MSIX package resources is an
/// access-controlled app resource and cannot be launched by Token Station.
/// Keep the scan one level deep and select the newest real CLI so retained
/// desktop update directories do not appear as conflicting installations.
fn newest_codex_cli(root: PathBuf) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    entries
        .take(MAX_CODEX_CLI_DIRECTORIES)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let directory = entry.path();
            let directory_metadata = std::fs::symlink_metadata(&directory).ok()?;
            if !directory_metadata.is_dir() || directory_metadata.file_type().is_symlink() {
                return None;
            }
            let executable = directory.join("codex.exe");
            let metadata = std::fs::symlink_metadata(&executable).ok()?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return None;
            }
            let modified = metadata.modified().ok();
            Some((modified, executable))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)))
        .map(|(_, executable)| executable)
}

fn path_executable_names(name: &str, platform: Platform) -> Vec<String> {
    if platform == Platform::Windows && PathBuf::from(name).extension().is_none() {
        // Discovery intentionally never executes bat/PowerShell shims. Do not
        // manufacture permanently broken installation records for extensions
        // rejected by executable_permission().
        ["exe", "com", "cmd"]
            .into_iter()
            .map(|extension| format!("{name}.{extension}"))
            .collect()
    } else {
        vec![name.to_string()]
    }
}

fn app_bundle_suffix(path: &Path) -> Option<PathBuf> {
    let components: Vec<_> = path.components().collect();
    let start = components.iter().position(|component| {
        Path::new(component.as_os_str())
            .extension()
            .is_some_and(|extension| extension == "app")
    })?;
    Some(
        components[start..]
            .iter()
            .fold(PathBuf::new(), |suffix, component| {
                suffix.join(component.as_os_str())
            }),
    )
}

fn installation_path_matches(actual: &Path, declared: &Path) -> bool {
    actual == declared
        || app_bundle_suffix(actual).is_some_and(|actual_suffix| {
            app_bundle_suffix(declared)
                .is_some_and(|declared_suffix| actual_suffix == declared_suffix)
        })
}

pub(crate) fn config_candidates(
    descriptor: &AgentDescriptor,
    environment: &ScanEnvironment,
    installation_path: &std::path::Path,
) -> ConfigResolution {
    let mut candidates = Vec::new();
    let mut invalid_environment_names = Vec::new();
    let scoped_match = descriptor.config_locations.iter().any(|location| {
        location
            .installation_path_defaults
            .get(&environment.platform)
            .is_some_and(|templates| {
                templates.iter().any(|template| {
                    expand_template(template, environment)
                        .is_some_and(|path| installation_path_matches(installation_path, &path))
                })
            })
    });
    for location in &descriptor.config_locations {
        let scoped = !location.installation_path_defaults.is_empty();
        let matches_installation = location
            .installation_path_defaults
            .get(&environment.platform)
            .is_some_and(|templates| {
                templates.iter().any(|template| {
                    expand_template(template, environment)
                        .is_some_and(|path| installation_path_matches(installation_path, &path))
                })
            });
        if (scoped_match && !matches_installation) || (!scoped_match && scoped) {
            continue;
        }
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

    if descriptor.agent_id == "opencode" {
        add_opencode_jsonc_candidates(&mut candidates, environment);
    }
    let mut seen = BTreeSet::new();
    candidates
        .retain(|candidate| seen.insert(path_identity(&candidate.path, environment.platform)));
    if descriptor.agent_id == "opencode" {
        prioritize_opencode_candidates(&mut candidates, environment);
    }
    ConfigResolution {
        candidates,
        invalid_environment_names,
        inline_config_present: descriptor.agent_id == "opencode"
            && environment
                .present_environment
                .contains("OPENCODE_CONFIG_CONTENT"),
    }
}

fn add_opencode_jsonc_candidates(
    candidates: &mut Vec<ResolvedConfigCandidate>,
    environment: &ScanEnvironment,
) {
    let explicit_file = environment
        .variables
        .get("OPENCODE_CONFIG")
        .filter(|path| is_absolute_for(environment.platform, path))
        .map(|path| {
            path_identity(
                &PathBuf::from(normalize_separators(path)),
                environment.platform,
            )
        });
    let original = std::mem::take(candidates);
    for candidate in original {
        let is_default_json = candidate
            .path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.eq_ignore_ascii_case("opencode.json"))
            && explicit_file.as_deref()
                != Some(path_identity(&candidate.path, environment.platform).as_str());
        let jsonc = is_default_json.then(|| ResolvedConfigCandidate {
            path: candidate.path.with_extension("jsonc"),
            format: candidate.format,
        });
        candidates.push(candidate);
        candidates.extend(jsonc);
    }
}

fn prioritize_opencode_candidates(
    candidates: &mut Vec<ResolvedConfigCandidate>,
    environment: &ScanEnvironment,
) {
    // An explicit file is authoritative and is already the first Registry
    // candidate. Do not let a different existing global file outrank it.
    if environment.present_environment.contains("OPENCODE_CONFIG") {
        return;
    }

    let rank = |candidate: &ResolvedConfigCandidate| {
        let jsonc = candidate
            .path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonc"));
        match (candidate.path.is_file(), jsonc) {
            (true, true) => 0,
            (true, false) => 1,
            (false, false) => 2,
            (false, true) => 3,
        }
    };

    if let Some(directory) = environment.variables.get("OPENCODE_CONFIG_DIR") {
        if is_absolute_for(environment.platform, directory) {
            let directory = path_identity(
                &PathBuf::from(normalize_separators(directory)),
                environment.platform,
            );
            let mut scoped = Vec::new();
            let mut defaults = Vec::new();
            for candidate in candidates.drain(..) {
                let parent = candidate
                    .path
                    .parent()
                    .map(|path| path_identity(path, environment.platform));
                if parent.as_deref() == Some(directory.as_str()) {
                    scoped.push(candidate);
                } else {
                    defaults.push(candidate);
                }
            }
            scoped.sort_by_key(&rank);
            scoped.extend(defaults);
            *candidates = scoped;
            return;
        }
    }
    let mut parent_order = BTreeMap::new();
    for candidate in candidates.iter() {
        let parent = candidate
            .path
            .parent()
            .map(|path| path_identity(path, environment.platform))
            .unwrap_or_default();
        let next = parent_order.len();
        parent_order.entry(parent).or_insert(next);
    }
    candidates.sort_by_key(|candidate| {
        let parent = candidate
            .path
            .parent()
            .map(|path| path_identity(path, environment.platform))
            .unwrap_or_default();
        (
            parent_order.get(&parent).copied().unwrap_or(usize::MAX),
            rank(candidate),
        )
    });
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
            let drive_rooted = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && bytes[2] == b'/';
            let unc_share = normalized.strip_prefix("//").is_some_and(|rest| {
                let mut segments = rest.split('/');
                let server = segments.next().unwrap_or_default();
                let share = segments.next().unwrap_or_default();
                !server.is_empty()
                    && !share.is_empty()
                    && !matches!(server, "." | ".." | "?")
                    && !matches!(share, "." | "..")
            });
            drive_rooted || unc_share
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
    // Only the two macOS real-filesystem cases use `fs`, and both are cfg-excluded
    // on a Windows host, so the import is gated the same way to avoid an unused
    // import warning on Windows.
    #[cfg(not(target_os = "windows"))]
    use std::fs;

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
    fn discovery_accepts_a_windows_unc_share_as_an_absolute_environment_root() {
        let mut context = environment(Platform::Windows);
        context.variables.insert(
            "APPDATA".to_string(),
            r"\\fileserver\profiles\tester\AppData\Roaming".to_string(),
        );

        let expanded = expand_template("${APPDATA}/npm/kimi.cmd", &context)
            .expect("a complete UNC share is an absolute Windows root");

        assert_eq!(
            expanded.to_string_lossy().replace('\\', "/"),
            "//fileserver/profiles/tester/AppData/Roaming/npm/kimi.cmd",
        );
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

    #[cfg(windows)]
    #[test]
    fn process_scan_passes_only_allowlisted_user_roots_to_windows_probes() {
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        let context = ScanEnvironment::from_process(registry.descriptors());

        for name in ["USERPROFILE", "LOCALAPPDATA"] {
            let expected = context
                .variables
                .get(name)
                .unwrap_or_else(|| panic!("Windows must expose {name}"));
            assert_eq!(context.child_environment.get(name), Some(expected));
        }
        assert!(!context
            .child_environment
            .contains_key("TOKEN_STATION_API_KEY"));
        assert!(!context.child_environment.contains_key("OPENAI_API_KEY"));
    }

    #[cfg(windows)]
    #[test]
    fn opencode_existing_jsonc_outranks_json_but_json_remains_the_create_default() {
        let root = std::env::temp_dir().join(format!(
            "token-station-opencode-jsonc-{}",
            std::process::id()
        ));
        let config_dir = root.join(".config/opencode");
        let json = config_dir.join("opencode.json");
        let jsonc = config_dir.join("opencode.jsonc");
        std::fs::create_dir_all(&config_dir).unwrap();
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "opencode")
            .unwrap();
        let mut context = environment(Platform::Windows);
        context.variables.insert(
            "USERPROFILE".to_string(),
            root.to_string_lossy().into_owned(),
        );

        let missing = config_candidates(descriptor, &context, Path::new("C:/Tools/opencode.exe"));
        assert_eq!(missing.candidates.first().unwrap().path, json);

        std::fs::write(&jsonc, b"{ // existing\n}\n").unwrap();
        let existing = config_candidates(descriptor, &context, Path::new("C:/Tools/opencode.exe"));
        assert_eq!(existing.candidates.first().unwrap().path, jsonc);

        std::fs::write(&json, b"{}\n").unwrap();
        let both = config_candidates(descriptor, &context, Path::new("C:/Tools/opencode.exe"));
        assert_eq!(both.candidates.first().unwrap().path, jsonc);

        let xdg = root.join("xdg");
        context.variables.insert(
            "XDG_CONFIG_HOME".to_string(),
            xdg.to_string_lossy().into_owned(),
        );
        let xdg_json = xdg.join("opencode/opencode.json");
        let rooted = config_candidates(descriptor, &context, Path::new("C:/Tools/opencode.exe"));
        assert_eq!(rooted.candidates.first().unwrap().path, xdg_json);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    #[cfg(unix)]
    fn workbuddy_app_scan_stays_inside_the_two_application_roots() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "token-station-workbuddy-app-scan-{}",
            std::process::id()
        ));
        let applications = root.join("Applications");
        let user_applications = root.join("UserApplications");
        let domestic = applications.join("WorkBuddy.app");
        let global = user_applications.join("WorkBuddy AI.app");
        let too_deep = user_applications.join("nested/WorkBuddy.app");
        let rejected = applications.join("Pretender.app");
        for app in [&domestic, &global, &too_deep, &rejected] {
            fs::create_dir_all(app.join(WORKBUDDY_CLI_RELATIVE_PATH).parent().unwrap()).unwrap();
            fs::write(app.join(WORKBUDDY_CLI_RELATIVE_PATH), b"fixture").unwrap();
        }
        symlink(&domestic, applications.join("WorkBuddy Alias.app")).unwrap();

        let candidates =
            bounded_macos_app_candidates(&applications, Some(&user_applications), |app| {
                app.file_name()
                    .is_some_and(|name| name == "WorkBuddy.app" || name == "WorkBuddy AI.app")
            });

        assert_eq!(candidates.len(), 2);
        for app in [&domestic, &global] {
            assert!(candidates.contains(&app.join(WORKBUDDY_CLI_RELATIVE_PATH)));
        }
        assert!(!candidates.contains(&too_deep.join(WORKBUDDY_CLI_RELATIVE_PATH)));
        assert!(!candidates.contains(&rejected.join(WORKBUDDY_CLI_RELATIVE_PATH)));
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn workbuddy_installation_scope_follows_the_app_bundle_across_application_roots() {
        let declared = Path::new(
            "/Applications/WorkBuddy AI.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy",
        );
        let mounted = Path::new(
            "/Users/tester/Applications/WorkBuddy AI.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy",
        );
        let domestic = Path::new(
            "/Users/tester/Applications/WorkBuddy.app/Contents/Resources/app.asar.unpacked/cli/bin/codebuddy",
        );

        assert!(installation_path_matches(mounted, declared));
        assert!(!installation_path_matches(domestic, declared));
        assert!(!installation_path_matches(
            Path::new("/Users/tester/Applications/WorkBuddy AI.app/Contents/MacOS/WorkBuddy AI"),
            declared,
        ));
    }

    #[test]
    fn workbuddy_dynamic_scan_accepts_only_the_two_signed_product_identities() {
        for identifier in ["com.workbuddy.workbuddy", "com.workbuddy.workbuddy-ai"] {
            assert!(workbuddy_signing_identity_is_allowed(
                format!("Identifier={identifier}\nTeamIdentifier=FN2V63AD2J\n").as_bytes(),
            ));
        }
        assert!(!workbuddy_signing_identity_is_allowed(
            b"Identifier=com.example.pretender\nTeamIdentifier=FN2V63AD2J\n",
        ));
        assert!(!workbuddy_signing_identity_is_allowed(
            b"Identifier=com.workbuddy.workbuddy\nTeamIdentifier=OTHERTEAM\n",
        ));
    }

    #[test]
    fn codex_desktop_scan_selects_the_newest_bounded_user_cli() {
        let root = std::env::temp_dir().join(format!(
            "token-station-codex-user-cli-{}",
            std::process::id()
        ));
        let old = root.join("old/codex.exe");
        let current = root.join("current/codex.exe");
        let ignored = root.join("not-a-version/nested/codex.exe");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, b"old").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();
        std::fs::write(&current, b"current").unwrap();
        std::fs::create_dir_all(ignored.parent().unwrap()).unwrap();
        std::fs::write(&ignored, b"ignored").unwrap();

        assert_eq!(newest_codex_cli(root.clone()), Some(current));
        std::fs::remove_dir_all(root).ok();
    }

    // This case creates a real directory, treats it as HOME, and then simulates a
    // macOS scan. macOS absolute-path validation requires a leading `/`, but a
    // Windows host's temp_dir is `C:\…`, so a `/`-rooted real directory cannot be
    // created there. `macos_npm_prefix` also only runs on the macOS platform, so
    // exercising it on a Windows host adds no coverage — run on non-Windows only.
    #[cfg(not(target_os = "windows"))]
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn macos_node_agents_include_the_safe_npm_prefix_from_user_config() {
        let root = std::env::temp_dir().join(format!(
            "token-station-npm-prefix-platform-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(".npmrc"),
            format!("prefix={}\n", root.join("local/npm-global").display()),
        )
        .unwrap();

        let mut context = environment(Platform::Macos);
        context
            .variables
            .insert("HOME".to_string(), root.to_string_lossy().into_owned());
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        for agent_id in ["claude-code", "gemini-cli", "opencode", "openclaw"] {
            let descriptor = registry
                .descriptors()
                .iter()
                .find(|descriptor| descriptor.agent_id == agent_id)
                .unwrap();
            let executable = descriptor.executable_candidates[0].as_str();
            let expected = root.join("local/npm-global/bin").join(executable);
            assert!(
                executable_candidates(descriptor, &context)
                    .iter()
                    .any(|candidate| candidate.path == expected
                        && candidate.source == DiscoverySource::KnownPath),
                "{agent_id} did not include {}",
                expected.display()
            );
        }

        fs::write(root.join(".npmrc"), "prefix=$(touch /tmp/never-run)\n").unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "openclaw")
            .unwrap();
        assert!(executable_candidates(descriptor, &context)
            .iter()
            .all(|candidate| !candidate.path.to_string_lossy().contains("never-run")));
        fs::remove_dir_all(root).ok();
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn linux_node_agents_include_the_safe_npm_prefix_from_user_config() {
        let root = std::env::temp_dir().join(format!(
            "token-station-linux-npm-prefix-platform-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(".npmrc"),
            format!("prefix={}\n", root.join("local/npm-global").display()),
        )
        .unwrap();

        let mut context = environment(Platform::Linux);
        context
            .variables
            .insert("HOME".to_string(), root.to_string_lossy().into_owned());
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "claude-code")
            .unwrap();

        let expected = root.join("local/npm-global/bin/claude");
        assert!(
            executable_candidates(descriptor, &context)
                .iter()
                .any(|candidate| candidate.path == expected
                    && candidate.source == DiscoverySource::KnownPath),
            "Linux GUI discovery did not include {}",
            expected.display(),
        );
        fs::remove_dir_all(root).ok();
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn linux_node_agents_discover_nvm_version_bins_without_a_login_shell() {
        let root = std::env::temp_dir().join(format!(
            "token-station-linux-nvm-platform-{}",
            std::process::id()
        ));
        let expected = root.join(".nvm/versions/node/v22.14.0/bin/claude");
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        fs::write(&expected, "#!/usr/bin/env node\n").unwrap();

        let mut context = environment(Platform::Linux);
        context
            .variables
            .insert("HOME".to_string(), root.to_string_lossy().into_owned());
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "claude-code")
            .unwrap();

        assert!(
            executable_candidates(descriptor, &context)
                .iter()
                .any(|candidate| candidate.path == expected
                    && candidate.source == DiscoverySource::KnownPath),
            "Linux GUI discovery did not inspect the bounded NVM version bin"
        );
        fs::remove_dir_all(root).ok();
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn linux_node_agents_discover_fnm_installation_bins_without_a_login_shell() {
        let root = std::env::temp_dir().join(format!(
            "token-station-linux-fnm-platform-{}",
            std::process::id()
        ));
        let expected = root.join(".local/share/fnm/node-versions/v22.14.0/installation/bin/claude");
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        fs::write(&expected, "#!/usr/bin/env node\n").unwrap();

        let mut context = environment(Platform::Linux);
        context
            .variables
            .insert("HOME".to_string(), root.to_string_lossy().into_owned());
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "claude-code")
            .unwrap();

        assert!(
            executable_candidates(descriptor, &context)
                .iter()
                .any(|candidate| candidate.path == expected
                    && candidate.source == DiscoverySource::KnownPath),
            "Linux GUI discovery did not inspect the bounded fnm installation bin"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn macos_npm_prefix_rejects_values_that_expand_or_escape() {
        assert_eq!(
            safe_absolute_user_prefix("/Users/tester/local/npm-global", Platform::Macos),
            Some(PathBuf::from("/Users/tester/local/npm-global"))
        );
        for value in [
            "relative/npm-global",
            "${HOME}/npm-global",
            "$(touch /tmp/never-run)",
            "https://example.invalid/npm",
            "/Users/tester/../escape",
        ] {
            assert!(
                safe_absolute_user_prefix(value, Platform::Macos).is_none(),
                "{value}"
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn deepseek_harness_discovers_the_bounded_official_npx_cache_entry() {
        let root = std::env::temp_dir().join(format!(
            "token-station-dsh-npx-platform-{}",
            std::process::id()
        ));
        let expected = root.join(".npm/_npx/0123456789abcdef/node_modules/.bin/dsh");
        let rejected_name = root.join(".npm/_npx/not-a-cache-id/node_modules/.bin/dsh");
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        fs::create_dir_all(rejected_name.parent().unwrap()).unwrap();
        fs::write(&expected, b"fixture").unwrap();
        fs::write(&rejected_name, b"fixture").unwrap();

        let mut context = environment(Platform::Macos);
        context
            .variables
            .insert("HOME".to_string(), root.to_string_lossy().into_owned());
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "deepseek-harness")
            .unwrap();
        let candidates = executable_candidates(descriptor, &context);

        assert!(candidates.iter().any(|candidate| {
            candidate.path == expected && candidate.source == DiscoverySource::KnownPath
        }));
        assert!(candidates
            .iter()
            .all(|candidate| candidate.path != rejected_name));
        fs::remove_dir_all(root).ok();
    }

    // Same as above: it builds a macOS-style HOME on the real filesystem and
    // simulates the macOS platform. A Windows host's temp_dir is not a `/`-rooted
    // path, and `macos_python_user_candidates` only runs on macOS, so run on
    // non-Windows only.
    #[cfg(not(target_os = "windows"))]
    #[test]
    #[cfg(not(target_os = "windows"))]
    fn macos_hermes_checks_bounded_python_user_bins_without_path() {
        let root = std::env::temp_dir().join(format!(
            "token-station-python-user-platform-{}",
            std::process::id()
        ));
        for minor in 0..40 {
            fs::create_dir_all(root.join(format!("Library/Python/3.{minor}/bin"))).unwrap();
        }
        fs::create_dir_all(root.join("Library/Python/not-a-version/bin")).unwrap();

        let mut context = environment(Platform::Macos);
        context
            .variables
            .insert("HOME".to_string(), root.to_string_lossy().into_owned());
        let registry = super::super::registry::AgentRegistry::builtin().unwrap();
        let descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "nous-hermes-agent")
            .unwrap();
        let paths: Vec<_> = executable_candidates(descriptor, &context)
            .into_iter()
            .map(|candidate| candidate.path)
            .collect();

        let python_paths: Vec<_> = paths
            .iter()
            .filter(|path| path.starts_with(root.join("Library/Python")))
            .collect();
        assert_eq!(python_paths.len(), MAX_PYTHON_USER_VERSIONS);
        assert!(paths.contains(&root.join("Library/Python/3.12/bin/hermes")));
        assert!(!paths.contains(&root.join("Library/Python/not-a-version/bin/hermes")));
        fs::remove_dir_all(root).ok();
    }
}
