# OpenClaw 安全接入指南

Token Station 桌面端从 `0.1.0` 开始可以自动发现并接入 OpenClaw。当前内置兼容目录没有
版本 blocklist，因此不会只按一个精确版本阻断接入；这不代表任意未来版本都经过真实验收，
路径、配置结构、适配器就绪和内部计划复核仍会在写入前失败关闭。

官方依据：

- [OpenClaw v2026.6.11](https://github.com/openclaw/openclaw/releases/tag/v2026.6.11)
- [JSON5 配置与路径](https://docs.openclaw.ai/gateway/configuration)
- [自定义 Provider 字段](https://docs.openclaw.ai/gateway/config-tools)

## 1. 自动发现

桌面端只读检查：

- 可执行文件：`openclaw`，版本命令 `openclaw --version`；
- 显式路径：`OPENCLAW_CONFIG_PATH`；
- 状态目录：`OPENCLAW_STATE_DIR/openclaw.json`；
- 默认路径：`~/.openclaw/openclaw.json`；
- 兼容环境：macOS、Linux、Windows、WSL fixtures。

扫描不执行 install、update、doctor、repair，不创建 OpenClaw 目录，也不启动 Gateway。
多安装实例必须由用户选择唯一目标。

## 2. 接入内容

在 Agent 页面选择 OpenClaw 后点击“一键接入”，桌面端会生成内部计划并立即写入。首次
接入会在写入完成后展示关键改动。Connector 的 owned paths 是：

```text
/models/providers/tokenstation
/agents/defaults/model/primary
```

Connector 写入的核心结构：

```json5
{
  models: {
    providers: {
      tokenstation: {
        baseUrl: "http://127.0.0.1:8787/v1",
        apiKey: "<本地虚拟 Key>",
        api: "openai-completions",
        models: [{
          id: "auto",
          name: "Token Station Auto",
          reasoning: false,
          input: ["text"],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow: 200000,
          maxTokens: 32000,
        }],
      },
    },
  },
  agents: {
    defaults: { model: { primary: "tokenstation/auto" } },
  },
}
```

现有 `channels`、Gateway、MCP、浏览器、skills、其他 Provider 和其他 Agent 默认设置均
不属于 Token Station。JSON5 注释、尾逗号和未知字段通过语法树投影保留；重复键、无效
JSON5 或 owned path 父级类型错误会在写入前拒绝。

如果根对象或 owned path 的任一祖先使用 `$include`，当前 Connector 同样拒绝接入。原因是
直接增加 sibling 可能改变 OpenClaw 的 include 合并/覆盖语义；在实现 include-aware 多文件
事务前，不以丢配置风险换取表面兼容。

## 3. 写入和断开

只有以下条件同时满足才会写入：

1. Agent descriptor 已准入，且版本没有命中兼容目录 blocklist；
2. 安装实例和配置路径仍与最近扫描一致；
3. `agent-openai` 已在 Token Station 运行态加载；
4. 内部计划、文件 revision 和确认令牌仍有效。

点击“一键接入”即代表同意写入。后端先创建 AES-256-GCM 加密快照，再原子替换配置、重新解析、自检并提交
ownership。快照 master key 保存在本地私有 `snapshot-master.key`。

“恢复官方配置并断开”只移除上述 owned paths，保留接入后用户修改的其他字段。用户或
其他工具改动 owned values 时，Token Station 会拒绝覆盖并要求重新扫描。

历史 `openclaw.json.token-station.bak` 仅作为只读候选展示，不覆盖、不删除、不自动恢复。

## 4. OpenClaw 更新后的行为

OpenClaw 更新后，Token Station 会重新扫描版本、路径和配置结构。当前空 blocklist 不会
仅因版本号变化自动禁止接入，但签名兼容目录可以增加明确的阻断范围；配置 schema 或 owned
path 结构不兼容时，Connector 仍会在写入前拒绝。Token Station 不会自动降级或升级
OpenClaw，也不会通过兼容目录远程下发 Connector 代码。

如果新版本改变 `openclaw.json` schema，则新增 `openclaw-v2` Connector；旧版本继续
绑定 `openclaw-v1`，不能用同一个 Connector ID 静默改变 owned paths。

## 5. 协议边界

OpenClaw 的 `openai-completions` 请求进入 Token Station
`/v1/chat/completions`，复用 `agent-openai`。文本、流式和 function tool 主链沿用
现有协议回归；Gateway、远程 channel、MCP、浏览器、用户 skills 和 OpenClaw 自身安装升级
不在本功能范围。

该接入不修改 `crates/router-core/**`。Router 只看到归一后的 Canonical IR，不按
“OpenClaw”名称增加任何特判。
