# Token Station Agent 自动发现、版本兼容与安全接入实施计划

> 日期：2026-07-20
> 状态：待用户确认后实施
> 对应设计：[`2026-07-20-agent-discovery-version-compatibility-design.md`](../specs/2026-07-20-agent-discovery-version-compatibility-design.md)
> 当前授权：仅编写实施计划，不修改业务代码

## 1. 目标与硬边界

本计划把已确认的正式方案拆成可独立提交、验证和回退的任务，最终交付：

- Registry 驱动的 Agent 列表；
- 启动时和手动触发的只读自动发现；
- 版本、环境、损坏安装和多实例冲突识别；
- 内置兼容目录与签名远程兼容目录；
- 版本化 Connector；
- 配置计划、差异确认、加密快照、原子写入、断开和恢复；
- Claude Code、Codex、OpenCode、OpenClaw、Hermes 五种 Agent；
- 整体 80%、新增安全关键模块 90% 的行覆盖率；
- `router-core` 变更硬门禁。

硬红线：

1. 不修改 `crates/router-core/**`；
2. 不修改路由算法、规则匹配、评分、选池、排序、fallback、`RouterConfig` 或 `Decision`；
3. 不自动安装或升级任何第三方 Agent；
4. 不远程下发或执行 Connector 代码；
5. 不在扫描阶段修改外部 Agent 配置；
6. 不修改用户全局配置或正在运行的 Agent 进程来完成测试；
7. 不把多路由、供应商故障转移、视频异步或企业租户能力混入本计划；
8. 未经单独授权不 push、不发布、不创建远程兼容目录生产制品。

每个任务中的“提交”均指功能分支本地提交建议，不代表本轮已授权执行。

## 2. 实施前输入门

| 输入 | 阻塞范围 | 默认处理 |
|---|---|---|
| 首发平台顺序 | 真实跨平台验收 | macOS 先做真实验收；其余平台先做 fixtures，不对外承诺 |
| 兼容目录生产 URL | 远程目录上线 | 先完成内置目录和本地 mock；生产拉取保持关闭 |
| 兼容目录 Ed25519 公钥 | 远程目录验签 | 空公钥时 fail closed，只用内置目录 |
| 签名密钥托管人与吊销流程 | 目录发布 | 不在仓库或 CI 生成生产私钥，不发布生产目录 |
| Hermes 官方仓库和首测版本 | Hermes Connector | 只允许完成候选发现，不标记正式支持 |
| 五种 Agent 的正式支持版本范围 | 对外兼容承诺 | 使用测试版本构建内部矩阵，不扩大版本范围 |
| 灰度用户与故障响应人 | 灰度发布 | 完成代码和隔离验收后停在发布门前 |

这些输入不阻塞 Task 0–8 的本地基础框架，但会阻塞 Task 10–13 的正式支持和发布。

## 3. 全影响面地图

