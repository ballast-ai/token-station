# 桌面 App 接入 Agents 文档设计

- 日期：2026-07-17
- 状态：已获用户确认
- 最终文档：`docs/contributing/桌面App-Agent接入机制.md`
- 读者：使用桌面 App 接入 Agent 的用户，以及维护和扩展该能力的开发者

## 1. 目标

新增一篇独立贡献者文档，准确解释桌面 App 如何完成 Agent 接入。文档同时回答四个问题：

1. 用户点击 Agent 按钮后，App 修改了什么；
2. Agent 发出请求后，请求如何进入 token-station 并到达上游；
3. 当前 Claude Code、Codex、OpenCode 的行为有哪些差异和风险；
4. 维护者如何在不破坏现有边界的前提下扩展新的 Agent。

文档必须描述当前实现，不把计划、历史状态或推测写成已实现能力。

## 2. 事实来源

采用以下事实优先级：

1. 当前可执行源码与自动化测试；
2. 当前源码注释；
3. 现有设计、指南和验收文档。

当三者冲突时，以更高优先级为准。当前部分旧注释和旧文档仍称
`agent-anthropic` “尚未就位”，但桌面 App 默认配置、官方插件、manifest 和测试均已包含
该适配器。最终文档不得沿用这一过时结论。

主要源码锚点：

- `apps/desktop/src/App.tsx`：Agent 按钮和前端交互；
- `apps/desktop/src/api.ts`：`AgentKind` 与 `connectAgent` IPC 封装；
- `apps/desktop/src-tauri/src/lib.rs`：默认 Agent 列表、配置改写、备份、原子替换和
  `connect_agent` 分派；
- `apps/cli/src/config.rs`：`plugins.agents`、兼容旧 `plugins.agent` 的加载规则；
- `apps/cli/src/server.rs`：本地鉴权、fallback 入站入口；
- `apps/cli/src/gateway.rs`：多 Adapter 加载、`select_agent`、请求处理主链；
- `crates/plugin-runtime/src/agent.rs`：WASM `match_inbound` 调用边界；
- `plugins/official/agent-*`：三类入站协议的实际能力与 manifest；
- 桌面端、CLI proxy、plugin-runtime 和 conformance 的现有测试。

## 3. 交付范围

实现阶段应交付：

1. 新建 `docs/contributing/桌面App-Agent接入机制.md`；
2. 从 `docs/contributing/README.md` 增加入口；
3. 从 `docs/contributing/桌面app-设计与交接.md` 增加交叉链接；
4. 仅修正该交接文档中与当前 Agent 接入实现直接冲突的过时描述。

不修改业务代码，不新增 Agent，不实现取消接入按钮，也不扩展协议能力。

## 4. 文档结构

最终文档使用以下目录：

1. 文档目标与事实来源；
2. 用户如何完成一键接入；
3. 接入的两条链路概览；
4. 三种 Agent 的实现对照；
5. 配置文件写入与备份机制；
6. 多入站协议的匹配和转发；
7. 安全边界、已知限制与恢复方法；
8. 如何扩展一个新的 Agent；
9. 测试覆盖与源码索引。

结构采用“双链路”主线，而不是按 Agent 重复讲三遍完整流程，也不按代码模块堆砌实现细节。

## 5. 配置链路

文档应展示以下调用链：

```text
App 中的 Agent 按钮
→ api.ts::connectAgent(kind)
→ Tauri command::connect_agent
→ 检查 App 状态与本地代理是否运行
→ 读取 listen、virtual_key、plugins.agents
→ 按 Agent 类型修改对应配置文件
→ 备份旧文件并原子替换
```

当前三个 Agent 的行为对照：

| Agent | 配置文件 | Base URL | 本地鉴权 | 入站协议 |
|---|---|---|---|---|
| Claude Code | `~/.claude/settings.json` | `http://<listen>` | 写入 `ANTHROPIC_AUTH_TOKEN` | Anthropic Messages |
| Codex | `~/.codex/config.toml` | `http://<listen>/v1` | 配置读取 `TOKENSTATION_KEY` 环境变量 | OpenAI Responses |
| OpenCode | `~/.config/opencode/opencode.json` | `http://<listen>/v1` | 写入自定义 Provider 的 `apiKey` | OpenAI Chat Completions |

用户操作部分应说明：必须先配置可用上游、完成路由并启动本地代理，然后才能点击接入。
成功消息中的后续动作也必须如实保留，例如 Codex 仍要求用户在启动终端设置
`TOKENSTATION_KEY`。

## 6. 请求链路

文档应展示以下运行时主链：

```text
Agent HTTP 请求
→ token-station server
→ 本地虚拟 Key 鉴权
→ Gateway::select_agent
→ 首个 match_inbound 成功的 Agent Adapter
→ Canonical IR
→ Router
→ Provider Adapter
→ 上游模型
```

协议映射按当前实现写明：

- Claude Code：`/v1/messages` → `agent-anthropic`；
- Codex：`/v1/responses` → `agent-openai-responses`；
- OpenCode：`/v1/chat/completions` → `agent-openai`。

必须说明以下边界：

