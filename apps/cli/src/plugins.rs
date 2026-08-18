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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use token_station_plugin_api::{AdapterKind, AdapterManifest, validate_plugin_name};
use token_station_release::{plugin_package_digest, sha256_file};

use crate::config::{ClientConfig, PluginsConfig};

const MAX_PACKAGE_FILES: usize = 10_000;
const MAX_PACKAGE_DEPTH: usize = 16;
const MAX_PACKAGE_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PACKAGE_TOTAL_BYTES: u64 = 128 * 1024 * 1024;

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
            manifest_source: include_str!(env!("TS_BUILTIN_AGENT_GEMINI_MANIFEST")),
            wasm: include_bytes!(env!("TS_BUILTIN_AGENT_GEMINI_WASM")),
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
    /// package digest. Packages without conformance are catalogued but bind no
    /// dialects unless the operator vouches (see the module docs).
    pub conformance_passed: bool,
    /// Publisher identity is a separate claim from local protocol conformance.
    pub publisher_signature_verified: bool,
}

/// What `plugin install` records after a package passes its conformance suite:
/// the canonical recursive package digest the approval applies to. Approval
/// follows every installed byte, not a name or only the component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    #[serde(default)]
    pub package_digest: String,
    pub suite: String,
    #[serde(default)]
    pub publisher_signature_verified: bool,
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

    fn matching(
        &self,
        package: &str,
        package_digest: &str,
        required_suite: &str,
    ) -> Option<&Receipt> {
        self.entries.get(package).filter(|receipt| {
            !receipt.package_digest.is_empty()
                && receipt.package_digest == package_digest
                && receipt.suite == required_suite
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
        crate::private_fs::write_atomic_private(&self.path, rendered.as_bytes())
            .map_err(|error| format!("{}: {error}", self.path.display()))
    }
}

/// Which tier bound a dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    Discovered,
    Configured,
}

/// Whether the exact package bytes bound to a provider dialect passed the
/// local provider conformance suite. Operator permission to load unsigned
/// bytes is deliberately a different state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderPackageVerificationV1 {
    ConformancePassed,
    NotConformanceVerified,
}

/// Where a provider dialect resolves to.
#[derive(Debug)]
pub struct ProviderBinding {
    /// The package name an operator can `ls` (directory name) or `plugin
    /// list` (builtin manifest name).
    pub package: String,
    pub source: PackageSource,
    pub origin: Origin,
    package_verification: ProviderPackageVerificationV1,
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

