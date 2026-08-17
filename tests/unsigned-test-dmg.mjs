#!/usr/bin/env node

import assert from "node:assert/strict";
import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");

const filename = "token-station_1.1.3_aarch64_UNSIGNED-UNNOTARIZED.dmg";
const markerName = "未签名测试版.txt";
const provenanceName = "构建来源.txt";
const terminalCommandName = "终端启动命令.txt";
const terminalCommand =
  'sudo xattr -dr com.apple.quarantine "/Applications/token-station.app" && open "/Applications/token-station.app"';

const desktopBuilder = read("scripts/build-desktop.sh");
assert.match(desktopBuilder, /rustc --print sysroot/);
assert.match(desktopBuilder, /--remap-path-prefix=\$rust_sysroot=\/rustc/);

const desktopAuditor = read("scripts/audit-desktop-artifact.sh");
assert.match(desktopAuditor, /--rust-sysroot/);
assert.match(desktopAuditor, /leaks the Rust sysroot path/);

const packager = read("scripts/package-macos-dmg.sh");
assert.match(packager, /--unsigned-test/);
assert.match(packager, /UNSIGNED-UNNOTARIZED/);
assert.match(packager, /--app-source-commit/);
assert.match(packager, /packaging_source_commit/);
assert.match(packager, new RegExp(markerName));
assert.match(packager, new RegExp(provenanceName));
assert.match(packager, new RegExp(terminalCommandName));
assert.match(
  packager,
  /if \[\[ "\$unsigned_test" == "true" \]\]; then[\s\S]*\/bin\/cp "\$unsigned_terminal_command" "\$stage\/终端启动命令\.txt"/,
);
assert.match(packager, /finder_layout_template="\$formal_finder_layout_template"/);
assert.match(
  packager,
  /if \[\[ "\$unsigned_test" == "true" \]\]; then[\s\S]*finder_layout_template="\$unsigned_finder_layout_template"/,
);
assert.match(packager, /notarytool submit/);
assert.match(packager, /stapler staple/);
assert.match(packager, /-format UDRW/);
assert.match(packager, /\.DS_Store/);
assert.match(packager, /base64 -D[\s\S]*finder_layout_template[\s\S]*stage\/\.DS_Store/);
assert.match(packager, /hdiutil convert[\s\S]*-format UDZO/);
assert.doesNotMatch(packager, /hdiutil attach/);

const auditor = read("scripts/audit-macos-dmg.sh");
assert.match(auditor, /--unsigned-test/);
assert.match(auditor, /UNSIGNED-UNNOTARIZED/);
assert.match(auditor, /Signature=adhoc/);
assert.match(auditor, new RegExp(markerName));
assert.match(auditor, new RegExp(provenanceName));
assert.match(auditor, new RegExp(terminalCommandName));
assert.match(auditor, /正式 DMG 不得包含终端 Gatekeeper 绕过入口/);
assert.match(auditor, /stapler validate/);
assert.match(auditor, /mounted_ds_store/);
assert.match(auditor, /configure-dmg-layout\.applescript[\s\S]*inspect/);
assert.match(auditor, /\.fseventsd/);

const finderLayout = read("packaging/macos/configure-dmg-layout.applescript");
assert.match(finderLayout, /folder mountAlias/);
assert.match(finderLayout, /current view to icon view/);
assert.match(finderLayout, /arrangement to not arranged/);
assert.match(finderLayout, /icon size to 128/);
assert.match(finderLayout, /toolbar visible to false/);
assert.match(finderLayout, /sidebar width to 0/);
assert.match(finderLayout, /position of item "token-station\.app" .* to \{310, 170\}/);
assert.match(finderLayout, /position of item "Applications" .* to \{610, 170\}/);
assert.match(finderLayout, /if exists item "构建来源\.txt"/);
assert.match(finderLayout, /if exists item "未签名测试版\.txt"/);
assert.match(finderLayout, /if exists item "终端启动命令\.txt"/);
assert.match(finderLayout, /set bounds to \{100, 100, 1180, 700\}/);
assert.match(finderLayout, /position of item "终端启动命令\.txt" .* to \{640, 440\}/);

const terminalCommandGuide = read("packaging/macos/终端启动命令.txt");
for (const phrase of [
  "Token Station 官方 GitHub Releases",
  "先把 token-station.app 拖到 Applications",
  "管理员密码",
  "不会显示字符",
]) {
  assert.ok(terminalCommandGuide.includes(phrase), `终端命令说明缺少“${phrase}”`);
}
const executableLines = terminalCommandGuide
  .split(/\r?\n/)
  .filter((line) => line.startsWith("sudo "));
assert.deepEqual(executableLines, [terminalCommand]);
assert.doesNotMatch(terminalCommandGuide, /spctl\s+--master-disable/);
assert.doesNotMatch(terminalCommandGuide, /xattr[^\n]*(?:\/Applications[\s"']|~\/|\$HOME)/);

for (const [relativePath, expectedDigest] of [
  [
    "packaging/macos/dmg-layout.dsstore.base64",
    "cc2af966a7af4db45f1be70fe674fe506ec545604bed5068d5bf47c9e8b3d47b",
  ],
  [
    "packaging/macos/dmg-layout-unsigned.dsstore.base64",
    "a815904ef3a022812de177409688ab8b68e1e75e33cf3dcada978cde0bd83b4b",
  ],
]) {
  const finderLayoutTemplate = Buffer.from(read(relativePath).replace(/\s/g, ""), "base64");
  assert.equal(finderLayoutTemplate.subarray(4, 8).toString("ascii"), "Bud1");
  assert.equal(
    crypto.createHash("sha256").update(finderLayoutTemplate).digest("hex"),
    expectedDigest,
  );
}

const installer = read("packaging/macos/安装 Token Station.command");
assert.match(installer, /UNSIGNED_TEST_MARKER/);
assert.match(installer, /未签名、未经 Apple 公证/);
assert.match(installer, /spctl --assess --type execute/);
assert.match(installer, /xattr -dr com\.apple\.quarantine "\$DEST_APP"/);
assert.doesNotMatch(installer, /spctl\s+--master-disable/);

const testReadme = read("packaging/macos/installation-guide.md");
for (const phrase of ["未签名", "Apple 未对该文件完成公证", "仅供测试", "SHA-256"]) {
  assert.ok(testReadme.includes(phrase), `测试安装说明缺少“${phrase}”`);
}
assert.doesNotMatch(testReadme, /spctl\s+--master-disable/);

const notes = read("docs/release/v1.1.3.md");
for (const phrase of [filename, "未签名", "未公证", "仅供测试", ".sha256"]) {
  assert.ok(notes.includes(phrase), `Release 说明缺少“${phrase}”`);
}
assert.match(notes, /不提供[^\u3002]*CLI 二进制/);

console.log("unsigned test DMG policy: PASS");
