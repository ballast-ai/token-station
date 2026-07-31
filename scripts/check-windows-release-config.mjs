import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const root = resolve(scriptDir, "..");
const baseConfigPath = resolve(
  root,
  "apps/desktop/src-tauri/tauri.conf.json",
);
const windowsConfigPath = resolve(
  root,
  "apps/desktop/src-tauri/tauri.windows.conf.json",
);
const wixPath = resolve(
  root,
  "apps/desktop/src-tauri/wix/per-user-main.wxs",
);

const [baseSource, windowsSource, wix] = await Promise.all([
  readFile(baseConfigPath, "utf8"),
  readFile(windowsConfigPath, "utf8"),
  readFile(wixPath, "utf8"),
]);
const base = JSON.parse(baseSource);
const windows = JSON.parse(windowsSource);

assert.equal(
  base.bundle.targets,
  "all",
  "the cross-platform base config must retain the existing macOS bundle behavior",
);
assert.deepEqual(
  windows.bundle.targets,
  ["msi"],
  "Windows formal releases must only produce MSI",
);
assert.equal(windows.bundle.windows.allowDowngrades, false);
assert.equal(
  windows.bundle.windows.wix.upgradeCode,
  "bf3d3988-99ea-56e4-b81c-2aa4521c29c9",
);
assert.equal(
  windows.bundle.windows.wix.template,
  "wix/per-user-main.wxs",
);
assert.equal(
  Object.hasOwn(windows.bundle.windows, "nsis"),
  false,
  "NSIS must not be configured in the Windows release overlay",
);

for (const required of [
  'InstallScope="perUser"',
  'InstallPrivileges="limited"',
  '<Directory Id="LocalAppDataFolder">',
  '<Directory Id="ProgramsFolder" Name="Programs">',
  '<Directory Id="INSTALLDIR" Name="{{product_name}}"/>',
  'Root="HKCU" Key="Software\\Classes\\\\{{protocol}}"',
  'DowngradeErrorMessage="!(loc.DowngradeErrorMessage)"',
]) {
  assert.ok(wix.includes(required), `WiX template is missing: ${required}`);
}
for (const forbidden of [
  'InstallScope="perMachine"',
  'AllowDowngrades="yes"',
  'Root="HKLM" Key="Software\\Classes\\\\{{protocol}}"',
]) {
  assert.equal(
    wix.includes(forbidden),
    false,
    `WiX template retains forbidden per-machine behavior: ${forbidden}`,
  );
}

console.log("Windows release configuration: PASS");
