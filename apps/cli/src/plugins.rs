//! Plugin discovery: the registry mapping a provider dialect to the package
//! that speaks it (architecture section 12.2, stages B0-B1).
//!
//! Sources, merged in this order:
//!
//! 1. **Builtin packages** — official packages compiled into the binary by
//!    the release pipeline (`--features builtin-plugins`). A plain
//!    `cargo build` has none; an official release binary serves
//!    `openai-compatible` with an empty plugins directory.
//! 2. **Discovered packages** — one subdirectory of `plugins.dir` each
//!    (`manifest.json` + `adapter.wasm`), registered under every provider
//!    dialect their manifest declares. Dropping a package into the directory
//!    is all the registration there is.
//! 3. **Explicit `plugins.providers` entries** — operator intent from the
//!    config file, honored even when the package directory does not exist
//!    yet; the gateway reports the missing package at load, exactly as it
//!    did before discovery existed.
//!
//! Conflicts within a tier are refused, not resolved: two local packages
//! claiming the same dialect, or an explicit entry disagreeing with a
//! discovered manifest, are configuration bugs the operator must see — not
//! races won by scan order. Across tiers the builtin wins: a local package
//! cannot hijack `openai-compatible`, and a shipped layout that carries both
//! the embedded copy and a `plugins-dist/` copy of the same package must
//! start, so the loser is noted in `plugin list` instead of refused.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use token_station_plugin_api::{AdapterKind, AdapterManifest};

use crate::config::PluginsConfig;

/// The builtin tier's raw material. With the feature off the slice is empty
/// and everything below degrades to pure directory discovery.
mod builtin {
    pub(super) struct Package {
        pub manifest_source: &'static str,
        pub wasm: &'static [u8],
    }

    #[cfg(feature = "builtin-plugins")]
    pub(super) const PACKAGES: &[Package] = &[
        Package {
            manifest_source: include_str!(env!("TS_BUILTIN_AGENT_OPENAI_MANIFEST")),
            wasm: include_bytes!(env!("TS_BUILTIN_AGENT_OPENAI_WASM")),
        },
        Package {
            manifest_source: include_str!(env!("TS_BUILTIN_PROVIDER_OPENAI_MANIFEST")),
            wasm: include_bytes!(env!("TS_BUILTIN_PROVIDER_OPENAI_WASM")),
        },
    ];

    #[cfg(not(feature = "builtin-plugins"))]
    pub(super) const PACKAGES: &[Package] = &[];
}

/// Where a package's bytes come from. The loader runs the same gates on both.
#[derive(Debug, Clone)]
pub enum PackageSource {
    /// A directory under `plugins.dir`.
    Dir(PathBuf),
    /// Compiled into this binary (the builtin tier).
    Builtin {
        manifest_source: &'static str,
        wasm: &'static [u8],
    },
}

impl PackageSource {
    fn describe(&self) -> String {
        match self {
            Self::Dir(dir) => dir.display().to_string(),
            Self::Builtin { .. } => "builtin".to_owned(),
        }
    }
}

/// A package the registry knows: its parsed, validated manifest and where its
/// bytes live. Trusted only as far as a manifest can be — the identity gate
/// (manifest vs `metadata()`) still runs when the WASM loads.
#[derive(Debug)]
pub struct DiscoveredPackage {
    pub manifest: AdapterManifest,
    pub source: PackageSource,
}

/// Which tier bound a dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    Discovered,
    Configured,
}

/// Where a provider dialect resolves to.
#[derive(Debug)]
pub struct ProviderBinding {
    /// The package name an operator can `ls` (directory name) or `plugin
    /// list` (builtin manifest name).
    pub package: String,
    pub source: PackageSource,
    pub origin: Origin,
}

/// The provider dialects this installation can speak, and the packages that
/// speak them.
#[derive(Debug)]
pub struct PluginRegistry {
    plugins_dir: PathBuf,
    packages: Vec<DiscoveredPackage>,
    providers: BTreeMap<String, ProviderBinding>,
    /// Same-dialect losers to the builtin tier, kept for `plugin list`.
    shadowed: Vec<String>,
}

impl PluginRegistry {
    /// Seeds the builtin tier, scans `plugins.dir`, and merges the explicit
    /// `plugins.providers` map.
    ///
    /// A missing directory is not an error — it yields the builtin and
    /// explicit entries only, which is every pre-discovery configuration and
    /// every bare official binary.
    ///
    /// # Errors
    ///
    /// A human-readable reason the registry cannot be built: an unreadable or
    /// invalid manifest (named by path), or a same-tier dialect conflict
    /// (naming both claimants). Loud by design — a package that cannot be
    /// described must not silently drop out of the catalog.
    pub fn discover(plugins: &PluginsConfig) -> Result<Self, String> {
        let mut packages = builtin_packages()?;
        packages.extend(scan(&plugins.dir)?);
        let (mut providers, mut shadowed) = bind_declared_dialects(&packages)?;
        merge_explicit_entries(plugins, &packages, &mut providers, &mut shadowed)?;

        Ok(Self {
            plugins_dir: plugins.dir.clone(),
            packages,
            providers,
            shadowed,
        })
    }

