# 本地核心基线吸收 develop 修复的集成设计

## 1. 决策摘要

本次集成以本地分支 `codex/agent-gateway-taskbook` 的提交 `389e701921dd8e6b15ca8e6e2b8a975b9db571ce` 为唯一核心基线，选择性吸收远端 `develop` 的提交 `5770ba5b15e2f91b22e7bb4c468e3f22e6bed90c` 中经过复核的修复。共同祖先为 `f27138c8adc181f116e87807f252607712572a88`。

不执行整分支合并，也不以远端文件覆盖本地文件。所有远端能力按语义重新移植到本地架构，避免把远端已经退化的运行时、能力状态、错误模型和存储迁移带回本地。

本次必须同时完成 Agent 版本准入简化。终态不使用版本白名单，只依赖黑名单、只读预检、Connector 唯一性、快照、事务和失败回滚保证安全。

## 2. 集成原则

1. 本地架构优先。冲突发生时，先保持本地的数据模型、安全边界和生命周期，再移植远端行为。
2. 只移植可独立验证的修复。每一组改动应有明确测试，不接受顺带重写无关模块。
3. 不降低安全性。不得持久化请求可控的原始错误文本，不得放宽重定向、代理、密钥和事务边界。
4. Agent 接入默认放行。版本只参与黑名单匹配，不再承担允许清单门禁。
5. 开发态 metrics 历史允许重建。本地 schema v4 保持为唯一 schema v4，不兼容远端的另一套 v4。

## 3. 保留、吸收与拒绝范围

| 模块 | 决策 | 说明 |
| --- | --- | --- |
| Runtime Supervisor、revision、热切换、drain | 保留本地 | 本地实现具备完整运行时状态与切换语义 |
| Gateway T4、T5、T6 | 保留本地 | 保留受控请求体、SSE、失败收据和安全边界 |
| `CapabilityState` 四态模型 | 保留本地 | 不退回远端布尔能力模型 |
| Provider catalog、生命周期、tombstone | 保留本地 | 不接受直接删除 Provider 的退化行为 |
| 规范化无内容收据 | 保留本地 | 不持久化远端自由文本 `error_detail` |
| T7 profile 与 mount | 吸收远端 | 按本地 Provider、Gateway 和状态模型重新接入 |
| T9 人类可读错误映射 | 吸收远端 | 映射只用于展示，存储仍使用结构化错误码和安全字段 |
| Agent 插件加载容错 | 吸收远端 | 单个插件异常不得拖垮整体发现和启动流程 |
| Provider 安全更新体验 | 吸收远端 | 保留本地校验、密钥和生命周期约束 |
| Agent 版本准入简化 | 吸收并完成 | 删除远程签名目录和允许清单，保留黑名单与预检 |
| 简化运行时与 hash 状态 | 拒绝 | 会破坏本地热切换和 revision 语义 |
| 原始自由文本错误持久化 | 拒绝 | 存在泄露请求内容和扩大存储面的风险 |
| 非事务数据库迁移 | 拒绝 | DDL 与 `user_version` 必须原子提交 |
| 破坏式 keychain upsert | 拒绝 | 写入失败不得先删除现有凭据 |

## 4. Agent 版本准入终态

### 4.1 判定规则

Agent 可接入的充分条件为：程序能够运行、只找到一个 Connector、只读预检通过，并且未命中黑名单。

以下情况必须阻止接入：

- 命中 `blocked` 黑名单规则。
- 配置文件无法读取、无法解析，或只读预检失败。
- Connector 不唯一。
- 检测到多个安装。
- 安装处于 `InstalledBroken`，或发现流程本身失败。

版本号缺失或无法解析为 semver 时不得直接拒绝。此时跳过基于版本的黑名单匹配；其余安全检查通过后，状态统一为 `DetectedVerified`。`DetectedVerified` 在新模型中只表示“可接入”，不再表示命中了已验证版本白名单。

### 4.2 删除远程签名兼容目录

在 `apps/desktop/src-tauri/src/agent_integration/compatibility.rs` 删除以下内容：

- `CatalogTransport`、`UreqCatalogTransport`、`SignedCatalogResponse`、`RemoteCatalogConfig`。
- `production_remote_config`、`select_catalog`、缓存读写、离线合并和回退逻辑。
- `validate_https_endpoint`、远程目录校验分支及相关常量。
- 仅服务于远程签名、缓存、防篡改和回滚的测试。

`commands.rs::perform_scan` 直接构造 `CompatibilityCatalog::builtin(&self.registry)`，目录来源固定为 `CatalogSource::Builtin`。保留 `CompatibilityCatalog::builtin()` 和内置 JSON。

完成删除后全仓检查 `token_station_release`。如果没有其他调用，从桌面端 `Cargo.toml` 移除依赖，并同步更新锁文件。

### 4.3 目录退化为纯黑名单

`builtin-compatibility.json` 中每个 Agent 只保留 `blocked`，删除或清空 `verified` 与 `inferred`。

`compatibility.rs::evaluate_discovery` 只执行以下流程：

1. 处理多安装、损坏安装、发现失败和 Connector 唯一性。
2. 在版本可解析时检查 `blocked`。
3. 执行只读预检，读取或解析失败立即拒绝。
4. 其余情况默认放行并返回 `DetectedVerified`。

删除 `CompatibilityRule`、`validate_rule`、`requirement_matches`、`inferred_rule_matches`，并从 `AgentCompatibilityEntry` 删除 `verified` 和 `inferred` 字段。保留 `BlockedRule` 与 `blocked_requirement_matches`。

