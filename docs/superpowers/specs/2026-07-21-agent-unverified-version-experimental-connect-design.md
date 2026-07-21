# Token Station Agent 未验证版本试验性接入设计

日期：2026-07-21

## 1. 背景与决策

当前桌面 Agent 控制面把版本兼容目录同时用于展示兼容状态和决定是否允许写配置。内置目录只列出五个精确版本，因此用户安装更旧或更新的可运行版本时会进入 `DETECTED_UNKNOWN`，即使本地 Connector 能解析和安全投影其配置，也不能生成接入计划。

cc-Switch 将版本检测用于安装状态、版本展示和升级提示，不以“必须等于某个版本”作为配置切换的准入条件。Token Station 采用其中“版本检测与接入权限解耦”的原则，但保留更严格的只读预检、额外确认、配置所有权、快照和事务恢复能力。

本设计决定：

- 已验证版本继续正常接入；
- 命中明确 `blocked` 规则的版本继续禁止接入；
- 未命中版本规则、但满足本设计全部安全条件的 `DETECTED_UNKNOWN` 版本，可以在用户明确确认风险后试验性接入；
- 不新增兼容状态，保留 `DETECTED_UNKNOWN` 的“未经验证”语义；
- 版本无法解析、CLI 不可运行、只读发现 Agent、配置预检失败或 Connector 无法唯一确定时继续禁止接入。

本设计有意修订原 Agent 兼容方案中“未知版本默认禁止写配置”的产品准入策略，以及“兼容目录回退只能通过缩小已验证范围来收紧写入”的对应结论。其余安全边界不变。

## 2. 目标

1. 用户的受支持 Agent 不是内置目录中的精确版本时，不再仅因版本不同而无法接入。
2. 未验证版本必须显示明确风险提示，并由用户额外确认。
3. `blocked`、不可运行、无法解析和预检失败继续 fail closed。
4. 写入前后仍执行安装实例、版本、Connector、配置 revision、兼容目录 sequence 和代理运行态复核。
5. 不修改 IR、核心路由、协议转换、Adapter 或数据面运行逻辑。

## 3. 非目标与硬红线

以下内容明确不在本次范围：

- 不修改 `crates/router-core/**`；
- 不修改 IR 类型、IR 语义或中间表示转换；
- 不修改 OpenAI、Anthropic、Responses 等协议转换；
- 不修改 Agent Adapter 的请求处理能力；
- 不修改 Provider 路由、模型选择、fallback 或网关数据面；
- 不扩大 Connector 的 owned paths、目标配置路径或敏感字段范围；
- 不允许远程兼容目录下发代码、命令、动态 Connector 或新的写入规则；
- 不把版本无法解析解释为可接入；
- 不加入自动接入、静默接入或绕过用户确认的路径；
- 不上线兼容目录生产 URL、公钥或远程发布流程；
- 不顺带增加 Agent 安装、升级或卸载功能。

## 4. 状态与动作矩阵

| 状态 | 接入行为 | 额外确认 | 说明 |
|---|---|---|---|
| `DETECTED_VERIFIED` | 允许生成正常接入计划 | 否 | 命中已验证规则 |
| `DETECTED_INFERRED` | 只读预检通过后允许试验性接入 | 是 | 继续保留现有推定兼容语义 |
| `DETECTED_UNKNOWN` 且满足试验性准入条件 | 只读预检通过后允许试验性接入 | 是 | 版本未经目录验证 |
| `DETECTED_UNKNOWN` 但不满足条件 | 禁止 | 不适用 | 例如版本无法解析、无唯一 Connector |
| `DETECTED_BLOCKED` | 禁止 | 不适用 | `blocked` 优先级最高 |
| `INSTALLED_BROKEN` | 禁止 | 不适用 | CLI 存在但不可运行 |
| `MULTIPLE_INSTALLATIONS` | 先选择精确实例 | 不适用 | 选择后重新判断该实例 |
| `CONNECTED` | 允许断开和恢复 | 按原流程 | 不改变既有归属与快照规则 |

## 5. 未验证版本试验性准入条件

一个 `DETECTED_UNKNOWN` 安装实例只有同时满足以下条件，才能获得试验性接入能力：

