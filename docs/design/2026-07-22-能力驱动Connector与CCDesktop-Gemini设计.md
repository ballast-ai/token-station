# 能力驱动 Connector 与 Claude Desktop / Gemini CLI 设计

日期：2026-07-22
状态：已实现并通过自动化验收
对应任务：第二批协作任务 #4

## 1. 背景与问题

当前 Agent Registry 已把发现元数据移出核心枚举，但 Connector 仍有四处中心化知识：

1. `registry.rs` 维护 Connector ID / Agent / Adapter 的重复白名单；
2. `commands.rs::connector_for` 维护手写 `match`；
3. `connectors/mod.rs` 维护手写模块和 conformance 数组；
4. CLI 与前端分别维护 `KNOWN_AGENT_IDS`、`AGENT_ORDER`、`AGENT_MARKS`。

这意味着新增 Agent 仍需修改核心分支，未达到 capability-driven 的目标。Claude Desktop 还存在一个高风险歧义：它的 3P 配置与 Claude Code 的 `~/.claude/settings.json` 是两套契约，不能写后者冒充前者。

## 2. 官方契约与采用结论

### 2.1 Gemini CLI

官方 Gemini CLI 文档与源码确认：

- 用户设置位于 `~/.gemini/settings.json`；
- 用户级环境文件可位于 `~/.gemini/.env`；
- `GOOGLE_GEMINI_BASE_URL` 指向自定义 Gemini API 端点；非本机地址必须使用 HTTPS；
- 设置 `GOOGLE_GEMINI_BASE_URL` 时，当前源码自动选择 `gateway` 鉴权；
- `GEMINI_API_KEY` 可作为网关凭据，Token Station 只投影本地虚拟 key，不写上游真实凭据。

采用：Gemini Connector 以 `~/.gemini/.env` 为单一受管目标，结构化解析 dotenv，只拥有 `GOOGLE_GEMINI_BASE_URL` 与 `GEMINI_API_KEY` 两个键，保留注释、顺序和未知字段。无需改写 `settings.json`。

来源：

- https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md
- https://github.com/google-gemini/gemini-cli/blob/main/packages/core/src/core/contentGenerator.ts
- https://github.com/google-gemini/gemini-cli/blob/main/packages/cli/src/config/settings.ts

### 2.2 Claude Desktop 3P

Claude Desktop 官方 3P 文档确认：

