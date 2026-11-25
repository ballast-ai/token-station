import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const southRepository = "https://github.com/ballast-ai/token-station-south.git";
const southRevision = "174158d1abfc842cc47dc88c9c6ef0c18c0e4d2f";
const southRelease = "0.85.1";
// South versions its crates independently: a crate whose public surface did
// not change keeps its version across a release. One revision still pins all
// of them, so each per-crate version is an exact expectation of that revision.
const southPackageVersions = new Map([
  ["south-component-conformance", "0.81.1"],
  ["south-contracts", "0.57.0"],
  ["south-core", southRelease],
  ["south-host-grammars", southRelease],
  ["south-provider-api", "0.65.0"],
  ["south-provider-conformance", southRelease],
  ["south-provider-runtime", southRelease],
  ["south-testkit", southRelease],
  ["south-transport-reqwest", southRelease],
]);
// The kernel contracts of protocol 0.7.0, as the kernel mirror at
// kernelRevision records them. The host and every range-era manifest must
// declare exactly these.
const kernelContracts = { canonical_ir: 5, error_catalog: 1, stream: 3 };
const southSource = `git+${southRepository}?rev=${southRevision}#${southRevision}`;
const kernelRepository = "https://github.com/ballast-ai/token-station-kernel.git";
// South 0.85.1 resolves the kernel protocol crate at this revision. It is the
// one-way mirror of this repository's crates/protocol 0.7.0.
const kernelRevision = "ce6894d99f51283e4985d39a2d533307bda27b6a";
const kernelVersion = "0.7.0";
// The git tree hash of crates/protocol that the kernel mirror at
// kernelRevision records (compatibility.json mirror.subtrees).
const kernelProtocolTree = "0b5099bb04a24e59773e785a9919562257519105";
// Since South 0.16.0 (south #45), South pins the kernel protocol crate by
// revision rather than by tag. A tag can be moved onto different bytes and a
// revision cannot. Require the stricter form rather than accepting either.
const kernelSource = `git+${kernelRepository}?rev=${kernelRevision}#${kernelRevision}`;
// The nine crates the root manifest declares directly.
const declaredSouthPackages = new Set([
  "south-component-conformance",
  "south-contracts",
  "south-core",
  "south-host-grammars",
  "south-provider-api",
  "south-provider-conformance",
  "south-provider-runtime",
  "south-testkit",
  "south-transport-reqwest",
]);
// The resolved closure is exactly the declared set.
const expectedSouthPackages = new Set(declaredSouthPackages);
const expectedDesktopSouthPackages = new Set([
  "south-component-conformance",
  "south-contracts",
  "south-core",
  "south-host-grammars",
  "south-provider-api",
  "south-provider-runtime",
  "south-transport-reqwest",
]);

const rootManifest = fs.readFileSync(path.join(root, "Cargo.toml"), "utf8");
const runtimeManifest = fs.readFileSync(
  path.join(root, "crates/plugin-runtime/Cargo.toml"),
  "utf8",
);
const ciWorkflow = fs.readFileSync(
  path.join(root, ".github/workflows/full-ci.yml"),
  "utf8",
);
const cargoGitConfigPath = path.join(root, ".cargo/config.toml");
const southAccessActionPaths = [
  ".github/actions/setup-south-access/action.yml",
  ".github/actions/cleanup-south-access/action.yml",
];
const workflowsDirectory = path.join(root, ".github/workflows");

function isSouthPackage(pkg) {
  return (
    pkg.name.startsWith("south-") || pkg.source?.includes(southRepository) === true
  );
}

assert.equal(isSouthPackage({ name: "south-foreign", source: null }), true);
assert.equal(isSouthPackage({ name: "renamed-package", source: southSource }), true);
assert.equal(isSouthPackage({ name: "unrelated", source: null }), false);

function validateSouthClosure(metadata, expectedPackages, scope) {
  const packages = metadata.packages.filter(isSouthPackage);
  assert.equal(
    packages.length,
    expectedPackages.size,
    `${scope} must resolve exactly one instance of every approved South package`,
  );
  assert.deepEqual(
    new Set(packages.map((pkg) => pkg.name)),
    expectedPackages,
    `${scope} South closure must contain only the approved package names`,
  );
  for (const pkg of packages) {
    assert.equal(
      pkg.version,
      southPackageVersions.get(pkg.name),
      `${scope} ${pkg.name} must stay on its South ${southRelease} version`,
    );
    assert.equal(
      pkg.source,
      southSource,
      `${scope} ${pkg.name} must resolve from the one pinned South source and revision`,
    );
  }
  return packages;
}

