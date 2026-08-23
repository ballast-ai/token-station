# Azure OpenAI v1 with South Header Auth

## Scope

Use this flow only for Azure OpenAI v1 Chat Completions. It uses the fixed `api-key` header. It does not
support the legacy `deployments/...?...api-version=` API or Microsoft Entra ID OAuth.

Prepare an Azure OpenAI resource endpoint, a deployment name, and a resource API key. Use this Base URL:

```text
https://my-resource.openai.azure.com/openai/v1
```

Do not append `chat/completions`, query parameters, credentials, or a dated `api-version`.

## Desktop setup

1. Open Add Provider and select Custom setup.
2. Select `Azure OpenAI v1` as the API dialect.
3. Enter the `/openai/v1` Base URL.
4. Add the deployment name manually as the model.
5. Select the local credential store or an environment variable.
6. Save the provider. Restart the proxy when the App requests it.

South is the default transport, and its default tier carries Azure Header Auth, so nothing has to be
selected. The provider's details show a read-only transport row: `South active`, or `Legacy fallback`
with the host's reason when the provider's shape is outside the South slice. The narrower
`south_v1_buffered` and `south_v1_buffered_streaming` values, if written into a config by hand, do not
carry Azure Header Auth and keep Azure on Legacy.

`azure-openai-v1` is an official reserved dialect. Token Station rejects an existing third-party or
explicit mapping with the same dialect. Rename or remove that old mapping before upgrading.

## Equivalent JSON

```json
{
  "upstreams": {
    "azure": {
      "provider": "azure-openai-v1",
      "base_url": "https://my-resource.openai.azure.com/openai/v1",
      "auth": {
        "slot": "provider_api_key",
        "env": "AZURE_OPENAI_API_KEY"
      },
      "models": [
        {
          "model": "my-deployment"
        }
      ]
    }
  }
}
```

The provider, engine, and credential-header contracts are closed. The official adapter always renders
`api-key` for this dialect. Configuration cannot select an arbitrary credential header.

## Canary and rollback

After saving and restarting, run a real canary only with an authorized account:

```bash
token-station-cli upstream test azure \
  --model my-deployment
```

The probe uses the South transport by default, as served traffic does; `--transport legacy` probes the
fallback path.

This command sends one real Azure completion. It can consume quota and incur charges. A successful
canary sends one request to `/openai/v1/chat/completions`, uses only `api-key`, and does not replay a
South attempt through Legacy.

To pin this upstream to legacy, add `"provider_call": "legacy"` to its configuration and restart the
proxy. Keep the Base URL, deployment, and credential unchanged so the change tests only the execution
engine.

Current limits:

- Chat Completions only.
- Local-store and environment-variable credentials only for South.
- Direct egress and translated API dialect only.
- No legacy Azure query API, OAuth, multiple secrets, or arbitrary Header Auth.
- Deployment names are manual. Token Station does not use Bearer model discovery for Azure.
- Legacy remains available as the automatic fallback and as an explicit `provider_call` value.

---

# Azure OpenAI v1 与 South Header Auth

## 适用范围

此流程只适用于 Azure OpenAI v1 Chat Completions，并固定使用 `api-key` Header。它不支持旧版
`deployments/...?...api-version=` API，也不支持 Microsoft Entra ID OAuth。

准备 Azure OpenAI Resource Endpoint、Deployment Name 和 Resource API Key。Base URL 必须是：

```text
https://my-resource.openai.azure.com/openai/v1
```

不要附加 `chat/completions`、Query 参数、Credential 或带日期的 `api-version`。

## Desktop 配置

1. 打开“添加供应商”，选择“自定义配置”。
2. 将 API 方言设为 `Azure OpenAI v1`。
3. 填写以 `/openai/v1` 结尾的 Base URL。
4. 把 Deployment Name 作为模型手工加入。
5. 选择本地凭据存储或环境变量。
6. 保存 Provider。App 提示时重启代理。

South 是默认传输引擎，默认档位已包含 Azure Header Auth，无需任何选择。Provider 详情里有一行只读的
传输状态：`South 已启用`，或在 Provider 形态超出 South 范围时显示 `回落到 Legacy` 及宿主给出的原因。
手工写进配置的旧值 `south_v1_buffered` / `south_v1_buffered_streaming` 不包含 Azure Header Auth，
会让 Azure 停留在 Legacy。

`azure-openai-v1` 是官方保留方言。如果已有第三方或显式映射使用同名方言，Token Station 会
拒绝启动。升级前请先重命名或移除旧映射。

JSON 等价配置见上方英文部分。`provider`、`provider_call` 和 Credential Header 都是封闭契约。
官方 Adapter 会为该方言固定生成 `api-key`，配置不能选择任意 Credential Header。

## Canary 与回滚

保存并重启后，仅在已授权 Azure 账号上运行真实 Canary：

```bash
token-station-cli upstream test azure \
  --model my-deployment
```

探测默认使用 South 传输（与正式流量一致）；`--transport legacy` 探测回落路径。该命令会发出一次真实 Azure Completion，可能消耗额度并产生费用。成功 Canary 应只向
`/openai/v1/chat/completions` 发出一次请求，只使用 `api-key`，且 South 尝试不会通过 Legacy
重放。

要把该上游固定在 Legacy，在其配置中加入 `"provider_call": "legacy"` 并重启代理。不要同时修改
Base URL、Deployment 或 Credential，否则无法单独判断执行引擎的影响。

当前限制：

- 只支持 Chat Completions。
- South 只支持本地存储和环境变量凭据。
- 只支持直连 Egress 和 Translated API Dialect。
- 不支持旧版 Azure Query API、OAuth、多 Secret 或任意 Header Auth。
- Deployment Name 只能手工填写，不会对 Azure 使用 Bearer 模型发现。
- Legacy 仍然保留：既是自动回落目标，也可作为显式 `provider_call` 值。
