import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const southRepository = "https://github.com/ballast-ai/token-station-south.git";
const southRevision = "efbb9b655bf1b26ec639dd21bde7ade7fcdc11b8";
const southVersion = "0.16.0";
const southSource = `git+${southRepository}?rev=${southRevision}#${southRevision}`;
const kernelRepository = "https://github.com/ballast-ai/token-station-kernel.git";
const kernelRevision = "ab6bb2ffaab534e6732d1bfc53d24c7caa51fa35";
const kernelVersion = "0.3.0";
// South 0.16.0 pins the kernel protocol crate by revision rather than by tag
// (south #45). The revision is unchanged; the form is stricter, because a tag
// can be moved onto different bytes and a revision cannot. Require the stricter
// one rather than accepting either.
const kernelSource = `git+${kernelRepository}?rev=${kernelRevision}#${kernelRevision}`;
const expectedSouthPackages = new Set([
  "south-component-conformance",
  "south-contracts",
  "south-core",
  "south-provider-api",
  "south-provider-conformance",
  "south-provider-runtime",
  "south-testkit",
  "south-transport-reqwest",
]);
const expectedDesktopSouthPackages = new Set([
  "south-contracts",
  "south-core",
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
      southVersion,
      `${scope} ${pkg.name} must stay on South ${southVersion}`,
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

for (const packageName of expectedSouthPackages) {
  // The pinned trio, in order, is what this gate exists for: an exact version,
  // the official repository, and a commit revision. It used to require the
  // whole line to match verbatim, which also forbade a `features` list — so
  // enabling a feature looked identical to unpinning the revision. The
  // requirement is that the pin is there and nothing else on the line
  // relaxes it, not that the line is one exact string.
  const pinned = `${packageName} = { version = "=${southVersion}", git = "${southRepository}", rev = "${southRevision}"`;
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
const kernelPackages = rootGitPackages.filter(
  (pkg) => pkg.source === kernelSource,
);
assert.equal(
  kernelPackages.length,
  1,
  "the root workspace must resolve exactly one approved kernel package",
);
assert.equal(
  kernelPackages[0].name,
  "token-station-protocol",
  "the approved kernel source may supply only the protocol package",
);
assert.equal(
  kernelPackages[0].version,
  kernelVersion,
  `the kernel protocol package must stay on ${kernelVersion}`,
);

const workspaceMemberIds = new Set(metadata.workspace_members);
const nodeById = new Map(metadata.resolve.nodes.map((node) => [node.id, node]));
for (const pkg of southPackages) {
  const node = nodeById.get(pkg.id);
  assert.ok(node, `${pkg.name} must have a resolved dependency node`);
  const reverseHostDependencies = node.deps
    .map((dependency) => dependency.pkg)
    .filter((packageId) => workspaceMemberIds.has(packageId));
  assert.deepEqual(
    reverseHostDependencies,
    [],
    `${pkg.name} must not depend back on the community host workspace`,
  );
}

const cli = metadata.packages.find((pkg) => pkg.name === "token-station-cli");
assert.ok(cli, "token-station-cli must be present in workspace metadata");
const cliSouthDependencies = new Map(
  cli.dependencies
    .filter((dependency) => expectedSouthPackages.has(dependency.name))
    .map((dependency) => [dependency.name, dependency.kind ?? "normal"]),
);
assert.deepEqual(
  cliSouthDependencies,
  new Map([
    ["south-component-conformance", "dev"],
    ["south-contracts", "normal"],
    ["south-core", "normal"],
    ["south-provider-api", "normal"],
    ["south-provider-conformance", "dev"],
    ["south-provider-runtime", "normal"],
    ["south-testkit", "dev"],
    ["south-transport-reqwest", "normal"],
  ]),
  "the CLI must keep conformance/testkit test-only and only five South runtime dependencies",
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

// The host declares what it was built against, and South's loader now judges
// every component against that declaration. Nothing in the dependency graph
// carries the value, so it is a constant — and a constant nobody checks is
// precisely what let the compatibility tuple go unenforced for three releases.
// Close the loop here: the declaration, the pinned South version and every
// shipped component manifest must agree.
const southComponentSource = fs.readFileSync(
  path.join(root, "apps/cli/src/south_component.rs"),
  "utf8",
);
const declaredRuntime = southComponentSource.match(
  /^(?:pub\(crate\) )?const SOUTH_RUNTIME: &str = "([^"]+)";$/m,
)?.[1];
assert.equal(
  declaredRuntime,
  southVersion,
  "the host's declared SOUTH_RUNTIME must equal the pinned South version",
);

for (const componentDir of ["provider-openai-compatible-v2", "provider-anthropic-v2"]) {
  const manifest = JSON.parse(
    fs.readFileSync(
      path.join(root, "plugins/official", componentDir, "manifest.json"),
      "utf8",
    ),
  );
  assert.equal(
    manifest.compatibility.south_runtime,
    southVersion,
    `${componentDir} declares a south_runtime the host would refuse at load`,
  );
  for (const [field, constant] of [
    ["ir_schema_id", "IR_SCHEMA_ID"],
    ["kernel_version", "KERNEL_VERSION"],
    ["kernel_revision", "KERNEL_REVISION"],
  ]) {
    const declared = southComponentSource.match(
      new RegExp(`^(?:pub\\(crate\\) )?const ${constant}: &str = "([^"]+)";$`, "m"),
    )?.[1];
    assert.equal(
      manifest.compatibility[field],
      declared,
      `${componentDir}.${field} disagrees with the host's ${constant}`,
    );
  }
}

console.log("South dependency and MSRV policy check: PASS");
