# 桌面 App 构建、签名与发布

桌面版正式安装包必须通过 `scripts/build-desktop.sh` 构建。直接运行 `tauri build` 只能用于开发排查，不能作为发布产物，因为它可能没有启用官方插件内嵌、Updater 载荷和发布审计。

> 首个包含 Updater 的版本是引导版本。v1.1.2 及更早版本没有内置更新器和官方公钥，
> 必须最后手动安装一次引导版本；之后才能在 App 内更新。

## 本地 macOS 测试包

在仓库根目录运行：

```bash
scripts/build-desktop.sh --local
```

脚本会编译四个官方 WASI 插件，通过 `bundled-plugins` feature 把插件编进桌面二进制，构建 Tauri 安装包，并检查以下内容：

- 可执行文件不包含当前源码目录的绝对路径。
- 四个官方插件都已进入内嵌插件层。
- `.app` 通过 `codesign --verify --deep --strict`。
- `.app` 包含 Wasmtime 执行 WASM 插件所需的可执行内存 entitlement。

没有配置正式 Apple 证书时，本地模式使用 ad-hoc 签名。它适合当前电脑上的真实功能测试，但不能通过 Gatekeeper 正式发布门禁。

本地/源码构建不会内置正式 Updater 公钥，也不会生成 Updater 载荷。关于页会明确显示
当前构建只支持人工下载，不能出现可安装假象。

桌面端使用 Wasmtime 在运行时编译 WASM 插件。当前 Wasmtime 版本没有使用 macOS 的 `MAP_JIT`，因此 hardened runtime 需要 `com.apple.security.cs.allow-unsigned-executable-memory`。安装包审计会检查这个 entitlement，缺失时直接失败，避免 App 只在启动网关时被 macOS 以 `Code Signature Invalid` 终止。

指定 Rust 目标时使用：

```bash
scripts/build-desktop.sh --local --target aarch64-apple-darwin
```

## macOS 正式发布

正式模式需要 Developer ID Application 证书和 Apple 公证凭据：

```bash
export APPLE_SIGNING_IDENTITY="Developer ID Application: Example Corp (TEAMID)"
export APPLE_API_ISSUER="..."
export APPLE_API_KEY="..."
export APPLE_API_KEY_PATH="/secure/path/AuthKey_XXX.p8"
export TOKEN_STATION_UPDATER_PUBKEY="$(cat /secure/public/token-station-updater.pub)"
# 这里只允许使用一次性、无发布信任的临时 key 生成 .app.tar.gz；正式私钥不得上联网构建机。
export TAURI_SIGNING_PRIVATE_KEY_PATH="/tmp/throwaway-updater-ci.key"
scripts/build-desktop.sh --production --target aarch64-apple-darwin
```

也可以按 Tauri 官方支持的方式设置 `APPLE_ID`、`APPLE_PASSWORD` 和 `APPLE_TEAM_ID`。正式模式会检查 Developer ID 签名、Team ID、Gatekeeper、公证和票据装订状态，任一检查失败都会停止构建。

GitHub Actions 使用以下仓库密钥：

| 密钥 | 内容 |
|---|---|
| `APPLE_CERTIFICATE` | Base64 编码的 Developer ID `.p12` 文件 |
| `APPLE_CERTIFICATE_PASSWORD` | `.p12` 导出密码 |
| `APPLE_SIGNING_IDENTITY` | 完整 Developer ID Application 身份名称 |
| `APPLE_KEYCHAIN_PASSWORD` | CI 临时钥匙串密码 |
| `APPLE_API_ISSUER` | App Store Connect Issuer ID |
| `APPLE_API_KEY` | App Store Connect Key ID |
| `APPLE_API_KEY_CONTENT` | `.p8` 私钥原文 |

正式 Updater 公钥放在 GitHub Actions repository variable
`TOKEN_STATION_UPDATER_PUBKEY`，它可以公开。与它配对的正式私钥不得放入 GitHub Secret、
CI、联网开发机或构建日志。

## Windows 正式发布

首版公开桌面发布不支持 Windows，`desktop-release.yml` 会显式跳过 Windows job。若后续临时
构建 Windows MSI，只发布人工下载安装包；Windows 不注入 Updater 公钥、不生成 Updater
payload，也不写入 `latest.json`。

