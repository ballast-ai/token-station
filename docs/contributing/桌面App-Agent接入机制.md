# 桌面 App 的 Agent 发现、兼容与安全接入机制

本文描述当前可执行源码，面向维护 Token Station 桌面端 Agent 控制面的开发者。

“接入 Agent”仅表示：用户点击“一键接入”后修改目标 Agent 的本机配置，使其模型请求进入
Token Station 回环代理。桌面 App 不会安装、升级、启动或修复第三方 Agent，也不会改变
`crates/router-core/**` 中的路由算法和核心契约。

## 1. 当前能力

内置 Registry 当前有九种 Agent，其中八种提供通用 Connector，Cursor 使用被动发现和
专用 SQLite 接入：

| Agent | 发现 | 正式配置接入 | Connector | 入站 Adapter |
|---|---|---|---|---|
| Claude Code | 是 | 是 | `claude-code-v1` | `agent-anthropic` |
| Claude Desktop | 是 | 是 | `claude-desktop-3p-v1` | `agent-anthropic` |
| Codex | 是 | 是 | `codex-v1` | `agent-openai-responses` |
| Gemini CLI | 是 | 是 | `gemini-cli-v1` | `agent-gemini` |
| OpenCode | 是 | 是 | `opencode-v1` | `agent-openai` |
| OpenClaw | 是 | 是 | `openclaw-v1` | `agent-openai` |
| Hermes | 是 | 是 | `hermes-v1` | `agent-openai` |
| WorkBuddy | 是 | 是 | `workbuddy-v1` | `agent-openai` |
| Cursor | 被动发现 | macOS / Windows 专用 SQLite 接入 | 无 | OpenAI 兼容路由 |

列表由后端 Registry 动态返回，前端没有固定 Agent 联合类型。当前内置兼容目录的
blocklist 为空，不用最低版本或 SemVer 白名单阻断接入；精确安装路径、可运行性、
唯一 Connector、适配器就绪、计划令牌和目录安全边界仍然必须满足。签名兼容目录可以
用更高 sequence 新增明确阻断规则，但不能注入代码或扩大 Connector 权限。

## 2. 控制面流程

```text
启动页面或点击重新扫描
  → 后端只读扫描已知路径、环境覆盖和 PATH
  → 规范化安装路径、版本、配置指纹和多实例冲突
  → 内置/签名兼容目录计算状态和允许动作
  → 用户选择唯一安装实例
  → 后端生成短时、脱敏的配置计划
  → 同一次点击中前端取得目标、owned paths、差异和确认令牌
  → 前端立即提交 Apply
  → 后端复验扫描、版本、指纹、目录与代理运行态
  → 加密快照 → 原子写入 → 写后解析/自检 → ownership 提交
  → 首次接入在写入后展示改动字段
```

前端只能提交 `agent_id`、最近扫描中的精确 `installation_path`、`operation_id` 和
确认令牌，不能提交目标配置路径、patch、配置字节或命令。当前 UI 把计划和 Apply 放在
一次点击内；确认令牌绑定计划摘要、Webview
会话、随机挑战和过期时间；计划仅保存在内存中，成功或失败消费后不能重放。

当前 Tauri IPC：

- `scan_agents`
- `plan_agent_connection`
- `apply_agent_plan`
- `plan_agent_disconnect`
- `list_agent_snapshots`
- `plan_snapshot_restore`
- `apply_snapshot_restore`

旧 `connect_agent(kind)` 和 `connect_*_at` 直写入口已删除。

## 3. 发现与兼容状态

扫描只运行 Registry 声明的参数化版本命令，不经过 shell；有超时和输出上限，不创建目录、
不写配置、不执行 install/update/repair。相同 canonical path 去重，不同安装实例形成冲突组。

主要状态：

- `DETECTED_VERIFIED`：命中允许范围，可以生成内部计划并接入；
- `DETECTED_INFERRED`：命中受限补丁范围且配置指纹一致，仍需通过更严格的计划门禁；
- `DETECTED_UNKNOWN`：版本或指纹未知，只读展示；
- `DETECTED_BLOCKED`：命中明确阻断规则，禁止接入；
- `INSTALLED_BROKEN`：可找到程序但版本探测失败；
- `MULTIPLE_INSTALLATIONS`：必须先选择唯一安装实例；
- `CONNECTED`：存在有效 ownership 记录。