assert.match(
  rootManifest,
  /^rust-version = "1\.96"$/m,
  "the workspace MSRV must match South's Rust 1.96 baseline",
);
assert.match(
  runtimeManifest,
  /^rust-version = "1\.96"$/m,
  "the standalone plugin runtime MSRV must match the workspace",
);
assert.doesNotMatch(
  ciWorkflow,
  /1\.95(?:\.0)?|msrv-1\.95/,
  "CI must not retain a stale Rust 1.95 MSRV promise",
);
assert.match(
  ciWorkflow,
  /dtolnay\/rust-toolchain@1\.96\.0/,
  "CI must execute the declared Rust 1.96.0 minimum-version check",
);

for (const packageName of declaredSouthPackages) {
  // The pinned trio, in order, is what this gate exists for: an exact version,
  // the official repository, and a commit revision. It used to require the
  // whole line to match verbatim, which also forbade a `features` list — so
  // enabling a feature looked identical to unpinning the revision. The
  // requirement is that the pin is there and nothing else on the line
  // relaxes it, not that the line is one exact string.
  const pinned = `${packageName} = { version = "=${southPackageVersions.get(packageName)}", git = "${southRepository}", rev = "${southRevision}"`;
  const declaration = rootManifest
    .split("\n")
    .find((line) => line.startsWith(`${packageName} = `));
  assert.ok(
    declaration !== undefined && declaration.startsWith(pinned),
    `${packageName} must use an exact manifest version and revision`,
  );
  const tail = declaration.slice(pinned.length);
  assert.match(
    tail,
    /^(, features = \[[^\]]*\])? \}$/,
    `${packageName} may add only a features list after its pin, found: ${tail}`,
  );
  // `path` or `branch` would defeat the pin while keeping the prefix intact.
  for (const escape of ["path", "branch", "tag", "default-features"]) {
    assert.doesNotMatch(
      declaration,
      new RegExp(`\\b${escape}\\s*=`),
      `${packageName} must not use \`${escape}\`, which would undo the revision pin`,
    );
  }
}

for (const relativePath of southAccessActionPaths) {
  assert.equal(
    fs.existsSync(path.join(root, relativePath)),
    false,
    `${relativePath} is forbidden now that South is public`,
  );
}
const cargoGitConfig = fs.existsSync(cargoGitConfigPath)
  ? fs.readFileSync(cargoGitConfigPath, "utf8")
  : "";
assert.equal(
  cargoGitConfig.includes("git-fetch-with-cli"),
  false,
  "public South dependencies must not force credential-aware CLI git fetches",
);
const southWorkflows = fs
  .readdirSync(workflowsDirectory)
  .filter((name) => name.endsWith(".yml") || name.endsWith(".yaml"))
  .map((name) => fs.readFileSync(path.join(workflowsDirectory, name), "utf8"))
  .join("\n");
for (const forbiddenFragment of [
  "setup-south-access",
  "cleanup-south-access",
  "SOUTH_READER_APP_ID",
  "SOUTH_READER_APP_PRIVATE_KEY",
  "actions/create-github-app-token",
]) {
  assert.equal(
    southWorkflows.includes(forbiddenFragment),
    false,
    `public South workflows must not contain '${forbiddenFragment}'`,
  );
}

