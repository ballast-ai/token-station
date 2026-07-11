//! Plugin discovery: the registry mapping a provider dialect to the package
//! that speaks it (architecture §12.2, stage B0).
//!
//! Sources, merged in this order:
//!
//! 1. **Discovered packages** — one subdirectory of `plugins.dir` each
//!    (`manifest.json` + `adapter.wasm`), registered under every provider
//!    dialect their manifest declares. Dropping a package into the directory
//!    is all the registration there is.
//! 2. **Explicit `plugins.providers` entries** — operator intent from the
//!    config file, honored even when the package directory does not exist
//!    yet; the gateway reports the missing package at load, exactly as it
//!    did before discovery existed.
//!
//! Conflicts are refused, not resolved. Two packages claiming the same
//! dialect, or an explicit entry disagreeing with a discovered manifest, are
//! configuration bugs the operator must see — not races won by scan order.
//! (The builtin tier, whose names nothing may override, arrives with B1.)

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use token_station_plugin_api::{AdapterKind, AdapterManifest};

use crate::config::PluginsConfig;

/// A package found under `plugins.dir`: its parsed, validated manifest and
/// where it lives. Trusted only as far as a manifest can be — the identity
/// gate (manifest vs `metadata()`) still runs when the WASM loads.
#[derive(Debug)]
pub struct DiscoveredPackage {
    pub manifest: AdapterManifest,
    pub dir: PathBuf,
}

/// Where a provider dialect resolves to.
#[derive(Debug)]
pub struct ProviderBinding {
    /// The package directory name (discovered) or the configured package name
    /// (explicit) — the thing an operator can `ls`.
    pub package: String,
    pub dir: PathBuf,
    /// `true` when a scanned manifest declared the dialect; `false` when only
    /// an explicit `plugins.providers` entry did.
    pub discovered: bool,
}

/// The provider dialects this installation can speak, and the packages that
/// speak them.
#[derive(Debug)]
pub struct PluginRegistry {
    packages: Vec<DiscoveredPackage>,
    providers: BTreeMap<String, ProviderBinding>,
}

impl PluginRegistry {
    /// Scans `plugins.dir` and merges the explicit `plugins.providers` map.
    ///
    /// A missing directory is not an error — it yields the explicit entries
    /// only, which is every pre-discovery configuration.
    ///
    /// # Errors
    ///
    /// A human-readable reason the registry cannot be built: an unreadable or
    /// invalid manifest (named by path), or a dialect conflict (naming both
    /// claimants). Loud by design — a package that cannot be described must
    /// not silently drop out of the catalog.
    pub fn discover(plugins: &PluginsConfig) -> Result<Self, String> {
        let packages = scan(&plugins.dir)?;

        let mut providers: BTreeMap<String, ProviderBinding> = BTreeMap::new();
        for package in &packages {
            if package.manifest.kind != AdapterKind::Provider {
                continue;
            }
            let name = package_dir_name(&package.dir);
            for dialect in &package.manifest.providers {
                if let Some(existing) = providers.get(dialect) {
                    return Err(format!(
                        "provider dialect `{dialect}` is claimed by two packages, `{}` and \
                         `{name}`; remove one",
                        existing.package,
                    ));
                }
                providers.insert(
                    dialect.clone(),
                    ProviderBinding {
                        package: name.clone(),
                        dir: package.dir.clone(),
                        discovered: true,
                    },
                );
            }
        }

        for (dialect, package) in &plugins.providers {
            let dir = plugins.dir.join(package);
            match providers.get(dialect) {
                Some(existing) if existing.package == *package => {}
                Some(existing) => {
                    return Err(format!(
                        "plugins.providers maps `{dialect}` to `{package}`, but package `{}` \
                         ({}) already declares that dialect; drop the entry or the package",
                        existing.package,
                        existing.dir.display(),
                    ));
                }
                None => {
                    // The named package may have been scanned and simply not
                    // declare this dialect — that is a lie in the config, not
                    // a package to load and find out.
                    if let Some(found) = packages.iter().find(|candidate| candidate.dir == dir) {
                        return Err(format!(
                            "plugins.providers maps `{dialect}` to `{package}`, but its manifest \
                             ({}) does not declare that dialect",
                            found.dir.join("manifest.json").display(),
                        ));
                    }
                    providers.insert(
                        dialect.clone(),
                        ProviderBinding {
                            package: package.clone(),
                            dir,
                            discovered: false,
                        },
                    );
                }
            }
        }

        Ok(Self {
            packages,
            providers,
        })
    }

