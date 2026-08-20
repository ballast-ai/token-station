# Enterprise Managed Routing

This guide defines the desktop contract for an enterprise-managed route.
It also defines the minimum contract that an enterprise gateway must implement.

## Purpose and ownership

An enterprise-managed route delegates model selection and routing policy to an enterprise service.
Token Station does not import, select, or maintain the service's real model catalog.

```text
Agent
  -> Token Station local gateway
  -> Enterprise OpenAI-compatible endpoint
  -> Enterprise model and policy selection
  -> Selected model
```

Token Station owns the local Agent connection, protocol normalization, credential injection, and route activation.
The enterprise service owns the available models, selection rules, quotas, fallback, and policy changes.

## Connection fields

The enterprise routing page accepts these fields:

| Field | Required | Meaning |
|---|---:|---|
| Base URL | Yes | The OpenAI-compatible API root, such as `https://router.example.com/v1`. |
| API Key | Yes | The Bearer credential for the enterprise service. |
| Account name | No | A stable local name for this enterprise route. Token Station derives a name when this field is empty. |

Enter the API root. Do not include a credential, query string, or fragment in the Base URL.
An origin-only URL defaults to `/v1`.
A URL that ends in `/models`, `/chat/completions`, `/responses`, or `/messages` is normalized to its API root.

## Connection and activation flow

One **Connect and use** action runs this sequence:

1. Validate the required fields and the optional account name.
2. Reject an explicit account name that already exists.
3. Perform live endpoint and credential verification.
4. Store the endpoint with the managed route alias `auto`.
5. Set global routing to Direct.
6. Set the direct target to the new enterprise account and `auto`.
7. Save and apply the configuration.

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

The response must use the OpenAI-compatible `data` array shape.
An empty array is valid because Token Station does not use this catalog for enterprise routing.

```json
{
  "object": "list",
  "data": []
}
```

This request has a six-second global timeout.
The response body limit is 2 MiB.
Redirects are not followed.
A cached model response does not count as credential verification.

The returned model IDs are discarded.
They are not shown for selection and are not saved as enterprise models.

## Managed route alias

`auto` is a managed route alias.
It is not a model ID and does not identify a real model.

Token Station stores only this alias for the enterprise account:

```yaml
models:
  - model: auto
```

At runtime, the OpenAI-compatible provider adapter sends the alias in the request body:

```http
POST /v1/chat/completions HTTP/1.1
Authorization: Bearer <enterprise-api-key>
Content-Type: application/json
```

```json
{
  "model": "auto",
  "messages": [
    {
      "role": "user",
      "content": "Hello"
    }
  ]
}
```

The enterprise service must resolve `auto` to a real model and policy.
The service must return an OpenAI-compatible Chat Completions response.
It must support streaming when the originating Agent requests streaming.

## Enterprise gateway requirements

| Requirement | Required behavior |
|---|---|
| Transport | Use HTTPS for a remote endpoint. |
| Authentication | Accept `Authorization: Bearer <key>` for verification and runtime requests. |
| Verification | Return HTTP 200 and a JSON `data` array from `<base-url>/models`. |
| Runtime API | Accept POST requests at `<base-url>/chat/completions`. |
| Managed alias | Accept `model: "auto"` and select the real model on the server. |
| Streaming | Return OpenAI-compatible SSE when `stream: true`. |
| Tools and structured output | Support the request fields required by the connected Agents. |
| Errors | Return OpenAI-compatible status codes and error bodies. |

The enterprise service can change its real models or policies without a desktop configuration change.
Keep the Base URL, credential, and `auto` alias stable across these server-side changes.

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
| The explicit account name already exists | No network request or configuration change occurs. |
| The Base URL is invalid | No Provider is created. |
| `/models` returns 401 or 403 | The credential is rejected. |
| `/models` returns 404 | The endpoint does not satisfy the current enterprise connection contract. |
| `/models` returns invalid JSON | Verification fails. |
| Only a cached result is available | Verification fails and no Provider is created. |
| Route apply fails after Provider creation | The valid draft remains available. Retry the apply operation in global routing. |

## Current limitations

- The managed route alias is fixed to `auto`.
- Enterprise routing uses the OpenAI-compatible Chat Completions southbound API.
- Token Station does not test a real Completion during the connection action.
- Token Station does not display the enterprise service's models, prices, quotas, or policy details.
- Editing or replacing an existing enterprise account uses the normal Provider management flow.

---

# 企业托管路由

本文定义企业托管路由的桌面端契约，以及企业网关必须实现的最低接口契约。

## 目的与职责边界

企业托管路由把模型选择和路由策略交给企业服务处理。
Token Station 不导入、不选择，也不维护企业服务的真实模型目录。

