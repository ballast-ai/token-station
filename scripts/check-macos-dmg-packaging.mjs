#!/usr/bin/env node

import fs from "node:fs";
import crypto from "node:crypto";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const requiredFiles = [
  "packaging/macos/installation-guide.md",
  "macos-troubleshooting.md",
  "packaging/macos/安装 Token Station.command",
  "packaging/macos/终端启动命令.txt",
  "packaging/macos/configure-dmg-layout.applescript",
  "packaging/macos/dmg-layout.dsstore.base64",
  "packaging/macos/dmg-layout-unsigned.dsstore.base64",
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
    ["源 App 签名检查", /verify_app ["']\$SOURCE_APP["']/],
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
    ["未签名测试标记", /UNSIGNED_TEST_MARKER=/],
    ["未签名测试风险提示", /未签名、未经 Apple 公证/],
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

const readmePath = "packaging/macos/installation-guide.md";
const readme = read(readmePath);
if (readme) {
  const requiredReadmePhrases = [
    "Developer ID",
    "Apple 公证",
    "安装 Token Station.command",
    "管理员密码",
    "不会显示密码字符",
    "不会关闭系统全局安全检查",
    "右键",
    "UNSIGNED-UNNOTARIZED",
    "Read Before You Install Token Station",
  ];
  for (const phrase of requiredReadmePhrases) {
    if (!readme.includes(phrase)) failures.push(`${readmePath} 缺少“${phrase}”说明`);
  }
}

const troubleshootingPath = "macos-troubleshooting.md";
const troubleshootingGuide = read(troubleshootingPath);
if (troubleshootingGuide) {
  for (const phrase of [
    "If macOS Cannot Open Token Station",
    "macOS 无法打开 Token Station 时怎么办",
    "sudo xattr -dr com.apple.quarantine /Applications/token-station.app",
    "Do not disable Gatekeeper or SIP",
    "不要关闭 Gatekeeper",
  ]) {
    if (!troubleshootingGuide.includes(phrase)) failures.push(`${troubleshootingPath} 缺少“${phrase}”说明`);
  }
  if (/spctl\s+--master-disable/.test(troubleshootingGuide)) {
    failures.push(`${troubleshootingPath} 不能关闭 Gatekeeper`);
  }
}

const terminalCommandPath = "packaging/macos/终端启动命令.txt";
const terminalCommandGuide = read(terminalCommandPath);
if (terminalCommandGuide) {
  for (const phrase of [
    "Token Station 官方 GitHub Releases",
    "先把 token-station.app 拖到 Applications",
    "管理员密码",
    "不会显示字符",
  ]) {
    if (!terminalCommandGuide.includes(phrase)) failures.push(`${terminalCommandPath} 缺少“${phrase}”说明`);
  }
  const expectedCommand =
    'sudo xattr -dr com.apple.quarantine "/Applications/token-station.app" && open "/Applications/token-station.app"';
  const executableLines = terminalCommandGuide
    .split(/\r?\n/)
    .filter((line) => line.startsWith("sudo "));
  if (executableLines.length !== 1 || executableLines[0] !== expectedCommand) {
    failures.push(`${terminalCommandPath} 必须只包含一条针对 canonical App 的可执行命令`);
  }
  if (/spctl\s+--master-disable/.test(terminalCommandGuide)) {
    failures.push(`${terminalCommandPath} 不能关闭 Gatekeeper`);
  }
  if (/xattr[^\n]*(?:\/Applications[\s"']|~\/|\$HOME)/.test(terminalCommandGuide)) {
    failures.push(`${terminalCommandPath} 不能扩大 quarantine 清理范围`);
  }
}

const packagerPath = "scripts/package-macos-dmg.sh";
const packager = read(packagerPath);
if (packager) {
  const requiredPackagerPatterns = [
    ["Applications 精确链接", /ln -s \/Applications .*Applications/],
    ["安装说明", /installation-guide\.md/],
    ["安装脚本", /安装 Token Station\.command/],
    ["无法打开说明", /macos-troubleshooting\.md/],
    ["DMG 创建", /hdiutil create/],
    ["DMG 签名", /codesign .*--sign/],
    ["DMG 公证", /notarytool submit/],
    ["票据装订", /stapler staple/],
    ["临时 DMG 隔离", /temporary_dmg=/],
    ["通过审计后再无覆盖发布", /audit-macos-dmg\.sh[\s\S]*\/bin\/mv -n "\$temporary_dmg" "\$output_path"/],
    ["显式未签名测试模式", /--unsigned-test/],
    ["测试 DMG 警告文件名", /UNSIGNED-UNNOTARIZED/],
    ["App 源码提交", /--app-source-commit/],
    ["打包源码提交", /packaging_source_commit/],
    ["测试包可见警告", /未签名测试版\.txt/],
    ["测试包构建来源", /构建来源\.txt/],
    ["测试包终端启动命令", /终端启动命令\.txt/],
    ["终端启动命令仅进入测试包", /if \[\[ "\$unsigned_test" == "true" \]\]; then[\s\S]*\/bin\/cp "\$unsigned_terminal_command" "\$stage\/终端启动命令\.txt"/],
    ["正式包使用原布局模板", /finder_layout_template="\$formal_finder_layout_template"/],
    ["测试包使用独立布局模板", /if \[\[ "\$unsigned_test" == "true" \]\]; then[\s\S]*finder_layout_template="\$unsigned_finder_layout_template"/],
    ["可写布局镜像", /-format UDRW/],
    ["Finder 元数据落盘检查", /\.DS_Store/],
    ["解码受控 Finder 布局模板", /base64 -D[\s\S]*finder_layout_template[\s\S]*stage\/\.DS_Store/],
    ["只读压缩输出", /hdiutil convert[\s\S]*-format UDZO/],
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
  if (/hdiutil attach/.test(packager)) {
    failures.push(`${packagerPath} 不能挂载可写中间镜像，否则系统会写入临时目录`);
  }
}

for (const [finderLayoutTemplatePath, expectedDigest] of [
  [
    "packaging/macos/dmg-layout.dsstore.base64",
    "cc2af966a7af4db45f1be70fe674fe506ec545604bed5068d5bf47c9e8b3d47b",
  ],
  [
    "packaging/macos/dmg-layout-unsigned.dsstore.base64",
    "a815904ef3a022812de177409688ab8b68e1e75e33cf3dcada978cde0bd83b4b",
  ],
]) {
  const finderLayoutTemplate = read(finderLayoutTemplatePath);
  if (finderLayoutTemplate) {
    const decodedTemplate = Buffer.from(finderLayoutTemplate.replace(/\s/g, ""), "base64");
    if (decodedTemplate.subarray(4, 8).toString("ascii") !== "Bud1") {
      failures.push(`${finderLayoutTemplatePath} 不是有效的 Finder .DS_Store 模板`);
    }
    const templateDigest = crypto.createHash("sha256").update(decodedTemplate).digest("hex");
    if (templateDigest !== expectedDigest) {
      failures.push(`${finderLayoutTemplatePath} 与已验收布局不一致`);
    }
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
    ["未签名测试审计", /--unsigned-test/],
    ["ad-hoc App 检查", /Signature=adhoc/],
    ["测试包警告检查", /未签名测试版\.txt/],
    ["测试包来源检查", /构建来源\.txt/],
    ["测试包终端启动命令检查", /终端启动命令\.txt/],
    ["正式包拒绝终端绕过入口", /正式 DMG 不得包含终端 Gatekeeper 绕过入口/],
    ["Finder 元数据检查", /mounted_ds_store/],
    ["Finder 布局回读", /configure-dmg-layout\.applescript[\s\S]*inspect/],
    ["系统临时目录检查", /\.fseventsd/],
  ];
  for (const [label, pattern] of requiredAuditPatterns) {
    if (!pattern.test(auditor)) failures.push(`${auditorPath} 缺少${label}`);
  }
}

const finderLayoutPath = "packaging/macos/configure-dmg-layout.applescript";
const finderLayout = read(finderLayoutPath);
if (finderLayout) {
  const requiredFinderLayoutPatterns = [
    ["配置模式", /configure/],
    ["审计模式", /inspect/],
    ["普通目录与挂载根目录兼容", /folder mountAlias/],
    ["图标视图", /current view to icon view/],
    ["关闭自动排列", /arrangement to not arranged/],
    ["沿用 v1.1.2 图标尺寸", /icon size to 128/],
    ["隐藏 Finder 工具栏", /toolbar visible to false/],
    ["隐藏 Finder 侧边栏", /sidebar width to 0/],
    ["920×600 窗口", /set bounds to \{[0-9]+, [0-9]+, [0-9]+, [0-9]+\}/],
    ["未签名测试包 1080×600 窗口", /set bounds to \{100, 100, 1180, 700\}/],
    ["App 固定坐标", /position of item "token-station\.app" .* to \{310, 170\}/],
    ["Applications 固定坐标", /position of item "Applications" .* to \{610, 170\}/],
    ["安装脚本固定坐标", /position of item "安装 Token Station\.command"/],
    ["安装说明固定坐标", /position of item "installation-guide\.md"/],
    ["构建来源固定坐标", /position of item "构建来源\.txt"/],
    ["未签名提示固定坐标", /position of item "未签名测试版\.txt"/],
    ["无法打开说明固定坐标", /position of item "macos-troubleshooting\.md" .* to \{460, 440\}/],
    ["终端启动命令固定坐标", /position of item "终端启动命令\.txt" .* to \{640, 440\}/],
    ["正式包可省略构建来源", /if exists item "构建来源\.txt"[\s\S]*set position of item "构建来源\.txt"/],
    ["正式包可省略未签名提示", /if exists item "未签名测试版\.txt"[\s\S]*set position of item "未签名测试版\.txt"/],
    ["正式包可省略终端启动命令", /if exists item "终端启动命令\.txt"[\s\S]*set position of item "终端启动命令\.txt"/],
  ];
  for (const [label, pattern] of requiredFinderLayoutPatterns) {
    if (!pattern.test(finderLayout)) failures.push(`${finderLayoutPath} 缺少${label}`);
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
