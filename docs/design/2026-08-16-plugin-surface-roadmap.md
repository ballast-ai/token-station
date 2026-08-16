# Token Station 插件扩展路线图（V2）

> 目标仓库：`GlimpseEngine/token-station`（Apache-2.0）
>
> 文档定位：跨阶段技术路线图；每一阶段进入实现前，必须在 `docs/design/` 下拆出独立设计文档
>
> 状态：路线图已完成代码事实核对，尚未批准任何阶段进入实现
>
> 更新日期：2026-08-16

---

## 0. 背景与总体判断

当前插件面（`crates/plugin-api/wit/adapter.wit`）有两个 world：

- `agent-adapter-v1`：9 个导出函数；
- `provider-adapter-v1`：7 个导出函数；
- 合计 16 个导出函数，职责都是协议方言翻译。

现有安全边界清晰且必须保留：

- 两个 world 都不 import `wasi:filesystem` / `wasi:sockets`；
- 编译产物还会被 `FORBIDDEN_EVERYWHERE = ["wasi:sockets/", "wasi:http/"]` 二次检查；
- 只有 provider world import `host`，且只暴露 `sign(secret-ref, payload, algorithm)`；
- `SafeHeaders` 拒绝插件直接写入凭证头；
- `ProviderConfig::authorize` 在解析凭证前，先拒绝越出 `base_url` 或引用错误凭证 slot 的 descriptor；
- provider 请求不跟随重定向，避免凭证被带到未经授权的新 origin。

问题不在已有边界失效，而在当前插件契约只能表达对话协议翻译。最明确的缺口是：

| 场景 | 当前为什么做不了 |
|---|---|
| 接 AWS Bedrock、腾讯云、阿里云等请求签名上游 | 当前 host 只有单值凭证注入；没有类型化 credential bundle，也没有 host-owned canonical request signer |
| 分发第三方 adapter | 只有 `plugin install <本地目录>`，没有远程 artifact 解析、下载与更新协议 |
| 扩展计费、审计与指标导出 | 没有决策下游的只读扩展面，也没有跨进程可靠投递契约 |
| 路由 embedding / rerank / ASR / TTS | Canonical IR、router、provider world 和入站协议都围绕 `ChatRequest` 建模 |

这四项不能直接并列开工。版本协商、包来源和信任状态是共同前置，因此本路线图先增加 **F0 契约基础阶段**，再推进功能阶段。

---

## 1. 目标、成功指标与非目标

### 1.1 目标

1. 第三方 provider adapter 能接入 host 已实现的云签名方案，插件始终读不到凭证。
2. 第三方插件能从远程预构建 artifact 安装和更新；来源、实际字节、行为检查和发布者身份分别记录。
3. 提供位于路由与响应决策下游的 observer 扩展面；observer 失败永不改变请求结果。
4. 以 Chat + Embedding 的封闭 v2 envelope 跑通首个非 Chat 任务；流式能力通过独立 world 表达，不做“万能 modality”抽象。
5. 保持现有五个官方 adapter 的原始 manifest/WASM 在新 host 上零改动运行。

### 1.2 阶段成功指标

| 阶段 | 可验证结果 |
|---|---|
| F0 | 新 host 可加载全部旧 manifest；不兼容的新插件在安装期而非请求期被具名拒绝 |
| P1 | 至少一个第三方 adapter 在不修改 Token Station 源码的前提下接入一个云签名上游，并覆盖临时凭证 |
| P2 | 远程安装、失败清理、原子更新回滚和来源回执都有端到端测试 |
| P3-A | observer 可导出 content-free 指标，且 trap、超时、队列满均不改变请求结果 |
| P3-B | 只有 durable outbox、幂等与重投全部成立后，才允许宣称支持权威计费 |
| P4 | OpenAI-compatible Embedding 从入站到上游、响应、usage 和 receipt 全链路可用 |

### 1.3 非目标

- 不引入 Hermes 式 `pre_llm_call` / `post_tool_call` 生命周期 hook。
- 不放开 `SafeHeaders` 对凭证头的拒绝。
- 不允许插件直接访问网络、文件系统或明文凭证。
- 不从远程仓库执行 `build.rs`、shell、安装脚本或源码构建。
- 不在本路线图内落地 `router-policy-v1`。
- 不原地修改 `agent-adapter-v1` / `provider-adapter-v1` 的 WIT 定义。
- 不在完成 durable outbox 前把 observer 描述成可靠计费系统。
- 不一次性实现所有非 Chat modality。

---

## 2. 信任角色与四类边界

### 2.1 信任角色

| 角色 | 信任假设 | 能做什么 | 不能靠什么保护 |
|---|---|---|---|
| 运营者配置 | 可信控制面输入 | 配置 `base_url`、凭证来源、签名 profile、observer endpoint | 如果攻击者已能改运营者配置，正则或枚举无法恢复该信任边界 |
| 插件包 | 默认不可信 | 翻译请求、返回受约束 descriptor、处理 content-free observer 输入 | conformance 不能证明作者善意 |
| 发布者 | 默认身份未验证 | 发布 artifact 和可选签名 | 仓库名、tag 或 manifest 自报来源不能证明身份 |
| host | 可信计算基 | 授权目的地、解析凭证、签名、发送、持久化与代发 | 必须避免把安全决策重新委托给插件 |

### 2.2 四类边界必须分开描述

| 边界 | 负责回答的问题 | 主要机制 |
|---|---|---|
| 目的地授权 | 请求可以发到哪里？ | `ProviderEndpoint` + `ProviderConfig::authorize` |
| 签名作用域正确性 | 这次请求应以什么 profile、region、service 签名？ | host-side `CredentialProfile` + signer 校验 |
| 数据外发边界 | observer 能看到并导出哪些字段？ | `ObserverReceiptV1` 闭集 schema + host 代发 |
| 版本兼容边界 | 哪个 host 能加载哪个 manifest/world/JSON 契约？ | manifest schema version + `api_version` + `min_host_version` |