1. Registry 中的 Agent `admission` 为 `supported`；
2. 安装实例没有冲突组，且已选择精确 canonical path；
3. CLI 可运行，版本探测成功，版本能够规范化为 SemVer；
4. 兼容目录中存在该 Agent 条目；
5. 版本没有命中任何 `blocked` 规则；预发布版本继续继承相同 `major.minor.patch` 核心版本的阻断规则；
6. Registry 为该 Agent 声明且仅声明一个经过本地注册校验的 Connector；
7. 发现阶段没有配置读取、配置解析、环境变量覆盖或只读预检错误；
8. 计划阶段能够读取精确目标配置，并依次通过 Connector 前置条件、源配置校验、补丁生成、投影、重新解析和投影后校验；
9. 用户在 UI 中明确确认“当前版本未经验证，可能存在配置或行为差异”；
10. 应用阶段重新扫描后仍满足上述条件，且版本、配置指纹、Connector、目录 sequence、代理运行态和目标 revision 没有发生不允许的变化。

缺少兼容目录条目时不进行试验性接入。这样可以继续要求每个可管理 Agent 先由内置 Registry 和兼容目录共同声明，同时避免远程目录或未知 Agent 借本功能获得写入能力。

## 6. 兼容判定与 Connector 绑定

`evaluate_discovery` 继续按以下顺序判定：

1. 多安装冲突；
2. CLI 不可运行；
3. 版本无法解析；
4. `discovery_only`；
5. 缺少兼容目录条目；
6. `blocked`；
7. `verified`；
8. `inferred`；
9. 未命中任何规则的未知版本。

只有第 9 类以及“命中 inferred 范围但配置指纹变化”的未知版本可以尝试绑定 Registry 中唯一的本地 Connector。绑定成功时：

- 状态仍为 `DETECTED_UNKNOWN`；
- `connector_id` 设置为唯一的本地 Connector；
- `allowed_actions` 增加 `run_read_only_preflight`；
- 不在扫描阶段直接授予 `preview_connect`；
- 只有计划阶段的完整只读投影预检通过后，才增加 `confirm_experimental_connect` 并签发计划。

如果本地 Connector 数量不是一个，保持 `connector_id = None` 并禁止接入，避免未来多 Connector 场景下进行不可靠猜测。

## 7. 计划、用户确认与应用复核

### 7.1 交互顺序

未验证版本的接入流程为：

```text
用户点击试验性接入
→ UI 展示版本、安装路径、未经验证原因、快照与恢复说明
→ 用户明确确认风险
→ UI 将已展示的规范化版本作为 expected version 请求计划
→ 后端刷新扫描；版本不一致则拒绝并要求重新确认
→ 后端生成计划并执行完整只读投影预检
→ 计划包含 experimental_compatibility 确认要求
→ UI 提交计划令牌及 experimental_compatibility_confirmed=true
→ 后端重新扫描并复核准入、Connector、版本和 revision
→ 加密快照
→ 原子写入
→ 写后验证；失败则恢复
```

用户取消确认时不调用计划命令，因此不会累积未消费的待确认计划。

expected version 只用于与刷新后的服务端扫描结果做等值比较，不参与路径选择、Connector 选择或兼容性推断。Token Station 不负责下载、安装或升级 Agent；如果用户在外部改变 Agent 版本，旧确认不能授权新版本，必须重新扫描并展示当前版本。

### 7.2 IPC 确认参数

`apply_agent_plan` 增加向后兼容的可选参数 `experimental_compatibility_confirmed`：

- 已验证版本省略或传 `false`，行为不变；
- 计划要求 `ExperimentalCompatibility` 时，参数必须为 `true`；
- 参数缺失或为 `false` 时，后端返回稳定边界错误，不消费计划，允许用户重新确认后重试；
- 普通安装实例、目标配置和字段 diff 的既有确认令牌语义保持不变。

后端只在参数为 `true` 时，把 `ExperimentalCompatibility` 放入 `ConfirmedOperation.confirmations`。不得再根据计划中的 `required_confirmations` 自动声称用户已完成试验性确认。

`plan_agent_connection` 对试验性接入增加可选的 `expected_version`：

- UI 传入刚刚展示并由用户确认的规范化版本；
- 后端完成刷新扫描后，必须与选中安装实例的 `version_normalized` 精确相等；
- 不相等、缺失或过长时拒绝生成计划，不产生写入；
- 已验证版本的现有调用可省略该参数。

