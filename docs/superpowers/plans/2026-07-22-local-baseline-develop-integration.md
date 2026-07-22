# 本地核心基线吸收 develop 修复的实施计划

> 设计依据：`docs/superpowers/specs/2026-07-22-local-baseline-develop-integration-design.md`
>
> 核心基线：`389e701921dd8e6b15ca8e6e2b8a975b9db571ce`
>
> 远端参考：`5770ba5b15e2f91b22e7bb4c468e3f22e6bed90c`

## 执行约束

- 不合并整个 `origin/develop`，仅按提交语义移植。
- 每个批次先改测试或同步改测试，再运行该批次最小验证。
- 不改动用户现有未跟踪文件。
- 不接受宽泛 `allow`、非事务迁移、破坏式 keychain upsert、直接删除 Provider、自由文本错误持久化。
- 全部完成后统一运行 Rust、clippy、TypeScript 和 Vitest 验收。

## 批次 1：删除远程签名兼容目录

### 修改

- `apps/desktop/src-tauri/src/agent_integration/compatibility.rs`
  - 删除远程传输、签名响应、远程配置、目录选择、缓存、离线合并、HTTPS 校验和远程校验分支。
  - 保留 `CompatibilityCatalog::builtin()`、`CatalogSource::Builtin`、内置目录解析和黑名单校验。
  - 删除只覆盖远程签名、缓存、防篡改、离线和回滚的测试。
- `apps/desktop/src-tauri/src/agent_integration/commands.rs`
  - `perform_scan` 直接加载内置目录，来源固定为 `Builtin`。
  - 删除远程目录警告和 `Remote` 分支。
- `apps/desktop/src-tauri/Cargo.toml`、`Cargo.lock`
  - 全仓确认无其他调用后移除 `token_station_release` 桌面端依赖。

### 验证

```bash
rg -n "token_station_release|CatalogTransport|SignedCatalogResponse|select_catalog|PRODUCTION_CATALOG" apps/desktop/src-tauri
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml agent_integration::compatibility
```

## 批次 2：目录退化为纯黑名单并允许无版本接入

### 修改

- `apps/desktop/src-tauri/agent-registry/builtin-compatibility.json`
  - 删除 `verified`、`inferred`，只保留 `blocked`。
- `apps/desktop/src-tauri/src/agent_integration/compatibility.rs`
  - `AgentCompatibilityEntry` 只保留 `agent_id` 和 `blocked`。
  - 删除 `CompatibilityRule`、允许规则校验与推定匹配。
  - `evaluate_discovery` 顺序固定为多安装、可运行、准入类型、目录项、唯一 Connector、可解析版本的黑名单、只读预检、默认放行。
  - 无版本或非 semver 不再提前拒绝。
- `apps/desktop/src-tauri/src/agent_integration/types.rs`
  - 删除 `DetectedInferred`。
  - 删除只服务于白名单、推定和试验性路径的 `ReasonCode` 与 `AllowedAction`。
- `apps/desktop/src-tauri/src/agent_integration/plan.rs`
- `apps/desktop/src-tauri/src/agent_integration/transaction.rs`
- `apps/desktop/src-tauri/src/agent_integration/commands.rs`
  - 更新穷举匹配和测试夹具。
  - `expected_version` 仅在调用方提供版本时校验；无版本时仍执行预检、快照、事务和回滚。

### 测试

- 新增或改写以下 Rust 测试：
  - 未命中黑名单的任意 semver 默认 `DetectedVerified`。
  - 版本缺失和非 semver 默认可接入。
  - 黑名单对正式版本和 prerelease 仍有效。
  - 预检读取失败、解析失败仍拒绝。
  - Connector 为零个或多个时拒绝。
  - 多安装和损坏安装仍拒绝。
  - 无版本的 preview/connect 不要求 `expected_version`。
  - 有版本时保留 TOCTOU 不一致拒绝。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml agent_integration