const metadata = JSON.parse(
  execFileSync(
    "cargo",
    ["metadata", "--locked", "--format-version", "1"],
    { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  ),
);
const southPackages = validateSouthClosure(
  metadata,
  expectedSouthPackages,
  "the root workspace",
);

const desktopMetadata = JSON.parse(
  execFileSync(
    "cargo",
    [
      "metadata",
      "--locked",
      "--format-version",
      "1",
      "--manifest-path",
      "apps/desktop/src-tauri/Cargo.toml",
    ],
    { cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  ),
);
const desktopSouthPackages = validateSouthClosure(
  desktopMetadata,
  expectedDesktopSouthPackages,
  "the Desktop workspace",
);

const rootGitPackages = metadata.packages.filter((pkg) =>
  pkg.source?.startsWith("git+"),
);
for (const pkg of rootGitPackages) {
  assert.ok(
    pkg.source === southSource || pkg.source === kernelSource,
    `${pkg.name} must resolve from an exact approved Git source and commit`,
  );
}
// The root workspace patches the kernel source to crates/protocol, so South
// and the host share one protocol type set. No package may still resolve
// from the kernel git source.
assert.equal(
  rootGitPackages.filter((pkg) => pkg.source === kernelSource).length,
  0,
  "the root workspace must patch the kernel protocol crate to crates/protocol",
);
assert.match(
  rootManifest,
  new RegExp(
    `^\\[patch\\."${kernelRepository.replaceAll(".", "\\.")}"\\]\\ntoken-station-protocol = \\{ path = "crates/protocol" \\}$`,
    "m",
  ),
  "the root manifest must patch the kernel protocol crate to crates/protocol",
);
// The Desktop workspace builds the host as a path dependency. It needs the
// same patch, or south-component-conformance would bring a second protocol
// type set into the host build.
const desktopManifest = fs.readFileSync(
  path.join(root, "apps/desktop/src-tauri/Cargo.toml"),
  "utf8",
);
assert.match(
  desktopManifest,
  new RegExp(
    `^\\[patch\\."${kernelRepository.replaceAll(".", "\\.")}"\\]\\ntoken-station-protocol = \\{ path = "\\.\\./\\.\\./\\.\\./crates/protocol" \\}$`,
    "m",
  ),
  "the Desktop manifest must patch the kernel protocol crate to crates/protocol",
);
const desktopProtocol = desktopMetadata.packages.filter(
  (pkg) => pkg.name === "token-station-protocol",
);
assert.equal(
  desktopProtocol.length,
  1,
  "the Desktop workspace must resolve one protocol crate",
);
assert.equal(
  desktopProtocol[0].source,
  null,
  "the Desktop protocol crate must be the local path crate",
);
const localProtocol = metadata.packages.filter(
  (pkg) => pkg.name === "token-station-protocol",
);
assert.equal(localProtocol.length, 1, "the root workspace must resolve one protocol crate");
assert.equal(localProtocol[0].source, null, "the protocol crate must be the local path crate");
assert.equal(
  localProtocol[0].version,
  kernelVersion,
  `the protocol crate must stay on ${kernelVersion}`,
);
// The patch is safe only while crates/protocol is byte-identical to the
// mirror revision South compiles against. The kernel records the upstream git
// tree hash of the subtree it mirrors (compatibility.json mirror.subtrees at
// kernelRevision), so equal tree hashes mean equal bytes. A local IR change
// must go out through a kernel-v* tag, the mirror and a South release first.
const git = (...args) =>
  execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
assert.equal(
  git("rev-parse", "HEAD:crates/protocol"),
  kernelProtocolTree,
  "crates/protocol must equal the kernel mirror South pins; release a kernel-v* tag, sync the mirror and repin South first",
);
assert.equal(
  git("status", "--porcelain", "--", "crates/protocol"),
  "",
  "crates/protocol must have no uncommitted change while it is patched into South",
);

const workspaceMemberIds = new Set(metadata.workspace_members);
const nodeById = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
for (const pkg of southPackages) {
  const node = nodeById.get(pkg.id);
  assert.ok(node, `${pkg.name} must have a resolved dependency node`);
  // The patched protocol crate is the kernel South declares, not a host
  // crate, so it is the one workspace member South may reach.
  const reverseHostDependencies = node.deps
    .map((dependency) => dependency.pkg)
    .filter(
      (packageId) =>
        workspaceMemberIds.has(packageId) && packageId !== localProtocol[0].id,
    );
  assert.deepEqual(
    reverseHostDependencies,
    [],
    `${pkg.name} must not depend back on the community host workspace`,
  );
}

const cli = metadata.packages.find((pkg) => pkg.name === "token-station-cli");
assert.ok(cli, "token-station-cli must be present in workspace metadata");
const cliSouthDependencies = cli.dependencies
  .filter((dependency) => declaredSouthPackages.has(dependency.name))
  .map((dependency) => ({
    name: dependency.name,
    kind: dependency.kind ?? "normal",
    features: [...dependency.features].sort(),
  }))
  .sort((left, right) =>
    `${left.name} ${left.kind}`.localeCompare(`${right.name} ${right.kind}`),
  );
assert.deepEqual(
  cliSouthDependencies,
  [
    // Descriptor auth admission only. The sandboxed seam stays test-only.
    { name: "south-component-conformance", kind: "dev", features: ["sandbox"] },
    { name: "south-component-conformance", kind: "normal", features: [] },
    { name: "south-contracts", kind: "normal", features: [] },
    { name: "south-core", kind: "normal", features: [] },
    // The SSE decoder the server also cuts upstream frames with.
    { name: "south-host-grammars", kind: "normal", features: [] },
    { name: "south-provider-api", kind: "normal", features: [] },
    { name: "south-provider-conformance", kind: "dev", features: [] },
    { name: "south-provider-runtime", kind: "normal", features: [] },
    { name: "south-testkit", kind: "dev", features: [] },
    { name: "south-transport-reqwest", kind: "normal", features: [] },
  ],
  "the CLI must keep testkit and the sandboxed conformance seam test-only, and use seven South runtime dependencies",
);

const policyPackageIds = new Set([
  ...metadata.workspace_members,
  ...southPackages.map((pkg) => pkg.id),
]);
const reqwestOwners = metadata.packages
  .filter(
    (pkg) =>
      policyPackageIds.has(pkg.id) &&
      pkg.dependencies.some((dependency) => dependency.name === "reqwest"),
  )
  .map((pkg) => pkg.name)
  .sort();
assert.deepEqual(
  reqwestOwners,
  ["south-transport-reqwest"],
  "only the dedicated South transport may directly own reqwest",
);

// The host declares what it admits, and South's loader judges every component
// against that declaration. Nothing in the dependency graph carries it, so it is
// a set of constants, and a constant nobody checks is what let the tuple go
// unenforced for three releases. Close the loop here: the declaration must
// match the pinned South release, and every shipped manifest must fall inside
// it.
const southComponentSource = fs.readFileSync(
  path.join(root, "apps/cli/src/south_component.rs"),
  "utf8",
);
const declaredConstant = (constant) =>
  southComponentSource.match(
    new RegExp(`^(?:pub\\(crate\\) )?const ${constant}: &str = "([^"]+)";$`, "m"),
  )?.[1];
const triple = (version) => version.split(".").map(Number);
const compareVersions = (left, right) => {
  const [a, b] = [triple(left), triple(right)];
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index];
  }
  return 0;
};

