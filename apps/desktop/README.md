# Token Station Desktop

桌面 App 的官方开发入口：

```bash
npm ci
npm run tauri:dev
```

这个命令先用锁文件构建五个官方 WASI adapter，再以 `bundled-plugins` feature 启动
Tauri。开发 App 因此和正式安装包一样，不依赖手工复制 AppData 插件。

不要直接运行 `npx tauri dev`：普通 Cargo 开发构建有意不启用内置插件 feature，
无法证明正式 App 的插件组成。仅开发前端时可运行 `npm run dev`。

完整工具链、Windows 前置条件和门禁见
[`docs/contributing/开发环境.md`](../../docs/contributing/开发环境.md)。
