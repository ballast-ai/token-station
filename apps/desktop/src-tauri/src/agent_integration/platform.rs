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
        candidates.extend(windows_store_codex_candidates().into_iter().map(|path| {
            ExecutableCandidate {
                path,
                source: DiscoverySource::PackageManager,
                path_order: None,
            }
        }));
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

    if environment.platform == Platform::Macos
        && matches!(
            descriptor.version_probe.runtime.as_ref(),
            Some(ProbeRuntime::NodePackage { .. })
        )
    {
        if let Some(prefix) = macos_npm_prefix(environment) {
            for executable in &descriptor.executable_candidates {
                candidates.push(ExecutableCandidate {
                    path: prefix.join("bin").join(executable),
                    source: DiscoverySource::KnownPath,
                    path_order: None,
                });
            }
        }
    }

    if environment.platform == Platform::Macos && descriptor.agent_id == "nous-hermes-agent" {
        candidates.extend(macos_python_user_candidates(environment, "hermes"));
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

fn macos_npm_prefix(environment: &ScanEnvironment) -> Option<PathBuf> {
    let home = environment.variables.get("HOME")?;
    if !is_absolute_for(Platform::Macos, home) {
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
            prefix = safe_absolute_user_prefix(value);
        }
    }
    prefix
}

fn safe_absolute_user_prefix(value: &str) -> Option<PathBuf> {
    if value.is_empty()
        || value.contains(['\0', '$', '`'])
        || value.contains("://")
        || !is_absolute_for(Platform::Macos, value)
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

#[cfg(target_os = "windows")]
struct WindowsRuntimeApartment;

#[cfg(target_os = "windows")]
impl WindowsRuntimeApartment {
    fn initialize_mta() -> Option<Self> {
        use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

        // SAFETY: a successful call, including S_FALSE, is balanced by this
        // guard's Drop implementation on the same synchronous call stack.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
            .ok()
            .map(|()| Self)
    }
}

#[cfg(target_os = "windows")]
impl Drop for WindowsRuntimeApartment {
    fn drop(&mut self) {
        use windows::Win32::System::WinRT::RoUninitialize;

        // SAFETY: the guard exists only after RoInitialize succeeded on this
        // thread, and is dropped on the same stack before discovery returns.
        unsafe { RoUninitialize() };
    }
}

#[cfg(target_os = "windows")]
fn windows_store_codex_candidates() -> Vec<PathBuf> {
    use windows::core::HSTRING;
    use windows::Management::Deployment::PackageManager;

    // A desktop process may already be running in an STA. In that case
    // RoInitialize reports RPC_E_CHANGED_MODE, but WinRT remains usable on
    // the initialized apartment, so discovery still attempts the read-only API.
    let _apartment = WindowsRuntimeApartment::initialize_mta();
    let Ok(manager) = PackageManager::new() else {
        return Vec::new();
    };
    let Ok(packages) = manager.FindPackagesByUserSecurityId(&HSTRING::new()) else {
        return Vec::new();
    };

    let mut candidates = Vec::new();
    for package in packages {
        let Ok(id) = package.Id() else {
            continue;
        };
        let Ok(name) = id.Name() else {
            continue;
        };
        if !is_codex_store_package_name(&name.to_string()) {
            continue;
        }
        let Ok(location) = package.InstalledLocation() else {
            continue;
        };
        let Ok(root) = location.Path() else {
            continue;
        };
        candidates.extend(codex_store_relative_entries(PathBuf::from(
            root.to_string(),
        )));
    }
    candidates
}

#[cfg(not(target_os = "windows"))]
fn windows_store_codex_candidates() -> Vec<PathBuf> {
    Vec::new()
}

#[cfg(any(target_os = "windows", test))]
fn is_codex_store_package_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "openai.codex" || name == "openai.chatgpt" || name.starts_with("openai.codex.")
}

#[cfg(any(target_os = "windows", test))]
fn codex_store_relative_entries(root: PathBuf) -> Vec<PathBuf> {
    [
        ["app", "resources", "codex.exe"].as_slice(),
        ["resources", "codex.exe"].as_slice(),
    ]
    .into_iter()
    .map(|parts| {
        parts
            .iter()
            .fold(root.clone(), |path, part| path.join(part))
    })
    .collect()
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
    fn codex_store_identity_and_relative_cli_entries_are_closed() {
        assert!(is_codex_store_package_name("OpenAI.Codex"));
        assert!(is_codex_store_package_name("OpenAI.Codex.Preview"));
        assert!(is_codex_store_package_name("OpenAI.ChatGPT"));
        assert!(!is_codex_store_package_name("ThirdParty.Codex"));

        let paths = codex_store_relative_entries(PathBuf::from("C:/Program Files/WindowsApps/pkg"));
        assert_eq!(
            paths,
            vec![
                PathBuf::from("C:/Program Files/WindowsApps/pkg/app/resources/codex.exe"),
                PathBuf::from("C:/Program Files/WindowsApps/pkg/resources/codex.exe"),
            ]
        );
    }

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

    #[test]
    fn macos_npm_prefix_rejects_values_that_expand_or_escape() {
        assert_eq!(
            safe_absolute_user_prefix("/Users/tester/local/npm-global"),
            Some(PathBuf::from("/Users/tester/local/npm-global"))
        );
        for value in [
            "relative/npm-global",
            "${HOME}/npm-global",
            "$(touch /tmp/never-run)",
            "https://example.invalid/npm",
            "/Users/tester/../escape",
        ] {
            assert!(safe_absolute_user_prefix(value).is_none(), "{value}");
        }
    }

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