assert.equal(
  declaredConstant("SOUTH_RUNTIME"),
  southRelease,
  "the host's SOUTH_RUNTIME must be the South release it links",
);
const runtimeMin = declaredConstant("SOUTH_RUNTIME_MIN");
assert.ok(runtimeMin, "the host must declare SOUTH_RUNTIME_MIN");
const declaredKernelContracts = Object.fromEntries(
  [...(southComponentSource.match(/const KERNEL_CONTRACTS[^=]*=\s*\[([^\]]*)\];/s)?.[1] ?? "")
    .matchAll(/\("([a-z_]+)",\s*(\d+)\)/g)]
    .map(([, name, version]) => [name, Number(version)]),
);
assert.deepEqual(
  declaredKernelContracts,
  kernelContracts,
  "the host's KERNEL_CONTRACTS must equal the kernel mirror's contracts",
);

let oldestRangeRuntime;
for (const { dir: componentDir } of JSON.parse(
  fs.readFileSync(path.join(root, "plugins/official/packages.json"), "utf8"),
).packages.filter((entry) => entry.kind === "south-component")) {
  const manifest = JSON.parse(
    fs.readFileSync(
      path.join(root, "plugins/official", componentDir, "manifest.json"),
      "utf8",
    ),
  );
  const compatibility = manifest.compatibility;
  assert.equal(
    compatibility.runtime_abi,
    1,
    `${componentDir} must declare runtime ABI 1; a package without one predates the range handshake`,
  );
  assert.ok(
    compareVersions(compatibility.south_runtime, runtimeMin) >= 0 &&
      compareVersions(compatibility.south_runtime, southRelease) <= 0,
    `${componentDir} declares south_runtime ${compatibility.south_runtime}, outside ${runtimeMin}..=${southRelease}`,
  );
  assert.deepEqual(
    compatibility.kernel_contracts,
    kernelContracts,
    `${componentDir} declares kernel contracts the host does not distribute`,
  );
  if (
    oldestRangeRuntime === undefined ||
    compareVersions(compatibility.south_runtime, oldestRangeRuntime) < 0
  ) {
    oldestRangeRuntime = compatibility.south_runtime;
  }
}
// The floor is the oldest shipped runtime: lower admits packages
// nothing tests, higher refuses one the host ships.
assert.equal(
  runtimeMin,
  oldestRangeRuntime,
  "SOUTH_RUNTIME_MIN must equal the oldest south_runtime among the shipped components",
);