`[a-z0-9-]{1,64}` 之类正则只负责语法和“不能误贴凭证”，不负责目的地授权，也不能证明 region/service 的语义正确。

---

## 3. 阶段、依赖与规模

| 阶段 | 内容 | 破坏性 | 新 world | 粗略规模 |
|---|---|---|---|---|
| **F0** | manifest schema、host 兼容门、来源回执模型 | 新 manifest 对旧 host 不兼容 | 否 | 中 |
| **P1** | host-side 类型化云签名 | 仅新配置能力；旧插件保持 | 否 | 中 |
| **P2** | 远程 artifact 安装、来源校验与原子更新 | 无运行时破坏 | 否 | 中至大 |
| **P3-A** | decision-read-only observer，best-effort 导出 | 新 adapter kind | 是 | 中 |
| **P3-B** | durable outbox、幂等、重投与计费语义 | 新持久化状态 | 否 | 大 |
| **P4.0** | 已选 v2 IR/world 结构的可行性 spike | 无产品行为 | 仅原型 | 中 |
| **P4.1** | OpenAI-compatible Embedding 纵切 | 新协议与 UI 能力 | 是 | 特大 |
| **Deferred** | `router-policy-v1` | 路由语义变化 | 是 | 中，但风险高 |

依赖关系：

```text
                       ┌──> P1（云签名）
F0（契约与兼容基础）───┼──> P2（远程安装）──> P3-A（Observer）──> P3-B（权威计费）
                       └──> P4.0（架构可行性）──> P4.1（Embedding 纵切）
```

F0 完成后，P1、P2、P4.0 可以并行。P1 与 P2 可以进入同一发布周期，但不应各自发明一次 manifest bump；它们共享 F0 定义的一次兼容迁移。P3-A 技术上只依赖 F0，但从生态价值考虑，应在 P2 之后发布。

### 3.1 资源估算口径

以下是用于排期的**净工程日区间**，不是承诺日期。口径为一名熟悉 Rust/WIT 和本仓库的工程师，包含阶段设计、公开行为测试、实现、文档、全量验证和适用时的本地 Desktop 验收；不包含外部账号审批、第三方安全审计、CI 排队和发布等待。多人并行只能缩短部分日历时间，不能把依赖链上的工程日直接相除。

| 阶段 | 净工程日 | 置信度 | 最大不确定性 |
|---|---:|---|---|
| F0 | 4–6 | 中 | legacy/v2 双解析与错误分类 |
| P1（AWS Bedrock） | 8–12 | 中 | 精确签名字节、临时凭证和真实账号验收 |
| P2 | 12–18 | 中低 | 三平台原子替换、归档攻击面和 worker 回收 |
| P3-A | 8–12 | 中 | observer schema、队列与代发边界 |
| P3-B | 15–25 | 低 | outbox 事务、幂等、重投和运维可见性 |
| P4.0 | 4–6 | 中 | WIT/bindgen 代码共享与双 world dispatch |
| P4.1a | 8–12 | 中 | Embedding protocol、入站和单 provider 纵切 |
| P4.1b | 15–22 | 低 | v1/v2 runtime、conformance 和多任务 adapter 并存 |
| P4.1c | 8–12 | 中低 | receipt、配置、CLI/Desktop 和发布验收 |

P4.1 合计 31–46 工程日，连同 P4.0 为 35–52 工程日。它大于任一前序单阶段，并与 P1 + P2 + P3-A 的 28–42 工程日处在同一量级；不能再用一个“特大”掩盖。P4.0 结束后必须重估 P4.1；若新区间相对当前上下界偏差超过 30%，重新做资源决策，不把偏差吞进实施期。

---

## 4. F0：manifest、ABI 与来源回执基础

### 4.1 当前事实

`AdapterManifest` 当前没有独立的 `manifest_version`。`api_version` 直接等于 WIT world 名，例如 `provider-adapter-v1`；`deny_unknown_fields` 会让旧 host 拒绝任何携带新字段的 manifest。

因此，“给 manifest 加字段并 version bump”目前不是一个已经存在的操作。F0 必须先定义机制，避免 P1/P2/P3 各自形成不兼容中间态。

### 4.2 方案

1. 新增显式 `manifest_version`：
   - 旧 manifest 缺失该字段时，在新 host 中按 legacy v1 解析；
   - 只有 v2 manifest 可以携带 `min_host_version` 和后续新字段；
   - v1 manifest 携带 v2 字段时拒绝，不能靠默认值静默接受；
   - 旧 host 拒绝 v2 manifest 是正确失败方向。
2. `api_version` 继续只表示 WIT world，不与 manifest schema 混为一谈。
3. 安装时先检查 manifest schema 和 `min_host_version`，再加载或执行 WASM，使不兼容在安装期失败。
4. `source` 不进入包内 manifest。来源 URL、GitHub release/tag/commit、下载时间和下载摘要由 host 从实际下载过程写进 receipt，插件不能自报来源。
5. receipt 中分开记录：
   - `package_digest`：当前递归包摘要，绑定 manifest、WASM、fixtures 等已安装字节；
   - `archive_sha256`：下载 artifact 的原始摘要；
   - `source`：host 实际解析到的来源；
   - `conformance`：行为检查结果；
   - `publisher_signature`：`verified` / `unverified`，不得由 conformance 推导。

### 4.3 验收标准

