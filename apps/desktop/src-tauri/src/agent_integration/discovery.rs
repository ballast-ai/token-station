use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use command_group::CommandGroup;
use semver::Version;
use sha2::{Digest, Sha256};

use super::config_codec::{parse_rendered, semantic_json, DocumentFormat};
use super::platform::{
    config_candidates, executable_candidates, expand_template, path_identity, ExecutableCandidate,
    ResolvedConfigCandidate, ScanEnvironment,
};
use super::registry::AgentRegistry;
use super::types::{
    AgentDescriptor, BinarySource, ConfigFormat, Diagnostic, DiscoveryEvidence, DiscoveryRecord,
    DiscoverySource, Platform, ProbeRuntime, ReasonCode, RuntimeResolutionSource,
    VersionOutputMatcher, VersionProbe,
};

const CONFIG_READ_LIMIT_BYTES: u64 = 2 * 1024 * 1024;
const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(5);
const OUTPUT_READER_GRACE: Duration = Duration::from_millis(100);

pub struct ProbeOutcome {
    pub runnable: bool,
    pub version_raw: Option<String>,
    pub version_normalized: Option<String>,
    pub diagnostics: Vec<Diagnostic>,
}

pub trait ProbeRunner {
    fn run(
        &self,
        executable: &Path,
        observed_entry: &Path,
        probe: &VersionProbe,
        environment: &ScanEnvironment,
    ) -> ProbeOutcome;
}

pub struct SystemProbeRunner;

impl ProbeRunner for SystemProbeRunner {
    fn run(
        &self,
        executable: &Path,
        observed_entry: &Path,
        probe: &VersionProbe,
        environment: &ScanEnvironment,
    ) -> ProbeOutcome {
        if probe.retry_on_timeout {
            run_with_timeout_retry(|| {
                run_probe_once(executable, observed_entry, probe, environment)
            })
        } else {
            run_probe_once(executable, observed_entry, probe, environment)
        }
    }
}

fn run_with_timeout_retry(mut attempt: impl FnMut() -> ProbeOutcome) -> ProbeOutcome {
    let first = attempt();
    if !first
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.reason_code == ReasonCode::VersionProbeTimeout)
    {
        return first;
    }

    std::thread::sleep(Duration::from_millis(100));
    let mut second = attempt();
    if second.runnable {
        second.diagnostics.insert(
            0,
            Diagnostic {
                reason_code: ReasonCode::VersionProbeTimeout,
                message: "首次版本探测超时，第二次尝试成功".to_string(),
            },
        );
    }
    second
}

struct ResolvedProbeCommand {
    observed_executable: PathBuf,
    canonical_executable: PathBuf,
    observed_program: PathBuf,
    canonical_program: PathBuf,
    arguments: Vec<std::ffi::OsString>,
}

fn run_probe_once(
    executable: &Path,
    observed_entry: &Path,
    probe: &VersionProbe,
    environment: &ScanEnvironment,
) -> ProbeOutcome {
    if unsupported_script_shim(executable, environment.platform)
        && !matches!(probe.runtime, Some(ProbeRuntime::NodePackage { .. }))
    {
        return broken_probe(
            ReasonCode::ExecutableNotRunnable,
            "已发现脚本 shim；为避免调用系统 shell，本版本不执行该入口",
        );
    }

    let resolved = match resolve_probe_command(executable, observed_entry, probe, environment) {
        Ok(resolved) => resolved,
        Err(outcome) => return outcome,
    };
    if !probe_command_still_matches(&resolved) {
        return broken_probe(
            ReasonCode::ExecutableNotRunnable,
            "版本探测入口或运行时在启动前发生变化",
        );
    }

    let mut command = Command::new(&resolved.canonical_program);
    command
        .args(&resolved.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_clear();
    for (name, value) in &environment.child_environment {
        if name != "PATH" {
            command.env(name, value);
        }
    }
    let path = match probe_child_path(&resolved.canonical_program, environment) {
        Ok(path) => path,
        Err(()) => {
            return broken_probe(
                ReasonCode::ExecutableNotRunnable,
                "无法构造受限的版本探测 PATH",
            );
        }
    };
    command.env("PATH", path);

    let mut child = match command.group_spawn() {
        Ok(child) => child,
        Err(_) => {
            return broken_probe(ReasonCode::ExecutableNotRunnable, "版本探测进程无法启动");
        }
    };
    let stdout = child.inner().stdout.take();
    let stderr = child.inner().stderr.take();
    let stdout_reader = stdout.map(|stream| spawn_output_reader(stream, probe.max_output_bytes));
    let stderr_reader = stderr.map(|stream| spawn_output_reader(stream, probe.max_output_bytes));

    let deadline = Instant::now() + Duration::from_millis(probe.timeout_ms);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(PROBE_POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(ReasonCode::VersionProbeTimeout);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break Err(ReasonCode::VersionProbeExitFailure);
            }
        }
    };

    if let Some(reader) = &stdout_reader {
        reader.stop();
    }
    if let Some(reader) = &stderr_reader {
        reader.stop();
    }
    let output_deadline = Instant::now() + OUTPUT_READER_GRACE;
    let stdout = finish_output_reader(stdout_reader, output_deadline);
    let stderr = finish_output_reader(stderr_reader, output_deadline);
    let output = combine_output(&stdout.bytes, &stderr.bytes, probe.max_output_bytes);
    let truncated = stdout.truncated || stderr.truncated || output.truncated;
    let raw = sanitize_output(&output.bytes, probe.max_output_bytes);

    let status = match status {
        Ok(status) => status,
        Err(reason_code) => {
            return ProbeOutcome {
                runnable: false,
                version_raw: None,
                version_normalized: None,
                diagnostics: vec![Diagnostic {
                    reason_code,
                    message: match reason_code {
                        ReasonCode::VersionProbeTimeout => {
                            "版本探测超时，进程组已终止并回收".to_string()
                        }
                        _ => "版本探测进程异常，已终止并回收".to_string(),
                    },
                }],
            };
        }
    };

    if !status.success() {
        return ProbeOutcome {
            runnable: false,
            version_raw: None,
            version_normalized: None,
            diagnostics: vec![Diagnostic {
                reason_code: ReasonCode::VersionProbeExitFailure,
                message: format!(
                    "版本探测以非零状态退出（code={}）",
                    status
                        .code()
                        .map_or_else(|| "signal".to_string(), |code| code.to_string())
                ),
            }],
        };
    }

    let (version_raw, normalized) = match probe.output_matcher {
        VersionOutputMatcher::SemverAnywhere => {
            let normalized = raw.as_deref().and_then(normalize_version);
            (raw, normalized)
        }
        VersionOutputMatcher::SuccessOnly => (None, None),
    };
    let mut diagnostics = Vec::new();
    if truncated {
        diagnostics.push(Diagnostic {
            reason_code: ReasonCode::VersionOutputTruncated,
            message: format!(
                "版本输出超过 {} 字节，已截断且未写入普通日志",
                probe.max_output_bytes
            ),
        });
    }
    if probe.output_matcher == VersionOutputMatcher::SemverAnywhere && normalized.is_none() {
        diagnostics.push(Diagnostic {
            reason_code: ReasonCode::VersionOutputUnparseable,
            message: "版本命令成功，但输出中没有可识别的 SemVer".to_string(),
        });
    }
    ProbeOutcome {
        runnable: true,
        version_raw,
        version_normalized: normalized,
        diagnostics,
    }
}

fn resolve_probe_command(
    executable: &Path,
    observed_entry: &Path,
    probe: &VersionProbe,
    environment: &ScanEnvironment,
) -> Result<ResolvedProbeCommand, ProbeOutcome> {
    let canonical_executable = std::fs::canonicalize(executable).map_err(|_| {
        broken_probe(
            ReasonCode::ExecutableNotRunnable,
            "版本探测入口无法再次解析真实路径",
        )
    })?;
    match probe.runtime.as_ref().unwrap_or(&ProbeRuntime::Direct) {
        ProbeRuntime::Direct => Ok(ResolvedProbeCommand {
            observed_executable: observed_entry.to_path_buf(),
            canonical_executable: canonical_executable.clone(),
            observed_program: observed_entry.to_path_buf(),
            canonical_program: canonical_executable,
            arguments: probe.argv.iter().map(std::ffi::OsString::from).collect(),
        }),
        ProbeRuntime::EnvShebang {
            interpreter_candidates,
            resolution_sources,
            known_install_locations,
        } => {
            if !matches_declared_env_shebang(&canonical_executable, interpreter_candidates) {
                return Err(broken_probe(
                    ReasonCode::ExecutableNotRunnable,
                    "版本探测脚本的 shebang 与内置 Registry 声明不匹配",
                ));
            }
            let (observed_program, canonical_program) = resolve_interpreter(
                observed_entry,
                interpreter_candidates,
                resolution_sources,
                known_install_locations,
                environment,
            )
            .map_err(|message| broken_probe(ReasonCode::ExecutableNotRunnable, message))?;
            let mut arguments = Vec::with_capacity(probe.argv.len() + 1);
            arguments.push(canonical_executable.as_os_str().to_os_string());
            arguments.extend(probe.argv.iter().map(std::ffi::OsString::from));
            Ok(ResolvedProbeCommand {
                observed_executable: observed_entry.to_path_buf(),
                canonical_executable,
                observed_program,
                canonical_program,
                arguments,
            })
        }
        ProbeRuntime::NodePackage {
            interpreter_candidates,
            resolution_sources,
            known_install_locations,
        } => {
            if is_native_executable(&canonical_executable) {
                return Ok(ResolvedProbeCommand {
                    observed_executable: observed_entry.to_path_buf(),
                    canonical_executable: canonical_executable.clone(),
                    observed_program: observed_entry.to_path_buf(),
                    canonical_program: canonical_executable,
                    arguments: probe.argv.iter().map(std::ffi::OsString::from).collect(),
                });
            }
            let script = if unsupported_script_shim(executable, environment.platform) {
                resolve_npm_shim_entry(&canonical_executable)
                    .map_err(|message| broken_probe(ReasonCode::ExecutableNotRunnable, message))?
            } else {
                if !matches_declared_env_shebang(&canonical_executable, interpreter_candidates) {
                    return Err(broken_probe(
                        ReasonCode::ExecutableNotRunnable,
                        "版本探测脚本既不是受支持的 npm shim，也不匹配内置 node shebang",
                    ));
                }
                canonical_executable.clone()
            };
            let (observed_program, canonical_program) = resolve_interpreter(
                observed_entry,
                interpreter_candidates,
                resolution_sources,
                known_install_locations,
                environment,
            )
            .map_err(|message| broken_probe(ReasonCode::ExecutableNotRunnable, message))?;
            let mut arguments = Vec::with_capacity(probe.argv.len() + 1);
            arguments.push(script.into_os_string());
            arguments.extend(probe.argv.iter().map(std::ffi::OsString::from));
            Ok(ResolvedProbeCommand {
                observed_executable: observed_entry.to_path_buf(),
                canonical_executable,
                observed_program,
                canonical_program,
                arguments,
            })
        }
    }
}

