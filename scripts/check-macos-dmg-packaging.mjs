#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const failures = [];
const read = (relativePath) => {
  const absolutePath = path.join(root, relativePath);
  if (!fs.existsSync(absolutePath)) {
    failures.push(`${relativePath} 不存在`);
    return "";
  }
  return fs.readFileSync(absolutePath, "utf8");
};

const requiredFiles = [
  "packaging/macos/README.md",
  "packaging/macos/background.png",
  "packaging/macos/background.svg",
  "packaging/macos/configure-dmg-layout.applescript",
  "scripts/package-macos-dmg.sh",
  "scripts/audit-macos-dmg.sh",
];
for (const relativePath of requiredFiles) read(relativePath);

for (const removedPath of [
  "packaging/macos/README.zh-CN.md",
  "packaging/macos/Install Token Station.command",
  "packaging/macos/安装 Token Station.command",
  "packaging/macos/终端启动命令.txt",
]) {
  if (fs.existsSync(path.join(root, removedPath))) failures.push(`${removedPath} 不应再作为独立入口存在`);
}

const expectedCommand =
  'sudo xattr -dr com.apple.quarantine "/Applications/token-station.app" && open "/Applications/token-station.app"';
const readme = read("packaging/macos/README.md");
for (const phrase of [
  "Install Token Station",
  "unsigned and not notarized",
  "Terminal hides password characters",
  "SHA-256",
]) {
  if (!readme.includes(phrase)) failures.push(`README.md 缺少“${phrase}”说明`);
}
const executableLines = readme.split(/\r?\n/).filter((line) => line.startsWith("sudo "));
if (executableLines.length !== 1 || executableLines[0] !== expectedCommand) {
  failures.push("README.md 必须只包含一条针对 canonical App 的可执行命令");
}
if (/spctl\s+--master-disable/.test(readme)) failures.push("README.md 不能关闭 Gatekeeper");
if (/xattr[^\n]*(?:\/Applications[\s"']|~\/|\$HOME)/.test(readme)) {
  failures.push("README.md 不能扩大 quarantine 清理范围");
}

const background = fs.readFileSync(path.join(root, "packaging/macos/background.png"));
if (background.subarray(1, 4).toString("ascii") !== "PNG") failures.push("background.png 不是 PNG 图片");

const packager = read("scripts/package-macos-dmg.sh");
const packagerPatterns = [
  ["Applications 精确链接", /ln -s \/Applications .*Applications/],
  ["唯一可见 README", /cp "\$readme" "\$stage\/README\.md"/],
  ["隐藏背景目录", /mkdir "\$stage\/\.background"/],
  ["背景资源", /background\.png/],
  ["隐藏发布元数据", /mkdir "\$stage\/\.release-metadata"/],
  ["DMG 创建", /hdiutil create/],
  ["DMG 签名", /codesign .*--sign/],
  ["DMG 公证", /notarytool submit/],
  ["票据装订", /stapler staple/],
  ["显式未签名测试模式", /--unsigned-test/],
  ["测试 DMG 警告文件名", /UNSIGNED-UNNOTARIZED/],
  ["App 源码提交", /--app-source-commit/],
  ["打包源码提交", /packaging_source_commit/],
  ["测试包隐藏风险标记", /unsigned-test-warning\.txt/],
  ["测试包隐藏构建来源", /provenance\.txt/],
  ["可写布局镜像", /-format UDRW/],
  ["挂载真实布局镜像", /hdiutil attach "\$writable_dmg" -readwrite -nobrowse -mountpoint "\$layout_mount"/],
  ["写入真实 Finder 布局", /osascript "\$finder_layout_script" configure "\$layout_mount"/],
  ["Finder 元数据", /layout_mount\/\.DS_Store/],
  ["清理 Finder 临时目录", /layout_mount\/\.fseventsd/],
  ["只读压缩输出", /hdiutil convert[\s\S]*-format UDZO/],
  ["通过审计后发布", /audit-macos-dmg\.sh[\s\S]*\/bin\/mv -n "\$temporary_dmg" "\$output_path"/],
];
for (const [label, pattern] of packagerPatterns) {
  if (!pattern.test(packager)) failures.push(`scripts/package-macos-dmg.sh 缺少${label}`);
}
if (/APPLE_PASSWORD|--password/.test(packager)) failures.push("打包器不能把 Apple 密码放进进程参数");
if (/README\.zh-CN|Install Token Station\.command|终端启动命令\.txt/.test(packager)) {
  failures.push("打包器仍会加入多余的可见帮助文件");
}

const auditor = read("scripts/audit-macos-dmg.sh");
for (const [label, pattern] of [
  ["只读挂载", /hdiutil attach .*readonly/],
  ["根目录精确清单", /expected_entries=/],
  ["唯一 README", /mounted_readme="\$mount_point\/README\.md"/],
  ["隐藏背景检查", /mounted_background_dir/],
  ["隐藏元数据检查", /mounted_metadata/],
  ["Applications 链接检查", /readlink .*mounted_applications/],
  ["App 签名检查", /codesign --verify --deep --strict/],
  ["Gatekeeper 检查", /spctl .*--assess/],
  ["票据检查", /stapler validate/],
  ["版本检查", /CFBundleShortVersionString/],
  ["架构检查", /lipo -archs/],
  ["ad-hoc App 检查", /Signature=adhoc/],
  ["Finder 布局回读", /configure-dmg-layout\.applescript[\s\S]*inspect/],
  ["系统临时目录检查", /\.fseventsd/],
]) {
  if (!pattern.test(auditor)) failures.push(`scripts/audit-macos-dmg.sh 缺少${label}`);
}

const finderLayout = read("packaging/macos/configure-dmg-layout.applescript");
for (const [label, pattern] of [
  ["配置模式", /configure/],
  ["审计模式", /inspect/],
  ["图标视图", /current view to icon view/],
  ["关闭自动排列", /arrangement to not arranged/],
  ["128 图标尺寸", /icon size to 128/],
  ["1180×640 窗口", /set bounds to \{100, 100, 1280, 740\}/],
  ["背景图片", /backgroundFile/],
  ["App 坐标", /position of item "token-station\.app" .*\{300, 240\}/],
  ["Applications 坐标", /position of item "Applications" .*\{880, 240\}/],
  ["README 坐标", /position of item "README\.md" .*\{590, 455\}/],
]) {
  if (!pattern.test(finderLayout)) failures.push(`configure-dmg-layout.applescript 缺少${label}`);
}
if (/README\.zh-CN|Install Token Station\.command|终端启动命令\.txt/.test(finderLayout)) {
  failures.push("Finder 布局仍引用多余入口");
}

const buildScript = read("scripts/build-desktop.sh");
if (!/package-macos-dmg\.sh/.test(buildScript)) failures.push("build-desktop.sh 没有调用 DMG 打包器");
const appBundleSelections = (buildScript.match(/--bundles app|macos_bundle_kind="app"/g) ?? []).length;
if (appBundleSelections < 2) failures.push("build-desktop.sh 没有限制 Tauri 只生成 App");

for (const workflowPath of [".github/workflows/full-ci.yml", ".github/workflows/desktop-release.yml"]) {
  const workflow = read(workflowPath);
  if (!workflow.includes("node scripts/check-macos-dmg-packaging.mjs")) {
    failures.push(`${workflowPath} 没有检查 DMG 策略`);
  }
}

if (failures.length) {
  console.error("macOS DMG 还不能发布：");
  for (const failure of failures) console.error(`- ${failure}`);
  process.exit(1);
}
console.log("macOS DMG packaging policy: PASS");
