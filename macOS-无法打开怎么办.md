# macOS 无法打开 Token Station 时怎么办

## 先确认文件安全

只使用团队确认的下载地址。核对 DMG 同目录 `.sha256` 文件中的 SHA-256。来源不明或校验值
不一致时，请删除文件并停止安装。

文件名含 `UNSIGNED-UNNOTARIZED` 表示该包未签名、未经 Apple 公证，仅供测试。macOS
拦截该文件属于正常行为。

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
根目录。不要全局关闭 Gatekeeper，也不要关闭 SIP。

## 仍然无法打开

请在 [GitHub Issues](https://github.com/ballast-ai/token-station/issues) 中提供以下信息：

- macOS 版本和 Mac 芯片类型。
- DMG 文件名和 SHA-256。
- 完整错误文字或截图。

不要上传密码、Token、私钥或个人配置。
