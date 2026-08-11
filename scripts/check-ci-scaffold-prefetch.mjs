import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const workflow = fs.readFileSync(path.join(root, ".github/workflows/ci.yml"), "utf8");

function jobBody(jobName) {
  const startMarker = `  ${jobName}:\n`;
  const start = workflow.indexOf(startMarker);
  if (start === -1) {
    throw new Error(`CI job '${jobName}' is missing.`);
  }

  const remaining = workflow.slice(start + startMarker.length);
  const nextJob = remaining.search(/^  [a-z0-9_-]+:\n/m);
  return nextJob === -1 ? remaining : remaining.slice(0, nextJob);
}

const prefetch = [
  "cargo fetch --locked --target wasm32-wasip2",
  "--manifest-path plugins/official/provider-openai-compatible/Cargo.toml",
];

for (const [jobName, testCommand] of [
  ["rust", "cargo test --workspace"],
  ["rust-coverage", "cargo llvm-cov --workspace"],
  ["windows-rust", "cargo test --workspace"],
]) {
  const body = jobBody(jobName).replace(/\s+/g, " ");
  const testIndex = body.indexOf(testCommand);
  if (testIndex === -1) {
    throw new Error(`CI job '${jobName}' no longer runs '${testCommand}'.`);
  }

  const prefetchIndexes = prefetch.map((fragment) => body.indexOf(fragment));
  if (prefetchIndexes.some((index) => index === -1 || index > testIndex)) {
    throw new Error(
      `CI job '${jobName}' runs the offline scaffold test without first fetching its locked dependencies.`,
    );
  }
}

console.log("CI scaffold dependency prefetch check: PASS");
