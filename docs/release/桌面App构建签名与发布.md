# 桌面 App 构建、签名与发布

桌面版正式安装包必须通过 `scripts/build-desktop.sh` 构建。直接运行 `tauri build` 只能用于开发排查，不能作为发布产物，因为它可能没有启用官方插件内嵌和发布审计。

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

## Windows 正式发布

Windows CI 导入代码签名证书后，将证书指纹和时间戳地址交给构建脚本。需要以下仓库密钥：

| 密钥 | 内容 |
|---|---|
| `WINDOWS_CERTIFICATE` | Base64 编码的 `.pfx` 文件 |
| `WINDOWS_CERTIFICATE_PASSWORD` | `.pfx` 导出密码 |
| `WINDOWS_TIMESTAMP_URL` | 证书颁发机构提供的时间戳服务地址 |

构建脚本会生成只存在于临时目录的 Tauri Windows 签名配置，不会把证书指纹或私钥写入仓库。发布前会使用 Authenticode 同时验证主程序和安装程序。

## CI 发布门禁

`.github/workflows/desktop-release.yml` 在 `v*` tag 和手动触发时运行，分别生成 Apple Silicon、Intel 和 Windows x64 安装包。CI 缺少签名密钥时会直接失败，不会退化为未签名发布。

桌面 CI 与现有 CLI 可复现构建流水线相互独立。桌面任务只上传已经通过插件、路径、签名和公证检查的安装包。

## 参考资料

- [Tauri macOS 签名与公证](https://v2.tauri.app/zh-cn/distribute/sign/macos/)
- [Tauri Windows 代码签名](https://v2.tauri.app/zh-cn/distribute/sign/windows/)
- [Tauri GitHub Actions 发布流程](https://v2.tauri.app/zh-cn/distribute/pipelines/github/)