### 7.3 应用时重新准入

应用阶段继续先刷新扫描，再消费计划。重新计算后的状态必须与计划证据兼容：

- `DETECTED_VERIFIED` 计划只能以 verified 状态应用；
- `DETECTED_INFERRED` 计划只能以 inferred 状态应用；
- `DETECTED_UNKNOWN` 试验性计划只能在仍为 eligible unknown、Connector 未变化且确认存在时应用；
- 状态变为 `DETECTED_BLOCKED`、`INSTALLED_BROKEN`、无法解析、无唯一 Connector 或预检错误时必须拒绝且不写配置；
- 恢复和断开继续依赖已有 ownership 与快照，不因当前版本变为 unknown 而失去恢复能力。

## 8. 前端设计

`AgentRoutePage` 对 eligible unknown 显示：

- 状态：`版本未经验证`；
- 说明：`当前版本未在兼容目录中验证，可在确认风险后试验性接入。`；
- 主按钮：`试验性接入`；
- 点击后展示明确确认界面，至少包含 Agent 名称、实际版本、canonical path、风险说明以及“写入前会创建快照，失败将自动恢复”的说明；
- 取消时不调用 `plan_agent_connection`；
- 确认后才生成计划，并在应用时传递 `experimental_compatibility_confirmed=true`。

非 eligible unknown 继续禁用接入按钮，并展示后端兼容原因。UI 不根据版本号自行推断 eligibility，而使用后端返回的 `connector_id`、状态和 `allowed_actions`。

## 9. 兼容目录与离线行为

兼容目录职责调整为：

- `verified`：决定是否显示已验证并走正常确认；
- `inferred`：决定是否显示目录支持的试验性推定；
- `blocked`：明确阻断已知不兼容版本；
- 未命中规则：在满足本设计条件时允许本地 Connector 预检和试验性确认。

远程目录仍未在生产启用。本次不配置生产 URL 或公钥。未来启用远程目录后，网络失败、验签失败或目录过期时继续使用内置目录并合并已缓存的阻断规则；离线状态不会把 `blocked` 降级为可接入。未曾获取的新远程阻断规则在离线首启时不可知，这是阻断列表模式的固有限制，UI 的未验证警告、用户确认、Connector 预检和事务恢复用于控制剩余风险，不能宣称完全消除风险。

## 10. 影响面地图

| 维度 | 文件或符号 | 动作 | 验证 | 状态 |
|---|---|---|---|---|
| 兼容判定 | `compatibility.rs::evaluate_discovery` | 为 eligible unknown 绑定唯一 Connector 和只读预检动作 | 状态矩阵单测 | 必须修改 |
| 计划入口 | `commands.rs::plan_connection` | 对 eligible unknown 执行预检并授予试验性确认 | 正负向命令测试 | 必须修改 |
| IPC 契约 | `apply_agent_plan`、`api.ts::applyAgentPlan` | 增加可选试验性确认参数 | API 参数映射测试 | 必须修改 |
| 计划契约 | `plan.rs` | unknown 计划要求 `ExperimentalCompatibility` | 计划单测 | 必须修改 |
| 事务准入 | `transaction.rs` | 允许已确认的 eligible unknown，继续拒绝 blocked/变化状态 | 故障注入与零写入测试 | 必须修改 |
| 前端调用方 | `AgentRoutePage.tsx` | 展示警告、确认和试验性按钮 | RTL 交互测试 | 必须修改 |
| 内置目录 | `builtin-compatibility.json` | 不扩大已验证范围，不删除 blocked | 目录校验测试 | 已检查，不修改 |
| Registry | `builtin-agents.json`、`registry.rs` | 继续提供唯一可信本地 Connector | Registry 契约测试 | 已检查，不修改 |
| Connector | `connectors/**` | 复用现有前置条件、投影与验证 | 现有 Connector 矩阵 | 已检查，不扩大 owned paths |
| 数据面 | IR、Router、Adapter、协议转换 | 禁止修改 | `git diff` 路径审计 | 明确不在范围 |
| 文档 | 本设计、原兼容方案相关结论 | 用本设计明确修订 unknown 准入策略 | 文档一致性检查 | 必须同步 |

