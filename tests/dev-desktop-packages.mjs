import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const registry = JSON.parse(readFileSync(join(root, "plugins/official/packages.json"), "utf8"));

test("the desktop launcher stages every official package before starting Tauri", { skip: process.platform === "win32" }, () => {
  const fixture = realpathSync(mkdtempSync(join(tmpdir(), "token-station-dev-packages-")));
  try {
    mkdirSync(join(fixture, "scripts"));
    mkdirSync(join(fixture, "apps/desktop"), { recursive: true });
    mkdirSync(join(fixture, "plugins/official"), { recursive: true });
    mkdirSync(join(fixture, "bin"));
    copyFileSync(join(root, "scripts/dev-desktop.mjs"), join(fixture, "scripts/dev-desktop.mjs"));
    writeFileSync(join(fixture, "plugins/official/packages.json"), JSON.stringify(registry));
    for (const packageEntry of registry.packages) {
      const source = join(fixture, "plugins/official", packageEntry.dir);
      mkdirSync(join(source, "target/wasm32-wasip2/release"), { recursive: true });
      writeFileSync(join(source, "manifest.json"), JSON.stringify({ id: packageEntry.id }));
      writeFileSync(join(source, "Cargo.toml"), "[package]\n");
      writeFileSync(join(source, "target/wasm32-wasip2/release", `${packageEntry.dir.replaceAll("-", "_")}.wasm`), packageEntry.id);
    }
    for (const command of ["rustup", "cargo", "npx"]) {
      writeFileSync(join(fixture, "bin", command), `#!${process.execPath}
const fs = require("node:fs");
fs.appendFileSync(process.env.DEV_LAUNCHER_LOG, JSON.stringify({command:${JSON.stringify(command)},args:process.argv.slice(2),cwd:process.cwd(),dist:process.env.TOKEN_STATION_PLUGINS_DIST}) + "\\n");
if (${JSON.stringify(command)} === "rustup") process.stdout.write("wasm32-wasip2\\n");
`, { mode: 0o755 });
    }
    const log = join(fixture, "commands.jsonl");
    const result = spawnSync(process.execPath, [join(fixture, "scripts/dev-desktop.mjs")], {
      encoding: "utf8",
      env: { ...process.env, PATH: `${join(fixture, "bin")}${delimiter}${process.env.PATH}`, DEV_LAUNCHER_LOG: log },
    });
    assert.equal(result.status, 0, result.stderr);
    const commands = readFileSync(log, "utf8").trim().split("\n").map(line => JSON.parse(line));
    const builds = commands.filter(command => command.command === "cargo");
    assert.equal(builds.length, registry.packages.length);
    for (const [index, packageEntry] of registry.packages.entries()) {
      assert.deepEqual(builds[index].args, ["build", "--locked", "--release", "--manifest-path", join(fixture, "plugins/official", packageEntry.dir, "Cargo.toml"), "--target", "wasm32-wasip2"]);
      const staged = join(fixture, "plugins-dist", packageEntry.dir);
      assert.equal(JSON.parse(readFileSync(join(staged, "manifest.json"), "utf8")).id, packageEntry.id);
      assert.equal(readFileSync(join(staged, packageEntry.wasm), "utf8"), packageEntry.id);
    }
    const launch = commands.at(-1);
    assert.equal(launch.command, "npx");
    assert.deepEqual(launch.args, ["tauri", "dev", "--features", "bundled-plugins"]);
    assert.equal(launch.cwd, join(fixture, "apps/desktop"));
    assert.equal(launch.dist, join(fixture, "plugins-dist"));
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});
