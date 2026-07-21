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
//!
//! # Trust (architecture section 12.3, stage B2)
//!
//! Dropping a package into the directory registers it in the catalog, but is
//! not enough to receive traffic. A discovered package binds its dialects
//! only when one of three things vouches for it: it is builtin (shipped
//! inside the signed binary), `plugin install` ran the conformance suite on
//! it and left a [`Receipts`] entry matching its current `adapter.wasm`
//! hash, or the operator wrote it down — an explicit `plugins.providers`
//! entry, or `plugins.allow_unsigned = true` for the whole directory.
//! Receipts live in the data directory, not the package directory, so a
//! downloaded package cannot ship its own approval.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use token_station_plugin_api::{AdapterKind, AdapterManifest};
use token_station_release::sha256_file;

use crate::config::{ClientConfig, PluginsConfig};

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
            manifest_source: include_str!(env!("TS_BUILTIN_AGENT_ANTHROPIC_MANIFEST")),
            wasm: include_bytes!(env!("TS_BUILTIN_AGENT_ANTHROPIC_WASM")),
        },
        Package {
            manifest_source: include_str!(env!("TS_BUILTIN_AGENT_OPENAI_RESPONSES_MANIFEST")),
            wasm: include_bytes!(env!("TS_BUILTIN_AGENT_OPENAI_RESPONSES_WASM")),
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
    /// Builtin, or carries a conformance receipt matching its current
    /// `adapter.wasm`. Unverified packages are catalogued but bind no
    /// dialects unless the operator vouches (see the module docs).
    pub verified: bool,
}

/// What `plugin install` records after a package passes its conformance suite:
/// the hashes the approval applies to. Approval follows bytes, not names — and
/// not just the `adapter.wasm` bytes. The `manifest.json` is bound too, so a
/// package that quietly widens its `permissions` (network, filesystem, a new
/// secret) or changes any other declared behavior invalidates the receipt even
/// when the WASM is byte-for-byte identical.
///
/// A receipt with an empty `manifest_sha256` is a legacy receipt from before
/// this binding existed; it no longer verifies, and the package must be
/// re-installed to earn a full-package approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub sha256: String,
    #[serde(default)]
    pub manifest_sha256: String,
    pub suite: String,
}

/// The conformance receipts, one per installed package, stored under the
/// data directory (`plugin-receipts.json`).
#[derive(Debug)]
pub struct Receipts {
    path: PathBuf,
    entries: BTreeMap<String, Receipt>,
}