fn resolve_npm_shim_entry(shim: &Path) -> Result<PathBuf, &'static str> {
    const SHIM_LIMIT: u64 = 64 * 1024;
    const PACKAGE_LIMIT: u64 = 64 * 1024;

    let metadata = std::fs::metadata(shim).map_err(|_| "无法读取 npm shim")?;
    if !metadata.is_file() || metadata.len() > SHIM_LIMIT {
        return Err("npm shim 不是受限大小的普通文件");
    }
    let text = std::fs::read_to_string(shim).map_err(|_| "npm shim 不是 UTF-8 文本")?;
    if text.contains('\0') || text.lines().count() > 256 {
        return Err("npm shim 超出安全解析边界");
    }
    let parent = shim.parent().ok_or("npm shim 没有父目录")?;
    let node_modules = std::fs::canonicalize(parent.join("node_modules"))
        .map_err(|_| "npm shim 相邻 node_modules 不存在")?;
    let normalized = text.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    let marker = "node_modules/";
    let marker_start = lower
        .find(marker)
        .ok_or("npm shim 没有声明 node_modules 入口")?;
    let tail = &normalized[marker_start + marker.len()..];
    let entry_tail = tail
        .split(|character: char| {
            character.is_whitespace()
                || matches!(character, '"' | '\'' | '%' | '&' | '|' | '<' | '>')
        })
        .next()
        .filter(|value| !value.is_empty())
        .ok_or("npm shim 入口格式无效")?;
    let components = entry_tail.split('/').collect::<Vec<_>>();
    let package_component_count = if components.first().is_some_and(|item| item.starts_with('@')) {
        2
    } else {
        1
    };
    if components.len() <= package_component_count
        || components
            .iter()
            .any(|component| component.is_empty() || matches!(*component, "." | ".."))
    {
        return Err("npm shim 包路径无效");
    }
    let package_relative = components[..package_component_count].join("/");
    let package_root = std::fs::canonicalize(node_modules.join(&package_relative))
        .map_err(|_| "npm shim 指向的包目录不存在")?;
    if !package_root.starts_with(&node_modules) {
        return Err("npm shim 包目录逃逸 node_modules");
    }
    let package_json = package_root.join("package.json");
    let package_metadata =
        std::fs::metadata(&package_json).map_err(|_| "npm 包缺少 package.json")?;
    if !package_metadata.is_file() || package_metadata.len() > PACKAGE_LIMIT {
        return Err("npm package.json 超出安全解析边界");
    }
    let package: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&package_json).map_err(|_| "无法读取 npm package.json")?,
    )
    .map_err(|_| "npm package.json 不是有效 JSON")?;
    let command_name = shim
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or("npm shim 命令名无效")?;
    let bin = match package.get("bin") {
        Some(serde_json::Value::String(bin)) => bin.as_str(),
        Some(serde_json::Value::Object(bins)) => bins
            .get(command_name)
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                (bins.len() == 1)
                    .then(|| bins.values().next().and_then(serde_json::Value::as_str))
                    .flatten()
            })
            .ok_or("npm package.json.bin 没有匹配 shim 命令")?,
        _ => return Err("npm package.json.bin 无效"),
    };
    let bin_path = Path::new(bin);
    if bin_path.is_absolute()
        || bin_path.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err("npm package.json.bin 不是包内相对路径");
    }
    let extension = bin_path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "js" | "mjs" | "cjs") {
        return Err("npm package.json.bin 不是受支持的 node 脚本");
    }
    let entry = std::fs::canonicalize(package_root.join(bin_path))
        .map_err(|_| "npm package.json.bin 入口不存在")?;
    if !entry.starts_with(&package_root)
        || !std::fs::metadata(&entry).is_ok_and(|metadata| metadata.is_file())
    {
        return Err("npm package.json.bin 入口逃逸包目录");
    }
    let declared = format!(
        "node_modules/{}/{}",
        package_relative,
        bin.replace('\\', "/").trim_start_matches("./")
    )
    .to_ascii_lowercase();
    if !lower.contains(&declared) {
        return Err("npm shim 与 package.json.bin 不一致");
    }
    Ok(entry)
}

fn matches_declared_env_shebang(executable: &Path, candidates: &[String]) -> bool {
    let mut file = match std::fs::File::open(executable) {
        Ok(file) => file,
        Err(_) => return false,
    };
    let mut bytes = Vec::new();
    if file.by_ref().take(256).read_to_end(&mut bytes).is_err() {
        return false;
    }
    let first_line = bytes
        .split(|byte| *byte == b'\n')
        .next()
        .and_then(|line| std::str::from_utf8(line).ok())
        .map(str::trim_end);
    first_line.is_some_and(|line| {
        candidates
            .iter()
            .any(|candidate| line == format!("#!/usr/bin/env {candidate}"))
    })
}

fn resolve_interpreter(
    observed_entry: &Path,
    candidates: &[String],
    sources: &[RuntimeResolutionSource],
    known_locations: &BTreeMap<Platform, Vec<String>>,
    environment: &ScanEnvironment,
) -> Result<(PathBuf, PathBuf), &'static str> {
    for source in sources {
        let paths: Vec<PathBuf> = match source {
            RuntimeResolutionSource::ObservedEntrySibling => observed_entry
                .parent()
                .into_iter()
                .flat_map(|parent| candidates.iter().map(move |name| parent.join(name)))
                .collect(),
            RuntimeResolutionSource::KnownInstallLocations => known_locations
                .get(&environment.platform)
                .into_iter()
                .flatten()
                .filter_map(|template| expand_template(template, environment))
                .collect(),
            RuntimeResolutionSource::Path => environment
                .path_entries
                .iter()
                .flat_map(|directory| candidates.iter().map(move |name| directory.join(name)))
                .collect(),
        };
        let mut resolved = BTreeMap::new();
        for path in paths {
            if let Some(canonical) = canonical_native_executable(&path, environment.platform) {
                resolved
                    .entry(path_identity(&canonical, environment.platform))
                    .or_insert((path, canonical));
            }
        }
        match resolved.len() {
            0 => continue,
            1 => {
                return Ok(resolved
                    .into_values()
                    .next()
                    .expect("one runtime candidate"))
            }
            _ => return Err("同一优先级发现多个不同的版本探测运行时"),
        }
    }
    Err("未找到与安装入口匹配的版本探测运行时")
}

fn canonical_native_executable(path: &Path, platform: Platform) -> Option<PathBuf> {
    let entry = std::fs::symlink_metadata(path).ok()?;
    if !entry.is_file() && !entry.file_type().is_symlink() {
        return None;
    }
    let canonical = std::fs::canonicalize(path).ok()?;
    let metadata = std::fs::metadata(&canonical).ok()?;
    (metadata.is_file()
        && executable_permission(Some(&metadata), &canonical, platform)
        && is_native_executable(&canonical))
    .then_some(canonical)
}

fn is_native_executable(path: &Path) -> bool {
    let mut magic = [0_u8; 4];
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    if file.read_exact(&mut magic).is_err() {
        return false;
    }
    matches!(
        magic,
        [0x7f, b'E', b'L', b'F']
            | [b'M', b'Z', _, _]
            | [0xfe, 0xed, 0xfa, 0xce]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
    )
}

fn probe_command_still_matches(command: &ResolvedProbeCommand) -> bool {
    std::fs::canonicalize(&command.observed_executable)
        .is_ok_and(|path| path == command.canonical_executable)
        && std::fs::canonicalize(&command.observed_program)
            .is_ok_and(|path| path == command.canonical_program)
}

