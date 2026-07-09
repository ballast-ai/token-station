# Tauri 典型架构技术说明

> 调研文档 · 2026-07-08
> 适用版本：Tauri 2.x（与 1.x 的差异在文中单独标注）

## 1. 一句话概括

Tauri 的核心思想：**用操作系统自带的 WebView 渲染前端界面，用 Rust 进程承载后端逻辑，两者之间通过 IPC 通信**。最终产物是一个单一的原生可执行文件。

## 2. 整体架构

```
┌─────────────────────────────────────────────┐
│              Tauri 应用（单个可执行文件）        │
│                                             │
│  ┌───────────────┐       ┌────────────────┐ │
│  │  Core 进程     │  IPC  │  WebView 窗口   │ │
│  │  (Rust)       │◄─────►│  (系统 WebView) │ │
│  │               │       │                │ │
│  │ · 业务逻辑     │       │ · HTML/CSS/JS  │ │
│  │ · 文件/网络/DB │       │ · React/Vue/   │ │
│  │ · 系统 API    │       │   Svelte 随意   │ │
│  │ · 窗口管理     │       │ · 无 Node.js   │ │
│  │ · 插件系统     │       │                │ │
│  └───────────────┘       └────────────────┘ │
└─────────────────────────────────────────────┘
```

## 3. 三个关键层

### 3.1 前端层（WebView）

- 就是普通的 Web 项目，可用任何框架（React / Vue / Svelte / 纯 HTML），构建产物是静态文件。
- 渲染引擎使用**操作系统自带的 WebView**：

| 平台 | WebView 实现 | 内核 |
|------|-------------|------|
| macOS / iOS | WKWebView | WebKit（Safari） |
| Windows | WebView2 | Chromium（Edge） |
| Linux | WebKitGTK | WebKit |
| Android | Android System WebView | Chromium |

- 这是 Tauri 与 Electron 最本质的区别：Electron 打包整个 Chromium（80MB+），Tauri 复用系统组件，安装包可以做到 3–10MB。

### 3.2 核心层（Rust Core 进程）

- 应用的入口和"大脑"，负责窗口创建、系统托盘、原生菜单、应用生命周期。
- 所有真正需要系统能力的操作都在这里执行：文件读写、数据库、网络请求、调用原生 API。
- 自定义后端逻辑以 **Command** 形式暴露——用 `#[tauri::command]` 宏标注的 Rust 函数，前端可直接调用。
- 底层依赖两个库：
  - **wry**：跨平台 WebView 抽象层
  - **tao**：跨平台窗口管理（fork 自 winit）

### 3.3 IPC 通信层

前端与 Rust 之间有两种主要通信方式：

**Command（前端 → Rust，请求/响应模式）**

```javascript
// 前端：像调用本地 API 一样，返回 Promise
import { invoke } from '@tauri-apps/api/core';
const result = await invoke('read_config', { path: 'app.toml' });
```

