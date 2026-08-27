# Enterprise Managed Routing

This guide defines the desktop contract for an enterprise-managed route.
It also defines the minimum contract that an enterprise gateway must implement.

## Purpose and ownership

An enterprise-managed route uses a model exposed by an enterprise service.
Token Station verifies the live model catalog and requires the user to select one model during connection.

```text
Agent
  -> Token Station local gateway
  -> Enterprise OpenAI-compatible endpoint
  -> User-selected enterprise model
```

Token Station owns the local Agent connection, protocol normalization, credential injection, and route activation.
The enterprise service owns the available models, quotas, fallback, and policy changes.

## Connection fields

The Models page opens an Enterprise routing dialog with these values:

| Field | Required | Meaning |
|---|---:|---|
| Base URL | Yes | The OpenAI-compatible API root, such as `https://router.example.com/v1`. |
| API Key | Yes | The Bearer credential for the enterprise service. |
| Model | Yes | One model returned by live endpoint verification. |

The local provider name is fixed to `Token-station`.

Enter the API root. Do not include a credential, query string, or fragment in the Base URL.
An origin-only URL defaults to `/v1`.
A URL that ends in `/models`, `/chat/completions`, `/responses`, or `/messages` is normalized to its API root.

## Connection and activation flow

The connection flow runs this sequence:

1. Validate the Base URL and API key.
2. Perform live endpoint and credential verification.
3. Show the returned model IDs and require one explicit selection.
4. Reject an existing provider named `Token-station`.
5. In one backend draft mutation, store the selected model, set global routing to Direct, and set its direct target.
6. Save and apply the configuration.

Agents that inherit global routing use the enterprise route after the apply operation succeeds.
Agent-specific route overrides remain unchanged.

## Verification request

Token Station verifies the endpoint with this request:

```http
GET /v1/models HTTP/1.1
Authorization: Bearer <enterprise-api-key>
Accept: application/json
User-Agent: token-station-desktop/model-discovery
```

The exact path uses the configured API root.
For example, `https://router.example.com/api/v4` resolves to `/api/v4/models`.

The response must use the OpenAI-compatible `data` array shape and contain at least one model ID.

```json
{
  "object": "list",
  "data": [
    { "id": "enterprise-reasoner", "object": "model" }
  ]
}
```

This request has a six-second global timeout.
The response body limit is 2 MiB.
Redirects are not followed.
A cached model response does not count as credential verification.

The dialog shows the returned model IDs for selection. Token Station saves only the selected model on the
`Token-station` provider; verification does not write the result to the general model-catalog cache.

## Managed provider identity

Token Station stores the selected model and marks the provider with `managed_route: true`:

```yaml
models:
  - model: enterprise-reasoner
managed_route: true
```

This identity lets the desktop restore the connected enterprise state and distinguish it from an ordinary provider.

At runtime, the OpenAI-compatible provider adapter sends the selected model in the request body:

```http
POST /v1/chat/completions HTTP/1.1
Authorization: Bearer <enterprise-api-key>
Content-Type: application/json
```

```json
{
  "model": "enterprise-reasoner",
  "messages": [
    {
      "role": "user",
      "content": "Hello"
    }
  ]
}
```

The selected model must support the image input, tools, structured output, and translated `reasoning_effort`
preferences required by connected Agents.
The service must return an OpenAI-compatible Chat Completions response.
It must support streaming when the originating Agent requests streaming.

## Enterprise gateway requirements

| Requirement | Required behavior |
|---|---|
| Transport | Use HTTPS for a remote endpoint. |
| Authentication | Accept `Authorization: Bearer <key>` for verification and runtime requests. |
| Verification | Return HTTP 200 and a JSON `data` array from `<base-url>/models`. |
| Runtime API | Accept POST requests at `<base-url>/chat/completions`. |
| Selected model | Accept the exact model ID returned by `/models` and selected by the user. |
| Streaming | Return OpenAI-compatible SSE when `stream: true`. |
| Tools and structured output | Support the request fields required by the connected Agents. |
| Images and reasoning | The selected model must support image input or `reasoning_effort` when connected Agents send them. |
| Errors | Return OpenAI-compatible status codes and error bodies. |