    /// Returns byte-bound conformance evidence for the package serving a
    /// provider dialect. A configured or globally allowed unsigned package
    /// remains loadable, but is not eligible for the first South slice.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn provider_package_verification(
        &self,
        dialect: &str,
    ) -> Option<ProviderPackageVerificationV1> {
        self.providers
            .get(dialect)
            .map(|binding| binding.package_verification)
    }

    /// True only when `dialect` resolves to the named official package and the
    /// exact bytes carry conformance evidence. The first South slice accepts
    /// only packages embedded by the signed release pipeline. A persisted
    /// publisher claim is not cryptographic identity.
    #[must_use]
    pub fn provider_package_south_approved(&self, dialect: &str, package: &str) -> bool {
        self.providers.get(dialect).is_some_and(|binding| {
            binding.package == package
                && binding.package_verification == ProviderPackageVerificationV1::ConformancePassed
                && binding.origin == Origin::Builtin
        })
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
            let trust = if package.conformance_passed {
                if package.publisher_signature_verified {
                    "conformance-passed; publisher-signature-verified"
                } else {
                    "conformance-passed; publisher-signature-unverified"
                }
            } else {
                "conformance-unverified; publisher-signature-unverified"
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
            conformance_passed: true,
            publisher_signature_verified: true,
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
        if !package.conformance_passed && !allow_unsigned {
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
                            package_verification: if package.conformance_passed {
                                ProviderPackageVerificationV1::ConformancePassed
                            } else {
                                ProviderPackageVerificationV1::NotConformanceVerified
                            },
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
                        package_verification: ProviderPackageVerificationV1::NotConformanceVerified,
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
        // Approval follows the complete recursive package bytes. Legacy
        // receipts without a package digest fail closed and require reinstall.
        let matching_receipt = plugin_package_digest(&package_dir).ok().and_then(|digest| {
            receipts
                .matching(
                    &package_dir_name(&package_dir),
                    &digest,
                    &manifest.conformance.required_suite,
                )
                .cloned()
        });
        packages.push(DiscoveredPackage {
            manifest,
            source: PackageSource::Dir(package_dir),
            conformance_passed: matching_receipt.is_some(),
            publisher_signature_verified: matching_receipt
                .is_some_and(|receipt| receipt.publisher_signature_verified),
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
    fs::create_dir_all(&config.plugins.dir)
        .map_err(|error| format!("{}: {error}", config.plugins.dir.display()))?;
    let plugin_root = fs::canonicalize(&config.plugins.dir)
        .map_err(|error| format!("{}: {error}", config.plugins.dir.display()))?;
    let dest = plugin_root.join(&name);
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

    let mut staging = StagingGuard::create(&plugin_root)?;
    copy_package(
        source,
        staging.path(),
        &manifest.conformance.fixtures,
        &mut CopyBudget::default(),
    )?;
    let staged_manifest_path = staging.path().join("manifest.json");
    let staged_source = fs::read_to_string(&staged_manifest_path)
        .map_err(|error| format!("{}: {error}", staged_manifest_path.display()))?;
    let staged_manifest: AdapterManifest = serde_json::from_str(&staged_source)
        .map_err(|error| format!("{}: {error}", staged_manifest_path.display()))?;
    token_station_conformance::accepts_manifest(&staged_manifest)
        .map_err(|error| format!("{}: {error}", staged_manifest_path.display()))?;
    if staged_manifest != manifest {
        return Err(
            "plugin source changed while it was being staged; retry from stable bytes".into(),
        );
    }

    let package_digest = plugin_package_digest(staging.path())?;
    let report = run_conformance(staging.path(), &staged_manifest)?;
    if !report.is_passing() {
        return Err(format!("conformance refused the package:\n{report}"));
    }

    fs::rename(staging.path(), &dest)
        .map_err(|error| format!("publish staged plugin to `{}`: {error}", dest.display()))?;
    staging.disarm();
    receipts.record(
        name.clone(),
        Receipt {
            package_digest,
            suite: report.suite().to_owned(),
            publisher_signature_verified: false,
        },
    );
    if let Err(error) = receipts.save() {
        let _ = fs::remove_dir_all(&dest);
        return Err(error);
    }

    Ok(format!(
        "installed `{name}` {} into {} — {} conformance passed; publisher signature unverified",
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
    validate_plugin_name(name).map_err(|error| format!("plugin name refused: {error}"))?;
    let plugin_root = fs::canonicalize(&config.plugins.dir)
        .map_err(|error| format!("{}: {error}", config.plugins.dir.display()))?;
    let logical_dir = config.plugins.dir.join(name);
    let metadata = fs::symlink_metadata(&logical_dir)
        .map_err(|error| format!("{}: {error}", logical_dir.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "installed package path `{}` is a symbolic link; refusing removal",
            logical_dir.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "installed package path `{}` is not a directory",
            logical_dir.display()
        ));
    }
    let dir = fs::canonicalize(&logical_dir)
        .map_err(|error| format!("{}: {error}", logical_dir.display()))?;
    if dir.parent() != Some(plugin_root.as_path()) {
        return Err(format!(
            "installed package `{}` does not resolve directly under the configured plugin root",
            logical_dir.display()
        ));
    }
    let manifest_metadata = fs::symlink_metadata(dir.join("manifest.json"))
        .map_err(|error| format!("{}: {error}", dir.join("manifest.json").display()))?;
    if manifest_metadata.file_type().is_symlink() || !manifest_metadata.is_file() {
        return Err(format!(
            "no real installed package manifest at {}; `plugin list` shows what exists",
            dir.display()
        ));
    }

    let registry = PluginRegistry::for_config(config)?;
    for (upstream, entry) in &config.upstreams {
        let serves_through_this =
            registry
                .provider_binding(&entry.provider)
                .is_some_and(|binding| {
                    matches!(
                        &binding.source,
                        PackageSource::Dir(candidate)
                            if fs::canonicalize(candidate).is_ok_and(|resolved| resolved == dir)
                    )
                });
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
        "conformance: {}",
        if package.conformance_passed {
            "passed"
        } else {
            "unverified — `plugin install` it to run conformance"
        }
    );
    let _ = writeln!(
        out,
        "publisher signature: {}",
        if package.publisher_signature_verified {
            "verified"
        } else {
            "unverified"
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
    if let PackageSource::Dir(dir) = &package.source
        && let Ok(sha256) = sha256_file(&dir.join("adapter.wasm"))
    {
        let _ = writeln!(out, "adapter.wasm sha256: {sha256}");
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

#[derive(Default)]
struct CopyBudget {
    files: usize,
    bytes: u64,
}

/// Owns a private, same-filesystem staging directory until publication.
struct StagingGuard {
    path: PathBuf,
    armed: bool,
}

fn private_dir_builder() -> fs::DirBuilder {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    }
    #[cfg(not(unix))]
    {
        fs::DirBuilder::new()
    }
}

impl StagingGuard {
    fn create(plugin_root: &Path) -> Result<Self, String> {
        for _ in 0..16 {
            let mut random = [0_u8; 16];
            getrandom::fill(&mut random)
                .map_err(|error| format!("generate plugin staging name: {error}"))?;
            let suffix = random
                .iter()
                .fold(String::with_capacity(32), |mut output, byte| {
                    let _ = write!(output, "{byte:02x}");
                    output
                });
            let path = plugin_root.join(format!(".install-{suffix}.tmp"));
            let builder = private_dir_builder();
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path, armed: true }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("{}: {error}", path.display())),
            }
        }
        Err("could not allocate a unique plugin staging directory".to_owned())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for StagingGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// Copies exactly the package bytes that will be verified and published.
/// Every source entry is inspected without following links; every regular
/// file is opened no-follow and streamed into a private create-new target.
fn copy_package(
    source: &Path,
    dest: &Path,
    fixtures_rel: &str,
    budget: &mut CopyBudget,
) -> Result<(), String> {
    let source_metadata =
        fs::symlink_metadata(source).map_err(|error| format!("{}: {error}", source.display()))?;
    if source_metadata.file_type().is_symlink() || !source_metadata.is_dir() {
        return Err(format!(
            "plugin source `{}` must be a real directory, not a link or special file",
            source.display()
        ));
    }

    for file in ["manifest.json", "adapter.wasm"] {
        copy_regular_file(&source.join(file), &dest.join(file), budget)?;
    }
    copy_dir(
        &source.join(fixtures_rel),
        &dest.join(fixtures_rel),
        1,
        budget,
    )?;

    let signature = source.join("signature.sig");
    match fs::symlink_metadata(&signature) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "optional signature `{}` must be a real regular file",
                signature.display()
            ));
        }
        Ok(_) => copy_regular_file(&signature, &dest.join("signature.sig"), budget)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("{}: {error}", signature.display())),
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path, depth: usize, budget: &mut CopyBudget) -> Result<(), String> {
    if depth > MAX_PACKAGE_DEPTH {
        return Err(format!(
            "plugin package exceeds the maximum directory depth of {MAX_PACKAGE_DEPTH}"
        ));
    }
    let metadata =
        fs::symlink_metadata(from).map_err(|error| format!("{}: {error}", from.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "plugin package directory `{}` must be a real directory; symbolic links are forbidden",
            from.display()
        ));
    }

    let builder = private_dir_builder();
    builder
        .create(to)
        .map_err(|error| format!("{}: {error}", to.display()))?;

    let mut entries = fs::read_dir(from)
        .map_err(|error| format!("{}: {error}", from.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", from.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let source = entry.path();
        let target = to.join(entry.file_name());
        let file_type = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", source.display()))?;
        if file_type.is_symlink() {
            return Err(format!(
                "plugin package entry `{}` is a symbolic link; symbolic links are forbidden",
                source.display()
            ));
        }
        if file_type.is_dir() {
            copy_dir(&source, &target, depth + 1, budget)?;
        } else if file_type.is_file() {
            copy_regular_file(&source, &target, budget)?;
        } else {
            return Err(format!(
                "plugin package entry `{}` is not a regular file or directory",
                source.display()
            ));
        }
    }
    Ok(())
}

fn copy_regular_file(from: &Path, to: &Path, budget: &mut CopyBudget) -> Result<(), String> {
    let before =
        fs::symlink_metadata(from).map_err(|error| format!("{}: {error}", from.display()))?;
    if before.file_type().is_symlink() || !before.is_file() {
        return Err(format!(
            "plugin package entry `{}` must be a real regular file; symbolic links are forbidden",
            from.display()
        ));
    }
    if before.len() > MAX_PACKAGE_FILE_BYTES {
        return Err(format!(
            "plugin package file `{}` exceeds the {} byte limit",
            from.display(),
            MAX_PACKAGE_FILE_BYTES
        ));
    }
    if budget.files >= MAX_PACKAGE_FILES {
        return Err(format!(
            "plugin package exceeds the maximum of {MAX_PACKAGE_FILES} files"
        ));
    }

    let mut source_options = fs::OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        source_options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        source_options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut source = source_options
        .open(from)
        .map_err(|error| format!("open plugin package file `{}`: {error}", from.display()))?;
    let opened = source
        .metadata()
        .map_err(|error| format!("{}: {error}", from.display()))?;
    if !opened.is_file() {
        return Err(format!(
            "plugin package entry `{}` changed type while being staged",
            from.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != before.dev() || opened.ino() != before.ino() {
            return Err(format!(
                "plugin package entry `{}` changed while being staged",
                from.display()
            ));
        }
        if opened.nlink() != 1 {
            return Err(format!(
                "plugin package entry `{}` has multiple hard links; hard links are forbidden",
                from.display()
            ));
        }
    }

    let mut target = create_private_file(to)?;

    budget.files += 1;
    let mut file_bytes = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| format!("{}: {error}", from.display()))?;
        if read == 0 {
            break;
        }
        file_bytes = file_bytes
            .checked_add(read as u64)
            .ok_or_else(|| "plugin package size overflow".to_owned())?;
        if file_bytes > MAX_PACKAGE_FILE_BYTES {
            return Err(format!(
                "plugin package file `{}` grew beyond the {} byte limit",
                from.display(),
                MAX_PACKAGE_FILE_BYTES
            ));
        }
        budget.bytes = budget
            .bytes
            .checked_add(read as u64)
            .ok_or_else(|| "plugin package size overflow".to_owned())?;
        if budget.bytes > MAX_PACKAGE_TOTAL_BYTES {
            return Err(format!(
                "plugin package exceeds the {MAX_PACKAGE_TOTAL_BYTES} byte total limit"
            ));
        }
        target
            .write_all(&buffer[..read])
            .map_err(|error| format!("{}: {error}", to.display()))?;
    }
    target
        .flush()
        .and_then(|()| target.sync_all())
        .map_err(|error| format!("{}: {error}", to.display()))
}

fn create_private_file(path: &Path) -> Result<fs::File, String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::config::PluginsConfig;

    use super::{PackageSource, PluginRegistry, ProviderPackageVerificationV1};

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
    fn a_receipt_matching_the_package_binds_and_a_stale_one_does_not() {
        let dir = scratch("receipts");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        let wasm = dir.join("provider-x/adapter.wasm");
        fs::write(&wasm, b"component bytes").expect("temp dir is writable");

        let data = dir.join("data");
        let mut receipts = super::Receipts::load(&data).expect("missing file is empty");
        receipts.record(
            "provider-x".to_owned(),
            super::Receipt {
                package_digest: token_station_release::plugin_package_digest(
                    &dir.join("provider-x"),
                )
                .expect("package exists"),
                suite: "provider-protocol-v1".to_owned(),
                publisher_signature_verified: false,
            },
        );
        receipts.save().expect("data dir is writable");

        let mut config = plugins(dir.clone(), &[]);
        config.allow_unsigned = false;
        let reloaded = super::Receipts::load(&data).expect("round-trips");
        let registry = PluginRegistry::discover(&config, &reloaded).expect("a receipt vouches");
        assert!(registry.provider_binding("x").is_some());
        assert_eq!(
            registry.provider_package_verification("x"),
            Some(ProviderPackageVerificationV1::ConformancePassed),
            "a matching full-package receipt is eligible evidence for South"
        );
        assert!(
            registry
                .package("provider-x")
                .expect("catalogued")
                .conformance_passed
        );

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
    fn discovered_receipts_never_prove_official_south_identity() {
        let dir = scratch("south-official-identity");
        write_package(
            &dir,
            "provider-openai-compatible",
            &provider_manifest("provider-openai-compatible", &["forged-openai-compatible"]),
        );
        let package_dir = dir.join("provider-openai-compatible");
        fs::write(package_dir.join("adapter.wasm"), b"component bytes")
            .expect("temp dir is writable");
        let data = dir.join("data");
        let mut receipts = super::Receipts::load(&data).expect("missing file is empty");
        receipts.record(
            "provider-openai-compatible".to_owned(),
            super::Receipt {
                package_digest: token_station_release::plugin_package_digest(&package_dir)
                    .expect("package exists"),
                suite: "provider-protocol-v1".to_owned(),
                publisher_signature_verified: false,
            },
        );
        receipts.save().expect("data dir is writable");
        let reloaded = super::Receipts::load(&data).expect("round-trips");
        let registry = PluginRegistry::discover(&plugins(dir.clone(), &[]), &reloaded)
            .expect("the conformant package remains available to Legacy");

        assert!(
            registry
                .provider_binding("forged-openai-compatible")
                .is_some()
        );
        assert!(
            !registry.provider_package_south_approved(
                "forged-openai-compatible",
                "provider-openai-compatible",
            ),
            "local conformance does not prove official publisher identity"
        );

        receipts.record(
            "provider-openai-compatible".to_owned(),
            super::Receipt {
                package_digest: token_station_release::plugin_package_digest(&package_dir)
                    .expect("package exists"),
                suite: "provider-protocol-v1".to_owned(),
                publisher_signature_verified: true,
            },
        );
        receipts.save().expect("data dir is writable");
        let reloaded = super::Receipts::load(&data).expect("round-trips");
        let registry = PluginRegistry::discover(&plugins(dir.clone(), &[]), &reloaded)
            .expect("verified official package is available");
        assert!(
            !registry.provider_package_south_approved(
                "forged-openai-compatible",
                "provider-openai-compatible",
            ),
            "a persisted publisher claim is not cryptographic identity"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_digest_matching_receipt_for_the_wrong_suite_is_not_conformance_evidence() {
        let dir = scratch("receipt-wrong-suite");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        let package_dir = dir.join("provider-x");
        fs::write(package_dir.join("adapter.wasm"), b"component bytes")
            .expect("temp dir is writable");
        let data = dir.join("data");
        let mut receipts = super::Receipts::load(&data).expect("missing file is empty");
        receipts.record(
            "provider-x".to_owned(),
            super::Receipt {
                package_digest: token_station_release::plugin_package_digest(&package_dir)
                    .expect("package exists"),
                suite: "some-other-suite".to_owned(),
                publisher_signature_verified: false,
            },
        );
        receipts.save().expect("data dir is writable");
        let mut config = plugins(dir.clone(), &[("x", "provider-x")]);
        config.allow_unsigned = false;

        let reloaded = super::Receipts::load(&data).expect("receipt reloads");
        let registry = PluginRegistry::discover(&config, &reloaded)
            .expect("operator entry may still bind the package");

        assert_eq!(
            registry.provider_package_verification("x"),
            Some(ProviderPackageVerificationV1::NotConformanceVerified),
            "suite identity is part of conformance evidence"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn changing_the_manifest_invalidates_a_receipt_even_if_the_wasm_is_identical() {
        let dir = scratch("receipts-manifest");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        let wasm = dir.join("provider-x/adapter.wasm");
        fs::write(&wasm, b"component bytes").expect("temp dir is writable");

        let data = dir.join("data");
        let mut receipts = super::Receipts::load(&data).expect("missing file is empty");
        receipts.record(
            "provider-x".to_owned(),
            super::Receipt {
                package_digest: token_station_release::plugin_package_digest(
                    &dir.join("provider-x"),
                )
                .expect("package exists"),
                suite: "provider-protocol-v1".to_owned(),
                publisher_signature_verified: false,
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
            !registry
                .package("provider-x")
                .expect("catalogued")
                .conformance_passed,
            "the receipt binds the whole package, not just the WASM"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_legacy_receipt_without_a_package_digest_no_longer_verifies() {
        let dir = scratch("receipts-legacy");
        write_package(&dir, "provider-x", &provider_manifest("provider-x", &["x"]));
        let wasm = dir.join("provider-x/adapter.wasm");
        fs::write(&wasm, b"component bytes").expect("temp dir is writable");

        let data = dir.join("data");
        let mut receipts = super::Receipts::load(&data).expect("missing file is empty");
        // A receipt from before recursive package binding: no package digest.
        receipts.record(
            "provider-x".to_owned(),
            super::Receipt {
                package_digest: String::new(),
                suite: "provider-protocol-v1".to_owned(),
                publisher_signature_verified: false,
            },
        );
        receipts.save().expect("data dir is writable");

        let mut config = plugins(dir.clone(), &[]);
        config.allow_unsigned = false;
        let reloaded = super::Receipts::load(&data).expect("round-trips");
        let registry = PluginRegistry::discover(&config, &reloaded).expect("a note, not an error");
        assert!(
            !registry
                .package("provider-x")
                .expect("catalogued")
                .conformance_passed,
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
        assert_eq!(
            registry.provider_package_verification("x"),
            Some(ProviderPackageVerificationV1::NotConformanceVerified),
            "operator permission to load is not South conformance evidence"
        );
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
