#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const wasmTarget = "wasm32-wasip2";
const output = join(root, "plugins-dist");
const plugins = [
  ["agent-openai", "adapter.wasm"],
  ["agent-anthropic", "adapter.wasm"],
  ["agent-openai-responses", "adapter.wasm"],
  ["agent-gemini", "adapter.wasm"],
  // The South provider components ship beside the agents under the South
  // loader's file name.
  ["provider-openai-compatible-v2", "component.wasm"],
  ["provider-anthropic-v2", "component.wasm"],
];

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    env: process.env,
    stdio: "inherit",
    ...options,
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

const installedTargets = spawnSync("rustup", ["target", "list", "--installed"], {
  cwd: root,
  encoding: "utf8",
});
if (installedTargets.error) {
  throw installedTargets.error;
}
if (installedTargets.status !== 0) {
  process.stderr.write(installedTargets.stderr);
  process.exit(installedTargets.status ?? 1);
}
if (!installedTargets.stdout.split(/\r?\n/u).includes(wasmTarget)) {
  process.stderr.write(
    `missing Rust target ${wasmTarget}; run: rustup target add ${wasmTarget}\n`,
  );
  process.exit(1);
}

for (const [plugin, wasmFile] of plugins) {
  const source = join(root, "plugins", "official", plugin);
  run("cargo", [
    "build",
    "--locked",
    "--release",
    "--manifest-path",
    join(source, "Cargo.toml"),
    "--target",
    wasmTarget,
  ]);
  const destination = join(output, plugin);
  mkdirSync(destination, { recursive: true });
  copyFileSync(join(source, "manifest.json"), join(destination, "manifest.json"));
  copyFileSync(
    join(
      source,
      "target",
      wasmTarget,
      "release",
      `${plugin.replaceAll("-", "_")}.wasm`,
    ),
    join(destination, wasmFile),
  );
}

const npx = process.platform === "win32" ? "npx.cmd" : "npx";
run(npx, ["tauri", "dev", "--features", "bundled-plugins"], {
  cwd: join(root, "apps", "desktop"),
  env: {
    ...process.env,
    TOKEN_STATION_PLUGINS_DIST: output,
  },
});