- macOS 本地配置库：`~/Library/Application Support/Claude-3p/configLibrary/`；
- Windows 本地配置库：`%LOCALAPPDATA%\Claude-3p\configLibrary\`；
- `_meta.json` 保存已应用配置 ID，每个配置是同目录 `<uuid>.json`；
- Gateway 使用 `inferenceProvider=gateway`、`inferenceGatewayBaseUrl`、`inferenceGatewayApiKey`、`inferenceGatewayAuthScheme`；
- 配置只在 Claude Desktop 启动时读取；受管 MDM 配置存在时优先且本地配置被忽略。

本机已安装 Claude Desktop 1.22209.3 的只读核对进一步确认 `_meta.json` 结构为：

```json
{
  "appliedId": "<uuid>",
  "entries": [{ "id": "<uuid>", "name": "Token Station" }]
}
```

采用：Claude Desktop Connector 使用固定、合法且可重复识别的 UUID 配置文件，并把 `_meta.json` 作为同一原子计划的第二个投影目标。两份文件都进入加密快照、ownership、漂移与回滚。Linux 在能力查询和计划阶段返回结构化 `unsupported_platform`，绝不回退到 Claude Code 配置。

来源：

- https://claude.com/docs/third-party/claude-desktop/configuration
- https://claude.com/docs/third-party/claude-desktop/in-app-configuration
- https://claude.com/docs/third-party/claude-desktop/data-storage

## 3. 目标架构

### 3.1 单一能力来源

每个 Connector 模块导出一个 `CONNECTOR` 静态实例；实例通过 `capabilities()` 声明：

- `connector_id`、`agent_id`、展示名；
- 支持平台；
- 入站 Adapter 与 URL 形态；
- 一个或多个配置投影：路径、格式、受管字段、敏感字段；
- 是否需要本地虚拟 key；
- 应用后是否需要重启。

构建脚本扫描 `connectors/*.rs` 自动生成模块注册与 `BUILTIN_CONNECTORS`。新增 Connector 只需新增模块，不再修改 `commands.rs`、`registry.rs` 或枚举分支。

Agent 描述 JSON 只引用 Connector ID；Registry 从生成的 Connector registry 校验 Agent / Adapter / 平台一致性，不再维护第二份白名单。

### 3.2 控制面派生

`AgentUiMetadata` 增加能力视图，前端完全遍历后端 Registry：

- 顺序使用描述表中的 `ui_order`；
- 导航标记使用描述表中的 `nav_mark`；
- 是否可连接、当前平台支持状态、协议与配置格式来自 Connector capability；
- 删除 `AGENT_ORDER`、`AGENT_MARKS` 和 Agent ID 特判。

CLI 的 route map 不再用固定 Agent ID 数组校验，只校验 lower-kebab 标识符；Desktop 需要枚举 Agent 时从 Registry 获取。这样新增合法 Agent ID 不要求修改 CLI。

### 3.3 多文件原子投影

内部计划从“一个目标文件”升级为 `Vec<PreparedFileProjection>`：

- 每个目标独立记录存在性、原始哈希、预期哈希、权限、owned paths 和脱敏 diff；
- apply 前逐个复验哈希；
- 所有临时文件先完成解析校验，再依次原子替换；
- 任一失败时按逆序恢复已替换目标；
- 快照与 ownership 按文件记录，计划层共享同一 operation ID；
- disconnect / restore 同样覆盖全部投影目标。

此模型用于 Claude Desktop 的配置 JSON + `_meta.json`，也为后续 #6 `ConnectorProjection` 的字段级恢复提供正确底座。

## 4. TDD 执行顺序

1. 红测：虚拟 Agent 描述加入 Registry 后自动出现在 UI，前端不存在 ID 分支；合法新 route ID 被 CLI 接受、非法 ID 拒绝。
2. 红测：Connector 自动注册表能查到模块，Registry 能从真实 Connector capability 校验绑定；删除手写 `connector_for` match。
3. 红测：平台能力返回支持/不支持；Claude Desktop 在 Linux 为 `unsupported_platform`，且所有路径均不包含 `.claude/settings.json`。
4. 红测：dotenv round-trip 保留注释、未知键与顺序，只修改两个 Gemini 键，disconnect 恢复受管字段。
5. 红测：Claude Desktop 双文件计划完整展示、双文件 apply、失败回滚、disconnect/restore、未知字段保留、敏感值不进入 IPC/日志。
6. 实现最小代码使各批红测转绿；每批运行定向测试。
7. 运行 Rust/前端全量测试、clippy、format、diff check；更新验收文档。

## 5. 退出条件

遇到以下任一情况暂停 #4 并记录阻塞证据：

- 官方契约不足以确定写入路径或字段，继续实现会有误写风险；
- 多文件事务无法证明失败可恢复或密钥不泄漏；
- 发现与任务边界冲突，需要修改 Capability 四态或发布签名门；
- 用户要求暂停或调整范围。

## 6. 通过条件

- 新增 Agent 不需要修改核心枚举、`commands.rs` match、CLI ID 数组或前端 Agent ID 分支；
- Claude Desktop 与 Gemini CLI 均由能力表生成入口；
- Claude Desktop 只写 3P `configLibrary`，macOS/Windows 支持，Linux 返回 `unsupported_platform`；
- Gemini dotenv、OpenClaw JSON5、Hermes YAML 的未知字段与注释通过 round-trip 测试；
- Claude Desktop 多文件写入可预览、可回滚、可恢复，凭据仅使用本地虚拟 key；
- 通用测试门全部通过。

## 7. 交付产物

- Connector capability 类型、自动注册代码与 Registry 校验；
- 去中心化 CLI/Desktop/前端 Agent 列表；
- dotenv lossless codec 与 Gemini Connector；
- Claude Desktop 3P Connector 与多文件原子投影；
- 单元、契约、事务、前端回归测试；
- `docs/verification/2026-07-22-第二批协作任务-T4能力驱动Connector验收.md`。