| 维度 | 文件/符号或系统 | 动作 | 验证 | 状态 |
|---|---|---|---|---|
| 设计契约 | `docs/superpowers/specs/2026-07-20-*.md` | 作为唯一设计基线，不反向扩大范围 | 对照最终验收清单 | 已确认 |
| 核心红线 | `crates/router-core/**` | 不修改；增加专项 diff 门禁 | 模拟违规变更必须使 CI 失败 | 必须新增门禁 |
| 桌面后端入口 | `apps/desktop/src-tauri/src/lib.rs` | 将 Agent 逻辑迁出单文件；注册新 IPC | Tauri 单测、IPC 契约测试 | 必须修改 |
| Agent 控制面 | `apps/desktop/src-tauri/src/agent_integration/**` | 新增 Registry、发现、兼容、Connector、事务、快照 | 单测、故障注入、覆盖率 | 必须新增 |
| 前端调用方 | `apps/desktop/src/api.ts` | 静态 `AgentKind` 改为结构化视图和计划 API | TypeScript、契约测试 | 必须修改 |
| 前端页面 | `App.tsx`、`pages/Agents.tsx`、组件和 CSS | 静态按钮改为动态 Agent 页面和确认流程 | Vitest、交互测试、build | 必须修改 |
| 配置数据 | 外部 Agent JSON/TOML/YAML | 只按 owned paths 生成计划和事务写入 | 往返、并发、非法配置、恢复测试 | 必须修改行为 |
| 快照数据 | Tauri app data 下 `agent-snapshots/**` | 新增加密载荷和索引，默认保留最近 5 份 | 密文、权限、损坏恢复测试 | 必须新增 |
| 密钥 | OS keychain | 新增本机 snapshot master key | mock keyring + 真实 keychain 测试 | 必须新增 |
| 兼容数据 | 内置 JSON + 远程签名 JSON | 新增 schema、验签、缓存、回退 | mock HTTP、篡改、过期、回滚测试 | 必须新增 |
| 现有协议 Adapter | `plugins/official/agent-*` | 第一批优先复用，不因 Agent 名增加特判 | conformance、真实 E2E | 已检查，预计不改 |
| Server/Gateway | `apps/cli/src/server.rs`、`gateway.rs` | 复用现有三协议入口 | 现有 proxy 回归 | 已检查，预计不改 |
| Protocol/IR | `crates/protocol/**` | 不为配置接入扩展 IR | conformance 回归 | 已检查，不在范围 |
| Plugin Runtime | `crates/plugin-runtime/**` | 不扩大 WASM 文件或网络权限 | runtime 回归 | 已检查，不在范围 |
| HTTP/OpenAPI/SDK | 无新 HTTP 管理 API | 特权操作继续仅走 Tauri IPC | 搜索路由与生成物无新增 | 已检查，不适用 |
| SQL/迁移 | 当前不使用数据库保存快照 | 使用版本化文件索引，不新增 SQL migration | 数据目录 fixtures | 已检查，不适用 |
| 运行环境 | PATH、平台路径、WSL、配置环境变量 | 只读检测；参数化命令、超时、限长 | 平台 fixtures、真实机验收 | 必须新增 |
| Tauri capabilities | `src-tauri/capabilities/default.json` | 不开放前端 shell/fs capability | capability diff 审计 | 待核实后预计不改 |
| Rust 依赖 | `src-tauri/Cargo.toml`、`Cargo.lock` | 增加 semver、超时、哈希、AEAD、keyring 等最小依赖 | clippy、deny、audit | 必须修改 |
| Workspace 边界 | 根 `Cargo.toml` 明确 exclude 桌面 Tauri crate | 不把桌面强塞入 workspace；CI 单独测试、审计、统计覆盖率 | 独立 desktop-rust job | 必须新增门禁 |
| 前端测试依赖 | `package.json`、lockfile | 增加 Vitest、Testing Library、coverage | test、build | 必须修改 |
| CI | `.github/workflows/ci.yml`、专项脚本 | 路径红线、前端测试、覆盖率、平台测试 | PR 模拟和完整 CI | 必须修改 |
| CODEOWNERS | `.github/CODEOWNERS` | 当前 `* @actly` 已覆盖新目录 | owner diff 检查 | 已检查，不需要改 |
| 发布信任链 | `crates/release`、`ts-release sign` | 复用 `verify_bytes/sign_bytes`，不引入第二套算法 | 签名/篡改测试 | 预计只依赖，不改契约 |
| 发布流程 | `.github/workflows/release.yml`、目录发布流程 | App 发布与兼容目录发布分离 | 离线签名演练 | 生产发布待授权 |
| 文档 | Agent 接入机制、测试、发布和维护手册 | 更新能力、恢复、目录发布和准入流程 | 链接和事实核对 | 必须修改 |

## 4. 最小实施顺序

```text
基线与红线
→ 测试/覆盖率基础设施
→ 领域契约与 Registry
→ 现有 Connector 抽离但保持旧行为
→ 只读 Discovery
→ 兼容目录与状态机
→ 配置事务和加密快照
→ 新 IPC 与动态 UI
→ 三个现有 Agent 迁移
→ OpenClaw
→ Hermes
→ 全链路、覆盖率与灰度
```

先建立防护和失败测试，再进入配置写入。任何阶段都不得以“后面会补测试”为由跳过当前退出条件。

## Task 0：冻结事实基线和实施工作区

### 文件

- 新增 `docs/verification/2026-07-20-agent-integration-baseline.md`；
- 不修改业务代码。

### 动作

1. 记录实施分支起点提交、`crates/router-core/**` 文件清单和目录 SHA-256；
2. 记录当前三个静态 Agent 入口、三个配置函数和对应测试；
3. 记录五种 Agent 的本机存在状态和版本；
4. 固定每个 Agent 的官方仓库/文档 URL、取证日期和目标版本；
5. 特别确认 Hermes 指 NousResearch Hermes Agent；
6. 只记录环境变量名和路径，不记录密钥值；
7. 记录当前 Rust/前端测试命令和覆盖率工具状态。

### 核对命令

```bash
git rev-parse HEAD
git ls-files crates/router-core | sort
git ls-tree -r --full-tree b0c96a846d6db712c98248947cb3354c4bb7d157 -- crates/router-core
git diff --no-ext-diff --no-renames --exit-code \
  b0c96a846d6db712c98248947cb3354c4bb7d157 HEAD -- crates/router-core/
rg -n "AGENTS|AgentKind|connect_agent|connect_cc_at|connect_codex_at|connect_opencode_at" \
  apps/desktop/src apps/desktop/src-tauri/src/lib.rs
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
```