```text
Agent
  -> Token Station 本地网关
  -> 企业 OpenAI 兼容端点
  -> 企业侧模型与策略选择
  -> 实际模型
```

Token Station 负责本地 Agent 接入、协议归一化、凭据注入和路由启用。
企业服务负责可用模型、选择规则、额度、回退和策略变更。

## 接入字段

企业路由页面接收以下字段：

| 字段 | 是否必填 | 含义 |
|---|---:|---|
| Base URL | 是 | OpenAI 兼容 API 根地址，例如 `https://router.example.com/v1`。 |
| API Key | 是 | 企业服务使用的 Bearer 凭据。 |
| 账户名称 | 否 | 企业路由在本地使用的稳定名称。留空时由 Token Station 自动生成。 |

请填写 API 根地址。
Base URL 中不要包含凭据、查询参数或片段。
只有 Origin 的 URL 默认使用 `/v1`。
以 `/models`、`/chat/completions`、`/responses` 或 `/messages` 结尾的 URL 会被归一化为 API 根地址。

## 接入与启用流程

一次“接入并使用”操作会执行以下流程：

1. 校验必填字段和可选账户名称。
2. 拒绝已存在的显式账户名称。
3. 实时验证端点和凭据。
4. 使用托管路由别名 `auto` 保存端点。
5. 把全局路由设置为单独路由。
6. 把直连目标设置为新企业账户和 `auto`。
7. 保存并应用配置。

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

响应必须使用 OpenAI 兼容的 `data` 数组结构。
空数组是有效响应，因为 Token Station 不使用该目录进行企业路由。

```json
{
  "object": "list",
  "data": []
}
```

请求的全局超时为六秒。
响应正文上限为 2 MiB。
Token Station 不跟随重定向。
缓存的模型响应不能作为凭据验证结果。

接口返回的模型 ID 会被丢弃。
Token Station 不显示这些模型，也不会把它们保存为企业模型。

## 托管路由别名

`auto` 是托管路由别名。
它不是模型 ID，也不指向某个真实模型。

Token Station 只为企业账户保存这个别名：

```yaml
models:
  - model: auto
```

运行时，OpenAI 兼容 Provider 适配器会在请求正文中发送这个别名：

```http
POST /v1/chat/completions HTTP/1.1
Authorization: Bearer <enterprise-api-key>
Content-Type: application/json
```

```json
{
  "model": "auto",
  "messages": [
    {
      "role": "user",
      "content": "Hello"
    }
  ]
}
```

企业服务必须把 `auto` 解析为真实模型和策略。
企业服务必须返回 OpenAI 兼容的 Chat Completions 响应。
当来源 Agent 请求流式输出时，企业服务必须支持流式响应。

## 企业网关要求

| 要求 | 必须实现的行为 |
|---|---|
| 传输 | 远程端点必须使用 HTTPS。 |
| 鉴权 | 验证请求和运行时请求必须接受 `Authorization: Bearer <key>`。 |
| 验证 | `<base-url>/models` 必须返回 HTTP 200 和 JSON `data` 数组。 |
| 运行时 API | 必须接受发送到 `<base-url>/chat/completions` 的 POST 请求。 |
| 托管别名 | 必须接受 `model: "auto"`，并在服务端选择真实模型。 |
| 流式响应 | `stream: true` 时必须返回 OpenAI 兼容 SSE。 |
| 工具与结构化输出 | 必须支持已接入 Agent 所需的请求字段。 |
| 错误 | 必须返回 OpenAI 兼容的状态码和错误正文。 |

企业服务可以在不修改桌面配置的情况下变更真实模型或策略。
这些服务端变更应保持 Base URL、凭据和 `auto` 别名稳定。

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
| 显式账户名称已存在 | 不发起网络请求，也不修改配置。 |
| Base URL 不合法 | 不创建 Provider。 |
| `/models` 返回 401 或 403 | 凭据验证失败。 |
| `/models` 返回 404 | 端点不符合当前企业接入契约。 |
| `/models` 返回无效 JSON | 验证失败。 |
| 只能获取缓存结果 | 验证失败，并且不创建 Provider。 |
| 创建 Provider 后应用路由失败 | 有效草稿会保留。请在全局路由中重试应用。 |

## 当前限制

- 托管路由别名固定为 `auto`。
- 企业路由的出站协议固定为 OpenAI 兼容 Chat Completions。
- 接入操作不会发送一次真实 Completion 测试。
- Token Station 不显示企业服务的模型、价格、额度或策略详情。
- 编辑或替换已有企业账户需要使用普通 Provider 管理流程。