    /// The package directory serving `dialect`, if any plugin speaks it.
    #[must_use]
    pub fn provider_dir(&self, dialect: &str) -> Option<&Path> {
        self.providers
            .get(dialect)
            .map(|binding| binding.dir.as_path())
    }

    /// Every dialect an `upstream add --provider` may name, sorted.
    #[must_use]
    pub fn provider_dialects(&self) -> Vec<&str> {
        self.providers.keys().map(String::as_str).collect()
    }

    /// The `plugin list` rendering: dialects first (they are what an operator
    /// configures against), then the discovered packages.
    #[must_use]
    pub fn render_list(&self, scanned: &Path) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "provider dialects ({}):", self.providers.len());
        for (dialect, binding) in &self.providers {
            let origin = if binding.discovered {
                "discovered"
            } else if binding.dir.join("manifest.json").is_file() {
                "configured"
            } else {
                "configured; package missing"
            };
            let _ = writeln!(
                out,
                "  {dialect} -> {} ({}) [{origin}]",
                binding.package,
                binding.dir.display(),
            );
        }
        let _ = writeln!(
            out,
            "packages discovered under {} ({}):",
            scanned.display(),
            self.packages.len()
        );
        for package in &self.packages {
            let manifest = &package.manifest;
            let (kind, bindings) = match manifest.kind {
                AdapterKind::Provider => (
                    "provider-adapter",
                    format!("providers: {}", manifest.providers.join(", ")),
                ),
                AdapterKind::Agent => (
                    "agent-adapter",
                    format!("protocols: {}", manifest.agent_protocols.join(", ")),
                ),
            };
            let _ = writeln!(
                out,
                "  {} {} {kind} — {bindings}",
                package_dir_name(&package.dir),
                manifest.version,
            );
        }
        out
    }
}

/// Reads every `<dir>/<package>/manifest.json`, in name order.
fn scan(dir: &Path) -> Result<Vec<DiscoveredPackage>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut package_dirs: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|error| format!("plugins dir {}: {error}", dir.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("manifest.json").is_file())
        .collect();
    package_dirs.sort();

    let mut packages = Vec::new();
    for package_dir in package_dirs {
        let manifest_path = package_dir.join("manifest.json");
        let source = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        let manifest: AdapterManifest = serde_json::from_str(&source)
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        manifest
            .validate()
            .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
        packages.push(DiscoveredPackage {
            manifest,
            dir: package_dir,
        });
    }
    Ok(packages)
}