Agent 版本检测只运行只读 `--version`/`version` 命令。命令不存在记录为“未安装”，不得安装。

### 退出条件

- 基线可从干净工作区复现；
- `router-core` 基线明确；
- 文件模式、Git blob 内容和路径进入同一个确定性目录摘要，不把未跟踪文件算入基线；
- Hermes 官方来源明确，否则 Task 11 保持阻塞；
- 不读取或写入用户 Agent 配置。

### 建议提交

```text
docs: 记录 Agent 兼容工程基线
```

## Task 1：建立红线、前端测试和覆盖率基础设施

### 文件

- 新增 `scripts/check-router-core-redline.sh`；
- 修改 `.github/workflows/ci.yml`；
- 修改 `apps/desktop/package.json`、`package-lock.json`；
- 新增 `apps/desktop/vitest.config.ts`、`src/test/setup.ts`；
- 新增 `apps/desktop/src/App.test.tsx` 的最小现状测试；
- 修改 `docs/contributing/测试指南.md`。

### TDD/门禁顺序

1. 先测试红线脚本：无 diff 返回 0，模拟核心目录 diff 返回非 0；
2. CI 对 PR 使用 base/head，对 `main` push 使用 before/after SHA；
3. 加入 Vitest、jsdom、Testing Library 和 coverage provider；
4. 固定当前三个 Agent 按钮可见、点击调用 IPC 的基线测试；
5. 新增 frontend job：`npm ci`、test、build；
6. 新增独立 desktop-rust job，安装 Tauri Linux 官方系统依赖后运行桌面 crate test、clippy；
7. 分别生成 workspace、desktop Rust、frontend 三份覆盖率基线；
8. 根 workspace 明确排除了桌面 crate，禁止用 workspace 报告冒充桌面覆盖率；
9. 最终 fail-under 门在 Task 12 开启。
10. `cargo audit` 对 desktop Cargo.lock 至少启用 `-D unsound`；已发现的 `glib 0.18.5 / RUSTSEC-2024-0429` 必须升级解决，或形成有期限、有负责人和依据的显式例外，不能因默认 exit 0 忽略。

### 验证

```bash
bash scripts/check-router-core-redline.sh HEAD HEAD
npm --prefix apps/desktop ci
npm --prefix apps/desktop run test -- --run
npm --prefix apps/desktop run build
cargo llvm-cov --workspace --summary-only
cargo llvm-cov --manifest-path apps/desktop/src-tauri/Cargo.toml --summary-only
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

### 退出条件

- 模拟修改核心路由时红线脚本和 CI 都失败；
- 不修改核心路由时通过；
- 前端已有可运行测试入口；
- 当前覆盖率有真实数字。

### 建议提交

```text
ci: 建立 Agent 兼容工程红线与覆盖率基线
```

## Task 2：定义 Agent 领域契约和内置 Registry

### 文件

- 新增 `apps/desktop/src-tauri/src/agent_integration/{mod.rs,types.rs,registry.rs}`；
- 新增 `apps/desktop/src-tauri/agent-registry/builtin-agents.json`；
- 修改 `apps/desktop/src-tauri/src/lib.rs`，仅注册模块；
- 修改 `apps/desktop/src-tauri/Cargo.toml`、`Cargo.lock`，加入本任务必要依赖。

### 首先写失败测试

1. 重复 `agent_id` 被拒绝；
2. 未知 schema version 被拒绝；
3. 引用不存在的 Adapter/Connector 被拒绝；
4. 版本命令包含 shell 字符串或空 argv 被拒绝；
5. 路径模板越界或包含未允许变量被拒绝；
6. Registry 稳定输出五条 Agent 记录；
7. UI 展示元数据不再需要前端写死联合类型。

### 实现

实现 `AgentDescriptor`、`DiscoveryRecord`、`CompatibilityDecision`、`ConfigChangePlan`、`SnapshotRecord`、状态原因码、Registry 校验和确定性排序。

Descriptor 只能声明数据，不能携带脚本、下载器或路由配置。OpenClaw/Hermes 只登记候选身份和“未正式准入”，不包含未核实写入契约。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml agent_integration::registry
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cargo fmt --all -- --check
git diff -- crates/router-core
```

### 建议提交

```text
feat(desktop): 定义 Agent Registry 契约
```

## Task 3：抽离现有 Connector，保持旧入口行为不变

### 文件

- 新增 `agent_integration/connectors/{mod.rs,claude_code.rs,codex.rs,opencode.rs}`；
- 新增 `agent_integration/config_codec.rs`；
- 修改 `apps/desktop/src-tauri/src/lib.rs`，把现有 `connect_*_at` 变成临时兼容包装；
- 移动并扩展现有桌面 Rust 测试，不删除原断言。