rg -n "DetectedInferred|CompatibilityRule|verified|inferred|ConfirmExperimentalConnect" apps/desktop/src-tauri/src apps/desktop/src-tauri/agent-registry
```

## 批次 3：前端统一为可接入状态

### 修改

- `apps/desktop/src/pages/AgentRoutePage.tsx`
  - 删除 `isExperimentalCompatibility` 和“未在验证目录”提示。
  - 可接入状态统一显示“可接入”。
- `apps/desktop/src/App.test.tsx` 及直接相关页面测试
  - 删除试验性与 verified 的区分断言。
  - 覆盖统一可接入展示和不可接入负向状态。
- `apps/desktop/src/api.ts`
  - 如果前端类型显式枚举兼容状态，同步删除 `DETECTED_INFERRED`。

### 验证

```bash
cd apps/desktop
npx tsc --noEmit
npx vitest run src/App.test.tsx
```

## 批次 4：Agent 插件加载容错

### 修改

- `apps/cli/src/gateway.rs`
  - 参考远端 `3931d41`，将插件逐项加载，单个插件失败转为结构化诊断并继续。
  - 保持本地 Gateway、能力四态、出站策略、错误预算和回执实现。
- `apps/cli/tests/proxy.rs`
  - 增加一个损坏插件不影响其他插件和代理启动的测试。
  - 保留重复 Connector 冲突的拒绝行为。

### 验证

```bash
cargo test -p token-station-cli --test proxy
cargo test -p token-station-cli
```

## 批次 5：T7 Profile 与挂载

### 修改

- `apps/cli/src/config.rs`
  - 参考远端 `fdc8c22` 引入克制版 Profile 与 Agent 挂载配置。
  - 保留本地配置版本账本和运行时发布边界。
- `apps/desktop/src-tauri/src/lib.rs`
  - 增加 Profile 查询、保存、挂载和删除命令。
  - 所有写操作进入现有草稿、保存和应用流程，不直接改活动 Gateway。
- `apps/desktop/src/api.ts`、`apps/desktop/src/App.tsx`
  - 增加对应 IPC 类型和调用封装。
- `apps/desktop/src/pages/HomePage.tsx`
  - 增加内联 Profile 创建和管理，不使用 `window.prompt`。
- `apps/desktop/src/pages/AgentRoutePage.tsx`
  - 增加挂载板和持久化动作。
- `apps/desktop/src/App.css`、相关测试
  - 补齐最小样式及创建、挂载、解除挂载、删除和不存在目标测试。

### 验证

```bash
cargo test -p token-station-cli config
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cd apps/desktop
npx tsc --noEmit
npx vitest run
```

## 批次 6：T9 人类可读错误映射

### 修改

- `apps/desktop/src/errors.ts`
  - 参考远端 `45e7885` 建立结构化错误码到“哪层坏了、怎么办”的映射。
  - 未知错误使用受控兜底文案，不回显原始请求或上游正文。
- `apps/desktop/src/errors.test.ts`
  - 覆盖鉴权、余额、模型、网络、协议、Provider、本地代理和未知错误。
- `apps/desktop/src/components/RecentRequests.tsx`
  - 展示映射结果并支持复制请求 ID。
- `apps/desktop/src/App.css`
  - 增加最小错误提示和复制交互样式。

### 验证

```bash
cd apps/desktop
npx tsc --noEmit
npx vitest run src/errors.test.ts src/App.test.tsx
```

## 批次 7：Provider 安全更新体验

### 修改

- `apps/desktop/src/pages/AddProviderPage.tsx`
  - 识别同一 Provider 已存在时进入更新语义，保留明确提示。
- `apps/desktop/src/pages/AddProviderPage.test.tsx`
  - 覆盖新增、已存在更新、校验失败保留旧配置。
- `apps/desktop/src/App.tsx`、`apps/desktop/src/App.css`
  - 接入更新状态和最小样式。
- Rust 侧仅在现有命令缺少原子更新能力时补充，必须复用本地 Provider 生命周期和 keychain 安全写入。

### 验证

```bash
cd apps/desktop
npx tsc --noEmit
npx vitest run src/pages/AddProviderPage.test.tsx
```

## 批次 8：完整验收与红线复核

### 自动验证

```bash
scripts/prepare-desktop-test-plugins.sh
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cd apps/desktop
npx tsc --noEmit
npx vitest run
```

### 静态复核

```bash
rg -n "DetectedInferred|CompatibilityRule|PRODUCTION_CATALOG|CatalogTransport|token_station_release" apps/desktop/src-tauri
rg -n "max_redirects\(0\)|proxy\(None\)" apps/cli apps/desktop/src-tauri
git diff --check
```

### 人工不变量

- 黑名单、只读预检、Connector 唯一性、多安装、`InstalledBroken` 和发现失败仍会阻止接入。
- 无版本和非 semver 版本不会独立阻止接入。
- Agent 接入仍执行快照、事务和失败回滚。
- Runtime Supervisor、revision、热切换和 drain 未被远端简化实现覆盖。
- Provider 更新失败不删除旧配置或旧凭据。
- metrics 保持本地 schema v4，未引入远端自由文本 `error_detail`。
