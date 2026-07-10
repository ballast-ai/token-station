# GLM-5.2 本地启动指南

本包把 GLM 当作一个 OpenAI-compatible 的 BYOK 上游：请求先发给本机
`127.0.0.1:8787`，再由 token-station 转发到你配置的 GLM 端点。API Key 只从
环境变量读取，不写入 `token-station.json`、日志或本包。

## 1. 配置 API Key

在本包目录执行：

```bash
export GLM_API_KEY='你的 GLM API Key'
```

不要把此命令写入 shell 历史、配置文件或提交到 Git。若需要持久保管，请改用
系统钥匙串：

```bash
./token-station-cli key set glm provider_api_key
```

然后将 `token-station.json` 内的 `auth` 改为：

```json
{ "slot": "provider_api_key", "keyring": true }
```

## 2. 选择正确端点

本包面向 GLM Coding Plan，模板默认使用其专用 API：

```text
https://api.z.ai/api/coding/paas/v4
```

如果你持有的是通用按量 API Key，而不是 Coding Plan，把
`token-station.json` 的 `base_url` 改为：

```text
https://api.z.ai/api/paas/v4
```

Z.AI 将 Coding endpoint 限定为 Coding 场景；通用 API Key 或一般调用不要使用
它。模型名 `glm-5.2` 来自本地试用目标；若账户控制台显示的可用模型名不同，以
控制台为准并同步修改配置中的两个 `glm-5.2` 值。

## 3. 启动与验证

启动服务：

```bash
./token-station-cli serve
```

首次启动会打印一次本地虚拟 Key（`ts-...`）；立即保存。另开终端：

```bash
export TS_KEY='启动时打印的 ts-...'

curl http://127.0.0.1:8787/v1/models \
  -H "Authorization: Bearer $TS_KEY"

curl http://127.0.0.1:8787/v1/chat/completions \
  -H "Authorization: Bearer $TS_KEY" \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "glm-5.2",
    "messages": [{"role": "user", "content": "回复 OK"}]
  }'
```

IDE 或 Agent 的 OpenAI-compatible 配置为：Base URL
`http://127.0.0.1:8787/v1`，API Key 为本地虚拟 Key，模型为 `glm-5.2`。

## 4. 常用诊断

```bash
./token-station-cli upstream list
./token-station-cli upstream test glm --model glm-5.2
./token-station-cli stats --since 24h --by model
```

`upstream test` 会产生真实上游请求，可能消耗额度。停止服务用 `Ctrl-C`。

## 5. 安全边界

- 本地虚拟 Key 位于 `data/virtual-key`，权限为仅当前用户可读；不要提交它。
- 文件日志与指标库不保存 prompt/response 内容，但会记录用量、延迟和路由元数据。
- 关闭服务不等于撤销 GLM Key；如怀疑泄露，应在 GLM 控制台撤销 Key，并执行
  `key remove glm provider_api_key`（若使用了钥匙串）。

GLM 端点与鉴权方式依据 [Z.AI 官方 HTTP API 文档](https://docs.z.ai/guides/develop/http/introduction)。
