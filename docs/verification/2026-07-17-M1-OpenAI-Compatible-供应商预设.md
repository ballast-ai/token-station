# M1 OpenAI-compatible 供应商预设核对与验收记录

- 核对日期：2026-07-17
- 迁移日期：2026-07-21
- 目标分支：`develop`
- 当前阶段：预设验证完成；MiniMax 中国与 GLM 中国已复测，其他新增路径真实 E2E 尚未执行

## 1. 范围与边界

本轮只扩展桌面端 OpenAI-compatible 供应商目录，复用现有
`provider-openai-compatible`。不修改 Canonical IR、Router、Gateway、Provider WASM
或用户全局配置。

预设只包含公开 Base URL 和可编辑的推荐模型。普通按量 API、Coding Plan、中国站与
国际站使用独立预设，避免将不同类型的 Key 发往错误端点。需要 Workspace ID 的阿里云
国际地域不写入静态伪地址，用户应通过“自定义供应商”填写完整 Base URL。

迁移到当前桌面架构时，保留现有 DeepSeek `/v1` Base URL，避免改变已经工作的请求和
`/models` 发现路径；旧版 `App.tsx` 展示逻辑已迁移到 `pages/AddProviderPage.tsx`，目录
测试已接入现有 Vitest 门禁。

## 2. 影响面

| 维度 | 文件或系统 | 动作 | 验证 |
| --- | --- | --- | --- |
| 目录契约 | `apps/desktop/src/catalog.ts` | 增加缺失供应商并拆分区域/套餐 | Vitest 校验 ID、URL、模型与变体 |
| 桌面调用方 | `apps/desktop/src/pages/AddProviderPage.tsx` | 展示预设注意事项 | 组件测试、TypeScript 与 Vite 构建 |
| 样式 | `apps/desktop/src/App.css` | 增加主题化说明样式 | Vite 构建 |
| 运行态 | `<baseUrl>/models`、Bearer Key | 维持现有发现与鉴权方式 | 真实 Key 到位后逐家 probe |
| 协议与数据 | Router、Canonical IR、Rust Provider | 不修改 | Git diff 审计 |

## 3. 官方资料复核