fn probe_child_path(
    program: &Path,
    environment: &ScanEnvironment,
) -> Result<std::ffi::OsString, ()> {
    let mut entries = Vec::new();
    if let Some(parent) = program.parent() {
        entries.push(parent.to_path_buf());
    }
    match environment.platform {
        Platform::Macos | Platform::Linux | Platform::Wsl => {
            for path in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
                let path = PathBuf::from(path);
                if !entries.contains(&path) {
                    entries.push(path);
                }
            }
        }
        Platform::Windows => {
            for root in ["SYSTEMROOT", "WINDIR"]
                .into_iter()
                .filter_map(|name| environment.child_environment.get(name))
            {
                for path in [PathBuf::from(root), PathBuf::from(root).join("System32")] {
                    if path.is_absolute() && !entries.contains(&path) {
                        entries.push(path);
                    }
                }
            }
        }
    }
    std::env::join_paths(entries).map_err(|_| ())
}

pub struct DiscoveryScanner<R = SystemProbeRunner> {
    environment: ScanEnvironment,
    runner: R,
}

impl DiscoveryScanner<SystemProbeRunner> {
    #[must_use]
    pub fn from_process(registry: &AgentRegistry) -> Self {
        Self {
            environment: ScanEnvironment::from_process(registry.descriptors()),
            runner: SystemProbeRunner,
        }
    }
}

impl<R: ProbeRunner> DiscoveryScanner<R> {
    #[must_use]
    pub fn new(environment: ScanEnvironment, runner: R) -> Self {
        Self {
            environment,
            runner,
        }
    }

    #[must_use]
    pub fn scan_registry(&self, registry: &AgentRegistry) -> Vec<DiscoveryRecord> {
        let scanned_at_ms = unix_time_ms();
        let mut records: Vec<_> = registry
            .descriptors()
            .iter()
            .flat_map(|descriptor| self.scan_descriptor_at(descriptor, scanned_at_ms))
            .collect();
        records.sort_by(|left, right| {
            left.agent_id
                .cmp(&right.agent_id)
                .then_with(|| left.canonical_path.cmp(&right.canonical_path))
        });
        records
    }

    #[must_use]
    pub fn scan_descriptor(&self, descriptor: &AgentDescriptor) -> Vec<DiscoveryRecord> {
        self.scan_descriptor_at(descriptor, unix_time_ms())
    }

    fn scan_descriptor_at(
        &self,
        descriptor: &AgentDescriptor,
        scanned_at_ms: u64,
    ) -> Vec<DiscoveryRecord> {
        let mut installations = collect_installations(
            executable_candidates(descriptor, &self.environment),
            self.environment.platform,
        );
        if installations.is_empty() {
            return Vec::new();
        }
        mark_path_default(&mut installations);

        let config = inspect_configs(descriptor, &self.environment);
        let conflict_group = (installations.len() > 1).then(|| {
            stable_conflict_group(
                &descriptor.agent_id,
                installations.keys().map(String::as_str).collect::<Vec<_>>(),
            )
        });

        installations
            .into_values()
            .map(|installation| {
                let mut diagnostics = installation.diagnostics;
                let probe = if installation.probe_allowed {
                    self.runner.run(
                        &installation.canonical_path,
                        &installation.observed_probe_path,
                        &descriptor.version_probe,
                        &self.environment,
                    )
                } else {
                    broken_probe(
                        ReasonCode::ExecutableNotRunnable,
                        "已发现入口，但它不是当前平台可安全执行的原生文件",
                    )
                };
                diagnostics.extend(probe.diagnostics);
                diagnostics.extend(config.diagnostics.clone());
                if conflict_group.is_some() {
                    diagnostics.push(Diagnostic {
                        reason_code: ReasonCode::MultipleCanonicalPaths,
                        message: "检测到同一 Agent 的多个独立安装实例，需要用户选择".to_string(),
                    });
                }
                let is_path_default = installation
                    .evidence
                    .iter()
                    .any(|evidence| evidence.is_path_default);
                let (binary_source, upgrade_command) = classify_binary_installation(
                    &installation.canonical_path,
                    &installation.evidence,
                );
                let (modified_at_ms, binary_sha256) = binary_facts(&installation.canonical_path);
                DiscoveryRecord {
                    agent_id: descriptor.agent_id.clone(),
                    executable_path: installation
                        .observed_probe_path
                        .to_string_lossy()
                        .into_owned(),
                    canonical_path: installation.canonical_path.to_string_lossy().into_owned(),
                    binary_source,
                    modified_at_ms,
                    binary_sha256,
                    upgrade_command,
                    version_raw: probe.version_raw,
                    version_normalized: probe.version_normalized,
                    environment: self.environment.platform,
                    evidence: installation.evidence,
                    is_path_default,
                    runnable: probe.runnable,
                    config_candidates: config.paths.clone(),
                    config_fingerprint: config.fingerprint.clone(),
                    conflict_group: conflict_group.clone(),
                    diagnostics,
                    scanned_at_ms,
                }
            })
            .collect()
    }
}

fn classify_binary_installation(
    canonical_path: &Path,
    evidence: &[DiscoveryEvidence],
) -> (BinarySource, Option<String>) {
    if let Some(formula) = homebrew_formula(canonical_path) {
        return (
            BinarySource::Homebrew,
            Some(format!("brew upgrade {formula}")),
        );
    }
    if let Some(package) = npm_package_name(canonical_path) {
        return (
            BinarySource::NpmGlobal,
            Some(format!("npm install --global {package}@latest")),
        );
    }
    if evidence
        .iter()
        .any(|item| item.source == DiscoverySource::PackageManager)
    {
        (BinarySource::MicrosoftStore, None)
    } else if evidence
        .iter()
        .any(|item| item.source == DiscoverySource::Path)
    {
        (BinarySource::Path, None)
    } else if evidence
        .iter()
        .any(|item| item.source == DiscoverySource::EnvOverride)
    {
        (BinarySource::EnvOverride, None)
    } else {
        (BinarySource::KnownPath, None)
    }
}

fn homebrew_formula(canonical_path: &Path) -> Option<String> {
    let mut components = canonical_path.components();
    while let Some(component) = components.next() {
        if component.as_os_str() != "Cellar" {
            continue;
        }
        let formula = components.next()?.as_os_str().to_str()?;
        let safe = !formula.is_empty()
            && formula.len() <= 100
            && formula.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.' | b'@' | b'+')
            });
        return safe.then(|| formula.to_string());
    }
    None
}

fn npm_package_name(canonical_path: &Path) -> Option<String> {
    let components: Vec<_> = canonical_path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    let node_modules = components
        .iter()
        .rposition(|component| component == "node_modules")?;
    let global_layout =
        node_modules > 0 && matches!(components[node_modules - 1].as_str(), "lib" | "npm");
    if !global_layout {
        return None;
    }
    for directory in canonical_path.ancestors().skip(1).take(8) {
        let package_path = directory.join("package.json");
        let Ok(metadata) = std::fs::metadata(&package_path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > 64 * 1024 {
            continue;
        }
        let Ok(bytes) = std::fs::read(package_path) else {
            continue;
        };
        let Ok(package) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(name) = package.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if valid_npm_package_name(name) {
            return Some(name.to_string());
        }
    }
    None
}

fn valid_npm_package_name(name: &str) -> bool {
    fn valid_part(part: &str) -> bool {
        !part.is_empty()
            && part.len() <= 100
            && part.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            })
            && part
                .as_bytes()
                .first()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }

    if let Some(scoped) = name.strip_prefix('@') {
        let mut parts = scoped.split('/');
        matches!((parts.next(), parts.next(), parts.next()), (Some(scope), Some(package), None) if valid_part(scope) && valid_part(package))
    } else {
        !name.contains('/') && valid_part(name)
    }
}

/// Binary hash cache entry. Reuse SHA-256 while path, mtime, and size remain unchanged.
struct CachedBinaryHash {
    modified_at_ms: Option<u64>,
    size: u64,
    sha256: String,
}

/// Process-wide binary hash cache keyed by canonical path. Each `DiscoveryScanner` is new for every scan, so store the cache
/// Persists globally across scans. Almost every Agent button starts a full scan first, and Agent binaries can be
/// hundreds of MB (claude 225 MB, opencode 138 MB). Without caching, every click reads them from start to finish for SHA-256
/// each time; in debug builds this can take several seconds. The cache lets unchanged binaries use stat without rehashing.
static BINARY_HASH_CACHE: LazyLock<Mutex<BTreeMap<PathBuf, CachedBinaryHash>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

fn binary_facts(path: &Path) -> (Option<u64>, Option<String>) {
    let metadata = std::fs::metadata(path).ok();
    let modified_at_ms = metadata
        .as_ref()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| duration.as_millis().try_into().ok());
    let size = metadata.as_ref().map(std::fs::Metadata::len);
    let binary_sha256 = cached_binary_sha256(path, modified_at_ms, size);
    (modified_at_ms, binary_sha256)
}

/// Reuse the global cache when path, mtime, and size match; otherwise hash the file and update the cache.
fn cached_binary_sha256(
    path: &Path,
    modified_at_ms: Option<u64>,
    size: Option<u64>,
) -> Option<String> {
    match BINARY_HASH_CACHE.lock() {
        Ok(mut cache) => lookup_or_hash(&mut cache, path, modified_at_ms, size, hash_file),
        // Do not crash on a poisoned lock; fall back to hashing the file.
        Err(_) => hash_file(path).ok(),
    }
}

