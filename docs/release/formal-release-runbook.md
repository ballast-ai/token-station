# 正式发布 Runbook：从配置缺口到已验证的发布闭环

日期：2026-08-25　状态：待 lv 执行（配置密钥与触发发布需要维护者单独授权）

## 1. 为什么现在的 Release run 是绿的但什么都没发布

`release.yml` 的 `release-mode` job 读取仓库变量 `TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED`；
不为 `true` 时 build、reproducibility、desktop-macos、publish 四个 job 全部 skip，workflow
仍显示 success。v1.2.1–v1.3.0 的每次 Release run 都在 5–7 秒内结束——只跑了模式判断。
**Workflow 绿色 ≠ 正式制品构建成功。**

## 2. 配置盘点（2026-08-25 实测，`gh variable list` / `gh secret list`）

| 项 | 类型 | 现状 |
|---|---|---|
| `TOKEN_STATION_UPDATER_PUBKEY` | variable | ✅ 已配置（2026-08-20） |
| `TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED` | variable | ❌ 缺失，发布链路因此整体跳过 |
| `TOKEN_STATION_RELEASE_PUBKEY_HEX` | variable | ❌ 缺失（64 位小写 hex，`check-release-readiness.mjs --formal` 强制校验） |
| `APPLE_CERTIFICATE` / `APPLE_CERTIFICATE_PASSWORD` / `APPLE_SIGNING_IDENTITY` / `APPLE_KEYCHAIN_PASSWORD` | secret | ❌ 全部缺失（Developer ID 签名） |
| `APPLE_API_ISSUER` / `APPLE_API_KEY` / `APPLE_API_KEY_CONTENT` | secret | ❌ 全部缺失（公证 App Store Connect key） |

## 3. 一次性配置步骤（维护者，线下）

1. **CLI 发布密钥对**：在离线环境执行
   `cargo run --locked -p token-station-release --bin ts-release -- keygen`。
   私钥（hex 种子文件）离线保存，永不进 CI；公钥配置为仓库变量
   `TOKEN_STATION_RELEASE_PUBKEY_HEX`。
2. **Updater 私钥**：`TOKEN_STATION_UPDATER_PUBKEY` 已在线；确认对应的 Tauri updater
   私钥仍离线可用（`sign-formal-release.sh --updater-key` 需要它）。CI 只用临时钥打包，
   正式签名全部离线完成。
3. **Apple 凭证**：配置上表 7 个 secret（Developer ID 证书 base64、证书密码、签名身份、
   临时 keychain 密码、App Store Connect issuer/key id/key 内容）。
4. 最后设置 `TOKEN_STATION_FORMAL_ARTIFACTS_ENABLED=true`。在此之前推 tag 不会产生任何制品。

## 4. 发布验证清单（下一个 v* tag，逐项确认，不看总状态）

- [ ] `release-mode` 输出「已启用正式 CLI 二进制构建」。
- [ ] `build` 四个平台（linux x86_64 / linux aarch64 / macOS aarch64 / macOS x86_64）各自产出 `dist-*` artifact，`check-release-readiness.mjs --formal` 通过。
- [ ] `reproducibility` 两次干净构建 sha256 逐字节一致。
- [ ] `desktop-macos` 两个 target 各产出 1 个 DMG + 1 个 app.tar.gz，签名与公证步骤真实执行。
- [ ] `publish` 生成未签名 manifest 并创建 **draft** release。
- [ ] 线下三步：`prepare-formal-release.sh`（下载并校验草稿）→ `sign-formal-release.sh`（CLI manifest 签名 + 双 updater 制品签名，双公钥回验）→ `publish-formal-release.sh`（资产比对后 draft 转正式）。
- [ ] 事后：`verify-release.sh` 对已发布制品做终验。

任何一项 skip 或造假都视为发布失败；不允许以 workflow 总状态绿色作为完成依据。
