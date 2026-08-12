import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const workflows = new Map(
  ["ci", "platform"].map((name) => [
    name,
    fs.readFileSync(path.join(root, `.github/workflows/${name}.yml`), "utf8"),
  ]),
);

function jobBody(workflowName, jobName) {
  const workflow = workflows.get(workflowName);
  const startMarker = `  ${jobName}:\n`;
  const start = workflow.indexOf(startMarker);
  if (start === -1) {
    throw new Error(`Job '${jobName}' is missing from ${workflowName}.yml.`);
  }

  const remaining = workflow.slice(start + startMarker.length);
  const nextJob = remaining.search(/^  [a-z0-9_-]+:\n/m);
  return nextJob === -1 ? remaining : remaining.slice(0, nextJob);
}

const prefetch = [
  "cargo fetch --locked --target wasm32-wasip2",
  "--manifest-path plugins/official/provider-openai-compatible/Cargo.toml",
];

// Every job that exercises the offline scaffold must fetch its locked
// dependencies first. `rust` covers both paths: pull requests run the suite
// directly, branch pushes run the same suite under llvm-cov instrumentation.
// Windows repeats the workspace suite from the platform workflow, where the
// Windows and macOS gates live — they are not part of basic CI.
for (const [workflowName, jobName, testCommand] of [
  ["ci", "rust", "cargo test --workspace"],
  ["ci", "rust", "cargo llvm-cov --workspace"],
  ["platform", "windows-rust", "cargo test --workspace"],
]) {
  const body = jobBody(workflowName, jobName).replace(/\s+/g, " ");
  const testIndex = body.indexOf(testCommand);
  if (testIndex === -1) {
    throw new Error(
      `Job '${jobName}' in ${workflowName}.yml no longer runs '${testCommand}'.`,
    );
  }

  const prefetchIndexes = prefetch.map((fragment) => body.indexOf(fragment));
  if (prefetchIndexes.some((index) => index === -1 || index > testIndex)) {
    throw new Error(
      `Job '${jobName}' in ${workflowName}.yml runs the offline scaffold test without first fetching its locked dependencies.`,
    );
  }
}

console.log("CI scaffold dependency prefetch check: PASS");