fn package_dir_name(dir: &Path) -> String {
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::config::PluginsConfig;

    use super::PluginRegistry;

    fn provider_manifest(name: &str, dialects: &[&str]) -> String {
        serde_json::json!({
            "name": name,
            "version": "1.0.0",
            "kind": "provider-adapter",
            "api_version": "provider-adapter-v1",
            "providers": dialects,
            "capabilities": ["chat"],
            "permissions": { "network": false, "filesystem": false, "secrets": ["provider_api_key"] },
            "conformance": { "required_suite": "provider-protocol-v1", "fixtures": "fixtures/" }
        })
        .to_string()
    }

    fn agent_manifest(name: &str) -> String {
        serde_json::json!({
            "name": name,
            "version": "1.0.0",
            "kind": "agent-adapter",
            "api_version": "agent-adapter-v1",
            "agent_protocols": ["openai-chat-completions"],
            "capabilities": ["chat"],
            "permissions": { "network": false, "filesystem": false, "secrets": [] },
            "conformance": { "required_suite": "agent-protocol-v1", "fixtures": "fixtures/" }
        })
        .to_string()
    }

    fn write_package(dir: &Path, package: &str, manifest: &str) {
        let package_dir = dir.join(package);
        fs::create_dir_all(&package_dir).expect("temp dir is writable");
        fs::write(package_dir.join("manifest.json"), manifest).expect("temp dir is writable");
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "token-station-plugins-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir is writable");
        dir
    }

    fn plugins(dir: PathBuf, providers: &[(&str, &str)]) -> PluginsConfig {
        PluginsConfig {
            dir,
            agent: "agent-openai".to_owned(),
            providers: providers
                .iter()
                .map(|(dialect, package)| ((*dialect).to_owned(), (*package).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    #[test]
    fn discovery_registers_every_declared_dialect() {
        let dir = scratch("discovers");
        write_package(
            &dir,
            "provider-x",
            &provider_manifest("provider-x", &["x", "x-lite"]),
        );
        write_package(&dir, "agent-openai", &agent_manifest("agent-openai"));

        let registry =
            PluginRegistry::discover(&plugins(dir.clone(), &[])).expect("both manifests are valid");

        assert_eq!(registry.provider_dialects(), ["x", "x-lite"]);
        assert_eq!(
            registry.provider_dir("x"),
            Some(dir.join("provider-x").as_path())
        );
        // The agent package is listed, not bound to a dialect.
        assert!(registry.render_list(&dir).contains("agent-openai"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn two_packages_claiming_one_dialect_are_refused() {
        let dir = scratch("conflict");
        write_package(&dir, "provider-a", &provider_manifest("provider-a", &["x"]));
        write_package(&dir, "provider-b", &provider_manifest("provider-b", &["x"]));

        let error = PluginRegistry::discover(&plugins(dir.clone(), &[]))
            .expect_err("the same dialect twice is a conflict, not a race");
        assert!(
            error.contains("provider-a") && error.contains("provider-b"),
            "{error}"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_explicit_entry_matching_discovery_is_redundant_not_conflicting() {
        let dir = scratch("redundant");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));

        let registry = PluginRegistry::discover(&plugins(dir.clone(), &[("x", "provider-x")]))
            .expect("agreement is not a conflict");
        assert_eq!(registry.provider_dialects(), ["x"]);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_explicit_entry_disagreeing_with_discovery_is_refused() {
        let dir = scratch("disagree");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));

        let error = PluginRegistry::discover(&plugins(dir.clone(), &[("x", "provider-other")]))
            .expect_err("two answers for one dialect");
        assert!(error.contains("provider-x"), "{error}");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_explicit_entry_its_package_does_not_declare_is_refused() {
        let dir = scratch("lie");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));

        let error = PluginRegistry::discover(&plugins(dir.clone(), &[("y", "provider-x")]))
            .expect_err("the manifest does not declare `y`");
        assert!(error.contains("does not declare"), "{error}");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_explicit_entry_for_an_absent_package_is_kept_for_the_loader_to_report() {
        let dir = scratch("absent");

        let registry = PluginRegistry::discover(&plugins(dir.clone(), &[("x", "provider-x")]))
            .expect("pre-discovery configs keep working");
        assert_eq!(
            registry.provider_dir("x"),
            Some(dir.join("provider-x").as_path())
        );
        assert!(registry.render_list(&dir).contains("package missing"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_broken_manifest_fails_the_scan_loudly() {
        let dir = scratch("broken");
        write_package(&dir, "provider-x", "{ not json");

        let error = PluginRegistry::discover(&plugins(dir.clone(), &[]))
            .expect_err("an undescribable package must not drop out silently");
        assert!(error.contains("manifest.json"), "{error}");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_missing_plugins_dir_yields_the_explicit_entries_only() {
        let dir = std::env::temp_dir().join("token-station-plugins-nowhere");
        let registry = PluginRegistry::discover(&plugins(dir, &[("x", "provider-x")]))
            .expect("a missing dir is not an error");
        assert_eq!(registry.provider_dialects(), ["x"]);
    }
}
