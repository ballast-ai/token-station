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
6. Save the provider and open its details.
7. Open Advanced runtime.
8. Select `South buffered + streaming + Header Auth`.
9. Save the details. Restart the proxy when the App requests it.

The older `south_v1_buffered` and `south_v1_buffered_streaming` values do not enable Azure Header Auth.
Azure stays on Legacy unless the fourth value is selected explicitly.

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
      "provider_call": "south_v1_buffered_streaming_header_auth",
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
  --model my-deployment \
  --transport south-v1
```

This command sends one real Azure completion. It can consume quota and incur charges. A successful
canary sends one request to `/openai/v1/chat/completions`, uses only `api-key`, and does not replay a
South attempt through Legacy.

To roll back, select Legacy in Provider details and restart the proxy. Keep the Base URL, deployment,
and credential unchanged so the rollback tests only the execution engine.

Current limits:

- Chat Completions only.
- Local-store and environment-variable credentials only for South.
- Direct egress and translated API dialect only.
- No legacy Azure query API, OAuth, multiple secrets, or arbitrary Header Auth.
- Deployment names are manual. Token Station does not use Bearer model discovery for Azure.
- South remains opt-in and Legacy remains available.

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
6. 保存 Provider，然后打开详情。
7. 展开“高级运行时”。
8. 选择 `South 非流式 + 流式 + Header Auth`。
9. 保存详情。App 提示时重启代理。

旧的 `south_v1_buffered` 和 `south_v1_buffered_streaming` 不会启用 Azure Header Auth。只有显式
选择第四档后，Azure 才会使用 South。

`azure-openai-v1` 是官方保留方言。如果已有第三方或显式映射使用同名方言，Token Station 会
拒绝启动。升级前请先重命名或移除旧映射。

JSON 等价配置见上方英文部分。`provider`、`provider_call` 和 Credential Header 都是封闭契约。
官方 Adapter 会为该方言固定生成 `api-key`，配置不能选择任意 Credential Header。

## Canary 与回滚

保存并重启后，仅在已授权 Azure 账号上运行真实 Canary：

```bash
token-station-cli upstream test azure \
  --model my-deployment \
  --transport south-v1
```

该命令会发出一次真实 Azure Completion，可能消耗额度并产生费用。成功 Canary 应只向
`/openai/v1/chat/completions` 发出一次请求，只使用 `api-key`，且 South 尝试不会通过 Legacy
重放。

需要回滚时，在 Provider 详情中选择 Legacy 并重启代理。不要同时修改 Base URL、Deployment 或
Credential，否则无法单独判断执行引擎的影响。

当前限制：

- 只支持 Chat Completions。
- South 只支持本地存储和环境变量凭据。
- 只支持直连 Egress 和 Translated API Dialect。
- 不支持旧版 Azure Query API、OAuth、多 Secret 或任意 Header Auth。
- Deployment Name 只能手工填写，不会对 Azure 使用 Bearer 模型发现。
- South 仍需显式启用，Legacy 仍然保留。