1. 当前五个官方 manifest 不修改即可被新 host 加载。
2. v2 manifest 在低于 `min_host_version` 的 host 上于安装期具名失败。
3. v1 manifest 携带 v2-only 字段时失败，而不是静默忽略。
4. 包内伪造的 repository/commit 不会进入 host receipt；receipt 只使用下载器观测值。
5. `api_version`、manifest schema 和 host version 的错误分别有独立错误码或稳定错误分类。

---

## 5. P1：host-side 类型化云签名

### 5.1 问题

当前 `ProviderConfig` 只有单一 `Option<SecretRef>`，`Auth` 只有 Bearer、Header、OAuth。云签名通常需要“公开标识 + 私密签名材料”，临时凭证还可能需要 session token；同时需要 host-owned 时间戳、nonce、内容摘要和 canonical request。

把这些信息压进单个 `SecretRef`，或者让插件返回 `region/service`，都会把配置归属和签名责任混淆。

### 5.2 方案

首个纵切固定为 **AWS Bedrock Runtime + SigV4**，只覆盖官方公开 `bedrock-runtime.{region}.amazonaws.com` endpoint 上的 OpenAI-compatible Chat Completions。选择它是工程边界决策，不是市场优先级判断：SigV4 同时覆盖 access key id、secret access key、可选 session token、region、service、时间和精确请求字节，能先验证最难的凭证形状。

签名方案由运营者在 host 配置中选择。`HostCredentialProfile` 是 CLI/gateway 的 host-only 类型，不加入会传给 WASM 的 `protocol::ProviderConfig`。host 为 adapter 构造现有 plugin-visible config view：AWS profile 对应的 `ProviderConfig.auth` 为 `None`，不会把多组 secret ref、region 或签名策略序列化给插件。插件仍使用 `provider-adapter-v1` 构造 `auth = None`、无凭证头的请求 descriptor；v2 manifest 的 `min_host_version` 表明它依赖 host-owned signer。WIT 函数签名不变，现有 v1 adapter 不迁移。

示意类型：

```rust
pub enum HostCredentialProfile {
    Bearer {
        secret: SecretRef,
    },
    Header {
        name: CredentialHeaderName,
        secret: SecretRef,
    },
    OAuth {
        grant: SecretRef,
        scopes: Vec<String>,
    },
    AwsBedrockSigV4 {
        access_key_id: SecretRef,
        secret_access_key: SecretRef,
        session_token: Option<SecretRef>,
        region: AwsBedrockRegion,
    },
}
```

`service` 不进入配置，Bedrock signer 内固定为 `bedrock`。`region` 不是自由字符串：host 从受支持的 Bedrock endpoint 解析 region，并与 `AwsBedrockRegion` 的维护列表交叉校验；首版拒绝自定义 endpoint、FIPS、PrivateLink 和无法识别的域名。这样语法合法但语义错误的 region/service 不能进入签名路径，也不能改变目的地。

实现优先使用 AWS 官方 `aws-sigv4` crate 的最小 HTTP signing feature，不引入完整 AWS SDK 配置加载链，也不手写 SigV4。具体版本在 P1 设计文档中锁定，并通过现有 MSRV、license 和供应链 gate；若官方 crate 不能通过这些 gate，P1 暂停重新决策，而不是静默切到自研密码实现。腾讯云 TC3 和阿里云 ACS3 等到 AWS 纵切验收后分别扩展，不能预先塞进一个未经验证的通用枚举。

### 5.3 关键约束

1. `region/service` 只来自 host-side profile，不来自请求、plugin-visible `ProviderConfig` 或插件输出；完整 profile 永不进入 WASM JSON。
2. 语法校验只防止非法值和误贴凭证；目的地仍由 `ProviderConfig::authorize` 单独授权。
3. P1 的 service 固定为 `bedrock`，region 必须同时通过 endpoint 推导和 host 维护的 Bedrock region 集合校验；不提供自由填写 service 的逃生口。
4. 自定义 endpoint、FIPS 和 PrivateLink 不在 P1 范围；后续支持它们时必须先定义可验证的 endpoint → signing scope 映射，不能降级成只跑正则。
5. host 先完成 URL 授权，再解析凭证。
6. 现有 Bearer/Header/OAuth adapter 继续使用 descriptor 中的 `Auth`；cloud-signed profile 要求 descriptor 的 `auth` 为空，由 host 在 URL 授权后按配置完成签名。`authorize` 必须显式区分“无需认证”“descriptor 声明简单认证”“host-owned 请求签名”三种状态。
7. host 把 descriptor 编码为最终发送字节一次，随后补齐 host-owned 时间戳、nonce、token 和摘要；签名与发送必须使用同一份 method、URL、headers 和 body bytes。
8. provider 请求继续禁止自动重定向。
9. `SafeHeaders` 不变；插件 descriptor 中出现凭证头仍然拒绝。
10. `host.sign` ABI 不扩权。它仍只服务“签名可以放普通 header 或 body”的既有场景，不能代替请求级 signer。
11. receipt 只记录签名 profile 类型和经过约束的配置引用，不记录凭证、Authorization、session token 或 canonical request 原文。

### 5.4 兼容策略

P1 不给 `Auth` JSON 增加新 variant，也不开新 world。`HostCredentialProfile::AwsBedrockSigV4` 是可信 host 配置；该 profile 与 descriptor 的 `auth = None` 组合无歧义地表示 host-owned 请求签名。新 adapter 通过 F0 的 v2 manifest 和 `min_host_version` 拒绝不支持该行为的旧 host；旧插件和现有简单认证路径保持原样。

当前 WIT 注释仍写着 AWS SigV4 “waits for `-v2`”，实施 P1 时必须更正这条过期注释，但不得修改 `provider-adapter-v1` 的函数、import/export 或 JSON wire contract。

