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
const lifecyclePath = resolve(root, "scripts/test-windows-msi.ps1");

const [baseSource, windowsSource, wix, lifecycle] = await Promise.all([
  readFile(baseConfigPath, "utf8"),
  readFile(windowsConfigPath, "utf8"),
  readFile(wixPath, "utf8"),
  readFile(lifecyclePath, "utf8"),
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

const pathComponent = wix.match(
  /<Component Id="Path"[\s\S]*?<\/Component>/,
)?.[0];
assert.ok(pathComponent, "WiX template must define the main executable component");
assert.equal(
  /<File Id="Path"[^>]*KeyPath="yes"/.test(pathComponent),
  false,
  "a file under the per-user profile cannot be the component KeyPath",
);
assert.match(
  pathComponent,
  /<RegistryValue Root="HKCU"[\s\S]*?Name="Path Component"[\s\S]*?KeyPath="yes"/,
  "the per-user main executable component must use an HKCU registry KeyPath",
);

const registryEntries = wix.match(
  /<Component Id="RegistryEntries"[\s\S]*?<\/Component>/,
)?.[0];
assert.ok(registryEntries, "WiX template must define registry entries");
assert.match(
  registryEntries,
  /<RemoveFolder[^>]*Directory="ProgramsFolder"[^>]*On="uninstall"/,
  "the per-user Programs directory must have an uninstall RemoveFile-table entry",
);

assert.match(
  lifecycle,
  /function Get-MsiUpgradeCode/,
  "the MSI lifecycle must query UpgradeCode through a dedicated helper",
);
assert.ok(
  lifecycle.includes('"SELECT ``UpgradeCode`` FROM ``Upgrade``"'),
  "the MSI lifecycle must read UpgradeCode from the Upgrade table",
);
assert.ok(
  lifecycle.includes("query returned no rows"),
  "MSI metadata queries must report missing rows before dereferencing null",
);
assert.equal(
  lifecycle.includes('Get-MsiProperty $newer "UpgradeCode"'),
  false,
  "UpgradeCode is not a Property-table value",
);
assert.ok(
  lifecycle.includes('plugins\\official\\packages.json'),
  "the MSI lifecycle must read the canonical official package catalog",
);
assert.ok(
  lifecycle.includes("$packageCatalog.packages"),
  "the MSI lifecycle must derive its expected builtin set from the catalog",
);
assert.equal(
  lifecycle.includes('"provider-openai-compatible"'),
  false,
  "the MSI lifecycle must not hardcode the builtin plugin set",
);

console.log("Windows release configuration: PASS");