impl Receipts {
    /// Reads the receipt file; a missing file is an empty store, which is
    /// every installation that never ran `plugin install`.
    ///
    /// # Errors
    ///
    /// The file exists but cannot be read or parsed — corrupt approvals must
    /// not silently degrade to "nothing is approved".
    pub fn load(data_dir: &Path) -> Result<Self, String> {
        let path = data_dir.join("plugin-receipts.json");
        let entries = match fs::read_to_string(&path) {
            Ok(source) => serde_json::from_str(&source)
                .map_err(|error| format!("{}: {error}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        Ok(Self { path, entries })
    }

    fn matches(&self, package: &str, sha256: &str, manifest_sha256: &str) -> bool {
        self.entries.get(package).is_some_and(|receipt| {
            // Both the WASM and the manifest must match, and a legacy receipt
            // with no bound manifest hash never verifies — the full package is
            // what was approved, not the bytes alone.
            !receipt.manifest_sha256.is_empty()
                && receipt.sha256 == sha256
                && receipt.manifest_sha256 == manifest_sha256
        })
    }

    fn record(&mut self, package: String, receipt: Receipt) {
        self.entries.insert(package, receipt);
    }

    fn forget(&mut self, package: &str) {
        self.entries.remove(package);
    }

    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
        }
        let mut rendered = serde_json::to_string_pretty(&self.entries)
            .map_err(|error| format!("receipts: {error}"))?;
        rendered.push('\n');
        fs::write(&self.path, rendered).map_err(|error| format!("{}: {error}", self.path.display()))
    }
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
    /// What the merge declined to bind and why — builtin shadowing, missing
    /// receipts — kept for `plugin list`.
    notes: Vec<String>,
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
    pub fn discover(plugins: &PluginsConfig, receipts: &Receipts) -> Result<Self, String> {
        let mut packages = builtin_packages()?;
        packages.extend(scan(&plugins.dir, receipts)?);
        let (mut providers, mut notes) = bind_declared_dialects(&packages, plugins.allow_unsigned)?;
        merge_explicit_entries(plugins, &packages, &mut providers, &mut notes)?;

        Ok(Self {
            plugins_dir: plugins.dir.clone(),
            packages,
            providers,
            notes,
        })
    }

    /// [`PluginRegistry::discover`] with the receipts a config's data
    /// directory holds — the composition every caller wants.
    ///
    /// # Errors
    ///
    /// As [`Receipts::load`] and [`PluginRegistry::discover`].
    pub fn for_config(config: &ClientConfig) -> Result<Self, String> {
        let receipts = Receipts::load(&config.data.dir)?;
        Self::discover(&config.plugins, &receipts)
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

    /// Finds a package by the name `plugin info/remove` would use: builtin
    /// manifest name or local directory name.
    #[must_use]
    pub fn package(&self, name: &str) -> Option<&DiscoveredPackage> {
        self.packages.iter().find(|package| match &package.source {
            PackageSource::Builtin { .. } => package.manifest.name == name,
            PackageSource::Dir(dir) => package_dir_name(dir) == name,
        })
    }

    /// The `plugin list` rendering: dialects first (they are what an operator
    /// configures against), then the known packages, then what got declined.
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
            let trust = if package.verified {
                "verified"
            } else {
                "unverified"
            };
            let _ = writeln!(
                out,
                "  {name} {} {kind} ({}) [{trust}] — {bindings}",
                manifest.version,
                package.source.describe(),
            );
        }
        for note in &self.notes {
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
            verified: true,
        });
    }
    Ok(packages)
}