### 5.5 验收标准

1. AWS Bedrock Runtime OpenAI-compatible Chat Completions 完成真实纵切，覆盖长期凭证和带 session token 的临时凭证。
2. 使用 AWS 公开 SigV4 golden vector 验证 canonical request、string-to-sign、签名和最终 headers，并用受控 AWS 账号执行发布前 smoke test。
3. descriptor 中出现凭证头一律失败；host 授权后的最终请求包含正确凭证头。
4. 传给 adapter 的完整 `ProviderConfig` JSON 不含 AWS access key secret refs、session-token ref、region 或 signer 配置 sentinel。
5. 错误 region/service 只能导致配置拒绝或上游认证失败，不能改变已授权目的地。
6. cloud-signed profile 收到非空 descriptor `auth` 时失败；简单认证 profile 的既有 descriptor 行为不变。
7. 签名、发送和日志测试证明使用同一份请求字节，且日志不含凭证头或签名材料。
8. 当前五个官方 adapter 原始 manifest/WASM 在新 host 上零改动运行并通过原 conformance。
9. OAuth 当前未实现的 501 行为不得被本阶段无意改变。

决策依据：[AWS SigV4 请求签名](https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_sigv-create-signed-request.html)、[Bedrock endpoint 与认证方式](https://docs.aws.amazon.com/bedrock/latest/userguide/endpoints.html)、[`aws-sigv4` HTTP signing API](https://docs.rs/aws-sigv4/latest/aws_sigv4/http_request/)。

---

## 6. P2：远程 artifact 安装与原子更新

### 6.1 问题

当前只有 `plugin install <本地目录>`。远程安装需要解决的不是“下载一个目录”，而是：artifact 如何定位、下载过程如何受限、安装期会执行什么、来源如何记录、更新如何回滚。

### 6.2 用户界面与 artifact 约定

```text
token-station-cli plugin install github:owner/repo[@tag]
token-station-cli plugin install https://example.com/plugin.tar.zst --sha256 <digest>
token-station-cli plugin update <name>
```

- GitHub 来源只解析 release asset，不 clone 仓库、不安装源码、不运行构建。
- 未指定 tag 时解析最新稳定 release，但安装确认和 receipt 必须展示最终 tag、commit、release/asset id 与摘要。
- 任意 HTTPS URL 必须显式提供 `--sha256`。
- GitHub asset 若缺少可验证摘要或发布者签名，也必须以“publisher unverified”展示，不能把仓库名当身份保证。
- 初期只支持公开 artifact；私有仓库认证另行设计，避免仓库 token 与 provider 凭证共用通道。

### 6.3 安装流程

任何一步失败都回收 staging，不改变当前已安装版本和 receipt：

1. 下载到私有临时文件，限制压缩包字节数和下载时长。
2. 下载器复用受控代理配置，但使用独立的 control-plane redirect 策略：每次重定向重新校验 scheme、目标和次数，且不把来源认证头带到新 origin。不能直接照搬 provider 的“完全不跟随重定向”，否则 GitHub release asset 无法工作。
3. 校验 `archive_sha256` 后，解压到与 `plugins.dir` 同文件系统的私有 staging。
4. 解压同时限制文件数、目录深度、单文件大小、总解压大小，并拒绝绝对路径、`..`、符号链接、硬链接、设备文件和其他特殊文件。
5. 只发布 `manifest.json`、`adapter.wasm`、manifest 指定的 `fixtures/` 和可选 `signature.sig`；README/LICENSE 可以存在于下载包中，但不进入运行时包摘要。
6. 检查 F0 manifest/host 兼容门。
7. 对 staging 计算递归 `package_digest`。
8. 父进程启动独立 conformance worker，在子进程内执行不可信 `adapter.wasm`。worker 先重算并核对 `package_digest`，再使用无网络、无文件系统、无 secrets 的 Wasmtime 配置执行；继承现有 64 MiB memory limit、2 秒单调用 timeout 和 epoch interruption，父进程另设整次检查的 wall-clock deadline。
9. conformance 通过后，父进程在发布前再次计算 `package_digest`；与第 7 步不一致时按 staging 被并发修改失败关闭。
10. 使用同文件系统 rename 原子发布；update 不原地覆盖，失败时保留旧目录和旧 receipt。
11. 原子写入包含来源、摘要、conformance 和发布者验证状态的新 receipt。
12. 明确提示运行中的 `serve` 何时生效；第一版沿用“重启后应用”，不偷偷热替换实例。

### 6.4 conformance worker 边界

P2 固定采用同一 CLI 可执行文件的隐藏子命令 `__plugin-conformance-worker`，不新增独立发行物。父子进程只交换有界、版本化 JSON：父进程传 staging 的规范化绝对路径、预期 `package_digest` 和 suite id；worker 重新计算摘要后加载包，并只返回结构化报告。具体约束：

1. 父进程使用 `env_clear()` 启动 worker，只补齐运行所需的最小环境；不传 provider、GitHub 或代理凭证。
2. worker 不读取用户配置和 secret backend，不接收任意 URL，也不执行下载或发布。
3. stdin、stdout、stderr 和报告大小都有上限；非 JSON 输出、超限输出、异常退出、signal/abort、超时和摘要不一致一律视为 conformance 失败。
4. 父进程在 deadline 后终止并回收 worker，再清理 staging；不得留下孤儿进程。
5. 三个平台分别验证进程创建、终止和回收。能用 job object / process group 的平台应防止 worker 派生进程残留，但当前 WASI 配置本身不提供进程创建能力。

这提供的是**崩溃、内存和超时故障隔离**，不是低权限安全边界：worker 仍与 CLI 使用同一 OS 用户，不能抵御 Wasmtime sandbox escape。真正的 OS 权限隔离（独立账户、seatbelt/sandbox-exec、AppContainer、seccomp 等）需要逐平台威胁模型，延期到 P2 后续安全加固；文案不得把本方案称为“低权限子进程”。安装期仍然会执行 WASM。

### 6.5 信任语义

| 状态 | 能证明什么 | 不能证明什么 |
|---|---|---|
| archive/package digest matched | 下载和安装后的字节与被批准字节一致 | 作者是谁、代码是否善意 |
| conformance passed | 当前字节通过有限公开行为测试 | 对所有输入都正确、没有恶意行为 |
| publisher signature verified | 持有受信公钥的一方签过该包 | 被签代码没有缺陷或恶意 |

首版只有内置官方公钥或运营者显式固定的公钥可以产生 `verified`；仓库 owner、用户名或包内自带公钥都不能自动成为信任根。

`plugins.allow_unsigned` 对远程安装无效。该配置只表示运营者主动信任自己放入本地目录的内容，不能扩大到网络下载。

### 6.6 验收标准

1. 安装过程不运行 shell、`build.rs`、安装脚本或源码构建；只执行受限 WASM conformance。
2. conformance 失败、超时或 trap 后，`plugins.dir`、receipt 和旧版本均保持原状。
3. archive 路径穿越、symlink/hardlink、解压炸弹、超限文件、异常重定向和摘要不匹配全部被具名拒绝。
4. `allow_unsigned = true` 时远程包仍强制 conformance。
5. receipt 使用递归 `package_digest`，不得退化成只绑定 `adapter.wasm`。
6. source、conformance、publisher signature 三种状态在 `plugin info/list` 中分开显示。
7. update 新版本失败时旧版本继续服务；成功更新在明确的重启边界后生效。
8. Windows、macOS、Linux 的 staging、rename、权限与失败清理行为都有平台测试或具名平台限制。
9. worker 的环境中不存在父进程凭证 sentinel；异常退出、超限输出、整次超时和摘要 TOCTOU 检查全部失败关闭，且父进程继续可用。

---

## 7. P3：decision-read-only observer

### 7.1 名称与边界

observer 是“对请求决策只读”，不是“无副作用”。它可以要求 host 把 content-free 数据代发到已声明 endpoint，因此：

- observer 不能改变路由、响应、是否重试或请求成功状态；
- observer 没有直接网络、文件系统或 secrets import；
- host 代发是受约束副作用，必须有独立的 endpoint、队列和错误模型；
- `permissions.egress` 表示 mediated egress，不等同于给 WASM 网络权限。

### 7.2 闭集数据契约

当前代码没有 `protocol::Receipt`。P3 必须新建独立、版本化的 `ObserverReceiptV1`，不能直接把内部 `RequestRecord`、管理端 `ReceiptView` 或任意 JSON Value 暴露为 ABI。

`ObserverReceiptV1` 只允许以下类别：

- host 生成的 request id、时间、耗时、状态；
- 闭集协议、path kind、错误码、stream outcome；
- 经过 host 规范化的 agent/model/upstream/pool 配置标识；
- 数值化 `RequestFeatures`、usage、cost、quota 和尝试次数；
- 闭集转换阶段与失败原因。

禁止：

- prompt、response、tool arguments、原始请求路径、原始 headers；
- `ErrorEnvelope.message` 或任意自由文本错误详情；
- generic `extensions` / `Value` / 任意键值容器；
- Authorization、secret ref 的实际值、canonical request 或签名材料。

`RequestFeatures: Copy` 只能保护该嵌套类型，不能证明整个 receipt content-free。每个字符串字段都必须使用受约束 wrapper 或证明来自 host/config 的规范化值。

### 7.3 建议 ABI

```wit
world observer-v1 {
    export observer;
}

interface observer {
    type json = string;

    metadata: func() -> adapter-metadata;
    healthcheck: func() -> adapter-health;

    /// `batch`: `ObserverReceiptBatchV1`。
    /// 返回 `ObserverExportBatchV1`，只引用 manifest 已声明的 endpoint id。
    transform: func(batch: json) -> result<json, json>;
}
```

host 负责读取记录、批处理、调用 observer、校验返回 schema、解析 endpoint id 和实际代发。插件不负责持久化攒批，也不能返回任意 URL。

### 7.4 P3-A：best-effort 可观测导出

1. host 只在请求记录写入完成后，把记录投递到独立有界队列。
2. 请求线程不等待 observer；队列满、observer trap/超时或代发失败只记受限错误和指标。
3. 初期语义明确为 best-effort，不作为权威账单来源。
4. endpoint 以 manifest 中的 id → HTTPS origin 映射声明；host 校验 scheme、origin、DNS/代理策略、响应大小和超时。
5. observer egress 不跟随任意重定向；如确有需要，必须像 P2 下载器一样逐跳重新授权。

### 7.5 P3-B：权威计费前置条件

只有全部满足后才允许宣称支持 PAYG/订阅权威计费：

- host 在本地事务中写入 durable outbox；
- 每条记录有稳定 delivery id；
- 至少一次投递与消费者幂等契约明确；
- cursor/ack 跨重启持久化；
- 重投、乱序、重复、永久失败、磁盘满和关机 flush 有定义；
- 账单与观测记录的权威来源明确，不能由 observer 返回值决定“是否计费”；
- 数据保留、删除和用户可见失败状态有设计。

### 7.6 验收标准

1. 当前五个官方 adapter 的原始 manifest/WASM 在新 host 上零改动加载并通过原 conformance。
2. 未配置 observer 时，路由、response、receipt、插件注册和请求性能无行为变化。
3. observer panic、trap、超时、队列满、返回非法 JSON 或代发失败均不影响在途请求。
4. 用包含 sentinel 的 prompt、response、请求路径、model、header 和错误消息跑真实请求，断言 observer 收到的完整 JSON 不含 sentinel。
5. `ObserverReceiptV1` 没有 generic JSON 容器或自由文本错误字段；schema 变更必须版本化。
6. 未声明 endpoint id、越出声明 origin 或重定向到新 origin 时，host 拒绝代发并具名记录。
7. observer world 的编译产物 import 不含 sockets、HTTP、filesystem、secrets host interface。
8. P3-A 文档和 CLI/UI 明示 best-effort；P3-B 验收前不出现“可靠计费”产品承诺。

---

## 8. P4：非 Chat 推理任务 IR

### 8.1 问题与规模

这不是单一插件改动。`ChatRequest` 当前同时穿过 protocol、router、conformance、plugin runtime、gateway 和五个官方 adapter；代码基线中有 24 个 Rust 文件、108 处 `ChatRequest` 引用，尚未计算 UI、文档和迁移。

因此 P4 不能只标一个“大”。本路线图给出排期区间并固定默认架构；P4.0 负责验证工具链和代码组织是否支持该架构，不再把核心产品边界留到编码中临时决定。

### 8.2 已选架构

选择 **封闭的多任务 v2 envelope + 流式/非流式双 provider world**：

1. `provider-core-v2` interface 处理 metadata、capability、构造请求、解析非流式响应和错误。
2. `provider-stream-v2` interface 只处理流式 chunk。
3. 两个 world 都沿用受限的 provider host import；P4 不新增读取 secret、网络或文件系统的能力。`provider-adapter-v2` 只 export `provider-core-v2`；`provider-streaming-adapter-v2` 同时 export core 和 stream。WIT 不使用“可选函数”，非流式 adapter 不需要实现空 stream 方法。
4. core 使用封闭的 `InferenceRequestV2 = Chat(ChatRequest) | Embedding(EmbeddingRequest)` 和成对的 `InferenceResponseV2`。其中 Chat payload 的现有 wire 保持不变；v2 不接受任意 modality 名或 generic payload。
5. v2 manifest 把 `tasks`（chat/embedding）与 `features`（stream/tool-call/json-schema）分开声明。host 在调用 WASM 前检查 task；adapter 返回未声明 task 的响应时失败。
6. 一个包只声明一个 `api_version`，但可在该 world 内声明多个 task。需要 Chat 流式的多任务 provider 选择 streaming world；Embedding-only provider 选择非流式 world。
7. v1/v2 runtime 和 conformance 并存，不把 v1 adapter 自动包装成 v2，也不要求五个现有 adapter 迁移。首个 OpenAI-compatible v2 adapter 作为新 artifact 发布，原始 v1 artifact 保留。
8. P4.1 的 `/v1/embeddings` 由 host 原生入站协议解析，不新增 `agent-adapter-v2`。只有出现真实的第三方非 Chat agent 协议需求时，才单独设计 agent v2。

manifest 校验还必须拒绝无意义组合：`stream`、`tool-call`、`json-schema` 首版都要求 `tasks` 包含 `chat`；只声明 `embedding` 的包不能借 feature 字段暗示未定义能力。

没有选择“所有任务共用开放 payload”，因为它把类型错误推迟到运行期；也没有选择“一任务一 world”，因为同一 provider 的 Chat + Embedding 会被拆成多个包和 runtime instance。v2 的任务集合故意只含 Chat + Embedding；加入 Rerank/ASR/TTS 时必须通过新 world 版本或新的窄 interface 决策，不能原地扩充 v2 闭集。

### 8.3 P4.0：可行性 spike

生产实现前建立最小、不可发布的 compile-only 原型。它必须证明：

1. 当前 Wasmtime bindgen 能同时生成 v1、v2 non-stream 和 v2 streaming bindings，且模块命名不冲突。
2. 两个 v2 world 能复用同一 core interface 和 Rust dispatch，不复制业务实现。
3. `InferenceRequestV2::Chat` 内层 `ChatRequest` 的序列化继续匹配当前 wire fixtures；Embedding 的请求/响应/error/usage fixture 成对闭合。
4. manifest 能表达一个 v2 包的 task/features，runtime 在进 WASM 前拒绝未声明 task。
5. 同一进程加载 v1 与两个 v2 world 时，memory/time limits、错误分类和 conformance suite 选择正确。

P4.0 只产出原型、决策记录和更新后的估算，不进入发布。任何一项失败都停止 P4.1 并回到架构评审，不能以“先在 manifest 声明、host 保证不调用”掩盖 WIT 结构缺陷。

### 8.4 P4.1：Embedding 纵切

第一版只支持 OpenAI-compatible Embedding：

1. 入站 `/v1/embeddings` 解析与错误映射；
2. `EmbeddingRequest` / `EmbeddingResponse`、批量输入、维度、编码格式与 usage；
3. provider 请求构造和响应解析；
4. model/task capability 过滤；
5. receipt、usage、成本和转换记录；
6. CLI 配置、桌面展示和真实 App 验收；
7. 旧 Chat 请求、官方 adapter 和路由行为回归。

第一版非 Chat 请求只支持 **Direct**。Quota first 的额度单位、conversation affinity 和回退语义尚未定义；Smart tiers 的对话复杂度特征显然不适用。不能把“跳过 Smart tiers”误写成“现有 Quota first 自动适用”。

### 8.5 P4 工作分解

| 子阶段 | 涉及范围 | 净工程日 | 交付边界 |
|---|---|---:|---|
| P4.0 | bindings、双 world dispatch、fixtures、兼容实验 | 4–6 | 不发布，只给 go/no-go |
| P4.1a | Embedding protocol、入站、Direct 路由和单 provider 纵切 | 8–12 | CLI 可端到端调用 |
| P4.1b | v2 world/runtime/conformance、多任务 OpenAI adapter、v1 并存 | 15–22 | 插件生态边界闭合 |
| P4.1c | capability 配置、receipt/usage、CLI/Desktop、文档和发布 | 8–12 | 真实 App 验收完成 |
| 第二种任务 | 验证抽象是否真正复用 | P4.1 后再估 | 不属于 P4.1 |

P4.1 当前合计 31–46 工程日，主要关键路径是 P4.1a → P4.1b → P4.1c。P4.0 后按 §3.1 的 30% 阈值重估。这个区间足以做资源占位，但不能替代阶段设计中的任务级估算。

### 8.6 验收标准

1. OpenAI-compatible Embedding 的单条和批量请求端到端成功。
2. ChatRequest wire、五个官方 adapter 和现有三种 Chat 路由模式无回归。
3. Embedding 只能进入声明支持该任务的模型和 provider。
4. Embedding 请求进入 Smart tiers 或未定义的 Quota first 时具名拒绝，不静默套用 Chat 特征。
5. `provider-adapter-v2` 不包含流式 export；`provider-streaming-adapter-v2` 同时通过 core 和 stream conformance，非流式 adapter 不实现空方法。
6. conformance 覆盖请求、响应、错误、usage、批量、维度和超限输入。
7. CLI、Desktop、README 和配置文档对支持范围保持一致。

---

## 9. Deferred：`router-policy-v1`

技术上可以设计纯函数式接口：host 传 `RequestFeatures` 和脱敏候选列表，插件返回下标与整数分数，host 校验下标并继续执行目的地与凭证授权。

暂不实施，因为它会削弱：

1. `Router::route` 的纯性和无需外部 artifact 的重放能力；
2. `DecidedBy` 对“为什么选中该上游”的完整归因；
3. Smart tiers “只做一次确定性路由决策”的产品承诺。

将来若实施，前置条件仍是：

- 独立 `Custom policy` mode，不冒充 Smart tiers；
- `DecidedBy::Plugin { name, package_digest }`；
- router world 禁 clocks/random，并做跨进程确定性检查；
- receipt 绑定 package digest，重放不匹配时标记 `unreplayable`；
- 只允许池内重排，不得覆盖 Rule、Hint、目的地授权和凭证授权。

---

## 10. 全局不变量

| # | 不变量 | 由什么保证 | 受影响阶段 |
|---|---|---|---|
| 1 | 插件无直接网络、文件系统和 secrets 读取能力 | WIT import、compiled artifact gate、locked-down WASI | P2/P3 必须保持 |
| 2 | 插件读不到凭证 | host-side secret resolution 和 signer | P1 |
| 3 | descriptor 不含凭证头 | `SafeHeaders` 构造与反序列化拒绝 | P1 |
| 4 | 目的地不由 region/service 或插件身份授权 | `ProviderConfig::authorize` 精确 origin/path 检查 | P1 |
| 5 | 签名和发送使用同一请求字节 | host 单次编码、签名后发送 | P1 |
| 6 | provider 请求不跟随重定向 | provider egress policy | P1 |
| 7 | 安装不执行原生脚本，但会执行受限 WASM conformance | 安装流程和 runtime limits | P2 |
| 8 | 完整性、conformance、发布者身份分别记录 | package receipt | F0/P2 |
| 9 | observer 不影响请求决策和结果 | 请求路径外有界队列、独立错误路径 | P3 |
| 10 | observer 输入无请求/响应正文和自由错误文本 | `ObserverReceiptV1` 闭集 schema + sentinel 测试 | P3 |
| 11 | 权威计费不得依赖 best-effort observer | durable outbox、delivery id、幂等与重投 | P3-B |
| 12 | Router 核心默认保持纯函数 | 无时钟、随机、IO 和外部插件 | Deferred |
| 13 | 现有 world ABI 不原地修改 | WIT 函数/import/export 锁定和 compatibility tests | P1/P4 |
| 14 | 新 host 继续加载旧 manifest/adapter | F0 legacy parser + 全套官方回归 | 全部阶段 |
| 15 | receipt 不存放在插件包目录 | data directory + atomic private write | P2 |

---

## 11. 测试、CI 与公开验收边界

本节只列本路线图新增 gate，不替代 `.github/workflows/ci.yml` 中已有的 Rust/desktop fmt、clippy、test、doc、coverage、MSRV、供应链、release gates 和前端测试构建。

### F0

- legacy/v2 manifest 兼容矩阵；
- `min_host_version` 安装期拒绝；
- `api_version`、manifest schema、host version 独立错误分类；
- host receipt 来源不可由包内数据伪造。

### P1

- 每家已支持 signer 的官方 golden vectors；
- 长期/临时凭证矩阵；
- descriptor 无凭证头、最终请求有正确 host-owned 凭证头；
- plugin-visible config 不含 host-only profile 字段或多组 secret refs；
- URL 授权先于 secret resolution；
- 签名字节等于实际发送字节；
- 日志、receipt、错误不含凭证和 canonical request。

### P2

- archive 摘要、解压限制、路径穿越、链接和特殊文件；
- redirect 逐跳授权与认证头剥离；
- worker 摘要复核、环境清空、输出上限、异常退出、trap/超时和孤儿回收；
- conformance 前后摘要不一致、失败或超时均无残留；
- update 成功、失败、receipt 写失败和重启生效边界；
- 三平台文件系统行为。

### P3

- observer world import gate；
- 五个官方 adapter 零改动兼容；
- 未配置 observer 的行为/性能基线；
- sentinel 隐私测试覆盖真实序列化结果；
- trap、超时、非法输出、队列满、代发失败；
- endpoint id、origin、redirect、响应体上限；
- P3-B durable outbox 重启、重复、乱序和磁盘失败。

### P4

- P4.0 三套 bindings、双 world core 复用、manifest task gate 的 compile-only 原型与 go/no-go 记录；
- Chat v1 / 新 world conformance 矩阵；
- Embedding 入站、provider、usage、receipt、错误和批量边界；
- CLI/Desktop 公开行为与真实 App 验收。

---

## 12. 用户可见行为、可访问性与失败处理

本路线图本身不批准具体 UI。每个阶段的独立设计文档必须补齐以下内容：

- CLI 安装确认必须展示最终来源、版本、摘要、conformance 和 publisher 状态；非交互模式必须有等价机器可读输出。
- update 失败必须明确说明旧版本仍在服务、是否需要重启、staging 是否清理。
- signer 配置失败必须区分凭证缺失、profile 不匹配、region/service 不一致、签名失败和目的地授权失败，不能统一成模糊“认证错误”。
- observer 必须显示 best-effort / durable 状态、最近成功时间、积压和丢弃计数；不能暗示未实现的可靠性。
- Desktop 若新增插件或 modality 页面，必须覆盖键盘操作、焦点顺序、屏幕阅读标签、错误公告、窄屏布局和中英文文案。
- 任何会改变 Desktop 行为或界面的阶段都必须运行 `scripts/install-local-desktop.sh` 并完成真实 App 验收。

---

## 13. 实现落点与阶段设计文档

| 阶段 | 主要落点 | 设计文档路径 | 必须额外回答 |
|---|---|---|---|
| F0 | `crates/plugin-api`、`apps/cli/src/plugins.rs` | `docs/design/<date>-plugin-manifest-v2.md` | schema 迁移、错误分类、host version 来源 |
| P1 | `crates/protocol`、`apps/cli/src/config.rs`、`gateway.rs`、secrets | `docs/design/<date>-aws-bedrock-sigv4.md` | profile wire、官方 crate 版本、临时凭证、精确签名字节 |
| P2 | `apps/cli/src/plugins.rs`、`main.rs`、`crates/release`、下载模块 | `docs/design/<date>-remote-plugin-artifact.md` | artifact 命名、redirect、worker 协议、三平台原子更新 |
| P3-A | `plugin-api/wit`、`plugin-runtime`、`conformance`、`metrics` | `docs/design/<date>-observer-v1.md` | `ObserverReceiptV1`、队列、endpoint、best-effort 边界 |
| P3-B | `metrics`、`store`、代发 worker | `docs/design/<date>-observer-durable-outbox.md` | 事务、delivery id、幂等、重投和运维状态 |
| P4 | `protocol`、`router-core`、runtime、conformance、gateway、官方插件、Desktop | `docs/design/<date>-inference-v2-embedding.md` | spike 结果、双 world、Embedding 全链路、迁移与 UI |

任何阶段开始编码前，都必须先完成对应设计文档、公开行为测试和验收矩阵；实施完成后回写状态、自动化结果、真实 App 验收和遗留项。

---

## 14. 已知遗留与明确延期

1. `plugin new` 当前只脚手架 provider adapter。`--kind agent` 与 `--kind observer` 分别在 P2/P3 后评估，不提前扩展。
2. Desktop 当前没有插件管理页面。P2 第一版仍可 CLI-first，但正式桌面入口必须单独设计，不能顺手塞进远程安装实现。
3. 默认凭证和 Desktop 请求正文的本地明文存储，会限制“WASM 沙箱”可对外承诺的安全范围。本路线图不顺带改造存储，但发布文案必须继续如实说明。
4. 第三方 publisher key registry、撤销、轮换和透明日志不在 P2 首版；现有未验证状态必须清晰展示。
5. 私有 GitHub/GitLab 仓库、企业代理认证和离线 registry 不在 P2 首版。
6. Rerank、ASR、TTS、Image 在 Embedding 纵切证明抽象后再排期。

---

## 15. 路线图验收状态

### 已完成

- 对照当前 WIT、manifest、runtime、provider authorization、plugin install、metrics、router 和 CI 完成事实核对。
- 修正导出函数数量、目标仓库、安装期执行、manifest 版本机制、package digest、Receipt 类型和 P4 规模表述。
- 明确区分目的地授权、签名作用域、数据外发和版本兼容四类边界。
- 补齐各阶段依赖、失败路径、兼容验收、用户可见要求和发布要求。
- 固定 P1 为 AWS Bedrock SigV4 纵切，service 固定且 region 由官方 endpoint 与维护集合共同约束。
- 固定 P2 使用独立 conformance worker，并明确它只提供故障隔离、不构成低权限安全边界。
- 固定 P4 为封闭 Chat + Embedding v2 envelope 与流式/非流式双 provider world，P4.0 只验证可行性。
- 给出阶段级净工程日、置信度、P4 工作分解和超过 30% 时的重估门。

### 实施前置（不属于路线图决策缺口）

- 各阶段获准启动时，必须按 §13 的固定路径先创建独立设计文档；路线图不能代替实施设计。
- P1 发布验收需要受控 AWS Bedrock 账号；没有账号时只能完成离线 golden vectors，不能宣称真实纵切完成。
- P2 的 Windows/macOS/Linux worker 回收和原子替换必须在对应 runner 上验证；单平台结果不能外推。
- P4.0 尚未执行，因此 35–52 工程日是资源占位区间，不是任务级承诺。
- 本次仅修改文档，不涉及代码、构建、本地 Desktop 安装或真实 App 验收。