- Server 通过 fallback 把非 `/v1/models` 请求交给 Gateway，不枚举所有协议路径；
- `plugins.agents` 的顺序是匹配优先级，旧 `plugins.agent` 仅在列表为空时回退；
- Gateway 在匹配前把请求头转为脱敏的 `HeaderDigest`；
- 某个 Adapter 在 `match_inbound` 阶段报错时会被记录并跳过，不会否决其他 Adapter；
- Agent Adapter 只负责入站协议归一化和响应渲染，不选择模型或上游；
- Router 不包含 Agent 或供应商特判。

## 7. 写入保护、风险与恢复

配置写入行为按 `read_json_object`、`backup_path` 和 `write_config` 的当前实现描述：

- 文件不存在时从空对象或空 TOML 表创建；
- 文件存在时先解析并保留无关字段；
- 非法 JSON/TOML 在写入前返回错误，原文件不变，也不创建备份；
- 原文件存在时创建 `<原文件名>.token-station.bak`；
- 新内容先写同目录 `.<文件名>.token-station.tmp`，再通过 `rename` 替换；
- 当前 App 没有取消接入入口，恢复操作是停止相关 Agent 后，用备份覆盖当前配置，再重启 Agent。

风险和限制必须显式写出：

- Claude Code 修改全局 `~/.claude/settings.json`，影响本机所有 Claude Code 进程；
- Claude Code 写入前检查已配置的入站 Adapter 名称是否包含 Anthropic；
- Claude Code 和 OpenCode 持久化的是本地虚拟 Key，不是上游供应商 Key；
- Codex 的本地虚拟 Key 不由 App 写入配置；
- Claude Code 接入会关闭 thinking、adaptive thinking、experimental betas 和非必要流量；
- 当前没有 App 内一键恢复；
- 三类 Adapter 的能力集合不同，文本、流式和本地工具闭环通过不等于完整协议兼容；
- 不支持的协议字段应返回明确能力错误，不得描述成已支持或静默降级。

恢复步骤不得使用宽泛删除命令。应针对三个明确配置文件和各自备份文件给出可审计的覆盖操作，
并提醒用户先退出对应 Agent、检查备份内容。

## 8. 新 Agent 扩展契约

文档将扩展分成两类。

### 8.1 复用现有入站协议

若新 Agent 使用现有 Chat Completions、Responses 或 Anthropic Messages：

1. 先确认现有 Adapter 能匹配其请求并覆盖所需能力；
2. 在 `api.ts` 扩展 `AgentKind`；
3. 在 `App.tsx` 的 `AGENTS` 增加入口；
4. 在 Tauri 后端增加 `connect_<agent>_at`，只负责安全修改该 Agent 的配置；
5. 在 `connect_agent` 分派中注册；
6. 增加配置保留、备份、非法配置和幂等测试。

此类扩展通常不修改 Server、Gateway、Router 或 Canonical IR。

### 8.2 引入新的入站协议

除上述桌面接入步骤外，还需要：

1. 新增 `agent-*` WASM Adapter 与 manifest；
2. 实现 `match_inbound`、请求归一化、响应渲染、流式转换和协议形状错误；
3. 将 Adapter 加入 `plugins.agents`，并明确优先级；
4. 增加 conformance fixtures 与真实 WASM 测试；
5. 只有 Canonical IR 不能无损表达新能力时，才提出 IR 变更；
6. 不因新 Agent 在 Router 中增加厂商或 Agent 特判。

## 9. 错误处理与测试证据

文档应按阶段说明错误：

- 接入前：代理未启动、App 状态不可编辑、目标配置无法解析、Anthropic 入站未就绪；
- 写入时：目录创建、备份、临时文件写入或 rename 失败；
- 请求时：本地鉴权失败、无 Adapter 匹配、Adapter 能力拒绝、上游错误。

测试章节不重新声称未执行的 E2E，只引用当前测试实际覆盖的行为：

- Codex 保留已有字段、生成备份，非法 TOML 不被覆盖；
- Claude Code 保留已有设置、生成备份，安全闸与非法 JSON 不写文件；
- OpenCode 保留已有 Provider，重复接入保持幂等；
- CLI proxy 覆盖三种入站路径与本地鉴权边界；
- plugin-runtime 和 conformance 覆盖官方 Adapter 的真实 WASM 与协议 fixtures。

若最终撰写时发现测试名称或实际断言与上述描述不一致，应收缩文档结论，不得扩大测试含义。

## 10. 验收标准

最终文档满足以下条件才算完成：

- 用户能看懂点击接入的前置条件、结果、风险和恢复方法；
- 维护者能沿源码锚点追踪配置链路和请求链路；
- 三种 Agent 的路径、协议、Adapter、配置文件和鉴权方式与当前代码一致；
- 新 Agent 扩展步骤明确区分“复用协议”和“新增协议”；
- 不把 OpenClaw 描述为桌面 App 已支持；
- 不包含“尚未就位”的过时 Anthropic 状态；
- 不含 `TBD`、`TODO`、占位内容或无法由源码和测试支持的结论；
- 相关文档入口可达，且旧交接文档不再与新文档直接矛盾。