### TDD 顺序

1. 提取当前三 Agent 配置 fixtures；
2. 增加“非归属字段保留”“非法配置零写入”“重复执行幂等”失败测试；
3. 定义 Connector trait：身份、支持范围、定位、读取、生成 patch、验证、断开 patch；
4. Claude Code/OpenCode 使用结构化 JSON 投影；
5. Codex 使用 `toml_edit` 保留无关表和格式；
6. 旧 `connect_agent(kind)` 暂时调用新 Connector，再经现有写入函数落盘，保证 UI 行为不变；
7. Adapter 未就绪时继续零写入。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml connector
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml connection
cargo test -p token-station-cli --test proxy
git diff -- crates/router-core crates/protocol apps/cli/src/gateway.rs apps/cli/src/server.rs
```

### 退出条件

- 三个现有按钮行为不回归；
- Connector 逻辑不再堆在 `lib.rs`；
- 非归属配置保留；
- 尚未引入自动扫描或新写入路径。

### 建议提交

```text
refactor(desktop): 抽离现有 Agent Connector
```

## Task 4：实现只读 Discovery Scanner

### 文件

- 新增 `agent_integration/discovery.rs`、`platform.rs`；
- 新增 `apps/desktop/src-tauri/tests/fixtures/discovery/**`；
- 修改 `Cargo.toml`、`Cargo.lock`，加入受限进程等待和哈希依赖；
- 修改内置 Registry，补齐已核实的只读发现规则。

### 首先写失败测试

1. 已知路径和 PATH 分别命中；
2. 同一 canonical path 去重，不同路径形成冲突组；
3. 找到文件但版本命令失败时是 `INSTALLED_BROKEN`；
4. 超时会终止子进程；
5. 超大 stdout/stderr 被截断且不进入普通日志；
6. 无法解析版本时保留 raw 并标记未知；
7. macOS、Linux、Windows、WSL 路径 fixtures；
8. 扫描前后所有候选配置文件哈希不变。

### 实现约束

- 使用 `Command` + argv，不经过 shell；
- 单次探测默认超时 2 秒，输出上限 64 KiB；
- 环境变量覆盖 → 已知路径 → PATH；
- canonical path 去重；
- 只读取存在的配置候选，不创建目录；
- 不运行 install/update/repair；
- 不记录环境变量值或配置内容。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml discovery
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml -- --ignored
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
```

真实平台忽略测试缺少 Agent 时报告 skip，不自动安装。

### 退出条件

- 发现全程只读；
- 未安装、损坏、多实例、未知版本可区分；
- 真实路径和 PATH 默认实例可追溯。

### 建议提交

```text
feat(desktop): 实现只读 Agent 自动发现
```

## Task 5：实现内置与签名兼容目录

### 文件

- 新增 `agent_integration/compatibility.rs`；
- 新增 `agent-registry/builtin-compatibility.json`；
- 新增 `tests/fixtures/compatibility/**`；
- 修改桌面 Cargo 依赖，直接依赖 `token-station-release`；
- 修改 `lib.rs`，注入 app data cache 路径和测试端点；
- 暂不修改 `crates/release`，复用原始字节 Ed25519 验签。

### 首先写失败测试

1. 已验证、补丁推定、未知、阻断四种版本矩阵；
2. 未知字段和未知 schema version 被拒绝；
3. 引用不存在 Connector 被拒绝；
4. 错误签名、篡改、过期目录被拒绝；
5. 远程目录 sequence 回滚被拒绝；
6. 网络失败回退内置目录；
7. 回退只能收紧支持范围；
8. 空生产公钥时不请求远程目录；
9. 远程目录不能改变 Descriptor、配置路径、owned paths 或执行代码。

### 实现约束

- 目录格式 `deny_unknown_fields`，sequence 单调递增；
- 签名覆盖服务器返回的原始 JSON bytes；
- 只有验签、schema、时效、回滚检查全部通过才原子更新缓存；
- 内置目录永远可用；
- `DETECTED_INFERRED` 同时要求补丁范围允许且配置指纹未变化；
- URL/公钥为空时不外联；
- 网络 body 限长、超时、固定域名。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml compatibility
cargo test -p token-station-release
cargo deny check
cargo audit
cargo deny --manifest-path apps/desktop/src-tauri/Cargo.toml check
cargo audit --file apps/desktop/src-tauri/Cargo.lock
```

### 退出条件

- 篡改、过期、回滚和未知 Connector 均 fail closed；
- 没有远程代码下发；
- 没有生产密钥或端点时不影响内置目录；
- 兼容判断不读取或修改 Router 配置。

### 建议提交

```text
feat(desktop): 增加签名 Agent 兼容目录
```

## Task 6：实现配置计划、加密快照和原子事务

### 文件

- 新增 `agent_integration/{plan.rs,transaction.rs,snapshot.rs,ownership.rs}`；
- 新增 `tests/fixtures/config/**`；
- 修改桌面 Cargo 依赖，加入直接 keyring、AEAD 和安全随机依赖；
- 仅在许可证确有需要时最小修改 `deny.toml`。

### 安全固定项

- snapshot master key 存 OS keychain，不进入仓库、日志或索引；
- 使用标准带认证加密 AEAD，每份独立随机 nonce；
- Unix 快照和索引权限不宽于 `0600`；
- 每个 Agent/配置文件默认保留最近 5 份，pinned 不清理；
- index 记录 schema、operation ID、hash、原权限、Connector/App 版本；
- 计划绑定 `before_hash`、安装实例、兼容目录 sequence 和有效期；
- 写入采用同目录临时文件、flush、fsync、权限恢复和 rename；
- 错误消息不得包含配置全文或凭证。

### 首先写失败测试

1. 计划只包含 owned paths；
2. 计划序列化不含配置全文和密钥；
3. 未确认时 apply 被拒绝；
4. `before_hash` 变化时 apply 被拒绝；
5. 快照失败时目标零写入；
6. 临时写入、fsync、rename 失败返回精确阶段；
7. 写后解析/自检失败自动恢复；
8. 恢复成功和恢复失败分别记录；
9. 断开只恢复/移除 owned paths；
10. 非归属字段在连接、断开、恢复后保持；
11. 快照不是明文，篡改后无法解密；
12. keychain 缺失或锁定时 fail closed；
13. 第 6 份触发保留策略，pinned 保留；
14. 当前值与归属记录冲突时要求重新预览。

### 实现顺序

1. 纯函数 `build_change_plan`；
2. SnapshotStore 接口与内存测试实现；
3. OS keychain + 文件 SnapshotStore；
4. 可故障注入的原子写入接口；
5. apply、disconnect、restore transaction；
6. 版本化 ownership index。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml transaction
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml snapshot
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml ownership
cargo llvm-cov --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --summary-only --fail-under-lines 90
cargo deny check
cargo audit
cargo deny --manifest-path apps/desktop/src-tauri/Cargo.toml check
cargo audit --file apps/desktop/src-tauri/Cargo.lock
```

真实 keychain round-trip 标为 ignored，只在对应受控 runner 执行。

### 退出条件

- 没有确认无法写入；
- 并发修改无法被覆盖；
- 写后失败自动恢复；
- 快照密文、权限和保留策略通过；
- 断开不删除用户非归属配置。

### 建议提交

```text
feat(desktop): 建立 Agent 配置安全事务
```

## Task 7：新增结构化 Tauri IPC，保留特权边界

### 文件

- 修改 `apps/desktop/src-tauri/src/lib.rs`；
- 新增 `agent_integration/commands.rs`；
- 修改 `apps/desktop/src/api.ts`；
- 新增 `apps/desktop/src/api.test.ts`。

### IPC 契约

```text
scan_agents() -> AgentView[]
plan_agent_connection(agent_id, installation_path) -> ConfigPlanView
apply_agent_plan(operation_id, confirmation_token) -> AgentOperationView
plan_agent_disconnect(agent_id, installation_path) -> ConfigPlanView
list_agent_snapshots(agent_id) -> SnapshotView[]
plan_snapshot_restore(snapshot_id) -> ConfigPlanView
apply_snapshot_restore(operation_id, confirmation_token) -> AgentOperationView
```

### 约束与负向测试

- scan 只读；plan 只创建内存中的短期计划；
- apply 只能引用服务端保存的 operation ID，前端不能提交任意 patch；
- token 与计划、会话和到期时间绑定；
- 前端不能提交任意目标路径、配置内容或命令；
- 特权操作只走 Tauri IPC，不增加 HTTP 写接口；
- 测试未知 Agent/实例、多实例未选、未知/阻断版本、计划过期、token 不匹配、plan 后文件变化、重复 apply、任意路径注入；
- scan 不启动代理服务也能工作；
- 旧 `connect_agent(kind)` 在新 UI 切换前保留，切换后删除。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml commands
npm --prefix apps/desktop run test -- --run api
rg -n "scan_agents|plan_agent_connection|apply_agent_plan|plan_agent_disconnect|snapshot" \
  apps/desktop/src-tauri/src apps/desktop/src/api.ts
rg -n "route\(|RouterConfig|Decision" apps/desktop/src-tauri/src/agent_integration
```

最后一条不得出现路由调用或核心契约依赖。

### 建议提交

```text
feat(desktop): 暴露安全 Agent 管理 IPC
```

## Task 8：实现动态 Agent 页面和确认流程

### 文件

- 新增 `apps/desktop/src/pages/Agents.tsx`；
- 新增 `components/AgentCard.tsx`、`AgentChangePreview.tsx`、`AgentSnapshotList.tsx`；
- 新增对应 `*.test.tsx`；
- 修改 `apps/desktop/src/{App.tsx,App.css,api.ts}`；
- 仅在新页面测试通过后删除静态 `AgentKind` 和 `AGENTS`。

### 交互顺序

1. App 启动后执行一次 `scan_agents`，失败只影响 Agent 页面；
2. 动态展示 Registry 返回的五种 Agent；
3. 用户可手动重新扫描；
4. 多实例时选择真实路径；
5. 接入前展示版本、兼容证据、目标文件、字段级 diff 和快照说明；
6. 用户明确确认后才 apply；
7. `DETECTED_INFERRED` 使用独立风险文案和额外确认；
8. 未知、阻断、损坏状态不显示可执行接入按钮；
9. 已接入状态提供断开和恢复；
10. 错误显示阶段和建议，不显示密钥或配置全文。

### 首先写失败测试

- 动态五 Agent 渲染，不依赖静态数组；
- scanning/loading/empty/error；
- 多实例选择；
- 未知和阻断状态按钮禁用；
- plan diff 出现前 apply 不可触发；
- 取消确认零 apply；
- operation 过期后要求重新预览；
- 接入成功后刷新；
- 断开和恢复二次确认；
- 深色/浅色状态下无浅底白字。

### 验证

```bash
npm --prefix apps/desktop run test -- --run
npm --prefix apps/desktop run test:coverage
npm --prefix apps/desktop run build
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
```

### 退出条件

- 静态 Agent 类型和按钮被结构化 API 取代；
- 扫描、计划、确认、应用可区分；
- 不支持状态没有绕过入口；
- 对比度测试或截图证据通过。

### 建议提交

```text
feat(desktop): 增加动态 Agent 管理页面
```

## Task 9：迁移 Claude Code、Codex、OpenCode

### 文件

- 修改 `connectors/{claude_code.rs,codex.rs,opencode.rs}`；
- 扩展 `tests/fixtures/config/{claude-code,codex,opencode}/**`；
- 删除 `lib.rs` 中旧 `connect_*` 和固定 `.bak` 写入函数；
- 修改 `docs/contributing/桌面App-Agent接入机制.md`。

### 必测版本/状态

每种 Agent 至少覆盖：

- 一个正式已验证版本；
- 一个允许补丁推定版本；
- 一个未知版本；
- 一个阻断 fixture；
- 配置不存在；
- 合法配置且含大量未知字段；
- 配置损坏；
- 旧版 Token Station 已接入并存在 `.token-station.bak`。

### 迁移规则

- 旧 `.token-station.bak` 只作为旧版恢复候选展示，不自动覆盖或删除；
- 首次新事务成功后建立 ownership；
- Claude Code 只管理确认过的 env 键；
- Codex 对顶层 `model`/`model_provider` 保存前值，断开时精确恢复；
- OpenCode 只管理 `provider.tokenstation`；
- 本地虚拟 Key 行为保持，日志和 diff 脱敏；
- Adapter 就绪检查必须精确匹配 ID。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml claude
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml codex
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml opencode
cargo test -p token-station-cli --test proxy
cargo test -p token-station-conformance --test official_plugins
git diff -- crates/router-core apps/cli/src/gateway.rs apps/cli/src/server.rs crates/protocol
```

### 退出条件

- 三种现有 Agent 只走新事务；
- 旧固定备份不再被覆盖；
- 接入、重复接入、断开、恢复和非法配置通过；
- 现有协议主链无回归。

### 建议提交

```text
feat(desktop): 迁移现有 Agent 到安全接入框架
```

## Task 10：准入 OpenClaw

### 文件

- 新增 `connectors/openclaw.rs`；
- 修改内置 Registry 和兼容目录；
- 新增 `tests/fixtures/config/openclaw/**`；
- 修改 `docs/guides/OpenClaw-接入指南.md`；
- 新增 `docs/verification/2026-07-20-OpenClaw-Connector-验收.md`。

### 准入前核实

- 执行日官方仓库、版本命令、配置路径优先级；
- JSON/JSON5 schema、是否提供 overlay 或环境变量注入；
- `openai-completions` Base URL、鉴权和模型字段；
- 写入后如何只读自检；
- gateway、远程 channel、MCP、浏览器和用户 skills 继续不在范围。

若官方配置使用 JSON5 且现有库不能保留注释/未知结构，优先使用官方 overlay/环境变量入口；不得重写整个文件造成注释丢失。找不到安全投影方式时，只交付发现，不交付接管。

### TDD/E2E

1. 固定目标版本官方配置 fixtures；
2. owned paths 往返、断开、恢复；
3. 未知字段/注释保留；
4. 未知版本和 schema 变化阻断；
5. 临时配置和独立 workspace 的文本、流式、function tool；
6. 本地 401、上游错误、日志脱敏；
7. 不修改 `~/.openclaw`，不启动用户现有实例。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml openclaw
cargo test -p token-station-cli --test proxy
rg -n "OpenClaw|openai-completions|版本|恢复|未知" \
  docs/guides/OpenClaw-接入指南.md \
  docs/verification/2026-07-20-OpenClaw-Connector-验收.md
```

缺少目标版本时真实 E2E 标记未执行，不能自动安装补齐。

### 建议提交

```text
feat(desktop): 准入 OpenClaw Connector
```

## Task 11：准入 Hermes Agent

### 文件

- 新增 `connectors/hermes.rs`；
- 修改内置 Registry 和兼容目录；
- 新增 `tests/fixtures/config/hermes/**`；
- 新增 `docs/guides/Hermes-Agent-接入指南.md`；
- 新增 `docs/verification/2026-07-20-Hermes-Agent-Connector-验收.md`。

### 准入前核实

- 官方项目锁定为 NousResearch Hermes Agent；
- 官方安装、版本命令、可执行文件名；
- `HERMES_HOME`、默认目录和配置优先级；
- YAML schema、未知字段和注释保留；
- 模型 Provider/协议、鉴权和模型字段；
- 是否复用 `agent-openai` 或其他现有 Adapter；
- 只读启动/配置验证方式。

cc-Switch 的 Hermes 实现只能作为竞品参考，不能代替官方证据。官方证据不完整时，停在发现和诊断，不实现配置写入。

### TDD/E2E

- 环境变量覆盖和平台默认路径；
- YAML 多版本 fixtures、未知字段、重复顶层键等异常；
- owned paths 往返、断开、恢复；
- 未知版本、指纹变化和非法 YAML 阻断；
- 临时 `HERMES_HOME` 的文本、流式、工具和错误链；
- 快照敏感字段加密且日志脱敏。

如果 YAML 库不能满足保留语义，先提交 codec 选型证据，不得以丢注释换取“完成”。

### 验证

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml hermes
cargo test -p token-station-cli --test proxy
rg -n "NousResearch|HERMES_HOME|YAML|版本|恢复|未知" \
  docs/guides/Hermes-Agent-接入指南.md \
  docs/verification/2026-07-20-Hermes-Agent-Connector-验收.md
```

### 建议提交

```text
feat(desktop): 准入 Hermes Agent Connector
```

## Task 12：全链路回归、覆盖率和平台矩阵

### 文件

- 修改 `.github/workflows/ci.yml`；
- 新增 `scripts/check-coverage-thresholds.mjs`；
- 新增 `docs/verification/2026-07-20-Agent-兼容工程总验收.md`；
- 修改 `docs/contributing/测试指南.md`；
- 根据测试事实补齐 Rust/前端测试，不为追数字修改无关行为。

### CI 最终门

1. `router-core` diff 门禁；
2. Rust fmt、clippy、workspace tests、rustdoc；
3. frontend test、coverage、build；
4. 主 workspace 有效源码行覆盖率 ≥80%；
5. desktop Rust crate 行覆盖率 ≥90%，`agent_integration/**` 再按 LCOV 文件路径单独校验 ≥90%；
6. frontend 行覆盖率 ≥80%；
7. workspace 与 desktop Cargo.lock 分别执行 deny、audit；
8. Linux 常规 CI；
9. macOS/Windows 平台发现和 keychain 定向测试；
10. WSL/Linux/macOS/Windows 路径 fixtures；
11. 五种 Agent 真实 E2E 作为受控验收，不在公共 CI 自动安装第三方 CLI。

### 全量验证

```bash
bash scripts/check-router-core-redline.sh "$(git merge-base HEAD main)" HEAD
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo llvm-cov --workspace --summary-only --fail-under-lines 80
cargo llvm-cov --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --summary-only --fail-under-lines 90
cargo llvm-cov --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --lcov --output-path target/desktop-agent.lcov
node scripts/check-coverage-thresholds.mjs \
  target/desktop-agent.lcov apps/desktop/src-tauri/src/agent_integration 90
npm --prefix apps/desktop ci
npm --prefix apps/desktop run test:coverage
npm --prefix apps/desktop run build
cargo deny check
cargo audit
cargo deny --manifest-path apps/desktop/src-tauri/Cargo.toml check
cargo audit --file apps/desktop/src-tauri/Cargo.lock
git diff --check
```

### 人工验收

- 五 Agent 状态正确；
- 多安装选择正确；
- 未知/阻断零写入；
- 接入、断开、恢复 diff 可理解；
- Agent 更新模拟进入正确保护状态；
- 深色/浅色 UI 可读；
- 日志、错误、截图无密钥；
- `crates/router-core/**` 哈希与 Task 0 基线一致。

### 退出条件

- 覆盖率命令真实达到门槛；
- 所有自动化 0 failure；
- 未执行的真实平台/Agent 验收明确列出；
- 核心路由基线一致。

### 建议提交

```text
test: 完成 Agent 兼容工程全量门禁
```

## Task 13：签名目录发布演练、灰度与文档交付

### 文件

- 新增 `docs/release/Agent-兼容目录发布与回滚.md`；
- 新增 `docs/contributing/Agent-Connector-准入指南.md`；
- 修改 `docs/contributing/桌面App-Agent接入机制.md`；
- 修改 `docs/product/功能总览.md`、`docs/product/配置详解.md`；
- 修改 `docs/README.md`；
- 生产配置仅在 URL、公钥和负责人确认后新增。

### 发布演练

1. 使用测试密钥生成目录并签名原始 bytes；
2. mock endpoint 提供目录和签名；
3. 验证正常拉取、缓存、离线回退；
4. 验证错误签名、篡改、过期、sequence 回滚；
5. 验证目录只能引用应用内 Connector；
6. 演练紧急阻断某 Agent 版本；
7. 演练撤回错误阻断并恢复到更高 sequence；
8. 生产私钥离线保存，CI 不持有；
9. URL/公钥未确认时，正式构建只使用内置目录。

### 灰度顺序

```text
内部只读扫描
→ 内部三 Agent 接入/断开
→ OpenClaw/Hermes 受控测试
→ 小范围灰度
→ 观察配置失败与恢复率
→ 扩大范围
```

任何用户配置丢失、恢复失败、目录验签异常或未知版本误放行，都立即停止扩量并回退到上一已验证应用/目录版本。

### 验证

```bash
rg -n "签名|私钥|回滚|sequence|阻断|内置目录" \
  docs/release/Agent-兼容目录发布与回滚.md
rg -n "Descriptor|Connector|owned paths|版本矩阵|E2E|router-core" \
  docs/contributing/Agent-Connector-准入指南.md
git diff --check
git diff --name-only -- crates/router-core
```

### 建议提交

```text
docs: 交付 Agent 兼容目录与 Connector 运维规范
```

## 5. 每个任务的统一执行规则

### 5.1 测试先行

每个行为按以下顺序：

1. 写表达需求的失败测试；
2. 运行并确认因缺少该行为而失败；
3. 实现最小行为；
4. 运行目标测试；
5. 运行相邻回归；
6. 检查 `router-core` diff；
7. 才允许形成本地提交。

只新增一个永远通过的测试不算测试先行。故障恢复测试必须通过注入故障证明失败分支实际执行。

### 5.2 工作区保护

- 开始前记录 `git status --short`；
- 用户已有修改、未跟踪文件和删除状态不进入本专项提交；
- 不使用 `git reset --hard`、`git checkout --` 或宽泛删除；
- 测试全部使用注入路径、临时目录和 mock endpoint；
- 不复用用户 `~/.claude`、`~/.codex`、`~/.config/opencode`、`~/.openclaw`、`~/.hermes`；
- 不停止或重启用户当前 Token Station/Agent 进程。

### 5.3 失败记录

每个失败记录：阶段、Agent/Connector ID、原因码、是否已写入、是否已尝试恢复、恢复结果和脱敏诊断。不得只返回“接入失败”。

### 5.4 提交与 PR

- 按 Task 独立本地提交，保证每个提交可构建、可测试、可回退；
- PR 前运行 Task 12 全量门禁；
- 未获用户授权不 push、不创建 PR；
- `.superpowers/`、临时截图、真实配置和密钥不进入提交；
- PR 描述列出 `router-core` 零 diff 证据和未执行的真实验收。

## 6. 完成定义

只有同时满足以下条件，实施任务才可标记完成：

1. Task 0–13 的退出条件全部满足；
2. 五种 Agent 按准入证据进入正确支持状态；
3. 未知/阻断版本和扫描过程零写入；
4. 配置接入、断开、恢复无非归属字段损失；
5. 快照加密、权限、保留和损坏处理通过；
6. 远程目录签名、过期、回滚、缓存和 fail-closed 通过；
7. Rust、前端、平台、协议和真实 E2E 证据齐全；
8. 主 workspace 和 frontend 行覆盖率分别 ≥80%，desktop Rust 与新增安全关键模块 ≥90%；
9. `crates/router-core/**` 与 Task 0 基线一致；
10. 没有自动安装、自动升级、远程代码、静默接管或范围外功能；
11. 用户确认验收报告后，才进入 push、PR 或发布动作。