If the enterprise service removes or renames the selected model, the user must update the Token-station provider configuration.

## Local state and security boundary

The default credential source stores the API key in the local `secrets.json` file.
The file uses mode `0600` on supported Unix systems.
The value is plaintext on disk and is not stored in the operating system Keychain.
Other processes that run as the same operating system user can read it.

The Base URL parser rejects embedded credentials, query strings, fragments, ambiguous separators, and path traversal forms.
Remote Provider endpoints require HTTPS.
Token Station injects the Bearer credential only after it authorizes the configured origin and path boundary.

The enterprise service receives all requests routed to it.
Its data retention, logging, training, and compliance policies remain outside Token Station's control.
Token Station can also store request bodies locally when desktop request-body logging is enabled.

## Failure behavior

| Failure | Result |
|---|---|
| A required field is empty | No network request or configuration change occurs. |
| A provider named `Token-station` already exists | No network request or configuration change occurs. Manage or remove the existing provider first. |
| The Base URL is invalid | No Provider is created. |
| `/models` returns 401 or 403 | The credential is rejected. |
| `/models` returns 404 | The endpoint does not satisfy the current enterprise connection contract. |
| `/models` returns invalid JSON | Verification fails. |
| Only a cached result is available | Verification fails and no Provider is created. |
| Route apply fails after Provider creation | Token Station reloads authoritative state and keeps the valid draft available. Retry the apply operation in global routing. |

## Current limitations

- Enterprise routing uses the OpenAI-compatible Chat Completions southbound API.
- Token Station does not test a real Completion during the connection action.
- Token Station displays model IDs only; it does not import prices, quotas, or policy details.
- Editing or replacing the existing `Token-station` provider uses the normal Provider management flow.

---

# 企业托管路由

本文定义企业托管路由的桌面端契约，以及企业网关必须实现的最低接口契约。

## 目的与职责边界

企业托管路由使用企业服务暴露的模型。
Token Station 会实时验证模型列表，并要求用户在接入时明确选择一个模型。

```text
Agent
  -> Token Station 本地网关
  -> 企业 OpenAI 兼容端点
  -> 用户选定的企业模型
```

Token Station 负责本地 Agent 接入、协议归一化、凭据注入和路由启用。
企业服务负责可用模型、额度、回退和策略变更。

## 接入字段

“模型”页的企业路由弹窗接收以下字段：

| 字段 | 是否必填 | 含义 |
|---|---:|---|
| Base URL | 是 | OpenAI 兼容 API 根地址，例如 `https://router.example.com/v1`。 |
| API Key | 是 | 企业服务使用的 Bearer 凭据。 |
| 模型 | 是 | 实时验证端点后返回的一个模型。 |

本地供应商名固定为 `Token-station`。

请填写 API 根地址。
Base URL 中不要包含凭据、查询参数或片段。
只有 Origin 的 URL 默认使用 `/v1`。
以 `/models`、`/chat/completions`、`/responses` 或 `/messages` 结尾的 URL 会被归一化为 API 根地址。

## 接入与启用流程

接入操作会执行以下流程：

1. 校验 Base URL 和 API Key。
2. 实时验证端点和凭据。
3. 展示返回的模型 ID，并要求明确选择一个。
4. 拒绝已存在的 `Token-station` 供应商。
5. 在一次后端草稿变更中保存选定模型，把全局路由设为单独路由，并设置直连目标。
6. 保存并应用配置。

应用成功后，继承全局路由的 Agent 会立即使用企业路由。
已有的 Agent 独立路由不会被修改。

## 验证请求

Token Station 使用以下请求验证端点：

```http
GET /v1/models HTTP/1.1
Authorization: Bearer <enterprise-api-key>
Accept: application/json
User-Agent: token-station-desktop/model-discovery
```

实际路径根据配置的 API 根地址生成。
例如，`https://router.example.com/api/v4` 会生成 `/api/v4/models`。

响应必须使用 OpenAI 兼容的 `data` 数组结构，并至少包含一个模型 ID。

```json
{
  "object": "list",
  "data": [
    { "id": "enterprise-reasoner", "object": "model" }
  ]
}
```