    /// The binding serving `dialect`, if any plugin speaks it.
    #[must_use]
    pub fn provider_binding(&self, dialect: &str) -> Option<&ProviderBinding> {
        self.providers.get(dialect)
    }

    /// Every dialect an `upstream add --provider` may name, sorted.
    #[must_use]
    pub fn provider_dialects(&self) -> Vec<&str> {
        self.providers.keys().map(String::as_str).collect()
    }

    /// Resolves the agent package `plugins.agent` names: a builtin agent
    /// package by that name wins; anything else is a directory under
    /// `plugins.dir`, resolvable or not — the loader reports a missing one.
    #[must_use]
    pub fn agent_source(&self, package: &str) -> PackageSource {
        self.packages
            .iter()
            .find(|candidate| {
                matches!(candidate.source, PackageSource::Builtin { .. })
                    && candidate.manifest.kind == AdapterKind::Agent
                    && candidate.manifest.name == package
            })
            .map_or_else(
                || PackageSource::Dir(self.plugins_dir.join(package)),
                |candidate| candidate.source.clone(),
            )
    }

    /// The `plugin list` rendering: dialects first (they are what an operator
    /// configures against), then the known packages, then what got shadowed.
    #[must_use]
    pub fn render_list(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "provider dialects ({}):", self.providers.len());
        for (dialect, binding) in &self.providers {
            let origin = match binding.origin {
                Origin::Builtin => "builtin",
                Origin::Discovered => "discovered",
                Origin::Configured if binding_dir_exists(binding) => "configured",
                Origin::Configured => "configured; package missing",
            };
            let _ = writeln!(
                out,
                "  {dialect} -> {} ({}) [{origin}]",
                binding.package,
                binding.source.describe(),
            );
        }
        let _ = writeln!(
            out,
            "packages ({}; scanned {}):",
            self.packages.len(),
            self.plugins_dir.display(),
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
            let name = match &package.source {
                PackageSource::Builtin { .. } => manifest.name.clone(),
                PackageSource::Dir(dir) => package_dir_name(dir),
            };
            let _ = writeln!(
                out,
                "  {name} {} {kind} ({}) — {bindings}",
                manifest.version,
                package.source.describe(),
            );
        }
        for note in &self.shadowed {
            let _ = writeln!(out, "note: {note}");
        }
        out
    }
}

fn binding_dir_exists(binding: &ProviderBinding) -> bool {
    match &binding.source {
        PackageSource::Dir(dir) => dir.join("manifest.json").is_file(),
        PackageSource::Builtin { .. } => true,
    }
}

/// Parses and validates the compiled-in tier. Fails only on a broken release
/// pipeline; a plain build has nothing to parse.
fn builtin_packages() -> Result<Vec<DiscoveredPackage>, String> {
    let mut packages = Vec::new();
    for package in builtin::PACKAGES {
        let manifest: AdapterManifest = serde_json::from_str(package.manifest_source)
            .map_err(|error| format!("builtin plugin manifest: {error}"))?;
        manifest
            .validate()
            .map_err(|error| format!("builtin plugin manifest `{}`: {error}", manifest.name))?;
        packages.push(DiscoveredPackage {
            manifest,
            source: PackageSource::Builtin {
                manifest_source: package.manifest_source,
                wasm: package.wasm,
            },
        });
    }
    Ok(packages)
}

/// Binds every dialect the given packages' manifests declare. Builtin wins
/// across tiers (noted); a same-tier double claim is refused.
fn bind_declared_dialects(
    packages: &[DiscoveredPackage],
) -> Result<(BTreeMap<String, ProviderBinding>, Vec<String>), String> {
    let mut providers: BTreeMap<String, ProviderBinding> = BTreeMap::new();
    let mut shadowed = Vec::new();
    for package in packages {
        if package.manifest.kind != AdapterKind::Provider {
            continue;
        }
        let (name, origin) = match &package.source {
            PackageSource::Builtin { .. } => (package.manifest.name.clone(), Origin::Builtin),
            PackageSource::Dir(dir) => (package_dir_name(dir), Origin::Discovered),
        };
        for dialect in &package.manifest.providers {
            match providers.get(dialect) {
                None => {
                    providers.insert(
                        dialect.clone(),
                        ProviderBinding {
                            package: name.clone(),
                            source: package.source.clone(),
                            origin,
                        },
                    );
                }
                // The builtin tier is not overridable; the shipped layout
                // (embedded copy + plugins-dist copy) must start.
                Some(existing) if existing.origin == Origin::Builtin => {
                    shadowed.push(format!(
                        "`{dialect}` from package `{name}` is shadowed by the builtin `{}`",
                        existing.package,
                    ));
                }
                Some(existing) => {
                    return Err(format!(
                        "provider dialect `{dialect}` is claimed by two packages, `{}` and \
                         `{name}`; remove one",
                        existing.package,
                    ));
                }
            }
        }
    }
    Ok((providers, shadowed))
}

