# If macOS Cannot Open Token Station

## Check the file

Use a download source that the team confirms. Check the DMG SHA-256 against its `.sha256` file. Delete
the file if the source or checksum does not match.

A file name that contains `UNSIGNED-UNNOTARIZED` identifies an unsigned test build. Apple did not
notarize this build. Gatekeeper can block it.

## Option 1: Open the app in Finder

1. Drag `token-station.app` to `/Applications`.
2. Open **Applications** in Finder.
3. Right-click `token-station.app` and select **Open**.
4. Select **Open** again in the macOS message.

You can also open **System Settings > Privacy & Security**. Select **Open Anyway** beside the security
message.

## Option 2: Remove quarantine from Token Station only

Confirm that the app is at `/Applications/token-station.app`. Run these commands in Terminal:

```bash
sudo xattr -dr com.apple.quarantine /Applications/token-station.app
open /Applications/token-station.app
```

Enter your administrator password when Terminal asks for it. Terminal does not show password characters.

The `xattr` command handles only Token Station. Do not change the path to `/Applications`, your home
folder, or the disk root. Do not disable Gatekeeper or SIP.

## Get help

Open an issue in [GitHub Issues](https://github.com/ballast-ai/token-station/issues). Include your macOS
version, Mac chip type, DMG file name, SHA-256, and the complete error text or screenshot. Do not include
passwords, tokens, private keys, or personal configuration.

---

# macOS 无法打开 Token Station 时怎么办

## 检查文件

只使用团队确认的下载来源。请用 DMG 同目录的 `.sha256` 文件核对 SHA-256。来源不明或校验值
不一致时，请删除文件并停止安装。

文件名含 `UNSIGNED-UNNOTARIZED` 表示该文件是未签名测试包。Apple 未对该文件完成公证，
Gatekeeper 可能拦截它。

## 方法一：使用 Finder 打开

1. 把 `token-station.app` 拖到 `/Applications`。
2. 在 Finder 中打开“应用程序”。
3. 右键点击 `token-station.app`，选择“打开”。
4. 在 macOS 提示中再次点击“打开”。

也可以打开“系统设置 > 隐私与安全性”，在安全提示旁点击“仍要打开”。

## 方法二：只解除 Token Station 的隔离标记

确认 App 位于 `/Applications/token-station.app` 后，在终端运行：

```bash
sudo xattr -dr com.apple.quarantine /Applications/token-station.app
open /Applications/token-station.app
```

终端要求管理员密码时，输入过程不会显示字符。输入完成后按回车。

这条 `xattr` 命令只处理 Token Station。不要把路径改成 `/Applications`、个人目录或磁盘
根目录。不要关闭 Gatekeeper，也不要关闭 SIP。

## 获取帮助

请在 [GitHub Issues](https://github.com/ballast-ai/token-station/issues) 中提供 macOS 版本、Mac
芯片类型、DMG 文件名、SHA-256 和完整错误文字或截图。不要上传密码、Token、私钥或个人配置。