| 供应商 | 预设 Base URL | 鉴权 | 推荐模型示例 | 资料与结论 |
| --- | --- | --- | --- | --- |
| OpenAI | `https://api.openai.com/v1` | Bearer | `gpt-5.6-sol`、`gpt-5.6-terra`、`gpt-5.6-luna` | [官方最新模型说明](https://developers.openai.com/api/docs/guides/latest-model.md)；保留旧模型选项，不做破坏性替换 |
| DeepSeek | `https://api.deepseek.com/v1` | Bearer | `deepseek-v4-flash`、`deepseek-v4-pro` | [官方首次调用](https://api-docs.deepseek.com/)；迁移时保留当前项目的 `/v1` 形式 |
| GLM 中国 | `https://open.bigmodel.cn/api/paas/v4` | Bearer | `glm-5.2` | 已有[真实 E2E 记录](./2026-07-16-Claude-Code-OpenAI-Compatible-Providers-E2E.md) |
| GLM 国际 / Coding Plan | `https://api.z.ai/api/paas/v4`、`https://api.z.ai/api/coding/paas/v4` | Bearer | `glm-5.2` | [Z.AI 工具接入说明](https://docs.z.ai/devpack/tool/others)明确区分通用与 Coding 端点 |
| Kimi 中国 / 国际 | `https://api.moonshot.cn/v1`、`https://api.moonshot.ai/v1` | Bearer | `kimi-k3`、`kimi-k2.6` | [官方快速开始](https://platform.moonshot.cn/docs/guide/start-using-kimi-api)与[问题排查](https://platform.moonshot.cn/docs/guide/faq) |
| Qwen 中国 / 美国 | `https://dashscope.aliyuncs.com/compatible-mode/v1`、`https://dashscope-us.aliyuncs.com/compatible-mode/v1` | Bearer | `qwen3.7-max`、`qwen-plus` | [OpenAI Chat 兼容说明](https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope)；其他地域需要 Workspace ID |
| MiniMax 中国 / 国际 | `https://api.minimaxi.com/v1`、`https://api.minimax.io/v1` | Bearer | `MiniMax-M3`、`MiniMax-M2.7` | [中国站 OpenAI SDK](https://platform.minimaxi.com/docs/api-reference/text-openai-api)与[国际站 OpenAI SDK](https://platform.minimax.io/docs/api-reference/text-openai-api) |
| Groq | `https://api.groq.com/openai/v1` | Bearer | `openai/gpt-oss-120b` | [OpenAI 兼容说明](https://console.groq.com/docs/openai)与[模型目录](https://console.groq.com/docs/models) |
| NVIDIA NIM | `https://integrate.api.nvidia.com/v1` | Bearer | `openai/gpt-oss-120b` | [API Catalog 快速开始](https://docs.api.nvidia.com/nim/docs/api-quickstart)；自托管 NIM 使用自定义 URL |
| Mistral | `https://api.mistral.ai/v1` | Bearer | `mistral-medium-3-5`、`mistral-small-2603` | [Chat API](https://docs.mistral.ai/api/endpoint/chat)与[模型说明](https://docs.mistral.ai/models) |
| xAI | `https://api.x.ai/v1` | Bearer | `grok-4.5` | [Chat Completions](https://docs.x.ai/developers/model-capabilities/legacy/chat-completions)仍兼容但已标为旧接口 |
| 火山方舟标准 / Coding | `https://ark.cn-beijing.volces.com/api/v3`、`https://ark.cn-beijing.volces.com/api/coding/v3` | Bearer | `doubao-seed-2-1-pro-260628`、`ark-code-latest` | [官方 Chat API](https://www.volcengine.com/docs/82379/1494384)；标准与套餐端点分离 |
| BytePlus 标准 / Coding | `https://ark.ap-southeast.bytepluses.com/api/v3`、`https://ark.ap-southeast.bytepluses.com/api/coding/v3` | Bearer | `seed-2-0-lite-260228`、`ark-code-latest` | [Base URL 与鉴权](https://docs.byteplus.com/en/docs/ModelArk/1298459)；标准 API 文档明确 Bearer 鉴权 |

## 4. 自动化验收

无密钥阶段必须通过：

```bash
cd apps/desktop
npm test -- --run
npm run build
```

目录与组件测试覆盖：

- 所有预设 ID 唯一；
- 推荐模型非空且不重复；
- 远端 Base URL 使用 HTTPS、无尾斜杠、无未解析占位符；
- MiniMax、NVIDIA NIM、Mistral、xAI、火山方舟与 BytePlus 必选项存在；
- 区域与 Coding Plan 端点没有被合并；
- 选择预设后显示正确 Base URL、模型和凭证边界说明。

## 5. 真实 E2E 状态

2026-07-16 已有 Qwen 中国、Kimi 中国、MiniMax 中国、GLM 中国的真实普通流式、工具
闭环和鉴权记录，详见[既有验收](./2026-07-16-Claude-Code-OpenAI-Compatible-Providers-E2E.md)。

### 5.1 MiniMax 中国与 GLM 中国复测

2026-07-17 在原 M1 分支使用 Claude Code `2.1.211`、release CLI 与重新构建安装的
`agent-anthropic` / `provider-openai-compatible` 完成复测。上游 Key 只从 macOS
钥匙串读取，测试后已删除本地钥匙串条目；厂商控制台注销由 Key 所有者执行。

| 供应商 | 真实 probe | 普通流式 | `Read` 工具闭环 | 鉴权与记录 | 结论 |
| --- | --- | --- | --- | --- | --- |
| MiniMax 中国 / `MiniMax-M3` | 通过，5892 ms | 命中固定标记，但 `<think>` 混入普通文本 | 通过，2 turns 后得到 `MAX_WIDTH=100` | 3 条请求均为 200、`stream=1`；错误本地 Key 返回 401 且 metrics 不增长 | 功能闭环通过，思考内容分层未兼容 |
| GLM 中国 / `glm-5.2` | 通过，12449 ms | 精确返回固定标记 | 通过，2 turns 后精确得到 `MAX_WIDTH=100` | 3 条请求均为 200、`stream=1`；错误本地 Key 返回 401 且 metrics 不增长 | 当前范围通过 |

两个测试目录的 local virtual key 文件权限均为 `0600`。服务停止后
`127.0.0.1:8787` 无监听；对 `requests.log` 与 `metrics.sqlite` 的精确值扫描未发现
上游 Key、本地 virtual key、错误测试 Key、固定标记、工具提示或最终答案。

### 5.2 尚未验证的新增路径

以下新增路径尚未取得真实 Key，因此不能宣称已兼容：

- MiniMax 国际；
- NVIDIA NIM；
- Mistral；
- xAI；
- 火山方舟标准 API / Coding Plan；
- BytePlus 标准 API / Coding Plan；
- Kimi 国际、Qwen 美国、GLM 国际 / Coding Plan。

真实 Key 到位后，每个端点必须分别完成最小 probe、普通文本、流式和工具调用，并在
PR 中附不含 prompt、响应内容或凭证的 upstream、model、状态与延迟证据。失败项必须
标记为“未兼容”或“有条件兼容”，不得以其他供应商的成功替代。
