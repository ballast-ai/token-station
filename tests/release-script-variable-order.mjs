#!/usr/bin/env node
// A release script that runs under `set -u` aborts the moment it expands a
// variable it has not assigned yet. That is not a style problem: the build
// dies partway through, after the wasm components have already compiled, so
// the failure looks like a toolchain problem rather than a script bug.
//
// This happened. `build-release.sh` called `stage_declared_fixtures` with
// "$STAGE/..." from the loop that only *builds* the components, eight lines
// above the `STAGE=` assignment. Nothing caught it, because no gate runs the
// release scripts and a fixture directory nobody has committed yet made the
// function return before doing anything visible. The build itself still died.
//
// So: for each release script that sets `set -u`, every variable the script
// assigns at top level must be assigned before its first expansion.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const scripts = [
  "scripts/build-release.sh",
  "scripts/build-desktop.sh",
  "scripts/prepare-desktop-test-plugins.sh",
];

// Names bash itself provides, plus the ones these scripts read from the
// environment rather than assign.
const AMBIENT = new Set([
  "HOME", "PATH", "PWD", "OSTYPE", "BASH_SOURCE", "RUSTFLAGS", "CARGO_HOME",
  "SOURCE_DATE_EPOCH", "CI", "TMPDIR", "USER", "SHELL", "LANG",
]);

const failures = [];

for (const relative of scripts) {
  const path = join(root, relative);
  let source;
  try {
    source = readFileSync(path, "utf8");
  } catch {
    failures.push(`${relative}: listed here but missing from the repository`);
    continue;
  }
  if (!/^set -[a-z]*u/m.test(source)) continue;

  const lines = source.split("\n");
  const assignedAt = new Map();
  const expandedAt = new Map();

  lines.forEach((line, index) => {
    const code = line.replace(/#.*$/, "");
    const lineNumber = index + 1;

    // `NAME=`, `local NAME=`, `export NAME=`, `for NAME in`, `read NAME`.
    for (const match of code.matchAll(
      /(?:^|\s|;)(?:local\s+|export\s+|declare\s+(?:-\w+\s+)?)?([A-Za-z_][A-Za-z0-9_]*)=/g,
    )) {
      const name = match[1];
      if (!assignedAt.has(name)) assignedAt.set(name, lineNumber);
    }
    for (const match of code.matchAll(/\bfor\s+([A-Za-z_][A-Za-z0-9_]*)\s+in\b/g)) {
      if (!assignedAt.has(match[1])) assignedAt.set(match[1], lineNumber);
    }

    // `$NAME` and `${NAME}` — but not `${NAME:-default}` / `${NAME:+…}`,
    // which are the documented way to expand something possibly unset.
    for (const match of code.matchAll(/\$\{([A-Za-z_][A-Za-z0-9_]*)([^}]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)/g)) {
      const name = match[1] ?? match[3];
      const suffix = match[2] ?? "";
      if (suffix.startsWith(":-") || suffix.startsWith(":+") || suffix.startsWith(":=")) continue;
      if (!expandedAt.has(name)) expandedAt.set(name, lineNumber);
    }
  });

  for (const [name, expandLine] of expandedAt) {
    if (AMBIENT.has(name)) continue;
    const assignLine = assignedAt.get(name);
    if (assignLine === undefined) continue; // read from the environment on purpose
    if (expandLine < assignLine) {
      failures.push(
        `${relative}:${expandLine}: expands $${name}, which the script does not assign ` +
          `until line ${assignLine}. Under \`set -u\` this aborts the build.`,
      );
    }
  }
}

if (failures.length > 0) {
  console.error("release script variable order:");
  for (const failure of failures) console.error(`  ${failure}`);
  process.exit(1);
}
console.log(`release script variable order: ${scripts.length} scripts checked`);
