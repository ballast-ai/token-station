# 桌面 App 接入 Agents 文档实施计划

## 目标与边界

按
[设计规格](../specs/2026-07-17-desktop-agent-integration-document-design.md)
新增 `docs/contributing/桌面App-Agent接入机制.md`，从用户操作和维护实现两个层次解释
Claude Code、Codex、OpenCode 如何通过桌面 App 接入 token-station，并给出新增 Agent 的
扩展契约、风险和恢复方法。

事实依据严格按“当前可执行源码与测试 > 源码注释 > 旧文档”排序。只修改文档，不修改
React、Tauri、CLI、Gateway、Router、Canonical IR 或 Adapter 实现。不把 OpenClaw 写成
桌面 App 已支持，不扩大任何协议兼容结论。

## Task 1：建立源码事实清单

### 核对范围

- `apps/desktop/src/App.tsx`：Agent 按钮与 `AGENTS` 列表；
- `apps/desktop/src/api.ts`：`AgentKind` 与 `connectAgent`；
- `apps/desktop/src-tauri/src/lib.rs`：`DESKTOP_AGENTS`、`write_config`、三个
  `connect_*_at`、`connect_agent` 及对应测试；
- `apps/cli/src/config.rs`：`plugins.agents` 与旧 `plugins.agent` 回退；
- `apps/cli/src/server.rs`：本地鉴权和 fallback；
- `apps/cli/src/gateway.rs`：Adapter 加载顺序、`select_agent` 与主处理链；
- `crates/plugin-runtime/src/agent.rs`：`match_inbound`；
- 三个官方 Agent manifest、plugin-runtime、conformance 与 CLI proxy 测试。

### 核对命令

```bash
rg -n "DESKTOP_AGENTS|connect_agent|connect_cc_at|connect_codex_at|connect_opencode_at|write_config" \
  apps/desktop/src apps/desktop/src-tauri/src/lib.rs
rg -n "effective_agents|select_agent|match_inbound|fallback|unauthorized" \
  apps/cli/src crates/plugin-runtime/src/agent.rs
rg -n 'agent_protocols|agent_tools|capabilities' plugins/official/agent-*/manifest.json
rg -n "preserves|backup|invalid|idempotent|v1/messages|v1/responses|chat/completions" \
  apps/desktop/src-tauri/src/lib.rs apps/cli/tests/proxy.rs \
  crates/plugin-runtime/tests/official_plugins.rs crates/conformance/tests/official_plugins.rs
```

发现注释或旧文档与实现冲突时，只把冲突记入本轮文档修改，不改源码注释以外的代码。

## Task 2：编写独立接入机制文档

### 文件

新增 `docs/contributing/桌面App-Agent接入机制.md`。

### 内容顺序

1. 目标、读者与事实来源；
2. 用户前置条件和一键接入步骤；
3. 配置链路与请求链路；
4. Claude Code、Codex、OpenCode 对照表；
5. 配置保留、备份和原子替换；
6. Server fallback、Gateway 选择和 Adapter 协议归一；
7. 全局配置、虚拟 Key、能力边界和手动恢复；
8. 复用现有协议与新增协议两类扩展流程；
9. 测试证据与源码索引。

### 必须精确记录的行为

- Claude Code 写 `~/.claude/settings.json`，Base URL 不带 `/v1`，写入本地虚拟 Key，
  并关闭当前 IR 不支持的 thinking/beta 相关能力；
- Codex 写 `~/.codex/config.toml`，使用 `/v1` Base URL、Responses wire API 和
  `TOKENSTATION_KEY`；
- OpenCode 写 `~/.config/opencode/opencode.json`，增加 `tokenstation` Provider；
- 三者分别进入 `/v1/messages`、`/v1/responses`、`/v1/chat/completions`；
- 入站 Adapter 分别是 `agent-anthropic`、`agent-openai-responses`、`agent-openai`；
- `plugins.agents` 顺序决定首个匹配者，匹配时请求头已经脱敏；
- Agent Adapter 不选择上游或模型；
- App 当前没有取消接入入口。

### 恢复说明

按三个明确配置路径分别给出恢复步骤：退出对应 Agent、检查
`<原文件名>.token-station.bak`、用备份覆盖当前文件、重启 Agent。不得使用递归删除、
宽泛 glob 或声称 App 已提供自动恢复。

## Task 3：接入文档导航并消除直接矛盾

### 文件

- 修改 `docs/contributing/README.md`；
- 修改 `docs/contributing/桌面app-设计与交接.md`。

### 修改范围

- 在贡献者入口增加新文档链接；
- 在桌面 App 交接文档的接入章节增加新文档链接；
- 将“`agent-anthropic` 尚未就位”“Claude Code 端到端仍待实现”等已与当前代码直接冲突
  的描述改为当前状态；
- 保留 Claude Code 全局配置风险和接入前安全闸，因为当前代码仍实际执行这些行为；
- 不顺带重写桌面 App 的路由、模型目录、用量或发布章节。

## Task 4：文档一致性与证据验证

### 自动化

运行当前源码中直接支撑文档结论的测试：

```bash
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo test -p token-station-cli --test proxy
cargo test -p token-station-plugin-runtime --test official_plugins
cargo test -p token-station-conformance --test official_plugins
```

若测试因环境依赖无法运行，最终状态必须区分“文档静态核对通过”和“测试未执行”，不得把
历史验收记录当成本轮执行结果。

### 文档检查

```bash
rg -n "Claude Code|Codex|OpenCode|OpenClaw|agent-anthropic|agent-openai-responses|agent-openai" \
  docs/contributing/桌面App-Agent接入机制.md docs/contributing/桌面app-设计与交接.md
rg -n "TBD|TODO|尚未就位|一键取消|完整兼容" \
  docs/contributing/桌面App-Agent接入机制.md docs/contributing/桌面app-设计与交接.md
git diff --check
git diff -- apps/desktop apps/cli crates plugins
```

人工核对所有文件路径、环境变量、Base URL、端点、Adapter 名称、备份名和恢复命令与源码
一致。确认业务源码目录无 diff，最终文档不存在占位内容、真实凭证或扩大结论。

## Task 5：提交与交付

将最终接入文档、贡献者导航和交接文档的定向修正作为一个本地文档提交，不 push。提交前
再次确认 `.superpowers/` Visual Companion 草稿未进入暂存区，且不夹带用户已有改动。

建议提交信息：

```text
docs: explain desktop Agent integration
```