`types.rs` 删除 `DetectedInferred`，清理只服务于允许清单或试验性状态的 `ReasonCode`。所有穷举 `match` 必须同步修改，不能增加吞掉未知状态的宽泛分支来绕过编译检查。

### 4.4 接入流水线的无版本处理

`commands.rs` 接入流水线中的 `expected_version` 二次校验仅在扫描阶段获得了版本号时执行。没有版本号时，跳过版本要求和对应的 TOCTOU 复核，但仍执行 Connector 唯一性、只读预检、快照、事务写入和失败回滚。

如果扫描阶段存在版本号，则保留二次读取和一致性校验，防止扫描后到接入前版本发生变化。

### 4.5 前端统一展示

`AgentRoutePage.tsx` 删除 `isExperimentalCompatibility`、未在验证目录的提示，以及试验性与 verified 的展示差异。所有可接入 Agent 统一显示“可接入”。

`App.test.tsx` 删除或改写对试验性兼容状态的断言，新增或保留以下行为测试：

- 可解析版本且未命中黑名单时可接入。
- 无版本或版本不可解析时，其他安全检查通过即可接入。
- 命中黑名单、预检失败、Connector 不唯一或多安装时不可接入。

## 5. 远端修复的语义移植

### 5.1 T7 profile 与 mount

移植远端 profile 和 mount 能力时，以本地 Provider catalog、运行时 revision 和 Gateway 生命周期为边界。配置变更先落入本地草稿和校验流程，再由现有保存与应用机制发布，不允许直接绕过 Runtime Supervisor 修改活动状态。

需要补齐 profile 创建、更新、挂载、解除挂载和不存在目标的测试，并验证运行中切换仍服从本地 drain 和回滚语义。

### 5.2 T9 人类可读错误映射

吸收远端面向用户的错误解释，但把它设计为结构化错误码到展示文案的单向映射。Gateway 收据和数据库继续保存稳定错误码、阶段、HTTP 状态等受控字段，不保存请求体、上游原始响应或自由文本异常。

前端可以基于错误码展示可执行建议。未知错误使用统一兜底文案，并保留内部诊断关联标识。

### 5.3 Agent 插件加载容错

插件发现和加载采用逐项隔离。单个插件缺失、格式错误或初始化失败时，记录结构化诊断并继续加载其他插件。内置插件和正常第三方插件不应受失败插件影响。

容错不得掩盖 Connector 冲突。如果多个有效插件声明同一 Connector，仍按 Connector 不唯一处理并阻止接入。

### 5.4 Provider 安全更新体验

吸收远端更新体验，但复用本地 Provider 校验、catalog、tombstone 和凭据写入规则。更新失败时保留旧 Provider 配置与旧凭据，禁止先删除再写入。

涉及活动 Provider 的更新必须经过现有保存、应用、快照和失败恢复流程。删除行为继续使用本地生命周期，不改为直接物理删除。

## 6. 数据与安全边界

本地 metrics schema v4 保持不变。由于两个分支使用了不兼容的 schema v4，且均未进入正式 Release 或 tag，本次不增加双 v4 猜测迁移。开发环境可以重建 metrics 数据库，损失范围仅限历史请求和收据指标，不影响 Provider 配置、API Key、Agent 配置和路由。

必须继续满足以下约束：

- 数据库迁移在事务中完成，DDL 与 `user_version` 原子提交。
- 模型目录请求禁止自动重定向和环境代理，避免 Bearer 凭据被转发。
- keychain 写入失败时保留旧凭据。
- Gateway 不持久化请求可控的自由文本。
- Agent 修改前创建快照，配置写入使用事务，任何失败触发回滚。
- 预检保持只读，不得借预检修改用户配置。

## 7. 实施批次与提交边界

实施按以下顺序进行，每一批独立提交并在提交前运行针对性测试：

1. Agent 远程签名目录删除与依赖清理。
2. Agent 纯黑名单判定、状态枚举和无版本放行。
3. Agent 前端统一展示与测试更新。
4. 插件加载容错。
5. T7 profile 与 mount 语义移植。
6. T9 错误展示映射。
7. Provider 安全更新体验。
8. 全量回归、clippy 清理和文档收尾。

若某批需要改变本设计明确保留的本地架构，应停止该批实现并重新评审，不在实现中临时扩大范围。

## 8. 验收标准

最终必须在集成后的实际工作树运行并通过：

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cd apps/desktop
npx tsc --noEmit
npx vitest run
```

除命令全绿外，还需人工复核以下不变量：

- `blocked`、`BlockedRule`、`blocked_requirement_matches` 仍存在且有效。
- 配置读取或解析失败仍会拒绝接入。
- Connector 唯一性和多安装检查仍会拒绝异常场景。
- 快照、事务和失败回滚路径仍有测试覆盖。
- 无版本和非 semver 版本不再成为独立拒绝原因。
- 前端只保留一个“可接入”状态，不再显示试验性兼容提示。
- `cargo clippy` 没有警告，不使用宽泛 `allow` 掩盖删除枚举后的遗漏。

## 9. 非目标

本次不处理与上述集成无关的架构重构，包括拆分大型 `gateway.rs`、统一 `AppInner` 与 `ConfigState` 草稿来源、调整生产环境成本上限，以及替换所有未锁定版本的依赖。除非它们直接阻断本次验收，否则记录为后续任务，不扩大当前变更面。

本次也不恢复远程兼容目录、版本允许清单或试验性兼容状态。未来若重新引入远程策略，必须作为独立安全设计重新评审，不能复用本次删除的死代码。
