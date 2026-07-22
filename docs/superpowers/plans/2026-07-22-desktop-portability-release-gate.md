# 桌面 App 可移植性与发布门实施计划

> 对应设计：`docs/superpowers/specs/2026-07-22-desktop-portability-release-gate-design.md`

## 任务 1：运行路径脱离源码目录

**文件：**

- 修改：`apps/desktop/src-tauri/src/lib.rs`

**步骤：**

1. 新增 `DesktopPaths`，统一保存配置、数据、可写插件和 Agent 集成目录。
2. 删除 `repo_root()` 及生产代码中的 `CARGO_MANIFEST_DIR`。
3. 让配置模板显式接收数据目录和插件目录。
4. 相对配置路径改为以配置文件父目录为基准。
5. 将 `AppStateManaged` 的构造和注册移入 Tauri `setup`。
6. 增加首次安装、相对路径、坏配置只读保护、目录创建和无源码回退测试。

## 任务 2：内嵌四个官方插件

**文件：**

- 修改：`apps/desktop/src-tauri/Cargo.toml`
- 新增：`scripts/build-desktop.sh`
- 新增：`scripts/audit-desktop-artifact.sh`
- 可能修改：`apps/desktop/package.json`

**步骤：**

1. 增加桌面版 `bundled-plugins` feature，并映射到 `token-station-cli/builtin-plugins`。
2. 发布脚本编译四个 WASI 插件并组装临时插件目录。
3. 设置 `TOKEN_STATION_PLUGINS_DIST`，启用 feature 后调用 Tauri 构建。
4. 审计脚本检查四个插件、源码绝对路径和平台签名状态。
5. 本地测试模式允许 ad-hoc 签名；正式模式不允许未签名产物通过。

## 任务 3：桌面版签名发布流水线

**文件：**

- 新增：`.github/workflows/desktop-release.yml`
- 新增或修改：`docs/release/桌面App构建签名与发布.md`

**步骤：**

1. macOS 分别构建 Apple Silicon 和 Intel 产物，从 CI 密钥读取签名与公证凭据。
2. Windows 构建安装程序，从 CI 密钥或签名服务读取签名配置。
3. 缺少生产凭据、签名失败、公证失败或产物审计失败时终止发布。
4. 只上传通过门禁的安装包，不改变现有 CLI 发布流程。

## 任务 4：代码与安装包验收

**命令：**

```bash
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
npm --prefix apps/desktop exec -- tsc --noEmit
npm --prefix apps/desktop exec -- vitest run
scripts/build-desktop.sh --local
```

检查安装包中不包含源码绝对路径，四个官方插件可用，macOS 本地测试包通过严格 codesign 校验。

## 任务 5：本机一次性迁移与真实测试

1. 停止旧版 App。
2. 备份新的系统应用目录。
3. 复制仓库根目录配置和数据，只改写 `data.dir` 与 `plugins.dir`。
4. 保留仓库根目录原文件，不复制 `plugins-dist`。
5. 安装新版 App，进行 ad-hoc 签名并启动。
6. 验证实际打开文件位于系统应用目录，插件注册表包含四个内嵌插件，网关可以启动并完成一次真实请求。
7. 失败时恢复系统目录备份，原数据不受影响。