/// Pure cache-decision logic with an injected hash function for testing. Reuse the cache when mtime and size match;
/// Otherwise, call `hasher` to recalculate and update the cache. Do not cache when size is unavailable because the file is absent or unreadable.
fn lookup_or_hash(
    cache: &mut BTreeMap<PathBuf, CachedBinaryHash>,
    path: &Path,
    modified_at_ms: Option<u64>,
    size: Option<u64>,
    hasher: impl Fn(&Path) -> std::io::Result<String>,
) -> Option<String> {
    let Some(size) = size else {
        return hasher(path).ok();
    };
    if let Some(entry) = cache.get(path) {
        if entry.size == size && entry.modified_at_ms == modified_at_ms {
            return Some(entry.sha256.clone());
        }
    }
    let sha256 = hasher(path).ok()?;
    cache.insert(
        path.to_path_buf(),
        CachedBinaryHash {
            modified_at_ms,
            size,
            sha256: sha256.clone(),
        },
    );
    Some(sha256)
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

struct Installation {
    canonical_path: PathBuf,
    observed_probe_path: PathBuf,
    evidence: Vec<DiscoveryEvidence>,
    path_orders: Vec<Option<usize>>,
    probe_allowed: bool,
    diagnostics: Vec<Diagnostic>,
}

fn collect_installations(
    candidates: Vec<ExecutableCandidate>,
    platform: Platform,
) -> BTreeMap<String, Installation> {
    let mut installations = BTreeMap::new();
    for candidate in candidates {
        let symlink_metadata = match std::fs::symlink_metadata(&candidate.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => continue,
        };
        let mut diagnostics = Vec::new();
        let canonical_path = match std::fs::canonicalize(&candidate.path) {
            Ok(path) => path,
            Err(_) => {
                diagnostics.push(Diagnostic {
                    reason_code: ReasonCode::ExecutableNotRunnable,
                    message: "可执行入口存在，但无法解析其真实路径".to_string(),
                });
                candidate.path.clone()
            }
        };
        let metadata = std::fs::metadata(&canonical_path).ok();
        let probe_allowed = symlink_metadata.file_type().is_symlink() || symlink_metadata.is_file();
        let probe_allowed = probe_allowed
            && metadata.as_ref().is_some_and(std::fs::Metadata::is_file)
            && executable_permission(metadata.as_ref(), &canonical_path, platform);
        if !probe_allowed && diagnostics.is_empty() {
            diagnostics.push(Diagnostic {
                reason_code: ReasonCode::ExecutableNotRunnable,
                message: "候选入口不是可执行的普通文件".to_string(),
            });
        }

        let identity = path_identity(&canonical_path, platform);
        let evidence = DiscoveryEvidence {
            source: candidate.source,
            observed_path: candidate.path.to_string_lossy().into_owned(),
            is_path_default: false,
        };
        installations
            .entry(identity)
            .and_modify(|installation: &mut Installation| {
                if !installation.evidence.iter().any(|existing| {
                    existing.source == evidence.source
                        && existing.observed_path == evidence.observed_path
                }) {
                    installation.evidence.push(evidence.clone());
                    installation.path_orders.push(candidate.path_order);
                }
                installation.probe_allowed |= probe_allowed;
                installation.diagnostics.extend(diagnostics.clone());
            })
            .or_insert_with(|| Installation {
                canonical_path,
                observed_probe_path: candidate.path,
                evidence: vec![evidence],
                path_orders: vec![candidate.path_order],
                probe_allowed,
                diagnostics,
            });
    }
    for installation in installations.values_mut() {
        let mut combined: Vec<_> = installation
            .evidence
            .drain(..)
            .zip(installation.path_orders.drain(..))
            .collect();
        combined.sort_by(|(left, _), (right, _)| {
            left.source
                .cmp(&right.source)
                .then_with(|| left.observed_path.cmp(&right.observed_path))
        });
        (installation.evidence, installation.path_orders) = combined.into_iter().unzip();
    }
    installations
}

fn mark_path_default(installations: &mut BTreeMap<String, Installation>) {
    let minimum = installations
        .values()
        .flat_map(|installation| installation.path_orders.iter().flatten().copied())
        .min();
    for installation in installations.values_mut() {
        for (evidence, order) in installation
            .evidence
            .iter_mut()
            .zip(&installation.path_orders)
        {
            evidence.is_path_default = evidence.source == DiscoverySource::Path
                && minimum.is_some_and(|minimum| order.is_some_and(|order| order == minimum));
        }
        if let Some(selected) = installation
            .evidence
            .iter()
            .min_by_key(|evidence| {
                (
                    if evidence.is_path_default {
                        0
                    } else if matches!(
                        evidence.source,
                        DiscoverySource::KnownPath | DiscoverySource::PackageManager
                    ) {
                        1
                    } else {
                        2
                    },
                    evidence.observed_path.as_str(),
                )
            })
            .map(|evidence| PathBuf::from(&evidence.observed_path))
        {
            installation.observed_probe_path = selected;
        }
    }
}

fn executable_permission(
    metadata: Option<&std::fs::Metadata>,
    path: &Path,
    platform: Platform,
) -> bool {
    if platform == Platform::Windows {
        return path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "exe" | "com" | "cmd"
                )
            });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_some_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        metadata.is_some()
    }
}

fn unsupported_script_shim(path: &Path, platform: Platform) -> bool {
    platform == Platform::Windows
        && path
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "cmd" | "bat" | "ps1"
                )
            })
}

fn broken_probe(reason_code: ReasonCode, message: &str) -> ProbeOutcome {
    ProbeOutcome {
        runnable: false,
        version_raw: None,
        version_normalized: None,
        diagnostics: vec![Diagnostic {
            reason_code,
            message: message.to_string(),
        }],
    }
}

struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