Windows CI 导入代码签名证书后，将证书指纹和时间戳地址交给构建脚本。需要以下仓库密钥：

| 密钥 | 内容 |
|---|---|
| `WINDOWS_CERTIFICATE` | Base64 编码的 `.pfx` 文件 |
| `WINDOWS_CERTIFICATE_PASSWORD` | `.pfx` 导出密码 |
| `WINDOWS_TIMESTAMP_URL` | 证书颁发机构提供的时间戳服务地址 |

构建脚本会生成只存在于临时目录的 Tauri Windows 签名配置，不会把证书指纹或私钥写入仓库。发布前会使用 Authenticode 同时验证主程序和安装程序。

## CI 发布门禁

`.github/workflows/desktop-release.yml` 在 `v*` tag 和手动触发时运行。首版只生成 Apple
Silicon 和 Intel 两个 macOS 安装包与应用内更新载荷，Windows job 显式跳过。CI 缺少
macOS 签名密钥时会直接失败，不会退化为未签名发布。

桌面 CI 与现有 CLI 可复现构建流水线相互独立。桌面任务只上传已经通过插件、路径、签名和公证检查的安装包。

## Updater 离线签名与发布

Tauri Updater 的签名校验不能关闭。生产构建通过
`bundle.createUpdaterArtifacts=true` 生成 macOS `.app.tar.gz` 更新载荷。
Tauri bundler 当前要求构建时存在签名 key，所以 CI 只生成一次性临时 key 让 bundler 产出
载荷；对应临时 `.sig` 不上传、不进入 GitHub Release，也不被 App 信任。Windows 首版不生成
Updater payload。

CI 完成后，发布者按以下顺序操作：

1. 从 Desktop Release Actions artifacts 下载两个已经完成系统代码签名和审计的 macOS 载荷：
   `darwin-aarch64`、`darwin-x86_64`。
2. 在离线签名机逐个执行：

   ```bash
   read -r -s -p 'Updater key password: ' TAURI_SIGNING_PRIVATE_KEY_PASSWORD
   export TAURI_SIGNING_PRIVATE_KEY_PASSWORD
   echo
   npx tauri signer sign \
     --private-key-path /offline/token-station-updater.key \
     /staging/token-station-darwin-aarch64.app.tar.gz
   unset TAURI_SIGNING_PRIVATE_KEY_PASSWORD
   ```

   对另一个 macOS 载荷重复执行。不要把密码写进仓库、聊天、shell history 或发布附件。
3. 人工核对两个 macOS 载荷、两个正式 `.sig`、版本号、系统代码签名和发布 tag 都属于同一批
   审计产物。
4. 生成静态 manifest：

   ```bash
   node scripts/create-desktop-update-manifest.mjs \
     --version 1.2.3 \
     --pub-date 2026-08-06T08:00:00Z \
     --release-base-url https://github.com/GlimpseEngine/token-station/releases/download/v1.2.3 \
     --notes-file /staging/release-notes.md \
     --output /staging/latest.json \
     --artifact darwin-aarch64=/staging/token-station-darwin-aarch64.app.tar.gz \
     --artifact darwin-x86_64=/staging/token-station-darwin-x86_64.app.tar.gz
   ```

   脚本要求两个 macOS 正式 `.sig` 与载荷相邻，并把签名内容内嵌进 `latest.json`；
   缺任一平台或签名会失败；传入 `windows-x86_64` 会被拒绝。
5. 把两个 macOS 载荷、两个正式 `.sig`、`latest.json`、DMG 和 CLI 发布资产上传到同一个
   draft Release。确认 manifest URL 能下载同一 Release 中的载荷后，才允许发布。

发布阻断条件：正式 keypair 尚未完成离线生成/备份、公钥不匹配、任一正式签名缺失、
系统代码签名/公证失败、`latest.json` 引用了临时签名，或尚未完成隔离的“引导版本 →
后续版本”真实更新演练。

## 参考资料

- [Tauri macOS 签名与公证](https://v2.tauri.app/zh-cn/distribute/sign/macos/)
- [Tauri Windows 代码签名](https://v2.tauri.app/zh-cn/distribute/sign/windows/)
- [Tauri GitHub Actions 发布流程](https://v2.tauri.app/zh-cn/distribute/pipelines/github/)
- [Tauri Updater 官方文档](https://v2.tauri.app/plugin/updater/)