/// Binds every dialect the given packages' manifests declare. Builtin wins
/// across tiers (noted); a same-tier double claim is refused; an unverified
/// package binds nothing unless the operator set `allow_unsigned` (noted).
fn bind_declared_dialects(
    packages: &[DiscoveredPackage],
    allow_unsigned: bool,
) -> Result<(BTreeMap<String, ProviderBinding>, Vec<String>), String> {
    let mut providers: BTreeMap<String, ProviderBinding> = BTreeMap::new();
    let mut shadowed = Vec::new();
    for package in packages {
        if package.manifest.kind != AdapterKind::Provider {
            continue;
        }
        if !package.verified && !allow_unsigned {
            if let PackageSource::Dir(dir) = &package.source {
                shadowed.push(format!(
                    "package `{}` has no conformance receipt; its dialects [{}] are not served \
                     — run `plugin install {}`, or set plugins.allow_unsigned",
                    package_dir_name(dir),
                    package.manifest.providers.join(", "),
                    dir.display(),
                ));
            }
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
                // (A scanned package that does declare it reached here only
                // because it lacks a receipt; the hand-written entry is the
                // operator vouching, so it binds below.)
                let scanned = packages.iter().find(
                    |candidate| matches!(&candidate.source, PackageSource::Dir(d) if *d == dir),
                );
                if scanned.is_some_and(|found| !found.manifest.providers.contains(dialect)) {
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

/// Reads every `<dir>/<package>/manifest.json`, in name order, checking each
/// package's `adapter.wasm` against the receipt store.
fn scan(dir: &Path, receipts: &Receipts) -> Result<Vec<DiscoveredPackage>, String> {
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
        // Approval follows bytes: a receipt for a different (or absent)
        // adapter.wasm or manifest.json vouches for nothing.
        let verified = match (
            sha256_file(&package_dir.join("adapter.wasm")),
            sha256_file(&manifest_path),
        ) {
            (Ok(wasm), Ok(manifest_hash)) => {
                receipts.matches(&package_dir_name(&package_dir), &wasm, &manifest_hash)
            }
            _ => false,
        };
        packages.push(DiscoveredPackage {
            manifest,
            source: PackageSource::Dir(package_dir),
            verified,
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

/// `plugin install`: the explicit confirmation that turns a package on disk
/// into one that may serve traffic. Manifest gate, full conformance suite,
/// copy into `plugins.dir` under the manifest's name, receipt.
///
/// # Errors
///
/// Whatever refused the package first: an unreadable or invalid manifest, a
/// dialect another package already provides, a failing conformance check
/// (the report names each failure), or the filesystem.
pub fn install(config: &ClientConfig, source: &Path) -> Result<String, String> {
    let manifest_path = source.join("manifest.json");
    let manifest_source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: AdapterManifest = serde_json::from_str(&manifest_source)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    token_station_conformance::accepts_manifest(&manifest)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;

    let name = manifest.name.clone();
    let dest = config.plugins.dir.join(&name);
    if dest.exists() {
        return Err(format!(
            "{} already exists; `plugin remove {name}` first",
            dest.display()
        ));
    }

    let mut receipts = Receipts::load(&config.data.dir)?;
    let registry = PluginRegistry::discover(&config.plugins, &receipts)?;
    for dialect in &manifest.providers {
        if let Some(existing) = registry.provider_binding(dialect) {
            // A binding under this package's own name is agreement, not a
            // conflict: a `plugins.providers` entry may pre-declare a package
            // before it is installed (discovery already treats the two
            // agreeing as redundant), and this install is that package
            // arriving. An on-disk package by this name was refused above.
            if existing.package != name {
                return Err(format!(
                    "dialect `{dialect}` is already provided by `{}` ({}); two providers for \
                     one dialect is a conflict",
                    existing.package,
                    existing.source.describe(),
                ));
            }
        }
    }

    let report = run_conformance(source, &manifest)?;
    if !report.is_passing() {
        return Err(format!("conformance refused the package:\n{report}"));
    }

    if let Err(error) = copy_package(source, &dest, &manifest.conformance.fixtures) {
        // A half-copied package must not sit where discovery scans.
        let _ = fs::remove_dir_all(&dest);
        return Err(error);
    }
    let sha256 = sha256_file(&dest.join("adapter.wasm"))
        .map_err(|error| format!("{}: {error}", dest.join("adapter.wasm").display()))?;
    let manifest_sha256 = sha256_file(&dest.join("manifest.json"))
        .map_err(|error| format!("{}: {error}", dest.join("manifest.json").display()))?;
    receipts.record(
        name.clone(),
        Receipt {
            sha256,
            manifest_sha256,
            suite: report.suite().to_owned(),
        },
    );
    receipts.save()?;

    Ok(format!(
        "installed `{name}` {} into {} — {} passed, receipt recorded",
        manifest.version,
        dest.display(),
        report.suite(),
    ))
}

/// `plugin remove`: deletes an installed package and its receipt. Refused
/// while a configured upstream still resolves through it.
///
/// # Errors
///
/// No such package, an upstream still depends on it, or the filesystem.
pub fn remove(config: &ClientConfig, name: &str) -> Result<String, String> {
    let dir = config.plugins.dir.join(name);
    if !dir.join("manifest.json").is_file() {
        return Err(format!(
            "no installed package at {}; `plugin list` shows what exists",
            dir.display()
        ));
    }

    let registry = PluginRegistry::for_config(config)?;
    for (upstream, entry) in &config.upstreams {
        let serves_through_this = registry
            .provider_binding(&entry.provider)
            .is_some_and(|binding| matches!(&binding.source, PackageSource::Dir(d) if *d == dir));
        if serves_through_this {
            return Err(format!(
                "upstream `{upstream}` still speaks `{}` through `{name}`; \
                 `upstream remove {upstream}` first",
                entry.provider
            ));
        }
    }

    fs::remove_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let mut receipts = Receipts::load(&config.data.dir)?;
    receipts.forget(name);
    receipts.save()?;
    Ok(format!("removed `{name}` ({})", dir.display()))
}

/// `plugin info`: one package, in full — for the operator deciding whether
/// to trust, install or remove it.
///
/// # Errors
///
/// No package by that name.
pub fn info(config: &ClientConfig, name: &str) -> Result<String, String> {
    let registry = PluginRegistry::for_config(config)?;
    let package = registry
        .package(name)
        .ok_or_else(|| format!("no package named `{name}`; `plugin list` shows what exists"))?;
    let manifest = &package.manifest;

    let mut out = String::new();
    let _ = writeln!(out, "name: {}", manifest.name);
    let _ = writeln!(out, "version: {}", manifest.version);
    let _ = writeln!(out, "api: {}", manifest.api_version);
    let _ = writeln!(out, "source: {}", package.source.describe());
    let _ = writeln!(
        out,
        "trust: {}",
        if package.verified {
            "verified"
        } else {
            "unverified — `plugin install` it to run conformance"
        }
    );
    match manifest.kind {
        AdapterKind::Provider => {
            let _ = writeln!(out, "kind: provider-adapter");
            let _ = writeln!(out, "providers: {}", manifest.providers.join(", "));
            let _ = writeln!(
                out,
                "secrets: {}",
                if manifest.permissions.secrets.is_empty() {
                    "none".to_owned()
                } else {
                    manifest.permissions.secrets.join(", ")
                }
            );
        }
        AdapterKind::Agent => {
            let _ = writeln!(out, "kind: agent-adapter");
            let _ = writeln!(out, "protocols: {}", manifest.agent_protocols.join(", "));
        }
    }
    let _ = writeln!(out, "suite: {}", manifest.conformance.required_suite);
    if let PackageSource::Dir(dir) = &package.source {
        if let Ok(sha256) = sha256_file(&dir.join("adapter.wasm")) {
            let _ = writeln!(out, "adapter.wasm sha256: {sha256}");
        }
    }
    Ok(out)
}

/// Loads the package from its source directory and runs the suite its
/// manifest requires — the same gates and the same fixtures a third party
/// ran in their CI. `plugin test` (scaffold) shares it, which is the point:
/// there is exactly one suite.
pub(crate) fn run_conformance(
    source: &Path,
    manifest: &AdapterManifest,
) -> Result<token_station_conformance::Report, String> {
    use token_station_conformance::{FixturePack, run_agent_suite, run_provider_suite};
    use token_station_plugin_runtime::{
        AgentPlugin, NoSecrets, PluginRuntime, ProviderPlugin, RuntimeLimits,
    };

    let runtime = PluginRuntime::new(RuntimeLimits::default())
        .map_err(|error| format!("wasm engine: {error}"))?;
    let fixtures = source.join(&manifest.conformance.fixtures);
    let fixture_failure = |error| format!("{}: {error}", fixtures.display());
    let load_failure = |error| format!("{}: {error}", source.display());

    match manifest.kind {
        AdapterKind::Provider => {
            let plugin = ProviderPlugin::load(&runtime, source, NoSecrets).map_err(load_failure)?;
            let pack = FixturePack::load(&fixtures).map_err(fixture_failure)?;
            Ok(run_provider_suite(&plugin, &pack))
        }
        AdapterKind::Agent => {
            let plugin = AgentPlugin::load(&runtime, source).map_err(load_failure)?;
            let pack = FixturePack::load(&fixtures).map_err(fixture_failure)?;
            Ok(run_agent_suite(&plugin, &pack))
        }
    }
}

/// Copies what the runtime and future re-verification need: the manifest,
/// the component, and the fixtures the suite ran on.
fn copy_package(source: &Path, dest: &Path, fixtures_rel: &str) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|error| format!("{}: {error}", dest.display()))?;
    for file in ["manifest.json", "adapter.wasm"] {
        fs::copy(source.join(file), dest.join(file))
            .map_err(|error| format!("{}: {error}", source.join(file).display()))?;
    }
    copy_dir(&source.join(fixtures_rel), &dest.join(fixtures_rel))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|error| format!("{}: {error}", to.display()))?;
    let entries = fs::read_dir(from).map_err(|error| format!("{}: {error}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("{}: {error}", from.display()))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)
                .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        }
    }
    Ok(())
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

    /// The dev-mode config most tests want: unsigned packages allowed, so a
    /// manifest-only package (no wasm, no receipt) still binds. The trust
    /// gate itself is exercised by the `*_receipt_*` tests below.
    fn plugins(dir: PathBuf, providers: &[(&str, &str)]) -> PluginsConfig {
        PluginsConfig {
            dir,
            agent: None,
            agents: vec!["agent-openai".to_owned()],
            providers: providers
                .iter()
                .map(|(dialect, package)| ((*dialect).to_owned(), (*package).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            allow_unsigned: true,
        }
    }

    /// Discovery with an empty receipt store (no data dir).
    fn discover(config: &PluginsConfig) -> Result<PluginRegistry, String> {
        let receipts = super::Receipts::load(Path::new("/nonexistent-token-station-data"))
            .expect("a missing receipt file is an empty store");
        PluginRegistry::discover(config, &receipts)
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

        let registry = discover(&plugins(dir.clone(), &[])).expect("both manifests are valid");

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

        let error = discover(&plugins(dir.clone(), &[]))
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

        let registry = discover(&plugins(dir.clone(), &[("x", "provider-x")]))
            .expect("agreement is not a conflict");
        assert!(registry.provider_dialects().contains(&"x"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_explicit_entry_disagreeing_with_discovery_is_refused() {
        let dir = scratch("disagree");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));

        let error = discover(&plugins(dir.clone(), &[("x", "provider-other")]))
            .expect_err("two answers for one dialect");
        assert!(error.contains("provider-x"), "{error}");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_explicit_entry_its_package_does_not_declare_is_refused() {
        let dir = scratch("lie");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));

        let error = discover(&plugins(dir.clone(), &[("y", "provider-x")]))
            .expect_err("the manifest does not declare `y`");
        assert!(error.contains("does not declare"), "{error}");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_explicit_entry_for_an_absent_package_is_kept_for_the_loader_to_report() {
        let dir = scratch("absent");

        let registry = discover(&plugins(dir.clone(), &[("x", "provider-x")]))
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

        let error = discover(&plugins(dir.clone(), &[]))
            .expect_err("an undescribable package must not drop out silently");
        assert!(error.contains("manifest.json"), "{error}");
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_missing_plugins_dir_yields_the_explicit_entries_only() {
        let dir = std::env::temp_dir().join("token-station-plugins-nowhere");
        let registry =
            discover(&plugins(dir, &[("x", "provider-x")])).expect("a missing dir is not an error");
        assert!(registry.provider_dialects().contains(&"x"));
    }

    #[test]
    fn an_agent_package_with_no_builtin_resolves_to_its_directory() {
        let dir = scratch("agent-dir");
        let registry = discover(&plugins(dir.clone(), &[])).expect("empty dir is fine");
        // A name no builtin carries, so this holds with the feature on or off.
        match registry.agent_source("agent-custom") {
            PackageSource::Dir(resolved) => assert_eq!(resolved, dir.join("agent-custom")),
            PackageSource::Builtin { .. } => {
                panic!("no builtin package is named agent-custom")
            }
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_unverified_package_binds_nothing_by_default() {
        let dir = scratch("trust-gate");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        fs::write(dir.join("provider-x/adapter.wasm"), b"not really wasm")
            .expect("temp dir is writable");
        let mut config = plugins(dir.clone(), &[]);
        config.allow_unsigned = false;

        let registry = discover(&config).expect("unverified is a note, not an error");
        assert!(
            registry.provider_binding("x").is_none(),
            "no receipt, no traffic"
        );
        assert!(registry.render_list().contains("no conformance receipt"));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_receipt_matching_the_wasm_binds_and_a_stale_one_does_not() {
        let dir = scratch("receipts");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        let wasm = dir.join("provider-x/adapter.wasm");
        let manifest = dir.join("provider-x/manifest.json");
        fs::write(&wasm, b"component bytes").expect("temp dir is writable");

        let data = dir.join("data");
        let mut receipts = super::Receipts::load(&data).expect("missing file is empty");
        receipts.record(
            "provider-x".to_owned(),
            super::Receipt {
                sha256: super::sha256_file(&wasm).expect("the file just got written"),
                manifest_sha256: super::sha256_file(&manifest).expect("the manifest exists"),
                suite: "provider-protocol-v1".to_owned(),
            },
        );
        receipts.save().expect("data dir is writable");

        let mut config = plugins(dir.clone(), &[]);
        config.allow_unsigned = false;
        let reloaded = super::Receipts::load(&data).expect("round-trips");
        let registry = PluginRegistry::discover(&config, &reloaded).expect("a receipt vouches");
        assert!(registry.provider_binding("x").is_some());
        assert!(registry.package("provider-x").expect("catalogued").verified);

        // The wasm changes; the approval must not follow the name.
        fs::write(&wasm, b"different bytes").expect("temp dir is writable");
        let registry =
            PluginRegistry::discover(&config, &reloaded).expect("stale is a note, not an error");
        assert!(
            registry.provider_binding("x").is_none(),
            "approval follows bytes"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn changing_the_manifest_invalidates_a_receipt_even_if_the_wasm_is_identical() {
        let dir = scratch("receipts-manifest");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        let wasm = dir.join("provider-x/adapter.wasm");
        let manifest = dir.join("provider-x/manifest.json");
        fs::write(&wasm, b"component bytes").expect("temp dir is writable");

        let data = dir.join("data");
        let mut receipts = super::Receipts::load(&data).expect("missing file is empty");
        receipts.record(
            "provider-x".to_owned(),
            super::Receipt {
                sha256: super::sha256_file(&wasm).expect("wasm exists"),
                manifest_sha256: super::sha256_file(&manifest).expect("manifest exists"),
                suite: "provider-protocol-v1".to_owned(),
            },
        );
        receipts.save().expect("data dir is writable");

        let mut config = plugins(dir.clone(), &[]);
        config.allow_unsigned = false;
        let reloaded = super::Receipts::load(&data).expect("round-trips");

        // Rewrite the manifest to widen the dialects it claims — the WASM is
        // untouched, but the approved package is not this one.
        write_package(
            &dir,
            "provider-x",
            &provider_manifest("provider-x", &["x", "y"]),
        );
        let registry = PluginRegistry::discover(&config, &reloaded)
            .expect("a mismatched manifest is a note, not an error");
        assert!(
            !registry.package("provider-x").expect("catalogued").verified,
            "the receipt binds the whole package, not just the WASM"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_legacy_receipt_without_a_manifest_hash_no_longer_verifies() {
        let dir = scratch("receipts-legacy");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        let wasm = dir.join("provider-x/adapter.wasm");
        fs::write(&wasm, b"component bytes").expect("temp dir is writable");

        let data = dir.join("data");
        let mut receipts = super::Receipts::load(&data).expect("missing file is empty");
        // A receipt from before the manifest binding existed: no manifest hash.
        receipts.record(
            "provider-x".to_owned(),
            super::Receipt {
                sha256: super::sha256_file(&wasm).expect("wasm exists"),
                manifest_sha256: String::new(),
                suite: "provider-protocol-v1".to_owned(),
            },
        );
        receipts.save().expect("data dir is writable");

        let mut config = plugins(dir.clone(), &[]);
        config.allow_unsigned = false;
        let reloaded = super::Receipts::load(&data).expect("round-trips");
        let registry = PluginRegistry::discover(&config, &reloaded).expect("a note, not an error");
        assert!(
            !registry.package("provider-x").expect("catalogued").verified,
            "a partial (WASM-only) approval must be re-earned as a full-package one"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn an_explicit_entry_vouches_for_an_unverified_package() {
        let dir = scratch("vouch");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        let mut config = plugins(dir.clone(), &[("x", "provider-x")]);
        config.allow_unsigned = false;

        let registry = discover(&config).expect("the hand-written entry is the confirmation");
        assert!(registry.provider_binding("x").is_some());
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

        let registry = discover(&plugins(dir.clone(), &[]))
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
