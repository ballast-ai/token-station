#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");

const filename = "token-station_1.1.3_aarch64_UNSIGNED-UNNOTARIZED.dmg";
const markerName = "未签名测试版.txt";
const provenanceName = "构建来源.txt";

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
assert.match(packager, /notarytool submit/);
assert.match(packager, /stapler staple/);

const auditor = read("scripts/audit-macos-dmg.sh");
assert.match(auditor, /--unsigned-test/);
assert.match(auditor, /UNSIGNED-UNNOTARIZED/);
assert.match(auditor, /Signature=adhoc/);
assert.match(auditor, new RegExp(markerName));
assert.match(auditor, new RegExp(provenanceName));
assert.match(auditor, /stapler validate/);

const installer = read("packaging/macos/安装 Token Station.command");
assert.match(installer, /UNSIGNED_TEST_MARKER/);
assert.match(installer, /未签名、未经 Apple 公证/);
assert.match(installer, /spctl --assess --type execute/);
assert.match(installer, /xattr -dr com\.apple\.quarantine "\$DEST_APP"/);
assert.doesNotMatch(installer, /spctl\s+--master-disable/);

const testReadme = read("packaging/macos/未签名测试版安装前必读.md");
for (const phrase of ["未签名", "未经 Apple 公证", "仅供测试", "SHA-256", filename]) {
  assert.ok(testReadme.includes(phrase), `测试安装说明缺少“${phrase}”`);
}
assert.doesNotMatch(testReadme, /已完成 Developer ID 签名和 Apple 公证/);
assert.doesNotMatch(testReadme, /spctl\s+--master-disable/);

const notes = read("docs/release/v1.1.3.md");
for (const phrase of [filename, "未签名", "未公证", "仅供测试", ".sha256"]) {
  assert.ok(notes.includes(phrase), `Release 说明缺少“${phrase}”`);
}
assert.match(notes, /不提供[^\u3002]*CLI 二进制/);

console.log("unsigned test DMG policy: PASS");
