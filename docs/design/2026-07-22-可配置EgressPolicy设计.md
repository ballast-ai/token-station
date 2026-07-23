# 可配置 EgressPolicy 设计

> 日期：2026-07-22
> 对应任务：第二批协作任务 #5
> 状态：本地实现与自动化验收通过

## 1. 背景

Token Station 原有 HTTP 客户端已经显式设置 `max_redirects(0)` 和 `proxy(None)`，能够拒绝环境代理污染与自动重定向，但企业网络用户无法选择 HTTP/SOCKS 代理、配置 `no_proxy` 或解释不同请求的实际出口。

本设计在既有 fail-closed 基线上加入一套共享出站策略，不改变“3xx 不自动跟随”和“跨主机不复制原 Authorization”的安全红线。

## 2. 目标

1. 支持 `direct`、HTTP/HTTPS CONNECT、SOCKS5/SOCKS5h 三类显式出口。
2. 支持精确主机、`.suffix`、`*.suffix` 与 `*` 形式的 `no_proxy`。
3. 代理凭据只引用 credential slot，不进入 URL、公开配置视图或日志。
4. Provider 生成请求、模型目录和健康探测共用同一策略；更新检查明确固定直连。
5. 控制面能按请求类别和 upstream 展示实际解析后的 direct/proxy/no_proxy 结果。
6. 不读取 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 等环境变量；不自动跟随任何重定向。

## 3. 契约与数据流

`ClientConfig.egress` 是唯一配置事实源：

```json
{
  "egress": {
    "mode": "http",
    "proxy_url": "http://proxy.company:8080",
    "no_proxy": ["localhost", "*.corp.internal"],
    "auth": {
      "username": "employee",
      "credential": { "slot": "corporate_proxy_password", "keyring": true }
    }
  }
}
```

解析顺序如下：

1. 校验 mode 与 scheme 一致，拒绝 URL userinfo、路径、查询、片段和非法 `no_proxy`。
2. 请求时从 `egress-proxy/<slot>` 解析密码；公共配置始终只包含 slot。
3. 为请求创建显式 ureq agent：direct 使用 `proxy(None)`，代理模式使用构造后的 `Proxy`，两者均设置 `max_redirects(0)`。
4. 数据面与 Desktop 模型目录使用相同 `EgressConfig` 和 `SecretStore`。
5. `/admin/egress` 从运行中 Gateway 返回解析结果；Desktop 未运行 Gateway 时，用相同 matcher 对草稿配置做预览。

出口类别：

| 请求类别 | 策略 |
|---|---|
| `provider_request` | 按 upstream 目标 URL 解析 EgressPolicy |
| `model_catalog` | 按 upstream 目标 URL 解析 EgressPolicy |
| `health_probe` | 按 upstream 目标 URL 解析 EgressPolicy |
| `update_check` | 固定 direct，显式禁用环境代理与重定向 |

## 4. 安全边界

- proxy URL 禁止内嵌用户名或密码；认证必须引用 slot。
- `no_proxy` 由配置校验器和实际 ureq matcher 共同解释，UI 展示的是 matcher 的实际判定，不自行复制一套近似规则。
- 所有 HTTP 客户端均设置 `max_redirects(0)`。因此每个 3xx 都在当前 hop 停止；不存在把原请求头隐式带到下一主机的路径。
- 后续若产品允许显式继续某个 Location，必须重新经过目标 URL 校验并新建请求，禁止复制原 Authorization；本阶段不开放该能力。
- 更新检查固定直连，避免软件更新信任路径被业务代理配置隐式改变。
- `/admin/egress` 受现有虚拟 Key 与 loopback CORS 控制，代理密码不进入响应。

## 5. 执行步骤

1. 扩展 CLI 配置与 secret store，建立严格校验和 slot 解析。
2. 把 Gateway 的正常生成、分层探测、目录和协议能力测试接到共享 EgressPolicy。
3. 为更新检查建立显式 direct client。
4. 把 Desktop 模型目录接到同一配置和 credential store。
5. 新增 `/admin/egress` 与 Desktop IPC fallback，展示运行态/草稿态出口。
6. 在设置页加入模式、URL、`no_proxy`、用户名和认证槽；提供 keyring 写入命令，不读取密码。
7. 用真实 TCP CONNECT、环境变量污染、matcher、认证槽和恶意重定向测试锁定契约。

## 6. 退出条件

- 成功：三种模式均可构造；真实 HTTP CONNECT 请求通过；`no_proxy` 和认证槽生效；出口可解释；重定向、环境变量和跨主机鉴权红线全绿。
- 阻塞：目标平台 ureq 构建不支持所需 SOCKS feature，或无法在不泄露凭据的前提下复用 credential store。
- 失败：需要依赖系统环境代理、自动跟随重定向，或必须把代理密码写入公开配置才能工作。

## 7. 通过条件

| 条件 | 判定 |
|---|---|
| direct / HTTP / SOCKS 可配置 | 通过 |
| `no_proxy` 使用数据面实际 matcher | 通过 |
| HTTP 代理真实承接请求且目标域名无需本机 DNS | 通过 |
| 代理认证仅通过 slot 解析 | 通过 |
| 每类请求出口可见 | 通过 |
| 环境代理不能旁路显式配置 | 通过 |
| 3xx 不自动跟随，跨主机不转发原 Authorization | 通过 |
| CLI、Desktop、前端和 Clippy 门禁 | 通过 |

## 8. 交付产物

- CLI：`config.rs`、`secrets.rs`、`gateway.rs`、`server.rs`、`upgrade.rs` 与集成测试。
- Desktop 后端：Egress-aware 模型目录、设置持久化和运行态出口 IPC。
- Desktop 前端：出站配置、认证槽说明和实际出口展示。
- 依赖：ureq 3.3.0 的 `socks-proxy` feature 与锁文件。
- 验收记录：`docs/verification/2026-07-22-第二批协作任务-T5EgressPolicy验收.md`。

## 9. 依据

- ureq 3.3.0 `Proxy` 官方 API：<https://docs.rs/ureq/3.3.0/ureq/struct.Proxy.html>
- ureq 3.3.0 crate source：`proxy.rs` 中定义 HTTP/HTTPS CONNECT、SOCKS4/4a/5/5h、`no_proxy` 和显式配置覆盖环境变量的行为。