/// Merges the explicit `plugins.providers` entries over the declared bindings.
fn merge_explicit_entries(
    plugins: &PluginsConfig,
    packages: &[DiscoveredPackage],
    providers: &mut BTreeMap<String, ProviderBinding>,
    shadowed: &mut Vec<String>,
) -> Result<(), String> {
    for (dialect, package) in &plugins.providers {
        let dir = plugins.dir.join(package);
        match providers.get(dialect) {
            Some(existing) if existing.origin == Origin::Builtin => {
                if existing.package != *package {
                    shadowed.push(format!(
                        "plugins.providers entry `{dialect}` -> `{package}` is shadowed by the \
                         builtin `{}`",
                        existing.package,
                    ));
                }
            }
            Some(existing) if existing.package == *package => {}
            Some(existing) => {
                return Err(format!(
                    "plugins.providers maps `{dialect}` to `{package}`, but package `{}` ({}) \
                     already declares that dialect; drop the entry or the package",
                    existing.package,
                    existing.source.describe(),
                ));
            }
            None => {
                // The named package may have been scanned and simply not
                // declare this dialect — that is a lie in the config, not a
                // package to load and find out.
                let scanned_but_silent = packages.iter().any(
                    |candidate| matches!(&candidate.source, PackageSource::Dir(d) if *d == dir),
                );
                if scanned_but_silent {
                    return Err(format!(
                        "plugins.providers maps `{dialect}` to `{package}`, but its manifest ({}) \
                         does not declare that dialect",
                        dir.join("manifest.json").display(),
                    ));
                }
                providers.insert(
                    dialect.clone(),
                    ProviderBinding {
                        package: package.clone(),
                        source: PackageSource::Dir(dir),
                        origin: Origin::Configured,
                    },
                );
            }
        }
    }
    Ok(())
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
            source: PackageSource::Dir(package_dir),
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

    use super::{PackageSource, PluginRegistry};

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

    fn dialect_dir<'registry>(
        registry: &'registry PluginRegistry,
        dialect: &str,
    ) -> Option<&'registry Path> {
        registry
            .provider_binding(dialect)
            .and_then(|binding| match &binding.source {
                PackageSource::Dir(dir) => Some(dir.as_path()),
                PackageSource::Builtin { .. } => None,
            })
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

        // `contains`, not equality: the builtin tier adds its own dialects
        // when the release feature is on.
        let dialects = registry.provider_dialects();
        assert!(
            dialects.contains(&"x") && dialects.contains(&"x-lite"),
            "{dialects:?}"
        );
        assert_eq!(
            dialect_dir(&registry, "x"),
            Some(dir.join("provider-x").as_path())
        );
        // The agent package is listed, not bound to a dialect.
        assert!(registry.render_list().contains("agent-openai"));
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
        assert!(registry.provider_dialects().contains(&"x"));
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
            dialect_dir(&registry, "x"),
            Some(dir.join("provider-x").as_path())
        );
        assert!(registry.render_list().contains("package missing"));
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
        assert!(registry.provider_dialects().contains(&"x"));
    }

    #[test]
    fn an_agent_package_with_no_builtin_resolves_to_its_directory() {
        let dir = scratch("agent-dir");
        let registry =
            PluginRegistry::discover(&plugins(dir.clone(), &[])).expect("empty dir is fine");
        // A name no builtin carries, so this holds with the feature on or off.
        match registry.agent_source("agent-custom") {
            PackageSource::Dir(resolved) => assert_eq!(resolved, dir.join("agent-custom")),
            PackageSource::Builtin { .. } => {
                panic!("no builtin package is named agent-custom")
            }
        }
        fs::remove_dir_all(dir).ok();
    }

    /// With `--features builtin-plugins` (the release pipeline), the official
    /// packages register with an empty plugins directory and win dialect
    /// conflicts. Exercised by the release build, not the default CI run.
    #[cfg(feature = "builtin-plugins")]
    #[test]
    fn the_builtin_tier_registers_and_wins() {
        let dir = scratch("builtin");
        write_package(
            &dir,
            "provider-imposter",
            &provider_manifest("provider-imposter", &["openai-compatible"]),
        );

        let registry = PluginRegistry::discover(&plugins(dir.clone(), &[]))
            .expect("a builtin conflict is shadowed, not refused");
        let binding = registry
            .provider_binding("openai-compatible")
            .expect("the builtin provider registers");
        assert_eq!(binding.package, "provider-openai-compatible");
        assert!(registry.render_list().contains("shadowed"));
        assert!(matches!(
            registry.agent_source("agent-openai"),
            PackageSource::Builtin { .. }
        ));
        fs::remove_dir_all(dir).ok();
    }
}