未知版本默认保护，不能用前端参数绕过。断开和恢复使用已绑定的 ownership，在兼容目录后来
收紧时仍保留安全退出通道。

兼容目录由内置目录和可选签名远程目录组成。远程目录只能声明版本范围、配置指纹和本地已有
Connector ID，不能携带脚本、下载器、配置路径或 Connector 代码。签名、schema、有效期、
sequence 回滚或缓存完整性任一检查失败时 fail closed；离线回退只能收紧写入能力。

## 4. 内置 Connector

### Claude Code

目标为 `~/.claude/settings.json`。owned paths 是以下 `env` 键：

```text
ANTHROPIC_BASE_URL
ANTHROPIC_AUTH_TOKEN
MAX_THINKING_TOKENS
CLAUDE_CODE_DISABLE_THINKING
CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING
CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS
CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC
```

其他顶层字段和其他 `env` 键保留。只有 `agent-anthropic` 运行态就绪时才能接入。

### Claude Desktop

目标为 Claude 3P profile 的固定配置库条目，同时管理 `_meta.json` 与两个
deployment mode companion。主 profile 只拥有 inference provider、回环 Base URL、
本地虚拟 Key、鉴权方式、chooser 开关和回环 egress allowlist。Claude Desktop
需要重启后生效。

### Codex

目标为 `~/.codex/config.toml`。owned paths：

```text
/model
/model_provider
/model_providers/tokenstation
```

Connector 使用 `toml_edit` 保留非归属表、字段和注释，写入 Responses provider，并让 Codex
从 `TOKENSTATION_KEY` 环境变量取本地虚拟 Key。只有 `agent-openai-responses` 就绪时
才能接入。

### Gemini CLI

主目标为 `~/.gemini/.env`，只拥有 `GOOGLE_GEMINI_BASE_URL` 和 `GEMINI_API_KEY`；
同时把 `~/.gemini/settings.json` 的 `security.auth.selectedType` 设为
`gemini-api-key`。请求由 `agent-gemini` 处理。

### OpenCode

目标为 `~/.config/opencode/opencode.json`。唯一 owned subtree 是
`/provider/tokenstation`，其他 Provider 和顶层字段保留。只有 `agent-openai` 就绪时
才能接入。

所有 Connector 都先解析源配置并验证父级结构。配置不存在时可从空对象/空文档生成；无效
UTF-8、无效 JSON/TOML 或 owned path 父级类型错误时，在快照和写入前拒绝。

### OpenClaw

目标为 `~/.openclaw/openclaw.json`，同时尊重 Registry 解析出的
`OPENCLAW_CONFIG_PATH` / `OPENCLAW_STATE_DIR`。owned paths 是
`/models/providers/tokenstation` 和 `/agents/defaults/model/primary`。官方配置是 JSON5，
Connector 使用 round-trip AST 保留注释、尾逗号和未知字段，并复用 `agent-openai`。

### Hermes Agent

目标为 `$HERMES_HOME/config.yaml`，默认 `~/.hermes/config.yaml`；Windows 默认根为
`%LOCALAPPDATA%/hermes`。owned paths 是 `model.default`、`model.provider`、
`model.base_url`、`model.api_key` 和 `model.api_mode`。Connector 固定 `provider: custom`、
`api_mode: chat_completions`，使用 lossless YAML 编辑器保留根注释和未知字段，并拒绝重复键、
merge key、多文档或 flow/deep-path 歧义写入。

### WorkBuddy

目标为已发现安装对应的 `models.json`，只拥有 `models` 与 `availableModels`，写入
`tokenstation-auto` 模型并指向本地 `/v1/chat/completions`。其他 JSON 字段保留。

### Cursor

Cursor 没有通用 Connector。macOS 与 Windows 上，用户必须先自行退出 Cursor；专用命令
随后备份 `applicationUser` 与 OpenAI Key 两条原值，并在一笔 SQLite 事务中写入
Token Station Base URL 和本机虚拟 Key，回读不匹配时立即恢复原值。该路径当前没有
ownership 记录或应用内断开按钮，不能套用下文通用 Connector 的恢复承诺；也可以在
Cursor 设置中手动填写 TS API。