请求的全局超时为六秒。
响应正文上限为 2 MiB。
Token Station 不跟随重定向。
缓存的模型响应不能作为凭据验证结果。

弹窗会展示接口返回的模型 ID 供用户选择。Token Station 只把选定模型保存到
`Token-station` 供应商；验证过程不会写入通用模型目录缓存。

## 托管供应商标识

Token Station 保存选定模型，并使用 `managed_route: true` 标记该供应商：

```yaml
models:
  - model: enterprise-reasoner
managed_route: true
```

桌面端据此在重启后恢复企业路由的已接入状态，并与普通供应商区分。

运行时，OpenAI 兼容 Provider 适配器会在请求正文中发送选定模型：

```http
POST /v1/chat/completions HTTP/1.1
Authorization: Bearer <enterprise-api-key>
Content-Type: application/json
```

```json
{
  "model": "enterprise-reasoner",
  "messages": [
    {
      "role": "user",
      "content": "Hello"
    }
  ]
}
```

选定模型必须支持已接入 Agent 需要的图像输入、工具、结构化输出和翻译后
`reasoning_effort` 偏好。
企业服务必须返回 OpenAI 兼容的 Chat Completions 响应。
当来源 Agent 请求流式输出时，企业服务必须支持流式响应。

## 企业网关要求

| 要求 | 必须实现的行为 |
|---|---|
| 传输 | 远程端点必须使用 HTTPS。 |
| 鉴权 | 验证请求和运行时请求必须接受 `Authorization: Bearer <key>`。 |
| 验证 | `<base-url>/models` 必须返回 HTTP 200 和 JSON `data` 数组。 |
| 运行时 API | 必须接受发送到 `<base-url>/chat/completions` 的 POST 请求。 |
| 选定模型 | 必须接受 `/models` 返回且用户选定的准确模型 ID。 |
| 流式响应 | `stream: true` 时必须返回 OpenAI 兼容 SSE。 |
| 工具与结构化输出 | 必须支持已接入 Agent 所需的请求字段。 |
| 图像与推理 | 已接入 Agent 发送图像输入或 `reasoning_effort` 时，选定模型必须支持这些能力。 |
| 错误 | 必须返回 OpenAI 兼容的状态码和错误正文。 |

如果企业服务删除或重命名选定模型，用户必须更新 `Token-station` 供应商配置。

## 本地状态与安全边界

默认凭据来源会把 API Key 保存到本地 `secrets.json` 文件。
在受支持的 Unix 系统中，该文件权限为 `0600`。
凭据以明文形式保存在磁盘上，不使用操作系统 Keychain。
以同一个操作系统用户运行的其他进程仍可读取该文件。

Base URL 解析器会拒绝内嵌凭据、查询参数、片段、歧义分隔符和路径穿越形式。
远程 Provider 端点必须使用 HTTPS。
Token Station 仅在确认配置的 Origin 和路径边界后注入 Bearer 凭据。

企业服务会收到所有路由给它的请求。
企业服务的数据保留、日志、训练和合规政策不受 Token Station 控制。
如果启用了桌面请求正文日志，Token Station 也可能在本地保存请求正文。

## 失败行为

| 失败情况 | 结果 |
|---|---|
| 必填字段为空 | 不发起网络请求，也不修改配置。 |
| 已存在名为 `Token-station` 的供应商 | 不发起网络请求，也不修改配置。请先管理或删除现有供应商。 |
| Base URL 不合法 | 不创建 Provider。 |
| `/models` 返回 401 或 403 | 凭据验证失败。 |
| `/models` 返回 404 | 端点不符合当前企业接入契约。 |
| `/models` 返回无效 JSON | 验证失败。 |
| 只能获取缓存结果 | 验证失败，并且不创建 Provider。 |
| 创建 Provider 后应用路由失败 | Token Station 会重新加载权威状态并保留有效草稿。请在全局路由中重试应用。 |

## 当前限制

- 企业路由的出站协议固定为 OpenAI 兼容 Chat Completions。
- 接入操作不会发送一次真实 Completion 测试。
- Token Station 只显示模型 ID，不导入价格、额度或策略详情。
- 编辑或替换已有 `Token-station` 供应商需要使用普通 Provider 管理流程。
