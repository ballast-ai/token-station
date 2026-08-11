#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const requiredFiles = [
  "packaging/macos/安装前必读.md",
  "packaging/macos/安装 Token Station.command",
  "packaging/macos/AGENTS.md",
  "scripts/package-macos-dmg.sh",
  "scripts/audit-macos-dmg.sh",
];

const failures = [];

function read(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!fs.existsSync(absolutePath)) {
    failures.push(`${relativePath} 不存在`);
    return "";
  }
  return fs.readFileSync(absolutePath, "utf8");
}

for (const relativePath of requiredFiles) {
  read(relativePath);
}

const installerPath = "packaging/macos/安装 Token Station.command";
const installer = read(installerPath);
if (installer) {
  const installerMode = fs.statSync(path.join(root, installerPath)).mode;
  if ((installerMode & 0o111) === 0) {
    failures.push(`${installerPath} 没有可执行权限`);
  }

  const requiredInstallerPatterns = [
    ["固定 bundle id", /EXPECTED_BUNDLE_ID=["']com\.tokenstation\.desktop["']/],
    ["固定安装目标", /DEST_APP=["']\/Applications\/token-station\.app["']/],
    ["源 App 签名检查", /codesign --verify --deep --strict ["']?\$SOURCE_APP/],
    ["源 App Gatekeeper 检查", /spctl --assess --type execute .*\$SOURCE_APP/],
    ["明确 y 或 Y 确认", /\[yY\]/],
    ["管理员密码说明", /密码.*不会显示|不会显示.*密码/],
    ["安装互斥锁", /LOCK_DIR=["']\/Applications\/\.token-station-install\.lock["']/],
    ["随机临时目录", /mktemp -d ["']\/Applications\/\.token-station-install\./],
    ["替换前备份", /BACKUP_APP=/],
    ["失败恢复", /restore_previous_app/],
    ["精确移除 quarantine", /xattr -dr com\.apple\.quarantine ["']?\$DEST_APP/],
    ["安装后重新验签", /verify_installed_app/],
    ["启动 App", /open ["']?\$DEST_APP/],
  ];
  for (const [label, pattern] of requiredInstallerPatterns) {
    if (!pattern.test(installer)) failures.push(`${installerPath} 缺少${label}`);
  }

  if (!/remove_managed_directory "\$BACKUP_ROOT"\nremove_managed_directory "\$TEMP_ROOT"/.test(installer)) {
    failures.push(`${installerPath} 成功后没有清理全新安装时的空备份目录`);
  }

  if (/\*\.app|token-station\*/.test(installer)) {
    failures.push(`${installerPath} 不能使用通配符定位或操作 App`);
  }
  if (/spctl\s+--master-disable/.test(installer)) {
    failures.push(`${installerPath} 不能关闭 Gatekeeper`);
  }
}

const readmePath = "packaging/macos/安装前必读.md";
const readme = read(readmePath);
if (readme) {
  const requiredReadmePhrases = [
    "Developer ID",
    "Apple 公证",
    "安装 Token Station.command",
    "管理员密码",
    "不会显示字符",
    "不会关闭系统全局安全检查",
    "右键",
    "xattr -dr com.apple.quarantine /Applications/token-station.app",
    "可信发布页面",
  ];
  for (const phrase of requiredReadmePhrases) {
    if (!readme.includes(phrase)) failures.push(`${readmePath} 缺少“${phrase}”说明`);
  }
}

const agentRulesPath = "packaging/macos/AGENTS.md";
const agentRules = read(agentRulesPath);
if (agentRules) {
  for (const phrase of [
    "安装前必读.md",
    "/Applications/token-station.app",
    "com.tokenstation.desktop",
    "不得关闭 Gatekeeper",
    "不得修改 SIP",
  ]) {
    if (!agentRules.includes(phrase)) failures.push(`${agentRulesPath} 缺少“${phrase}”红线`);
  }
}

const packagerPath = "scripts/package-macos-dmg.sh";
const packager = read(packagerPath);
if (packager) {
  const requiredPackagerPatterns = [
    ["Applications 精确链接", /ln -s \/Applications .*Applications/],
    ["安装说明", /安装前必读\.md/],
    ["安装脚本", /安装 Token Station\.command/],
    ["Agent 约束", /AGENTS\.md/],
    ["DMG 创建", /hdiutil create/],
    ["DMG 签名", /codesign .*--sign/],
    ["DMG 公证", /notarytool submit/],
    ["票据装订", /stapler staple/],
    ["临时 DMG 隔离", /temporary_dmg=/],
    ["通过审计后再无覆盖发布", /audit-macos-dmg\.sh[\s\S]*\/bin\/mv -n "\$temporary_dmg" "\$output_path"/],
  ];
  for (const [label, pattern] of requiredPackagerPatterns) {
    if (!pattern.test(packager)) failures.push(`${packagerPath} 缺少${label}`);
  }
  if (/APPLE_PASSWORD|--password/.test(packager)) {
    failures.push(`${packagerPath} 不能把 Apple 密码放进进程参数`);
  }
  if (/hdiutil create[^\n]*"\$output_path"/.test(packager)) {
    failures.push(`${packagerPath} 不能在正式输出路径上直接创建未审计 DMG`);
  }
}

const auditorPath = "scripts/audit-macos-dmg.sh";
const auditor = read(auditorPath);
if (auditor) {
  const requiredAuditPatterns = [
    ["只读挂载", /hdiutil attach .*readonly/],
    ["bundle id 检查", /com\.tokenstation\.desktop/],
    ["App 签名检查", /codesign --verify --deep --strict/],
    ["Applications 链接检查", /readlink .*Applications/],
    ["安装脚本权限检查", /-x .*mounted_installer/],
    ["Gatekeeper 检查", /spctl .*--assess/],
    ["票据检查", /stapler validate/],
    ["版本检查", /CFBundleShortVersionString/],
    ["架构检查", /lipo -archs/],
  ];
  for (const [label, pattern] of requiredAuditPatterns) {
    if (!pattern.test(auditor)) failures.push(`${auditorPath} 缺少${label}`);
  }
}

const buildScriptPath = "scripts/build-desktop.sh";
const buildScript = read(buildScriptPath);
if (!/package-macos-dmg\.sh/.test(buildScript)) {
  failures.push(`${buildScriptPath} 没有调用合规 DMG 打包器`);
}
if ((buildScript.match(/--bundles app/g) ?? []).length < 2) {
  failures.push(`${buildScriptPath} 没有限制 Tauri 只生成待二次打包的 App`);
}

const ciWorkflowPath = ".github/workflows/ci.yml";
const ciWorkflow = read(ciWorkflowPath);
for (const command of [
  "node scripts/check-macos-dmg-packaging.mjs",
  "node scripts/check-release-readiness.mjs",
]) {
  if (!ciWorkflow.includes(command)) failures.push(`${ciWorkflowPath} 没有运行 ${command}`);
}

const desktopWorkflowPath = ".github/workflows/desktop-release.yml";
const desktopWorkflow = read(desktopWorkflowPath);
if (!desktopWorkflow.includes("node scripts/check-macos-dmg-packaging.mjs")) {
  failures.push(`${desktopWorkflowPath} 没有在正式构建前检查 DMG 策略`);
}

if (failures.length > 0) {
  console.error("macOS DMG 还不能发布：");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}

console.log("macOS DMG packaging policy: PASS");