## 5. 配置事务、归属和恢复

所有新增写入统一经过 `TransactionEngine`：

1. 核验一键接入流程带回的确认令牌、计划有效期、兼容目录 sequence 和安装绑定；
2. 核验目标文件 revision，防止计划生成后被其他进程修改；
3. 从本地私有 `snapshot-master.key` 读取本机 master key；
4. 创建 AES-256-GCM 加密快照，索引和密文使用私有权限；
5. 同目录写临时文件、flush/fsync、恢复元数据并原子替换；
6. 写后重新读取、解析并执行 Connector 自检；
7. 以 HMAC 保护的 owned values 提交 ownership revision；
8. 任一步骤失败时按加密快照恢复；恢复失败明确报告 repair-required。

断开和快照恢复只投影 declared owned paths。用户在接入后新增或修改的非归属字段保留；如果
用户或其他工具修改了受管值，则拒绝写入并要求重新扫描。

历史版本可能留下：

```text
settings.json.token-station.bak
config.toml.token-station.bak
opencode.json.token-station.bak
```

这些文件只作为“旧版 .bak（只读候选）”展示，不解析内容、不自动恢复、不覆盖、不迁移、
不删除。新事务只创建加密快照。确需使用旧备份时应由用户在退出 Agent 后人工核对。

## 6. 请求数据面

```text
Agent HTTP 请求
  → 回环 Server 鉴权
  → Gateway 选择 Agent Adapter
  → Canonical IR
  → router-core（本项目红线，不由接入功能修改）
  → Provider Adapter
  → 上游模型
```

配置接入只改变控制面。Claude Code 使用 `/v1/messages`，Codex 使用 `/v1/responses`，
OpenCode 使用 `/v1/chat/completions`。新增 Agent 如果复用现有协议，优先复用已有
Adapter；不得因为 Agent 名称在 Router 中增加特判。

## 7. 第三方 Agent 更新后的适配

Agent 发布新版本后，默认结果是 `DETECTED_UNKNOWN`，不是继续盲写。维护顺序：

1. 只读记录官方版本、配置 schema、路径和版本输出；
2. 用隔离 HOME/fixtures 验证发现规则与配置指纹；
3. 用现有 Connector 跑缺失配置、未知字段、非法配置、重复接入、断开和恢复；
4. 协议级验证请求、流式、工具调用和错误形状；
5. Connector 不变时，仅扩展签名兼容目录中的精确验证范围；
6. 配置契约变化时新增版本化 Connector，旧 Connector 和快照仍可用于旧版本退出；
7. 发现破坏性版本时发布 blocked 规则；
8. 完成真实机灰度后才扩大正式支持承诺。

远程目录不能自动升级 Agent，也不能远程注入代码，因此 Claude Code/OpenCode 等升级不会把
未经验证的新配置写法直接带入用户机器。

## 8. 新增 Agent 的准入清单

1. 在 Registry 增加纯数据 Descriptor，先设 `discovery_only`；
2. 补 macOS/Linux/Windows/WSL 只读发现 fixtures；
3. 固定官方配置契约和目标版本证据；
4. 确认复用 Adapter 或另行实现、验证入站 Adapter；
5. 实现版本化 Connector，最小化 owned paths；
6. 补配置缺失、未知字段、非法配置、快照、并发、回滚、断开和恢复测试；
7. 在内置目录加入精确版本，未知和未来版本继续保护；
8. 完成真实环境 E2E 和 UI 首次接入改动展示验收；
9. 检查 `crates/router-core/**` 摘要和专项红线门禁；
10. 最后才把 admission 从 `discovery_only` 改为 `supported`。

任何一步都不得通过修改用户真实全局配置、安装第三方 Agent 或放宽核心路由红线来完成测试。

完整新增/升级流程见 [Agent Connector 准入指南](Agent-Connector-准入指南.md)，目录运维见
[Agent 兼容目录发布与回滚](../release/Agent-兼容目录发布与回滚.md)。