// Each WASM package under plugins/official is its own workspace with its own
// lockfile, so the root closure above never sees it. agent-openai-responses
// stayed on South v0.37.0 with a second kernel copy because of that.
// Hold every package lockfile to the same two sources and revisions.
// south-north-codec is reached only from a plugin package.
const pluginSouthVersions = new Map([
  ...southPackageVersions,
  ["south-north-codec", southRelease],
]);
function lockPackages(lockfile) {
  return lockfile
    .split(/^\[\[package\]\]$/m)
    .slice(1)
    .map((block) => ({
      name: block.match(/^name = "([^"]+)"$/m)?.[1],
      version: block.match(/^version = "([^"]+)"$/m)?.[1],
      source: block.match(/^source = "([^"]+)"$/m)?.[1],
    }));
}
function validatePluginLock(lockfile, scope) {
  const packages = lockPackages(lockfile);
  for (const pkg of packages.filter((entry) => entry.source?.startsWith("git+"))) {
    assert.ok(
      pkg.source === southSource || pkg.source === kernelSource,
      `${scope} ${pkg.name} must resolve from the approved South or kernel revision, found ${pkg.source}`,
    );
  }
  for (const pkg of packages.filter(isSouthPackage)) {
    assert.equal(
      pkg.source,
      southSource,
      `${scope} ${pkg.name} must resolve from the one pinned South source and revision`,
    );
    assert.equal(
      pkg.version,
      pluginSouthVersions.get(pkg.name),
      `${scope} ${pkg.name} must stay on its South ${southRelease} version`,
    );
  }
  const protocols = packages.filter((pkg) => pkg.name === "token-station-protocol");
  assert.ok(
    protocols.length <= 1,
    `${scope} must resolve one token-station-protocol, found ${protocols.length}`,
  );
  for (const pkg of protocols) {
    assert.equal(pkg.version, kernelVersion, `${scope} protocol must stay on ${kernelVersion}`);
  }
}

const staleLock = `[[package]]
name = "south-north-codec"
version = "0.37.0"
source = "git+${southRepository}?tag=v0.37.0#3cb625104df76df738645b69f1cafd17bcbcad8d"

[[package]]
name = "token-station-protocol"
version = "0.7.0"

[[package]]
name = "token-station-protocol"
version = "0.3.0"
source = "git+${kernelRepository}?rev=ab6bb2ffaab534e6732d1bfc53d24c7caa51fa35#ab6bb2ffaab534e6732d1bfc53d24c7caa51fa35"
`;
assert.throws(
  () => validatePluginLock(staleLock, "a stale package"),
  /must resolve from the approved South or kernel revision/,
  "the plugin lock check must refuse a tag-pinned South crate",
);
assert.throws(
  () =>
    validatePluginLock(
      staleLock.replace(/^source = "git\+.*$/gm, `source = "${kernelSource}"`)
        .replace('name = "south-north-codec"\nversion = "0.37.0"\n', 'name = "serde"\nversion = "1.0.0"\n'),
      "a doubled package",
    ),
  /must resolve one token-station-protocol, found 2/,
  "the plugin lock check must refuse two protocol crates",
);

const pluginsDirectory = path.join(root, "plugins/official");
const pluginPackages = fs
  .readdirSync(pluginsDirectory)
  .filter((name) => fs.existsSync(path.join(pluginsDirectory, name, "Cargo.toml")));
assert.ok(pluginPackages.length > 0, "plugins/official must hold WASM packages");
for (const name of pluginPackages) {
  const lockPath = path.join(pluginsDirectory, name, "Cargo.lock");
  assert.ok(fs.existsSync(lockPath), `plugins/official/${name} must commit its Cargo.lock`);
  validatePluginLock(fs.readFileSync(lockPath, "utf8"), `plugins/official/${name}`);
}

console.log("South dependency and MSRV policy check: PASS");