struct OutputReader {
    receiver: std::sync::mpsc::Receiver<CapturedOutput>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl OutputReader {
    fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    fn finish(mut self, deadline: Instant) -> CapturedOutput {
        let captured = self
            .receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(CapturedOutput {
                bytes: Vec::new(),
                truncated: true,
            });
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        captured
    }
}

impl Drop for OutputReader {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
fn pipe_bytes_available<R: std::os::fd::AsRawFd + ?Sized>(reader: &R) -> std::io::Result<usize> {
    let mut available: libc::c_int = 0;
    // SAFETY: FIONREAD only reads the available-byte count for this valid pipe fd.
    let result = unsafe { libc::ioctl(reader.as_raw_fd(), libc::FIONREAD, &mut available) };
    if result == 0 {
        usize::try_from(available).map_err(|_| std::io::Error::other("invalid pipe byte count"))
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn pipe_bytes_available<R: std::os::windows::io::AsRawHandle + ?Sized>(
    reader: &R,
) -> std::io::Result<usize> {
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let mut available = 0_u32;
    // SAFETY: the handle belongs to a live ChildStdout/ChildStderr pipe and all optional buffers
    // are null; PeekNamedPipe only writes the available-byte count.
    let result = unsafe {
        PeekNamedPipe(
            reader.as_raw_handle(),
            null_mut(),
            0,
            null_mut(),
            &mut available,
            null_mut(),
        )
    };
    if result != 0 {
        Ok(available as usize)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn spawn_output_reader<R>(mut reader: R, limit: usize) -> OutputReader
where
    R: Read + Send + 'static,
    R: PipeHandle,
{
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread = std::thread::spawn(move || {
        let mut retained = Vec::with_capacity(limit.min(8 * 1024));
        let mut buffer = [0_u8; 8 * 1024];
        let mut truncated = false;
        loop {
            let available = match reader.bytes_available() {
                Ok(0) if thread_stop.load(Ordering::Acquire) => break,
                Ok(0) => {
                    std::thread::sleep(PROBE_POLL_INTERVAL);
                    continue;
                }
                Ok(available) => available,
                Err(_) => break,
            };
            let read_limit = available.min(buffer.len());
            match reader.read(&mut buffer[..read_limit]) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let remaining = limit.saturating_sub(retained.len());
                    let kept = remaining.min(read);
                    retained.extend_from_slice(&buffer[..kept]);
                    truncated |= kept < read;
                    if thread_stop.load(Ordering::Acquire) && retained.len() >= limit {
                        truncated = true;
                        break;
                    }
                }
            }
        }
        let _ = sender.send(CapturedOutput {
            bytes: retained,
            truncated,
        });
    });
    OutputReader {
        receiver,
        stop,
        thread: Some(thread),
    }
}

#[cfg(unix)]
trait PipeHandle: std::os::fd::AsRawFd {
    fn bytes_available(&self) -> std::io::Result<usize> {
        pipe_bytes_available(self)
    }
}

#[cfg(unix)]
impl<T: std::os::fd::AsRawFd> PipeHandle for T {}

#[cfg(windows)]
trait PipeHandle: std::os::windows::io::AsRawHandle {
    fn bytes_available(&self) -> std::io::Result<usize> {
        pipe_bytes_available(self)
    }
}

#[cfg(windows)]
impl<T: std::os::windows::io::AsRawHandle> PipeHandle for T {}

fn finish_output_reader(reader: Option<OutputReader>, deadline: Instant) -> CapturedOutput {
    match reader {
        Some(reader) => reader.finish(deadline),
        None => CapturedOutput {
            bytes: Vec::new(),
            truncated: false,
        },
    }
}

fn combine_output(stdout: &[u8], stderr: &[u8], limit: usize) -> CapturedOutput {
    let mut bytes = Vec::with_capacity(limit.min(stdout.len() + stderr.len() + 1));
    let mut truncated = append_bounded(&mut bytes, stdout, limit);
    if !stdout.is_empty() && !stderr.is_empty() && bytes.len() < limit {
        bytes.push(b'\n');
    }
    truncated |= append_bounded(&mut bytes, stderr, limit);
    CapturedOutput { bytes, truncated }
}

fn append_bounded(target: &mut Vec<u8>, input: &[u8], limit: usize) -> bool {
    let remaining = limit.saturating_sub(target.len());
    target.extend_from_slice(&input[..remaining.min(input.len())]);
    input.len() > remaining
}

fn sanitize_output(bytes: &[u8], limit: usize) -> Option<String> {
    let mut value = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>();
    if value.len() > limit {
        let mut boundary = limit;
        while !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        value.truncate(boundary);
    }
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn normalize_version(raw: &str) -> Option<String> {
    raw.split(|character: char| {
        !character.is_ascii_alphanumeric() && !matches!(character, '.' | '-' | '+')
    })
    .filter_map(|token| {
        let candidate = token
            .strip_prefix('v')
            .or_else(|| token.strip_prefix('V'))
            .unwrap_or(token);
        candidate
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_digit)
            .then_some(candidate)
    })
    .find_map(|candidate| Version::parse(candidate).ok())
    .map(|version| version.to_string())
}

struct ConfigInspection {
    paths: Vec<String>,
    fingerprint: Option<String>,
    diagnostics: Vec<Diagnostic>,
}

fn inspect_configs(
    descriptor: &AgentDescriptor,
    environment: &ScanEnvironment,
) -> ConfigInspection {
    let resolution = config_candidates(descriptor, environment);
    let mut diagnostics: Vec<_> = resolution
        .invalid_environment_names
        .into_iter()
        .map(|name| Diagnostic {
            reason_code: ReasonCode::InvalidEnvironmentOverride,
            message: format!("配置路径环境变量 {name} 为空、非 Unicode 或不是绝对路径"),
        })
        .collect();
    if resolution.inline_config_present {
        diagnostics.push(Diagnostic {
            reason_code: ReasonCode::ReadOnlyPreflightFailed,
            message: "检测到 OpenCode 内联配置覆盖；为避免读取敏感内容，未读取该变量值".to_string(),
        });
    }

    let mut paths = Vec::new();
    let mut shapes = Vec::new();
    for candidate in resolution.candidates {
        paths.push(candidate.path.to_string_lossy().into_owned());
        if !candidate.path.exists() {
            continue;
        }
        let metadata = match std::fs::metadata(&candidate.path) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= CONFIG_READ_LIMIT_BYTES => {
                metadata
            }
            Ok(_) | Err(_) => {
                diagnostics.push(Diagnostic {
                    reason_code: ReasonCode::ConfigReadFailed,
                    message: "候选配置不是可安全读取的普通小文件".to_string(),
                });
                continue;
            }
        };
        let _ = metadata;
        match config_shape(&candidate) {
            Ok(shape) => shapes.push(shape),
            Err(reason_code) => diagnostics.push(Diagnostic {
                reason_code,
                message: "候选配置无法完成只读格式指纹解析".to_string(),
            }),
        }
    }
    let fingerprint = (!shapes.is_empty()).then(|| hash_parts(shapes.iter().map(String::as_bytes)));
    ConfigInspection {
        paths,
        fingerprint,
        diagnostics,
    }
}

fn config_shape(candidate: &ResolvedConfigCandidate) -> Result<String, ReasonCode> {
    let mut file =
        std::fs::File::open(&candidate.path).map_err(|_| ReasonCode::ConfigReadFailed)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(CONFIG_READ_LIMIT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReasonCode::ConfigReadFailed)?;
    if bytes.len() as u64 > CONFIG_READ_LIMIT_BYTES {
        return Err(ReasonCode::ConfigReadFailed);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| ReasonCode::ConfigParseFailed)?;
    match candidate.format {
        ConfigFormat::Json => {
            let value: serde_json::Value =
                serde_json::from_str(text).map_err(|_| ReasonCode::ConfigParseFailed)?;
            Ok(format!("json:{}", json_shape(&value)))
        }
        ConfigFormat::Jsonc | ConfigFormat::Json5 => {
            let value: serde_json::Value =
                json_five::from_str(text).map_err(|_| ReasonCode::ConfigParseFailed)?;
            Ok(format!("json:{}", json_shape(&value)))
        }
        ConfigFormat::Toml => {
            let value: toml::Value =
                toml::from_str(text).map_err(|_| ReasonCode::ConfigParseFailed)?;
            Ok(format!("toml:{}", toml_shape(&value)))
        }
        ConfigFormat::Yaml => {
            let document = parse_rendered(text, DocumentFormat::Yaml, "YAML discovery")
                .map_err(|_| ReasonCode::ConfigParseFailed)?;
            let value = semantic_json(&document).map_err(|_| ReasonCode::ConfigParseFailed)?;
            Ok(format!("yaml:{}", json_shape(&value)))
        }
        ConfigFormat::Dotenv => {
            let document = parse_rendered(text, DocumentFormat::Dotenv, "dotenv discovery")
                .map_err(|_| ReasonCode::ConfigParseFailed)?;
            let value = semantic_json(&document).map_err(|_| ReasonCode::ConfigParseFailed)?;
            Ok(format!("dotenv:{}", json_shape(&value)))
        }
    }
}

fn json_shape(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(_) => "bool".to_string(),
        serde_json::Value::Number(_) => "number".to_string(),
        serde_json::Value::String(_) => "string".to_string(),
        serde_json::Value::Array(values) => {
            let shapes = BTreeSet::from_iter(values.iter().map(json_shape));
            format!(
                "array[{}]",
                shapes.into_iter().collect::<Vec<_>>().join(",")
            )
        }
        serde_json::Value::Object(values) => format!(
            "object{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{key}:{}", json_shape(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn toml_shape(value: &toml::Value) -> String {
    match value {
        toml::Value::String(_) => "string".to_string(),
        toml::Value::Integer(_) => "integer".to_string(),
        toml::Value::Float(_) => "float".to_string(),
        toml::Value::Boolean(_) => "bool".to_string(),
        toml::Value::Datetime(_) => "datetime".to_string(),
        toml::Value::Array(values) => {
            let shapes = BTreeSet::from_iter(values.iter().map(toml_shape));
            format!(
                "array[{}]",
                shapes.into_iter().collect::<Vec<_>>().join(",")
            )
        }
        toml::Value::Table(values) => format!(
            "table{{{}}}",
            values
                .iter()
                .map(|(key, value)| format!("{key}:{}", toml_shape(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

fn stable_conflict_group(agent_id: &str, identities: Vec<&str>) -> String {
    hash_parts(
        std::iter::once(agent_id.as_bytes()).chain(identities.into_iter().map(str::as_bytes)),
    )
}

fn hash_parts<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    format!("{:x}", hasher.finalize())
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    #[test]
    fn lookup_or_hash_reuses_cache_until_mtime_or_size_changes() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let hasher = |_: &Path| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, std::io::Error>("sha".to_string())
        };
        let mut cache = BTreeMap::new();
        let path = Path::new("/bin/agent");

        // First lookup misses and hashes once.
        let first = lookup_or_hash(&mut cache, path, Some(100), Some(500), hasher).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // Unchanged mtime and size reuse the cache without hashing.
        let second = lookup_or_hash(&mut cache, path, Some(100), Some(500), hasher).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(first, second);
        // Recalculate when mtime changes.
        lookup_or_hash(&mut cache, path, Some(200), Some(500), hasher).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Recalculate when size changes.
        lookup_or_hash(&mut cache, path, Some(200), Some(600), hasher).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        // An unreadable size disables caching, so every call hashes.
        lookup_or_hash(&mut cache, path, Some(200), None, hasher).unwrap();
        lookup_or_hash(&mut cache, path, Some(200), None, hasher).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }

    struct FixedProbe;

    impl ProbeRunner for FixedProbe {
        fn run(
            &self,
            _executable: &Path,
            _observed_entry: &Path,
            _probe: &VersionProbe,
            _environment: &ScanEnvironment,
        ) -> ProbeOutcome {
            ProbeOutcome {
                runnable: true,
                version_raw: Some("tool 1.2.3".to_string()),
                version_normalized: Some("1.2.3".to_string()),
                diagnostics: Vec::new(),
            }
        }
    }

    #[test]
    fn npm_shim_resolves_only_the_adjacent_packages_declared_bin() {
        let root = scratch("npm-shim");
        let npm = root.join("npm");
        let package = npm.join("node_modules/@google/gemini-cli");
        let entry = package.join("dist/index.js");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(&entry, b"#!/usr/bin/env node\n").unwrap();
        std::fs::write(
            package.join("package.json"),
            br#"{"name":"@google/gemini-cli","bin":{"gemini":"dist/index.js"}}"#,
        )
        .unwrap();
        let shim = npm.join("gemini.cmd");
        std::fs::write(
            &shim,
            br#"@"%~dp0\node.exe" "%~dp0\node_modules\@google\gemini-cli\dist\index.js" %*"#,
        )
        .unwrap();

        assert_eq!(
            resolve_npm_shim_entry(&shim).unwrap(),
            std::fs::canonicalize(&entry).unwrap()
        );

        std::fs::write(
            package.join("package.json"),
            br#"{"name":"@google/gemini-cli","bin":{"gemini":"../outside.js"}}"#,
        )
        .unwrap();
        assert!(resolve_npm_shim_entry(&shim).is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn hermes_version_output_prefers_package_semver_and_yaml_fingerprint_fails_closed() {
        assert_eq!(
            normalize_version("Hermes Agent v0.18.0 (2026.7.1) · upstream 0f102fa4").as_deref(),
            Some("0.18.0")
        );
        assert!(parse_rendered(
            "model:\n  provider: custom\n  provider: openrouter\n",
            DocumentFormat::Yaml,
            "Hermes discovery"
        )
        .is_err());
        assert!(parse_rendered(
            "model:\n  provider: custom\n  broken: [\n",
            DocumentFormat::Yaml,
            "Hermes discovery"
        )
        .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn hermes_discovery_uses_a_local_only_probe_and_accepts_versionless_success() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("hermes-local-probe");
        let executable = root.join(".local/bin/hermes");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(
            &executable,
            b"#!/bin/sh\nif [ \"$1\" = \"--help\" ]; then echo 'usage: hermes'; exit 0; fi\nsleep 5\necho 'Hermes Agent v0.18.0'\n",
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

        let registry = AgentRegistry::builtin().unwrap();
        let mut descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "nous-hermes-agent")
            .unwrap()
            .clone();
        descriptor.version_probe.timeout_ms = 2_000;
        descriptor.version_probe.retry_on_timeout = false;
        let mut context = environment(&root);
        context.path_entries.clear();

        let records =
            DiscoveryScanner::new(context, SystemProbeRunner).scan_descriptor(&descriptor);

        assert_eq!(records.len(), 1);
        assert!(records[0].runnable, "{:?}", records[0].diagnostics);
        assert_eq!(records[0].version_raw, None);
        assert_eq!(records[0].version_normalized, None);
        assert!(records[0].diagnostics.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "token-station-discovery-{name}-{}-{}",
            std::process::id(),
            unix_time_ms()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn environment(root: &Path) -> ScanEnvironment {
        ScanEnvironment {
            platform: Platform::Macos,
            variables: BTreeMap::from([("HOME".to_string(), root.to_string_lossy().into_owned())]),
            path_entries: vec![root.join("bin")],
            present_environment: BTreeSet::new(),
            child_environment: BTreeMap::from([(
                "PATH".to_string(),
                std::env::var("PATH").unwrap_or_default(),
            )]),
        }
    }

    fn tree_manifest(root: &Path) -> BTreeMap<String, String> {
        fn visit(root: &Path, current: &Path, manifest: &mut BTreeMap<String, String>) {
            let mut entries: Vec<_> = std::fs::read_dir(current)
                .unwrap()
                .map(Result::unwrap)
                .collect();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                let metadata = std::fs::symlink_metadata(&path).unwrap();
                let modified = metadata
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos();
                #[cfg(unix)]
                let mode = {
                    use std::os::unix::fs::PermissionsExt;
                    metadata.permissions().mode()
                };
                #[cfg(not(unix))]
                let mode = u32::from(metadata.permissions().readonly());
                if metadata.is_dir() {
                    manifest.insert(relative, format!("dir:{mode:o}:{modified}"));
                    visit(root, &path, manifest);
                } else if metadata.file_type().is_symlink() {
                    manifest.insert(
                        relative,
                        format!(
                            "symlink:{}:{mode:o}:{modified}",
                            std::fs::read_link(&path).unwrap().display()
                        ),
                    );
                } else {
                    let bytes = std::fs::read(&path).unwrap();
                    manifest.insert(
                        relative,
                        format!(
                            "file:{}:{}:{mode:o}:{modified}",
                            bytes.len(),
                            hash_parts(std::iter::once(bytes.as_slice()))
                        ),
                    );
                }
            }
        }

        let mut manifest = BTreeMap::new();
        visit(root, root, &mut manifest);
        manifest
    }

    #[cfg(unix)]
    fn executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"fixture executable").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_deduplicates_canonical_paths_and_tracks_the_path_default() {
        use std::os::unix::fs::symlink;

        let root = scratch("dedup");
        let actual = root.join("actual/claude");
        executable(&actual);
        let path_alias = root.join("bin/claude");
        std::fs::create_dir_all(path_alias.parent().unwrap()).unwrap();
        symlink(&actual, &path_alias).unwrap();

        let registry = AgentRegistry::builtin().unwrap();
        let mut descriptor = registry.descriptors()[0].clone();
        descriptor
            .known_install_locations
            .insert(Platform::Macos, vec![actual.to_string_lossy().into_owned()]);
        let scanner = DiscoveryScanner::new(environment(&root), FixedProbe);

        let records = scanner.scan_descriptor(&descriptor);

        assert_eq!(records.len(), 1);
        assert!(records[0].is_path_default);
        assert_eq!(records[0].executable_path, path_alias.to_string_lossy());
        assert_eq!(records[0].evidence.len(), 2);
        assert_eq!(records[0].version_normalized.as_deref(), Some("1.2.3"));
        assert_eq!(records[0].binary_source, BinarySource::Path);
        assert!(records[0].modified_at_ms.is_some());
        assert_eq!(
            records[0].binary_sha256.as_deref(),
            Some("6f1af2dfc4d7f16dacf404b1f6c9fd4a65cfffb8edde6dcf957463a0e41fb1ed")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_identifies_an_npm_global_install_and_only_returns_a_copyable_upgrade_command() {
        use std::os::unix::fs::symlink;

        let root = scratch("npm-global-source");
        let package = root.join("lib/node_modules/@anthropic-ai/claude-code");
        let actual = package.join("cli.js");
        executable(&actual);
        std::fs::write(
            package.join("package.json"),
            br#"{"name":"@anthropic-ai/claude-code","version":"1.2.3"}"#,
        )
        .unwrap();
        let path_alias = root.join("bin/claude");
        std::fs::create_dir_all(path_alias.parent().unwrap()).unwrap();
        symlink(&actual, &path_alias).unwrap();

        let registry = AgentRegistry::builtin().unwrap();
        let descriptor = registry.descriptors()[0].clone();
        let scanner = DiscoveryScanner::new(environment(&root), FixedProbe);

        let records = scanner.scan_descriptor(&descriptor);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].binary_source, BinarySource::NpmGlobal);
        assert_eq!(
            records[0].upgrade_command.as_deref(),
            Some("npm install --global @anthropic-ai/claude-code@latest")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_does_not_mislabel_a_project_node_modules_binary_as_npm_global() {
        use std::os::unix::fs::symlink;

        let root = scratch("npm-local-source");
        let package = root.join("project/node_modules/@anthropic-ai/claude-code");
        let actual = package.join("cli.js");
        executable(&actual);
        std::fs::write(
            package.join("package.json"),
            br#"{"name":"@anthropic-ai/claude-code","version":"1.2.3"}"#,
        )
        .unwrap();
        let path_alias = root.join("bin/claude");
        std::fs::create_dir_all(path_alias.parent().unwrap()).unwrap();
        symlink(&actual, &path_alias).unwrap();

        let registry = AgentRegistry::builtin().unwrap();
        let descriptor = registry.descriptors()[0].clone();
        let scanner = DiscoveryScanner::new(environment(&root), FixedProbe);

        let records = scanner.scan_descriptor(&descriptor);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].binary_source, BinarySource::Path);
        assert_eq!(records[0].upgrade_command, None);
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_identifies_a_homebrew_cellar_install_without_invoking_brew() {
        use std::os::unix::fs::symlink;

        let root = scratch("homebrew-source");
        let actual = root.join("Cellar/claude-code/1.2.3/bin/claude");
        executable(&actual);
        let path_alias = root.join("bin/claude");
        std::fs::create_dir_all(path_alias.parent().unwrap()).unwrap();
        symlink(&actual, &path_alias).unwrap();

        let registry = AgentRegistry::builtin().unwrap();
        let descriptor = registry.descriptors()[0].clone();
        let scanner = DiscoveryScanner::new(environment(&root), FixedProbe);

        let records = scanner.scan_descriptor(&descriptor);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].binary_source, BinarySource::Homebrew);
        assert_eq!(
            records[0].upgrade_command.as_deref(),
            Some("brew upgrade claude-code")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_env_shebang_runtime_uses_the_observed_entry_sibling_with_a_slim_path() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = scratch("env-shebang-runtime");
        let canonical_script = root.join("lib/node_modules/openclaw/openclaw.mjs");
        std::fs::create_dir_all(canonical_script.parent().unwrap()).unwrap();
        std::fs::write(
            &canonical_script,
            b"#!/usr/bin/env node\nprintf 'OpenClaw 2026.6.11 (fixture)\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&canonical_script, std::fs::Permissions::from_mode(0o700))
            .unwrap();

        let observed_entry = root.join("bin/openclaw");
        std::fs::create_dir_all(observed_entry.parent().unwrap()).unwrap();
        symlink(&canonical_script, &observed_entry).unwrap();
        let sibling_node = root.join("bin/node");
        symlink("/bin/sh", &sibling_node).unwrap();

        let registry = AgentRegistry::builtin().unwrap();
        let mut descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "openclaw")
            .unwrap()
            .clone();
        assert_eq!(descriptor.agent_id, "openclaw");
        descriptor.known_install_locations.insert(
            Platform::Macos,
            vec![observed_entry.to_string_lossy().into_owned()],
        );
        let mut context = environment(&root);
        context.path_entries.clear();
        context.child_environment.insert(
            "PATH".to_string(),
            "/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
        );
        let scanner = DiscoveryScanner::new(context, SystemProbeRunner);

        let records = scanner.scan_descriptor(&descriptor);

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].executable_path, observed_entry.to_string_lossy());
        assert!(records[0].runnable, "{:?}", records[0].diagnostics);
        assert_eq!(records[0].version_normalized.as_deref(), Some("2026.6.11"));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_env_shebang_runtime_rejects_a_mismatched_interpreter() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = scratch("env-shebang-mismatch");
        let canonical_script = root.join("lib/openclaw.mjs");
        std::fs::create_dir_all(canonical_script.parent().unwrap()).unwrap();
        std::fs::write(&canonical_script, b"#!/usr/bin/env python\n").unwrap();
        std::fs::set_permissions(&canonical_script, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let observed_entry = root.join("bin/openclaw");
        std::fs::create_dir_all(observed_entry.parent().unwrap()).unwrap();
        symlink(&canonical_script, &observed_entry).unwrap();
        let sibling_node = root.join("bin/node");
        symlink("/bin/sh", &sibling_node).unwrap();

        let registry = AgentRegistry::builtin().unwrap();
        let mut descriptor = registry
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.agent_id == "openclaw")
            .unwrap()
            .clone();
        descriptor.known_install_locations.insert(
            Platform::Macos,
            vec![observed_entry.to_string_lossy().into_owned()],
        );
        let mut context = environment(&root);
        context.path_entries.clear();
        let scanner = DiscoveryScanner::new(context, SystemProbeRunner);

        let records = scanner.scan_descriptor(&descriptor);

        assert_eq!(records.len(), 1);
        assert!(!records[0].runnable);
        assert!(records[0].diagnostics.iter().any(|diagnostic| {
            diagnostic.reason_code == ReasonCode::ExecutableNotRunnable
                && diagnostic.message.contains("shebang")
        }));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_runtime_known_locations_are_a_fallback_and_conflicts_fail_closed() {
        use std::os::unix::fs::symlink;

        let root = scratch("runtime-known-locations");
        let observed_entry = root.join("bin/openclaw");
        let first = root.join("runtime-a/node");
        let second = root.join("runtime-b/node");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::create_dir_all(second.parent().unwrap()).unwrap();
        symlink("/bin/sh", &first).unwrap();
        symlink("/bin/echo", &second).unwrap();
        let context = environment(&root);
        let candidates = vec!["node".to_string()];
        let sources = vec![
            RuntimeResolutionSource::ObservedEntrySibling,
            RuntimeResolutionSource::KnownInstallLocations,
        ];

        let one = BTreeMap::from([(Platform::Macos, vec![first.to_string_lossy().into_owned()])]);
        let resolved =
            resolve_interpreter(&observed_entry, &candidates, &sources, &one, &context).unwrap();
        assert_eq!(resolved.1, std::fs::canonicalize(&first).unwrap());

        let conflict = BTreeMap::from([(
            Platform::Macos,
            vec![
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ],
        )]);
        assert!(
            resolve_interpreter(&observed_entry, &candidates, &sources, &conflict, &context,)
                .unwrap_err()
                .contains("多个")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_runtime_rejects_script_directory_and_non_executable_impersonators() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = scratch("runtime-native-only");
        let script = root.join("script-node");
        executable(&script);
        assert!(canonical_native_executable(&script, Platform::Macos).is_none());

        let directory = root.join("directory-node");
        std::fs::create_dir_all(&directory).unwrap();
        assert!(canonical_native_executable(&directory, Platform::Macos).is_none());

        let non_executable = root.join("non-executable-node");
        std::fs::copy("/bin/sh", &non_executable).unwrap();
        std::fs::set_permissions(&non_executable, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(canonical_native_executable(&non_executable, Platform::Macos).is_none());

        let native = root.join("native-node");
        symlink("/bin/sh", &native).unwrap();
        assert!(canonical_native_executable(&native, Platform::Macos).is_some());

        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_revalidates_observed_paths_before_spawn() {
        use std::os::unix::fs::symlink;

        let root = scratch("runtime-revalidation");
        let observed = root.join("node");
        symlink("/bin/sh", &observed).unwrap();
        let original = std::fs::canonicalize(&observed).unwrap();
        let command = ResolvedProbeCommand {
            observed_executable: observed.clone(),
            canonical_executable: original.clone(),
            observed_program: observed.clone(),
            canonical_program: original,
            arguments: Vec::new(),
        };
        assert!(probe_command_still_matches(&command));

        std::fs::remove_file(&observed).unwrap();
        symlink("/bin/echo", &observed).unwrap();
        assert!(!probe_command_still_matches(&command));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_probe_path_excludes_inherited_user_directories() {
        let root = scratch("probe-path-allowlist");
        let malicious = root.join("user-bin");
        let mut context = environment(&root);
        context.child_environment.insert(
            "PATH".to_string(),
            std::env::join_paths([malicious.clone(), PathBuf::from("/usr/bin")])
                .unwrap()
                .to_string_lossy()
                .into_owned(),
        );
        let runtime = root.join("runtime/node");

        let path = probe_child_path(&runtime, &context).unwrap();
        let entries: Vec<_> = std::env::split_paths(&path).collect();

        assert_eq!(entries.first(), Some(&root.join("runtime")));
        assert!(entries.contains(&PathBuf::from("/usr/bin")));
        assert!(!entries.contains(&malicious));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_probe_path_fails_closed_when_runtime_parent_cannot_be_encoded() {
        let root = scratch("probe-path-invalid");
        let context = environment(&root);
        let runtime = root.join("bad:path/node");

        assert!(probe_child_path(&runtime, &context).is_err());

        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_multiple_installations_share_a_stable_conflict_group() {
        let root = scratch("multiple");
        let first = root.join("one/claude");
        let second = root.join("two/claude");
        executable(&first);
        executable(&second);
        let registry = AgentRegistry::builtin().unwrap();
        let mut descriptor = registry.descriptors()[0].clone();
        descriptor.known_install_locations.insert(
            Platform::Macos,
            vec![
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ],
        );
        let mut context = environment(&root);
        context.path_entries.clear();
        let scanner = DiscoveryScanner::new(context, FixedProbe);

        let first_scan = scanner.scan_descriptor(&descriptor);
        let second_scan = scanner.scan_descriptor(&descriptor);

        assert_eq!(first_scan.len(), 2);
        assert_eq!(first_scan[0].conflict_group, first_scan[1].conflict_group);
        assert_eq!(first_scan[0].conflict_group, second_scan[0].conflict_group);
        assert!(first_scan.iter().all(|record| record
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.reason_code == ReasonCode::MultipleCanonicalPaths)));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_config_fingerprint_ignores_values_and_scan_is_zero_write() {
        let root = scratch("config-readonly");
        let binary = root.join("bin/claude");
        executable(&binary);
        let config = root.join(".claude/settings.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let original = br#"{"env":{"SECRET":"TS_SECRET_CONFIG"},"enabled":true}"#;
        std::fs::write(&config, original).unwrap();
        let before = std::fs::metadata(&config).unwrap();

        let registry = AgentRegistry::builtin().unwrap();
        let descriptor = registry.descriptors()[0].clone();
        let scanner = DiscoveryScanner::new(environment(&root), FixedProbe);
        let first = scanner.scan_descriptor(&descriptor);

        assert_eq!(std::fs::read(&config).unwrap(), original);
        assert_eq!(
            std::fs::metadata(&config).unwrap().modified().unwrap(),
            before.modified().unwrap()
        );

        std::fs::write(
            &config,
            br#"{"env":{"SECRET":"different"},"enabled":false}"#,
        )
        .unwrap();
        let second = scanner.scan_descriptor(&descriptor);

        assert_eq!(first[0].config_fingerprint, second[0].config_fingerprint);
        assert!(first[0]
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("TS_SECRET_CONFIG")));
        std::fs::write(&config, original).unwrap();
        assert_eq!(std::fs::read(&config).unwrap(), original);
        assert_eq!(std::fs::metadata(&config).unwrap().len(), before.len());
        assert!(!root.join(".codex").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_preserves_the_complete_five_agent_fixture_tree() {
        let root = scratch("five-agent-readonly");
        for executable_name in ["claude", "codex", "hermes", "openclaw", "opencode"] {
            executable(&root.join("bin").join(executable_name));
        }
        for (relative, bytes) in [
            (
                ".claude/settings.json",
                include_bytes!("../../tests/fixtures/discovery/config/claude/settings.json")
                    .as_slice(),
            ),
            (
                ".codex/config.toml",
                include_bytes!("../../tests/fixtures/discovery/config/codex/config.toml")
                    .as_slice(),
            ),
            (
                ".config/opencode/opencode.json",
                include_bytes!("../../tests/fixtures/discovery/config/opencode/opencode.json")
                    .as_slice(),
            ),
            (
                ".openclaw/openclaw.json",
                include_bytes!("../../tests/fixtures/discovery/config/openclaw/openclaw.json")
                    .as_slice(),
            ),
            (
                ".hermes/config.yaml",
                include_bytes!("../../tests/fixtures/discovery/config/hermes/config.yaml")
                    .as_slice(),
            ),
        ] {
            let path = root.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        let before = tree_manifest(&root);
        let registry = AgentRegistry::builtin().unwrap();
        let scanner = DiscoveryScanner::new(environment(&root), FixedProbe);

        let first = scanner.scan_registry(&registry);
        let second = scanner.scan_registry(&registry);

        let discovered = first
            .iter()
            .map(|record| record.agent_id.as_str())
            .collect::<BTreeSet<_>>();
        for fixture_agent in [
            "claude-code",
            "codex",
            "nous-hermes-agent",
            "openclaw",
            "opencode",
        ] {
            assert!(discovered.contains(fixture_agent), "{fixture_agent}");
        }
        assert_eq!(
            first
                .iter()
                .map(|record| (&record.agent_id, &record.config_fingerprint))
                .collect::<Vec<_>>(),
            second
                .iter()
                .map(|record| (&record.agent_id, &record.config_fingerprint))
                .collect::<Vec<_>>()
        );
        assert_eq!(tree_manifest(&root), before);
        assert!(!root.join(".token-station").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn discovery_normalizes_real_agent_version_shapes() {
        for (raw, expected) in [
            ("2.1.211 (Claude Code)", "2.1.211"),
            ("codex-cli 0.145.0-alpha.18", "0.145.0-alpha.18"),
            ("OpenClaw 2026.6.11 (e085fa1)", "2026.6.11"),
            ("Hermes Agent v0.18.0 (2026.7.1)", "0.18.0"),
        ] {
            assert_eq!(normalize_version(raw).as_deref(), Some(expected));
        }
        assert_eq!(normalize_version("claude 9.2.1.211"), None);
    }

    #[test]
    fn discovery_invalid_utf8_raw_output_stays_within_the_byte_limit() {
        let raw = sanitize_output(&[0xff; 64], 16).unwrap();
        assert!(raw.len() <= 16);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_success_without_semver_is_unknown_while_nonzero_is_broken() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("probe-status");
        let executable = root.join("probe-status.sh");
        std::fs::write(
            &executable,
            include_bytes!("../../tests/fixtures/discovery/probe/status.sh"),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let context = environment(&root);
        let probe = |argument: &str| VersionProbe {
            argv: vec![argument.to_string()],
            timeout_ms: 10_000,
            max_output_bytes: 1_024,
            output_matcher: super::super::types::VersionOutputMatcher::SemverAnywhere,
            retry_on_timeout: false,
            runtime: None,
        };

        let unknown = SystemProbeRunner.run(&executable, &executable, &probe("unknown"), &context);
        assert!(unknown.runnable);
        assert_eq!(unknown.version_raw.as_deref(), Some("release-current"));
        assert!(unknown.version_normalized.is_none());
        assert!(unknown.diagnostics.iter().any(|diagnostic| {
            diagnostic.reason_code == ReasonCode::VersionOutputUnparseable
                && !diagnostic.message.contains("release-current")
        }));

        let broken = SystemProbeRunner.run(&executable, &executable, &probe("fail"), &context);
        assert!(!broken.runnable);
        assert!(broken.version_raw.is_none());
        assert!(broken.diagnostics.iter().any(|diagnostic| {
            diagnostic.reason_code == ReasonCode::VersionProbeExitFailure
                && !diagnostic.message.contains("TS_SECRET_STDERR")
        }));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_system_probe_times_out_and_caps_combined_output_without_leaking_it() {
        use std::os::unix::fs::PermissionsExt;

        let root = scratch("system-probe");
        let timeout = root.join("timeout.sh");
        std::fs::write(
            &timeout,
            include_bytes!("../../tests/fixtures/discovery/probe/timeout.sh"),
        )
        .unwrap();
        std::fs::set_permissions(&timeout, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = root.join("oversize.sh");
        std::fs::write(
            &output,
            include_bytes!("../../tests/fixtures/discovery/probe/oversize.sh"),
        )
        .unwrap();
        std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut context = environment(&root);
        context.platform = Platform::Macos;
        let timeout_probe = VersionProbe {
            argv: vec![],
            timeout_ms: 50,
            max_output_bytes: 1024,
            output_matcher: super::super::types::VersionOutputMatcher::SemverAnywhere,
            retry_on_timeout: false,
            runtime: None,
        };
        let timeout_result = SystemProbeRunner.run(&timeout, &timeout, &timeout_probe, &context);
        assert!(!timeout_result.runnable);
        assert!(timeout_result.version_raw.is_none());
        assert!(timeout_result.diagnostics.iter().any(|diagnostic| {
            diagnostic.reason_code == ReasonCode::VersionProbeTimeout
                && !diagnostic.message.contains("TS_SECRET_TIMEOUT")
        }));

        let output_probe = VersionProbe {
            argv: vec![],
            timeout_ms: 10_000,
            max_output_bytes: 1024,
            output_matcher: super::super::types::VersionOutputMatcher::SemverAnywhere,
            retry_on_timeout: false,
            runtime: None,
        };
        let output_result = SystemProbeRunner.run(&output, &output, &output_probe, &context);
        assert!(output_result.runnable, "{:?}", output_result.diagnostics);
        assert!(output_result
            .version_raw
            .as_ref()
            .is_some_and(|raw| raw.len() <= 1024));
        assert!(output_result.diagnostics.iter().any(|diagnostic| {
            diagnostic.reason_code == ReasonCode::VersionOutputTruncated
                && !diagnostic.message.contains("TS_SECRET_OVERSIZE")
        }));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn discovery_timeout_retries_once_and_keeps_a_sanitized_diagnostic() {
        let mut attempts = 0;
        let outcome = run_with_timeout_retry(|| {
            attempts += 1;
            if attempts == 1 {
                broken_probe(
                    ReasonCode::VersionProbeTimeout,
                    "版本探测超时，进程组已终止并回收",
                )
            } else {
                ProbeOutcome {
                    runnable: true,
                    version_raw: Some("tool 1.2.3".to_string()),
                    version_normalized: Some("1.2.3".to_string()),
                    diagnostics: Vec::new(),
                }
            }
        });

        assert!(outcome.runnable, "{:?}", outcome.diagnostics);
        assert_eq!(attempts, 2);
        assert_eq!(outcome.version_normalized.as_deref(), Some("1.2.3"));
        assert!(outcome.diagnostics.iter().any(|diagnostic| {
            diagnostic.reason_code == ReasonCode::VersionProbeTimeout
                && diagnostic.message == "首次版本探测超时，第二次尝试成功"
        }));
        assert!(outcome
            .diagnostics
            .iter()
            .all(|diagnostic| !diagnostic.message.contains("TS_SECRET_TIMEOUT")));
    }

    #[test]
    fn discovery_retry_policy_stops_after_two_timeouts_and_never_retries_nonzero() {
        let mut timeouts = 0;
        let timed_out = run_with_timeout_retry(|| {
            timeouts += 1;
            broken_probe(ReasonCode::VersionProbeTimeout, "timeout")
        });
        assert_eq!(timeouts, 2);
        assert!(!timed_out.runnable);

        let mut failures = 0;
        let failed = run_with_timeout_retry(|| {
            failures += 1;
            broken_probe(ReasonCode::VersionProbeExitFailure, "nonzero")
        });
        assert_eq!(failures, 1);
        assert!(!failed.runnable);
    }

    #[test]
    fn discovery_output_reader_has_a_hard_collection_deadline() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let started = Instant::now();

        let captured = finish_output_reader(
            Some(OutputReader {
                receiver,
                stop: Arc::new(AtomicBool::new(false)),
                thread: None,
            }),
            Instant::now() + Duration::from_millis(20),
        );

        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(captured.truncated);
        assert!(captured.bytes.is_empty());
        drop(sender);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_output_reader_can_cancel_an_open_idle_pipe() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 5"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let reader = spawn_output_reader(stdout, 1024);
        let started = Instant::now();

        reader.stop();
        let captured =
            finish_output_reader(Some(reader), Instant::now() + Duration::from_millis(200));

        assert!(started.elapsed() < Duration::from_millis(200));
        assert!(captured.bytes.is_empty());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_output_reader_timeout_stops_and_drops_the_read_end() {
        use std::os::fd::{AsRawFd, RawFd};

        struct DropObservedReader {
            inner: std::process::ChildStdout,
            dropped: std::sync::mpsc::SyncSender<()>,
        }

        impl Read for DropObservedReader {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.inner.read(buffer)
            }
        }

        impl AsRawFd for DropObservedReader {
            fn as_raw_fd(&self) -> RawFd {
                self.inner.as_raw_fd()
            }
        }

        impl Drop for DropObservedReader {
            fn drop(&mut self) {
                let _ = self.dropped.send(());
            }
        }

        let mut child = Command::new("/bin/sh")
            .args(["-c", "sleep 5"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (dropped, observed) = std::sync::mpsc::sync_channel(1);
        let reader = spawn_output_reader(
            DropObservedReader {
                inner: child.stdout.take().unwrap(),
                dropped,
            },
            1024,
        );

        let captured =
            finish_output_reader(Some(reader), Instant::now() + Duration::from_millis(20));

        assert!(captured.truncated);
        observed
            .recv_timeout(Duration::from_millis(200))
            .expect("timed-out output reader must release its pipe");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    #[ignore = "read-only smoke test against Agents already installed on the current machine"]
    fn discovery_real_platform_probe_is_read_only_and_never_installs() {
        let registry = AgentRegistry::builtin().unwrap();
        let scanner = DiscoveryScanner::from_process(&registry);

        let records = scanner.scan_registry(&registry);

        for record in records {
            assert!(Path::new(&record.executable_path).exists());
            if matches!(record.agent_id.as_str(), "openclaw" | "nous-hermes-agent") {
                assert!(
                    record.runnable,
                    "{}: {:?}",
                    record.agent_id, record.diagnostics
                );
            }
            if record.agent_id == "openclaw" {
                assert!(
                    record.version_normalized.is_some(),
                    "{}: {:?}",
                    record.agent_id,
                    record.diagnostics
                );
            }
            assert!(record.diagnostics.iter().all(|diagnostic| {
                diagnostic.message.len() <= 256
                    && !diagnostic.message.chars().any(|character| {
                        character.is_control() && character != '\n' && character != '\t'
                    })
            }));
        }
    }
}
