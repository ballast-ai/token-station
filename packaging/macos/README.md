# Install Token Station

## Before you continue

Download this DMG only from [Token Station GitHub Releases](https://github.com/ballast-ai/token-station/releases).
Check its SHA-256 against the adjacent `.sha256` file. Stop if the source or checksum does not match.

This package is unsigned and not notarized by Apple. macOS can block its first launch. Use this test build
only if you trust its source. A future standard release must use a Developer ID signature and Apple notarization.

## Install

1. Drag `token-station.app` to the `Applications` shortcut.
2. Open **Applications** in Finder.
3. Right-click `token-station.app`, select **Open**, and then select **Open** again.

You can also select **Open Anyway** in **System Settings > Privacy & Security**.

If macOS still blocks the App, use the shared Terminal fallback at the end of this file. The command handles
only `/Applications/token-station.app`. It does not disable Gatekeeper or SIP. Terminal does not show password characters
when macOS requests your administrator password.

---

# 安装 Token Station

## 继续之前

只从 [Token Station GitHub Releases](https://github.com/ballast-ai/token-station/releases) 下载此 DMG。
请用同页的 `.sha256` 文件核对 SHA-256。来源不明或校验值不一致时，请停止安装。

这个测试包未签名且未经 Apple 公证。macOS 可能拦截首次启动。只有确认来源可信时才使用它。
后续标准发布包必须具备 Developer ID 签名和 Apple 公证。

## 安装

1. 把 `token-station.app` 拖到 `Applications` 快捷方式。
2. 在 Finder 中打开“应用程序”。
3. 右键点击 `token-station.app`，选择“打开”，再在提示中点击“打开”。

也可以打开“系统设置 > 隐私与安全性”，点击“仍要打开”。

如果仍然打不开，请使用文末的终端兜底命令。这条命令只处理
`/Applications/token-station.app`，不会关闭 Gatekeeper，也不会修改 SIP。macOS 请求管理员
密码时，终端不会显示密码字符。

## Terminal fallback / 终端兜底

Confirm that the App is in Applications. Copy this complete line into Terminal.

请先确认 App 已位于 Applications，然后把下面整行命令复制到终端。

```bash
sudo xattr -dr com.apple.quarantine "/Applications/token-station.app" && open "/Applications/token-station.app"
```

Enter an administrator password only for a DMG that you downloaded from the official Release page.

只有从官方 Release 页面下载并完成校验后，才可以输入管理员密码。
