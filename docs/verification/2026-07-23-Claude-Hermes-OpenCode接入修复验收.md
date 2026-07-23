# Claude Code、Hermes、OpenCode 接入修复验收

> 日期：2026-07-23
> 分支：`codex/agent-gateway-taskbook`
> 结论：三项接入故障均已修复，目标回归、全量门禁与 Hermes 本机只读 smoke test 全部通过。

## 1. 背景与目标

用户实测发现：Claude Code 多安装选择后仍无法接入；Hermes 入口存在但版本探测进程超时；OpenCode 已显示“可接入”，实际执行却报 `code=agent_operation_rejected`、缺少虚拟 Key。

本轮目标是修正 Discovery、UI 准入和 Connector 运行时输入之间的契约，同时保持“任意版本可接入”的既定策略：不设置最低版本或白名单，不要求一定解析出 SemVer；但精确安装路径、可运行性、唯一 Connector、适配器就绪和确认令牌等安全条件继续保留。

## 2. 根因与修复

| Agent | 根因 | 修复 | 安全边界 |
| --- | --- | --- | --- |
| Claude Code | 后端能按精确路径消解多安装冲突，但前端仍按扫描列表中的 `MULTIPLE_INSTALLATIONS` 禁用按钮 | 用户明确选择带 `conflict_group` 的精确安装后允许发起计划，并传入 canonical path 与扫描版本 | 未选择仍禁用；服务端继续重新查找并校验扫描记录 |
| Hermes | `hermes version` 同步触发更新/网络检查，超过 2 秒探测时限 | 改用本地 `hermes --help`，新增 `SUCCESS_ONLY` matcher；成功退出即证明 runnable，不伪造版本 | 不执行安装、升级或配置写入；非零退出仍失败关闭 |
| OpenCode | 元数据声明不需要虚拟 Key，运行时传 `None`，但 Connector 补丁必须写入 Key | `requires_virtual_key=true`，前置检查与补丁消费者统一；运行时契约测试直接生成补丁 | Key 只在后端运行时内存中使用，不进入可序列化计划或日志 |

## 3. TDD 证据

### 3.1 RED

1. OpenCode runtime matrix：预期 token 为 `Some("vk-runtime-matrix")`，实际为 `None`。
2. Claude UI：选择 `/Users/x/.local/bin/claude` 后“一键接入”仍为 disabled。
3. Hermes Discovery：假入口的 `version` 分支阻塞，扫描返回 `VersionProbeTimeout`。

### 3.2 GREEN

- `commands_runtime_connector_and_input_boundary_matrix_is_fail_closed`：OpenCode 获得虚拟 Key，`connect_patch` 成功。
- `connector_contract_matrix_covers_metadata_preconditions_projection_and_disconnect`：OpenCode 能力声明、缺 Key 失败关闭与投影契约一致。
- `AgentRoutePage.test.tsx`：多安装初始禁用；选择精确路径后显示“可接入”，并以该路径和 `2.1.211` 调用 `planAgentConnection`。
- `hermes_discovery_uses_a_local_only_probe_and_accepts_versionless_success`：慢 `version` 不再被调用，`--help` 成功后记录 runnable，版本字段为空且无误导诊断。
- `builtin_registry_is_stably_ui_ordered_and_exposes_ui_metadata`：Hermes Registry 固定为 `--help + SUCCESS_ONLY + no retry`。

## 4. 全量门禁

### 4.1 Desktop frontend

```text
npm run test:coverage  PASS
Test Files  17 passed
Tests       135 passed
Statements  81.61%
Branches    76.23%
Functions   80.17%
Lines       85.06%

npm run build          PASS
TypeScript + Vite production build succeeded
```

### 4.2 Desktop Rust

```text
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check  PASS
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings  PASS
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml  PASS

Desktop lib:       165 passed / 0 failed / 1 ignored
YAML regression:     3 passed / 0 failed
```

默认忽略项是只读真实环境 smoke test，已显式运行：

```text
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml \
  discovery_real_platform_probe_is_read_only_and_never_installs \
  -- --ignored --nocapture

Result: 1 passed / 0 failed
```

该 smoke test 对当前机器安装的 Agent 只做发现和进程探测；Hermes 已返回 runnable。未安装、未升级、未写配置。

### 4.3 静态检查

```text
git diff --check  PASS
目标文件未发现 DEBUG 标记、dbg! 或 console.log
```

## 5. 通过条件判定

- [x] Claude Code 多安装必须先选择，选择精确路径后可以进入配置投影预览。
- [x] Hermes 探测不再执行包含网络更新检查的版本命令，无版本号不阻断接入。
- [x] OpenCode 运行时提供虚拟 Key，不再触发“接入缺少虚拟 Key”。
- [x] 未恢复最低版本、白名单或 SemVer 强制准入。
- [x] 目标回归、Rust/React 全量测试、Clippy、格式、生产构建和静态检查全部通过。
- [x] Hermes 本机只读 smoke test 通过。

## 6. 退出条件与结论

设计文档规定的四项成功退出条件全部满足，没有待处理的测试失败或必须补齐的交付物。本轮 Goal 可以标记为 `complete`。

## 7. 交付产物

1. 设计与执行方案：`docs/design/2026-07-23-Agent任意版本接入修复设计.md`
2. 最终验收记录：本文档
3. Claude UI 回归：`apps/desktop/src/pages/AgentRoutePage.test.tsx`
4. OpenCode runtime/Connector 回归：`apps/desktop/src-tauri/src/agent_integration/commands.rs`、`connectors/mod.rs`
5. Hermes Discovery/Registry 回归：`apps/desktop/src-tauri/src/agent_integration/discovery.rs`、`registry.rs`

## 8. 本机 App 构建与安装

用户确认更新后，使用仓库规定的发布脚本完成本地 Apple Silicon release 构建：

```text
scripts/build-desktop.sh --local  PASS

App: apps/desktop/src-tauri/target/release/bundle/macos/token-station.app
DMG: apps/desktop/src-tauri/target/release/bundle/dmg/token-station_0.1.0_aarch64.dmg
desktop artifact audit: PASS
```

旧 `/Applications/token-station.app` 已先退出并移动到可恢复备份，新 bundle 随后安装到原路径并启动。安装后验证：

```text
bundle id: com.tokenstation.desktop
version: 0.1.0
running pid: 40320
installed binary SHA-256: 8a53cd61d2b0e0cd6cafb89706dc74645755bec5329a9cfd6906a03e125e849e
built/installed SHA-256: MATCH
codesign --verify --deep --strict: PASS
com.apple.security.cs.allow-unsigned-executable-memory: true
```

旧 App 已备份到本机会话临时目录。该备份未删除，可用于本次会话期间回退；系统清理临时目录后不保证长期保留。

本轮未执行提交、推送、正式发布、Agent 自动升级或用户配置删除。安装的是当前电脑使用的 ad-hoc 签名本地测试包，不是已公证的对外发布包。