## 11. 错误处理

新增或调整的边界错误应满足：

- 未完成试验性确认：稳定错误码，例如 `experimental_confirmation_required`；
- unknown 无唯一 Connector：继续使用 `not_admitted`，文案说明无法安全选择 Connector；
- 计划后变为 blocked：使用现有兼容或准入变化错误，不写配置；
- 预检失败：使用 `read_only_preflight_failed` 或现有解析/前置条件错误，不生成可执行计划；
- 用户取消：纯前端状态，不调用后端，不产生全局错误；
- 用户确认后版本已变化：返回 `discovery_changed_before_plan`，要求重新扫描和确认，不生成计划；
- 所有错误不得回显虚拟 Key、原配置内容、环境变量值或原始版本命令输出。

## 12. 测试与验收

### 12.1 Rust

1. 比内置精确版本更旧和更新的可解析版本进入 `DETECTED_UNKNOWN`，绑定唯一 Connector，并只获得只读预检动作。
2. blocked 优先于 unknown；相同核心版本的预发布版本继续被 blocked。
3. 无法解析版本、`discovery_only`、缺少目录条目和多 Connector 不获得 Connector 或接入动作。
4. eligible unknown 预检成功时生成带 `ExperimentalCompatibility` 的计划。
5. 计划请求的 expected version 与刷新扫描结果不一致时拒绝，防止旧确认授权新版本。
6. unknown 配置解析、Connector 前置条件或投影验证失败时不签发计划。
7. `experimental_compatibility_confirmed` 缺失或为 `false` 时拒绝且保留计划；传 `true` 后允许继续。
8. verified 计划不要求新参数，保持向后兼容。
9. 应用前版本、指纹、Connector 或目录状态变化时拒绝；变为 blocked 时证明目标文件零写入。
10. 快照、ownership、原子写入、写后验证和失败恢复矩阵继续通过。

### 12.2 前端

1. eligible unknown 显示“版本未经验证”和“试验性接入”。
2. 非 eligible unknown 仍禁用接入。
3. 用户取消确认时不调用计划或应用 IPC。
4. 用户确认后携带 UI 已展示的 expected version 调用计划，再以 `experimental_compatibility_confirmed=true` 调用应用。
5. verified 接入不显示试验性警告，现有流程不回归。
6. 后端预检或应用拒绝时展示脱敏错误并恢复按钮状态。

### 12.3 全量门禁

- `cargo fmt --check`；
- Rust 全量测试；
- Clippy `-D warnings`；
- 前端全量测试和覆盖率门禁；
- 前端生产构建；
- `git diff --check`；
- 路径审计确认未修改 IR、Router、Adapter、协议转换和其他数据面文件。

## 13. 实施后 Self-review

实现和自动化验证完成后，必须进行一次独立 self-review，至少检查：

1. 所有 unknown 分支是否都满足“可解析、supported、有目录条目、未 blocked、唯一 Connector”；
2. blocked 是否在扫描、计划和应用三个阶段始终优先；
3. UI 取消是否确实零 IPC、零待处理计划；
4. 后端是否真实校验试验性确认参数，而不是从计划要求自动推定已确认；
5. 计划是否在确认失败时保留，在刷新扫描失败时也不被消费；
6. apply 前状态变化是否全部 fail closed 且零写入；
7. 错误和日志是否泄露版本原始输出、配置内容或虚拟 Key；
8. diff 是否越过第 3 节硬红线；
9. 新增测试是否覆盖更旧、更高、预发布、blocked、无法解析、预检失败、取消和状态竞态；
10. 文档、前端文案、Rust 状态和 IPC 参数是否一致。

发现问题必须先修复并重新运行受影响验证，再对外报告完成。

## 14. 完成标准

只有同时满足以下条件才可声明完成：

- 非精确但满足安全条件的 Agent 版本可以通过明确警告和确认完成接入；
- blocked、不可运行、无法解析和预检失败仍不能接入；
- verified 版本现有体验不回归；
- 快照、revision、原子写入、写后验证和恢复保持有效；
- IR、Router、协议、Adapter 和数据面无变更；
- 全量门禁与实施后 self-review 通过；
- 所有实现修改与测试形成独立本地提交，是否 push 由用户另行授权。