```rust
// Rust 侧
#[tauri::command]
fn read_config(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

// 注册到应用
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![read_config])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

**Event（双向，发布/订阅模式）**

```rust
// Rust 主动推送（如后台任务进度）
app.emit("download-progress", ProgressPayload { percent: 42 })?;
```

```javascript
// 前端监听
import { listen } from '@tauri-apps/api/event';
await listen('download-progress', (event) => {
  console.log(event.payload.percent);
});
```

- Command 适合"前端要数据/要执行动作"；Event 适合"Rust 侧有状态变化要通知界面"。
- Tauri 2.x 还提供 **Channel**，用于大量数据的高吞吐单向流式传输（如下载进度、日志流），比 Event 更高效。

## 4. 安全模型（Tauri 的设计重点）

- WebView 里的 JS **默认没有任何系统能力**——不像 Electron 那样存在把 Node 能力泄漏进渲染进程的风险面。
- 所有能力必须通过 **Capability / Permission 配置**（Tauri 2.x）显式授权：哪个窗口能调用哪些 command、能访问哪些文件路径，都要在 JSON 中声明。

```json
// src-tauri/capabilities/default.json
{
  "identifier": "default",
  "windows": ["main"],
  "permissions": [
    "core:default",
    "fs:allow-read-text-file",
    { "identifier": "fs:scope", "allow": ["$APPDATA/**"] }
  ]
}
```

- 官方插件（fs、http、shell、dialog、notification 等）全部走这套权限体系。
- Tauri 1.x 使用的是 `tauri.conf.json` 里的 `allowlist`，2.x 迁移为更细粒度的 capabilities 机制。

## 5. 插件系统与 Sidecar

**插件（Plugin）**

- 官方插件覆盖常见需求：`fs`、`http`、`shell`、`dialog`、`store`（键值存储）、`sql`、`updater`（自动更新）、`deep-link` 等。
- 插件 = Rust 侧实现 + JS 绑定 + 权限声明，社区可自行扩展。

**Sidecar（内嵌外部二进制）**

- 可以把一个外部可执行文件（如 Python 服务、Node 脚本编译产物、ffmpeg）打包进应用，由 Rust 侧启动和管理。
- 适合复用已有非 Rust 技术栈的后端逻辑，代价是包体积增大和进程管理复杂度。

## 6. 典型项目目录

```
my-app/
├── src/                    ← 前端代码（Vite + 任意框架）
├── src-tauri/              ← Rust 后端
│   ├── src/
│   │   ├── main.rs         ← 入口
│   │   └── lib.rs          ← 注册 commands、插件
│   ├── Cargo.toml
│   ├── tauri.conf.json     ← 窗口、打包、更新配置
│   ├── capabilities/       ← 权限声明（2.x）
│   └── icons/
├── package.json
└── vite.config.ts
```

开发与构建命令：

```bash
npm run tauri dev     # 开发模式：前端热更新 + Rust 自动重编译
npm run tauri build   # 产出平台安装包（.dmg / .msi / .deb / .AppImage）
```

## 7. 与 Electron 的对比

| 维度 | Tauri | Electron |
|------|-------|----------|
| 渲染引擎 | 系统 WebView | 内置 Chromium |
| 后端运行时 | Rust | Node.js |
| 安装包体积 | 3–10MB | 80–150MB |
| 内存占用 | 低 | 高 |
| 跨平台渲染一致性 | 差一些（三平台内核不同） | 完全一致 |
| 安全模型 | 默认零权限 + 显式授权 | 需手动隔离渲染进程 |
| 移动端 | 2.x 原生支持 iOS/Android | 不支持 |
| 生态成熟度 | 较新，快速发展 | 非常成熟 |
| 团队技能要求 | 需要写 Rust | 全 JS 技术栈 |

## 8. 主要权衡与实际的坑

1. **WebView 兼容性**：三个平台内核不同（WebKit / Chromium / WebKitGTK），CSS 与 JS API 的兼容性要像做跨浏览器 Web 一样对待。macOS 上没有的 Chromium 特性（如某些新 CSS 属性）不能用。这是换取小体积的核心代价。
2. **Rust 门槛**：复杂后端逻辑需要 Rust 能力；团队没有 Rust 经验时，Command 层容易写得薄而杂。可用 sidecar 缓解，但引入新的复杂度。
3. **Linux 体验**：WebKitGTK 的渲染质量和性能相对最弱，Linux 用户占比高的产品要重点测试。
4. **编译时间**：Rust 编译慢，首次构建和 CI 时间明显长于纯 JS 项目。
5. **自动更新**：官方 `updater` 插件可用，但需要自建更新服务器或使用静态 JSON 方案，不如 Electron 生态（electron-updater + GitHub Releases）开箱即用。

## 9. 适用判断

**适合 Tauri**：追求小体积、低内存、强安全边界的桌面工具；团队有 Rust 能力或后端逻辑较薄；需要同一代码库覆盖桌面 + 移动端（2.x）。

**适合 Electron**：需要三平台像素级一致的渲染；重度依赖 Node 生态；团队纯 JS 技术栈且交付时间紧。

## 参考

- 官方文档：https://tauri.app/
- 架构说明：https://tauri.app/concept/architecture/
- IPC 概念：https://tauri.app/concept/inter-process-communication/
- wry：https://github.com/tauri-apps/wry
- tao：https://github.com/tauri-apps/tao
