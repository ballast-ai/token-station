# `token-station-south` 社区版接入方案

> 状态：待评审；诊断接入阶段可启动，生产流量切换仍有前置门禁
>
> 适用仓库：当前仓 `GlimpseEngine/token-station` 与独立仓
> `ballast-ai/token-station-south`
>
> 更新日期：2026-08-17

## 0. 结论

社区版不应直接把现有 `Gateway::send` 或 `Gateway::try_upstream` 整体替换成
South。当前两边存在四个实质差异：

1. 社区版主调用链是 `spawn_blocking + ureq` 的同步流水线，South v1 是
   `Tokio + reqwest` 的异步调用；
2. 社区版支持 streaming、GET、Header/OAuth 认证和显式 HTTP/SOCKS5 代理，South
   v1 只支持 Bearer、buffered JSON POST、direct egress；
3. 社区版会从成功响应的九个 provider header 更新配额账本，South v1 只返回
   `content-type` 与 `retry-after`；
4. 社区版当前 Rust MSRV 是 1.95，South 是 1.96。

因此采用两段式接入：

- **阶段 A：真实诊断接入。** 为 `upstream test` 增加显式
  `--transport south-v1`，由社区版真实配置、插件、secret store 和 South reqwest
  transport 完成一次 OpenAI-compatible Bearer JSON POST。它不进入用户生产流量，
  但足以证明社区宿主 adapter、凭证解析、网络调用和 provider 响应解析已经连通。
- **阶段 B：生产流量接入。** 先在 South 增加具名、有界的配额响应元数据契约，
  再按 upstream 显式 opt-in，仅迁移 direct egress 下的非流式
  OpenAI-compatible Bearer JSON POST。其余请求继续走 legacy ureq。

这里必须直接指出一个逻辑漏洞：**“South v1 已能发 OpenAI JSON POST”不等于“现有
OpenAI 生产路径可以无损迁移”。** 如果现在切换，调用本身大概率成功，但配额账本会
停止接收 provider 的权威窗口数据。这种退化不会立刻报错，比显式失败更危险。

## 1. 背景与事实基线

### 1.1 South 当前状态

截至 2026-08-17：

- South 最小纵切已经合入 private 仓 `main`；PR #1 已合并，远端 `quality` CI
  通过；
- 当前 `main` 提交为 `3b6c91fe3706757c2ea1891cee4399ab730f48c5`，尚无 release
  tag；
- `compatibility.json` 仍将 `token-station` 与 `token-station-server` 标为
  `not_verified`；
- 已实现 `south-contracts`、`south-core`、`south-transport-reqwest`、
  `south-provider-conformance` 与 `south-testkit` 的 provider-call v1；
- v1 只承诺一次有界、非流式、Bearer 认证的 JSON POST；stream、WIT、provider
  runtime、ureq transport、routing、retry、quota 和持久化均不在该纵切内；
- South 不读环境变量、配置文件、数据库、缓存或 secret store，也不应在社区接入时
  获得这些所有权。

### 1.2 社区版当前调用链

真实生产路径在 `apps/cli/src/gateway.rs`：

```text
server::chat
  -> tokio::task::spawn_blocking
  -> Gateway::chat_scoped
  -> routing / admission / attempt budget
  -> Gateway::try_upstream
  -> provider plugin build_http_request
  -> ProviderConfig::authorize
  -> Gateway::send / send_raw_with
  -> SecretStore::resolve
  -> ureq
  -> quota header harvest
  -> provider plugin parse_response / map_provider_error
  -> agent render / receipt / health / fallback
```

其中 `routing`、admission、attempt budget、health、fallback、quota、receipt、agent
render 都是社区宿主策略，继续留在当前仓。South 只替换上图中“一个已经选定的
upstream attempt 如何安全发出并得到有界响应”这一段。

### 1.3 现有 South v1 能力边界

| 维度 | South v1 | 社区版现状 | 首批处理 |
|---|---|---|---|
| 请求方法 | POST | GET、POST | 只接 POST |
| 请求体 | 一份完整、有界 JSON | JSON，也有无 body 请求 | 必须有 JSON body |
| 认证 | Bearer + 唯一 credential slot | Bearer、Header、OAuth、无认证 | 只接 Bearer |
| 响应 | 最大 32 MiB 的 UTF-8 buffered body | buffered + SSE stream | 只接非流式 |
| 响应元数据 | `content-type`、`retry-after` | 任意响应头，并解析九个 quota header | 诊断可接；生产前补契约 |
| 重定向 | 全部拒绝 | 全部拒绝 | 行为一致 |
| 代理 | 强制 no-proxy | direct、HTTP、HTTPS、SOCKS5、`no_proxy` | 只接 direct |
| retry/fallback | 不拥有 | 宿主拥有 | 继续由宿主拥有 |
| deadline/cancel | Tokio absolute deadline + `CancellationToken` | `std::Instant` + 自有原子 token | 分阶段桥接 |
| MSRV | 1.96 | 1.95 | 社区版统一升到 1.96 |

## 2. 目标、范围与非目标

### 2.1 目标

1. 让社区版成为 South provider-call v1 的第一个真实已验证宿主之一，而不是只运行
   South 自带的 reference executor。
2. 用现有社区配置中的 trusted base endpoint、credential slot 和 secret source
   组装 South binding/resolver，保留“先验证目的地和 slot，再解析 secret”的安全顺序。
3. 第一个真实网络调用通过当前 official OpenAI-compatible provider plugin 构造请求，
   通过 South 发出，再交回同一个 plugin 解析响应。
4. 保留现有用户可见错误分类、超时、取消、重试、健康、配额和 receipt 语义；无法保留
   的路径不迁移。
5. 通过 South 的 `south.provider-call.v1` 七用例 assembled-executor suite，并在两仓
   CI 中形成可复现的兼容证据。
6. 所有切换均可回滚；不以生产双发作为对比手段。

### 2.2 第一阶段范围

- `provider == "openai-compatible"`；
- `ApiDialect::Translated`；
- official/conformance-approved provider package；
- `POST`；
- `Auth::Bearer`；
- credential source 为已加载到内存的 local store 或环境变量；
- 有 JSON body；
- `request.stream == false`；
- `EgressMode::Direct`；
- descriptor URL 可被拆成 trusted `base_url` 与 South 合法相对路径；
- descriptor 无 query、fragment、userinfo 或模糊编码；
- 普通 header 能通过 South `SafeHeaders` 的数量、单项和总字节限制。

### 2.3 非目标

- 不迁移 SSE/streaming；
- 不迁移 `/models` 等 GET 调用；
- 不接 `Auth::Header`、OAuth、无认证 local provider；
- 第一阶段不接请求时同步读取的 `AuthConfig::file` secret source；
- 不接 HTTP/HTTPS/SOCKS5 proxy 或 `no_proxy`；
- 不接 Anthropic native passthrough；
- 不把 retry、fallback、routing、health、quota、receipt、billing 或持久化搬进 South；
- 不迁移 provider WIT/runtime，也不删除当前 provider protocol/runtime 类型；
- 不在真实 provider 上双发请求做 shadow comparison；
- 不因为诊断调用接通，就删除 legacy ureq 或宣称全部社区流量已迁移；
- 不让 South 读 SQLite、`secrets.json`、环境变量或 key 文件。

## 3. 必须先满足的决策门禁

### G0：依赖可获取性

当前 `token-station` 与 `token-station-south` 都是 private 仓，因此内部接入可以使用
private git dependency。但旧总方案要求 South 最终保持公开可编译，而当前事实与该
要求不一致。

本阶段的处理是：

- 集成开发先用 HTTPS git dependency，并精确 pin 到不可变 `rev`；
- 两仓之间使用只读 GitHub App installation token，不使用个人 PAT，不把 token 写进
  Cargo manifest、lockfile、日志或缓存 key；
- 本地开发者通过自己的 GitHub 凭证获取依赖；
- **社区版对外公开前**，必须二选一：将 South 仓公开，或发布任何社区贡献者都可获取
  且许可证一致的 source artifact。不能发布一个外部用户无法从源码构建的“社区版”。

推荐最终选择是公开 South 仓。把源码发布到 crate registry 而仓库保持 private 也会
实际公开 crate source，却让 issue、review 和变更历史不可见，社区协作体验更差。

### G1：版本与 MSRV

接入 PR 必须把社区版统一 MSRV 从 1.95 升至 1.96，包括：

- 根 `Cargo.toml`；
- 仍单独声明 1.95 的 crate manifest；
- `.github/workflows/ci.yml` 的 MSRV job 名称、toolchain 和 cache key；
- README 与开发环境文档；
- release/build 脚本中任何固定 1.95 的检查。

不能只让 Cargo 在开发机上用 stable 编过，而保留一个已经不真实的 1.95 CI 承诺。

### G2：South release 标识

当前 South 尚无 tag。接入按两步完成，避免兼容状态与 tag 形成循环依赖：

1. 社区集成 PR 开发期 pin 到已合入 `main` 的确切提交
   `3b6c91fe3706757c2ea1891cee4399ab730f48c5`；
2. 社区 adapter 与真实诊断调用验收后，在 South 用英文 PR 更新
   `compatibility.json` 和文档，并创建不可变 tag；
3. 社区集成 PR 再从临时 `rev` 切到该 tag，重新生成 `Cargo.lock` 并重跑全门禁。

正式合入的 manifest 不依赖 branch 名；tag 被移动视为供应链事故，CI 应通过 lockfile
和预期 commit 校验发现它。

### G3：生产配额元数据

生产切换前，South 必须新增一份独立设计，并用具名、有界字段携带社区版当前读取的
九个 header：

- `x-ratelimit-limit-tokens`
- `x-ratelimit-remaining-tokens`
- `x-ratelimit-reset-tokens`
- `anthropic-ratelimit-tokens-limit`
- `anthropic-ratelimit-tokens-remaining`
- `anthropic-ratelimit-tokens-reset`
- `anthropic-ratelimit-unified-limit`
- `anthropic-ratelimit-unified-remaining`
- `anthropic-ratelimit-unified-reset`

不接受“返回任意 response headers”作为快捷修复。应使用 closed、named、bounded
metadata contract，并覆盖重复 header、超长值、非法 UTF-8 和 redacted Debug。

## 4. 目标架构与所有权

```text
社区宿主（当前仓）
  routing / admission / fallback / quota / receipt / plugin runtime
                      |
                      v
  CommunitySouthAdapterV1
    - trusted binding projection
    - legacy descriptor -> South request
    - SecretStore resolver adapter
    - South error/response -> legacy protocol
                      |
                      v
South（独立仓）
  south-contracts -> south-core -> south-transport-reqwest
                      |
                      v
                 provider endpoint
```

### 4.1 社区宿主继续拥有

- upstream 配置和 `ProviderConfig::authorize`；
- provider plugin 的选择、加载、请求构造和响应解析；
- `SecretStore` 以及 store/env/file 三种 secret source；
- egress 模式选择；
- request lifecycle 的具体原因：client disconnect、server drain、deadline；
- routing、attempt budget、retry/fallback、health、quota、metrics、receipt；
- 用户可见错误文案和 HTTP status；
- Tokio runtime 的创建、关闭和 task accounting。

### 4.2 South 拥有

- endpoint、相对路径、credential slot、header、JSON body 的版本化边界；
- endpoint/slot binding 和 secret resolution 顺序；
- buffered provider call 的 absolute deadline/cancellation 编排；
- hardened reqwest client 与一次 HTTP I/O；
- transport error 的稳定 South code；
- provider-call v1 conformance fixture 与 runner。

### 4.3 新增社区 adapter 的位置

第一阶段放在 `apps/cli/src/south_provider_call.rs`，保持 `pub(crate)`：

- 它需要直接使用 `Gateway` 的 `SecretStore`、`Upstream` 和 legacy protocol 类型，属于
  社区宿主胶水；
- 目前只有一个消费方，不为单用途代码提前创建新 workspace crate；
- South 不能反向依赖它；
- 等企业宿主也出现相同、且确实不含宿主类型的投影逻辑后，再评估是否下沉到
  `south-migration`，现在不做猜测性抽象。

## 5. 依赖与 CI 设计

### 5.1 Cargo 依赖

生产依赖只引入：

- `south-contracts`
- `south-core`
- `south-transport-reqwest`

测试依赖引入：

- `south-provider-conformance`
- `south-testkit`

五个 package 必须来自同一个 git source 和同一个 commit/tag。根 workspace 统一声明，
`apps/cli` 不分别 pin 五个不同版本。`Cargo.lock` 必须提交。

### 5.2 Private cross-repo CI

推荐流程：

1. 创建只对 `ballast-ai/token-station-south` 有 `contents:read` 的 GitHub App；
2. 将 App 安装到 South，并让社区仓 workflow 能交换短期 installation token；
3. checkout 后将 token 仅注入当前 job 的 git credential/header；
4. 使用 `CARGO_NET_GIT_FETCH_WITH_CLI=true` 获取 private dependency；
5. job 结束无条件清理临时 credential 配置；
6. `cargo metadata --locked` 校验五个 South package 的 source commit 完全一致；
7. dependency policy 拒绝 South 反向依赖当前仓、第二份 reqwest 或 South 之外的直接
   reqwest owner。

日志中不得输出带 token 的 remote URL，也不得用 `set -x` 包住 credential 配置步骤。

### 5.3 双仓兼容证据

- 社区仓 CI 运行社区 adapter 的公开行为测试、assembled conformance 和全 workspace
  gate；
- South CI 保持自己的 99 项以上 library suite 与 boundary gate；
- South compatibility 更新 PR 记录社区仓 commit、South tag、suite id/version 和 CI
  URL；
- 单纯运行 `ReferenceProviderCallExecutorV1` 不算宿主验证，必须运行社区 adapter 组装
  出来的 executor，并做 wiring review。

## 6. 社区 adapter 的精确行为

### 6.1 输入与结果

adapter 接受：

- trusted upstream name；
- trusted `ProviderConfig`；
- provider plugin 产出的 `HttpRequestDescriptor`；
- absolute deadline；
- host cancellation；
- 注入的 `CredentialResolver` 与 `AsyncHttpTransport`。

adapter 返回：

- 可交给现有 plugin 的 `HttpResponseParts`；或
- 已映射为现有 `ErrorEnvelope` 的失败；或
- **在任何 secret/I/O 发生前**判定为 `Ineligible`，由宿主选择 legacy path。

`Ineligible` 是 host-local、closed enum，不进入 South error contract。它至少区分 method、
stream、auth、body、URL、headers、egress 和 metadata capability，`Debug` 只显示枚举值，
不携带 descriptor 内容。

### 6.2 Eligibility 判定

判定顺序固定如下：

1. 检查 rollout mode 是否允许 South；
2. 检查 provider/package、dialect 和 egress mode；
3. 检查 request 是否非流式 POST；
4. 检查 body 与 `Auth::Bearer` 是否存在；
5. 用 trusted `base_url` 构造 `ProviderEndpointV1`；
6. 从 descriptor absolute URL 提取相对路径，并再次证明 origin、effective port 和
   segment-aware base prefix 与 trusted endpoint 相同；
7. 将 legacy slot 解析为 `CredentialSlotV1`；
8. 将普通 header 投影为 South `SafeHeaders`；
9. 将已序列化 JSON 投影为 `JsonBodyV1`；
10. 构造 `ProviderBindingV1` 与 `JsonPostRequestV1`；
11. 只有以上全部成功，才允许 resolver 和 transport 被调用。

即使 South 自己还会做 binding 校验，社区版已有的
`ProviderConfig::authorize(&descriptor)` 仍保留为迁移期 defense-in-depth。两个检查不应
通过共享一个布尔结果互相短路，测试需要分别证明它们生效。

### 6.3 URL 投影

不能用字符串 `trim_start_matches(base_url)` 得到相对路径。投影必须：

- 分别解析 trusted endpoint 与 descriptor URL；
- 比较 scheme、normalized host、effective port；
- 按 path segment 比较 base prefix；
- 拒绝 query、fragment、userinfo、leading slash、dot segment、重复 slash、反斜杠、
  encoded separator、double encoding 和 scheme-like first segment；
- 得到 South 相对路径后，再调用 `resolve_against` 并断言结果等于 descriptor URL 的
  canonical form。

不允许先解析 secret 再做这个投影。

### 6.4 Credential resolver

`CommunityCredentialResolverV1` 只保存对 `SecretStore`、upstream name 和声明 slot 的
借用，不持久化 secret：

1. South 请求的 slot 必须等于 trusted upstream 声明的 slot；
2. 第一阶段只允许 local store 与 env source：store 值已在 Gateway 构造时加载，env
   lookup 不产生后台任务；`AuthConfig::file` 在 eligibility 阶段返回 `Ineligible`；
3. 调用现有 `SecretStore::resolve(upstream, slot)`，并在 South resolver future 的 poll
   内立即完成该次无后台任务的 lookup；
4. trim 规则保持现有行为；
5. 空值保持现有 auth failure；
6. 增加 host-local 16 KiB credential value 上限；超限返回
   `CredentialResolutionErrorV1`，不构造 header、不调用 transport；
7. 立即把 owned `String` 移交 `SecretValue::new`，不 clone，不 log；
8. resolver future 被 drop 时不得留下线程、task 或 I/O。local store/env 路径不 spawn
   工作；请求时同步文件读取不能被 future drop 中止，因此明确不进入第一阶段 South
   path。若未来接 file、Vault 或网络，必须先实现有界且真正 cancellation-safe 的
   capability。

16 KiB 是社区宿主策略，不冒充 South v1 contract。将来 South 若增加正式 credential
value limit，社区上限只能更严，不能更宽。

### 6.5 Request body 与 headers

- legacy `serde_json::Value` 只序列化一次为 owned JSON 字符串；
- South 重新验证它恰好是一份完整 JSON，并执行 32 MiB 上限；
- 首阶段接受 legacy DOM 加 South shared `Arc<str>` 的迁移期开销，但不再额外复制
  32 MiB body；
- header 名称和值逐项投影；credential、host/framing/hop-by-hop header 继续拒绝；
- 不“过滤后继续”。任何 header 不兼容都在 I/O 前得到 `Ineligible` 或明确 contract
  failure，避免悄悄改变 plugin 请求。

### 6.6 Response 投影

South `BufferedHttpResponseV1` 转回 legacy `HttpResponseParts` 时只构造：

- exact status；
- exact UTF-8 body；
- 可选 `content-type`；
- 可选 `retry-after`；
- 空 extensions。

official OpenAI-compatible plugin 的 `parse_response` 只依赖 body，
`map_provider_error` 依赖 status/body/`retry-after`，因此诊断路径足够。任何依赖其他
响应 header 的 plugin 不进入 v1 eligibility。

## 7. 异步、deadline 与 cancellation

### 7.1 诊断阶段

`upstream test --transport south-v1` 是同步 CLI 命令，但一次命令可以创建一个
current-thread Tokio runtime：

- runtime 每条 CLI 命令只创建一次，不为每个 model 或每次 HTTP attempt 创建；
- 所有 model probe 在该 runtime 内按现有顺序执行；
- 使用 `PROBE_TIMEOUT` 计算 South absolute deadline；
- CLI 中断或 deadline drop 整棵 future，不产生 detached task；
- 不在已有 Tokio runtime 内嵌套 `Runtime::block_on`。

### 7.2 生产阶段

生产请求仍在 `spawn_blocking` worker 中运行。最小迁移方案不是把整个 gateway 改成
async，而是由宿主注入一个随 server 生命周期存在的 Tokio `Handle`：

1. CLI server 与 Desktop server 都先创建 runtime，再构造启用 South 的 Gateway；
2. blocking worker 只通过该 handle 驱动一次 South future；
3. 禁止在普通 async worker 上调用该 blocking bridge；
4. runtime 由 server lifecycle 持有并在 drain 完成后关闭，Gateway 不自行创建或销毁
   runtime；
5. 每个执行 future 都在结构化 scope 内，不 `spawn` 一个无法 await 的 watcher。

现有 `CancelToken` 需要增加可异步等待的、同源的取消通知，而不是用 wall-clock polling
桥接。推荐内部组合 `tokio_util::sync::CancellationToken`，保留现有原子
`CancelReason` 作为 user-visible reason：

- child cancel 只取消当前请求；
- root drain 级联到所有 child；
- local client disconnect reason 优先于随后发生的 parent drain；
- South 收到同一个 child cancellation token；
- `RequestContext` 暴露 exact absolute deadline，转换为 `tokio::time::Instant`，不在调用
  中途重新用 `now + remaining` 制造漂移。

这些改动必须先有 paused-time、deterministic cancellation tests，不能用真实 sleep
证明时序。

## 8. 错误映射

South code 不直接暴露给现有 OpenAI/Anthropic 客户端。社区 adapter 使用一张 closed
映射表生成现有 `ErrorEnvelope`，日志只允许记录 South code，不记录 endpoint、path、
slot、header、secret 或 body。

### 8.1 建议映射表

| South code | 社区 `ErrorCode` / HTTP | 说明 |
|---|---|---|
| `INVALID_ENDPOINT` | `Internal` / 500 | trusted config 或投影 bug；应尽量在启动时拒绝 |
| `INVALID_RELATIVE_PATH` | `Internal` / 500 | plugin descriptor 不符合 frozen contract |
| `INVALID_CREDENTIAL_SLOT` | `Internal` / 500 | config/adapter 不兼容 |
| `INVALID_JSON_BODY` | `Internal` / 500 | 从 legacy `Value` 序列化后不应发生 |
| `REQUEST_BODY_TOO_LARGE` | `InvalidRequest` / 413 | 请求经 plugin 展开后超过边界，不跨 upstream 重试 |
| `URL_OUTSIDE_BINDING` | `Internal` / 500 | credential exfiltration gate，resolver/transport 必须为 0 |
| `CREDENTIAL_BINDING_MISMATCH` | `Internal` / 500 | 同上 |
| `CREDENTIAL_RESOLUTION_FAILED` | `Auth` / 401 | 保留现有 secret source 失败语义 |
| `CANCELLED` | 由 `RequestContext` reason 决定 | client=499、drain=503、deadline=504 |
| `DEADLINE_EXCEEDED` | `Timeout` / 504 | caller-owned total/attempt deadline |
| `CLIENT_BUILD_FAILED` | `Internal` / 500 | transport 应在 Gateway 启动时构造，避免请求时发生 |
| `TRANSPORT_TIMEOUT` | `Timeout` / 504 | transport timeout 先于 outer deadline |
| `CONNECT_FAILED` | `UpstreamUnavailable` / 502 | 可进入现有 fallback/health 逻辑 |
| `REQUEST_FAILED` | `UpstreamUnavailable` / 502 | 可进入现有 fallback/health 逻辑 |
| `RESPONSE_READ_FAILED` | `TransportTruncated` / 502 | buffered body 未完整读完 |
| `RESPONSE_BODY_TOO_LARGE` | `ProviderProtocolError` / 502 | provider response 越过边界 |
| `RESPONSE_BODY_NOT_UTF8` | `ProviderProtocolError` / 502 | 与现有 `into_parts` 行为一致 |
| `RESPONSE_METADATA_INVALID` | `ProviderProtocolError` / 502 | provider metadata 无法安全投影 |
| `REDIRECT_DENIED` | `UpstreamUnavailable` / 502 | 保持现有 redirect refusal 分类 |

对 4xx/5xx 不做 transport-error 映射：South 返回正常 buffered outcome，再交现有 provider
plugin 的 `map_provider_error` 分类。这样 401、402、429、529 等业务语义仍由 dialect
拥有。

### 8.2 双发与 fallback 规则

- **允许的 fallback：** eligibility 在 secret resolution 和 network I/O 之前返回
  `Ineligible`，宿主可以走 legacy；
- **禁止的 fallback：** South 已解析 secret、开始 transport 或拿到 provider response
  后，不得再用 legacy 重发同一真实请求；
- South transport failure 是否切换到另一个 upstream，仍由现有 attempt budget 和
  routing fallback 决定；它不是“同一个 upstream 换 ureq 再试一次”。

## 9. 用户可见交互与状态

### 9.1 诊断阶段 CLI

```text
token-station-cli upstream test <name> [--model <model>]
  [--transport legacy|south-v1]
```

- 默认 `legacy`，现有脚本和用户行为不变；
- `south-v1` 不满足 eligibility 时明确失败并列出安全的原因枚举，不静默改走 legacy；
- 成功输出继续是 `<model>: ok (<N> ms)`；
- 失败输出继续使用现有 `message (ErrorCode)` 结构，不包含 South 内部值；
- help 明确说明这是一次真实、可能计费的最小 completion；
- 多 model probe 保持串行，避免新增并发成本和 rate-limit 行为。

### 9.2 生产阶段配置

在 `UpstreamConfig` 增加默认值为 legacy 的字段：

```json
{
  "provider_call": "legacy"
}
```

候选值只有：

- `legacy`
- `south_v1_buffered`

`south_v1_buffered` 表示“满足 eligibility 的 buffered call 使用 South；stream、GET、
非 Bearer、proxy 等仍使用 legacy”，不是“该 upstream 全部请求都已迁移”。配置保存、
备份、恢复和 Desktop 表单必须保留该字段。

### 9.3 Desktop 与可访问性

诊断阶段只增加 CLI flag，不改 Desktop UI。生产阶段若暴露配置，Desktop upstream
高级设置增加“Provider call engine”选择器：

- 有持久可见 label 和说明，不只靠 placeholder；
- 键盘可聚焦、方向键/回车可选择；
- focus ring 不得被样式移除；
- 不满足 direct/Bearer/OpenAI-compatible 等静态条件时，South 选项 disabled，并在控件
  邻近给出文本原因；
- 不以颜色单独表达状态；
- 保存失败保持用户输入并把焦点移到错误摘要；
- 小屏下不横向溢出。

## 10. 分阶段实施计划

### PR 1：依赖与 MSRV 就绪

改动：

- 社区版 MSRV 1.95 -> 1.96；
- 配置 private dependency 的短期 CI 凭证；
- 添加同 commit 的 South production/dev dependencies；
- 更新 lockfile、deny/source policy、README 和开发环境文档；
- 添加 metadata gate，证明 South source 单一且 reqwest 只由 South transport 直接拥有。

验收：

- Rust 1.96 `cargo check --workspace --all-targets`；
- stable fmt/clippy/test/doc；
- root、desktop、plugin guest 的 locked build 均可获取依赖；
- 无运行时行为变化；
- 远端 PR CI 使用短期 token 通过，fork PR 在没有 secret 时明确 skip/报出不可获取原因，
  不表现成随机 Cargo failure。

### PR 2：host adapter 与 conformance

改动：

- 新增 `apps/cli/src/south_provider_call.rs`；
- 实现 eligibility、URL/body/header/slot 投影、credential resolver、response/error 映射；
- 用注入 fake resolver/transport 组装 community executor；
- 运行 `south.provider-call.v1` 七用例；
- 增加社区特有的投影和错误映射测试。

验收：

- 七用例完整通过，不是 reference executor 代跑；
- invalid path/slot 在 resolver/transport 前失败；
- pending resolver cancellation 与 pending transport deadline 都有 drop evidence；
- error mapping 19 个 code 全覆盖；
- secret、endpoint、path、slot、header、request/response sentinel 不出现在 Debug/Display；
- adapter 不读网络、数据库或全局环境。

### PR 3：真实诊断调用

改动：

- 增加 `--transport south-v1`；
- 一条 CLI 命令创建一次 current-thread runtime；
- `probe_model` 的 South 分支复用同一个 provider plugin build/authorize/parse 流程；
- legacy 继续默认；
- 添加 deterministic loopback provider 测试。

验收：

- loopback 精确观察一次 POST、正确 path、普通 header、Bearer 和 JSON body；
- 201/400/429/500、redirect、oversize、invalid UTF-8、timeout 均按冻结表映射；
- eligible South failure 不触发第二次 legacy 请求；
- ineligible 在 secret/network 前明确失败；
- 真实 provider smoke 只由 lv 明确提供的测试账号执行，不进入普通 CI；
- CLI help、成功/失败文本和收费提示完成验收。

完成 PR 3 后，可以说“社区宿主已验证 South provider-call v1 的真实诊断调用”，不能说
“社区生产流量已经迁移”。

### PR 4：South 兼容状态与 release

改动发生在 South 仓，代码和文档全部使用英文：

- 记录社区仓 commit 与 CI evidence；
- 更新 `compatibility.json` 的社区宿主状态，状态名必须表达 provider-call v1 范围，
  不用模糊的 `verified`；
- 更新 README 的 host adoption 状态；
- 创建 immutable release tag；
- 社区仓改用该 tag 并重跑全门禁。

### PR 5：生产前 South metadata contract

改动发生在 South 仓：

- 为九个 quota header 设计 closed、named、bounded response metadata；
- 不扩成 arbitrary header map；
- 扩充 reqwest transport、contracts、conformance、fuzz 与 compatibility version；
- 社区 adapter 把新 metadata 还原给现有 `parse_quota_windows`；
- 同一 loopback response 对 legacy 与 South 两条路径产生完全相同的
  `WindowSnapshot`。

### PR 6：生产非流式 canary

改动：

- 增加 `provider_call` 配置和 Desktop advanced selector；
- 建立同源 async cancellation bridge；
- server runtime 先于启用 South 的 Gateway 构造并注入 handle；
- 只在 `south_v1_buffered` 且 eligibility 通过时替换一个 upstream attempt；
- stream 和其他不兼容路径仍走 legacy；
- admin/receipt 记录安全的 transport engine 枚举，不能记录敏感上下文。

验收：

- routing、attempt count、health、retry/fallback、quota、receipt 在 loopback comparison 中
  与 legacy 一致；
- client disconnect、server drain、deadline 都能取消正在进行的 reqwest I/O；
- runtime shutdown 等待所有 provider call，没有 detached task；
- South path 每个 attempt 只发一次；
- `provider_call=legacy` 是即时、无需数据迁移的回滚；
- 完成本地 Desktop 安装、签名/bundle id 校验、启动和真实 UI 验收。

### PR 7 以后：扩大覆盖面

只有新 South contract 与独立设计通过后，才逐项扩展：

1. explicit proxy capability；
2. Header auth；
3. unauthenticated local provider；
4. GET；
5. streaming/SSE；
6. 其他 provider dialect；
7. 最终删除 legacy ureq。

这些项目相互独立，不绑成一次“大迁移”。

## 11. 测试矩阵

### 11.1 Contract/eligibility

- endpoint：scheme、host 大小写、default/non-default port、base path；
- path：leading slash、dot segment、重复 slash、encoded dot/separator/double encoding；
- URL：userinfo、query、fragment、跨 origin、相似前缀；
- slot：空、超 64 bytes、非法首字符、非 ASCII、合法 `provider_api_key`；
- body：空、多个 JSON value、32 MiB 边界、超限、深层/大节点 JSON；
- headers：credential、host/framing/hop-by-hop、重复、数量/单项/总量边界；
- eligibility：stream、GET、Header/OAuth、无 auth、proxy、Anthropic native、unsigned
  plugin、file secret source 均不误入 South。

### 11.2 Secret 与安全顺序

- bad endpoint/path/slot 时 resolver=0、transport=0；
- secret missing/empty/超 16 KiB 时 transport=0；
- resolver 正好一次；
- transport 正好一次；
- secret sentinel 不出现在 error、Debug、receipt、metrics 或 test failure snapshot；
- response/metadata/body sentinel 只通过显式 accessor 可见。

### 11.3 Lifecycle

- already-cancelled 的优先级高于同时 ready 的 completion/deadline；
- resolver pending 时 cancel/deadline drop resolver，transport=0；
- transport pending 时 cancel/deadline drop transport；
- client disconnect -> 499；
- server drain -> 503；
- outer deadline -> 504；
- transport timeout -> 504，但稳定保留不同 South code 供内部诊断；
- paused Tokio time，无真实 sleep；
- 每个 spawned/blocking task 都被 await/accounted。

### 11.4 Response 与业务等价

- 2xx exact body/status/content-type；
- 4xx/5xx 仍由同一 provider plugin `map_provider_error`；
- `retry-after` 的秒数映射不变；
- 九个 quota header 引入新契约后，legacy/South 生成完全相同的窗口；
- redirect 不跟随且不二次发送；
- body oversize、非 UTF-8、metadata invalid 均 fail closed；
- nonstream parse/render/usage/receipt 与 legacy fixture 相同。

### 11.5 工程与供应链

- fmt、Clippy `-D warnings`、workspace tests、rustdoc `-D warnings`；
- Rust 1.96 MSRV；
- root/desktop/plugin guest locked build；
- cargo deny/audit/machete；
- South dependency source/commit/features gate；
- Linux、Windows、macOS post-merge platform builds；
- 代码变更最终执行 `scripts/install-local-desktop.sh`。

## 12. 安全与数据红线

1. provider plugin 永远看不到 secret value，只能看到 slot；
2. endpoint containment 和 slot binding 必须在 secret resolution 前完成；
3. South 不读数据库、SQLite、secret store、env、key file 或系统 keychain；
4. 不把请求/响应正文、secret、endpoint、path、slot 或任意 header 值写入日志、错误、
   metrics、receipt 或 Debug；
5. 不跟随 redirect，不把 Bearer 带到第二个 origin；
6. 不读取 ambient proxy 环境变量；South path 只在 host 明确 direct 时启用；
7. URL 校验不能解决 DNS rebinding/private SSRF；社区宿主原有 endpoint/egress policy
   继续有效，未来 proxy capability 也必须显式注入；
8. 不做真实 provider 双发，不用“shadow”名义产生额外费用或副作用；
9. 不在失败后把同一 attempt 从 reqwest 改由 ureq 重放；
10. private repo token 最小权限、短生命周期、日志不可见；
11. South private 可见性不能成为未来公开社区版的隐式构建门槛；
12. 不降低或删除现有 tests/coverage 门槛来让跨仓依赖通过。

## 13. 回滚与故障处理

### 13.1 诊断阶段

- 默认始终是 legacy；去掉 `--transport south-v1` 即回滚；
- South probe 失败只影响该次显式诊断，不修改 config、health、quota 或 metrics；
- 不自动重发 legacy，因此回滚不会造成一次命令两次计费。

### 13.2 生产阶段

- 每个 upstream 将 `provider_call` 改回 `legacy`；
- 字段默认 legacy，旧 config 加载行为不变；
- 无 DB migration，无需回滚数据；
- rollout 期间 receipt/admin 只记录 engine enum 和 stable error code，用于比较失败率与
  latency，不记录内容；
- 若发现 cancellation、quota、proxy、error mapping 或 duplicate-send 任一回归，立即把
  受影响 upstream 切回 legacy，不扩大 canary；
- 只有覆盖矩阵全部迁移并经过一个完整 release 周期后，才单独设计删除 ureq。删除不是
  本方案的自动后续动作。

## 14. 实现落点

预计触及位置如下；每个 PR 仍需保持 surgical change：

| 位置 | 计划改动 |
|---|---|
| 根 `Cargo.toml` / `Cargo.lock` | South dependencies、MSRV、locked source |
| `.cargo/config.toml` | private git fetch 策略（如需） |
| `.github/workflows/ci.yml` | GitHub App token、MSRV 1.96、compatibility gate |
| `apps/cli/Cargo.toml` | production/dev dependencies |
| `apps/cli/src/south_provider_call.rs` | host adapter |
| `apps/cli/src/main.rs` | probe transport flag 与单命令 runtime |
| `apps/cli/src/gateway.rs` | 诊断分支；生产阶段的单 attempt 接点 |
| `apps/cli/src/request_context.rs` / `cancel.rs` | 生产阶段同源 async cancellation |
| `apps/cli/src/config.rs` | 生产阶段 opt-in 字段与验证 |
| `apps/cli/src/server.rs` | 生产阶段 runtime handle 生命周期 |
| `apps/desktop/src-tauri` 与前端 | 生产阶段配置透传、控件与真实 App 验收 |
| README / 开发环境文档 | MSRV、private/public dependency、CLI 使用说明 |
| South `compatibility.json` / README | 英文兼容证据与 release 状态 |

## 15. 完成定义

### 15.1 “社区诊断接入完成”

必须同时满足：

- 社区 adapter 跑过 `south.provider-call.v1` 七用例；
- `upstream test --transport south-v1` 通过 deterministic loopback；
- 一次经 lv 明确授权的真实测试账号 probe 通过；
- secret、URL、error、cancel/deadline 与 no-double-send 安全测试通过；
- 社区仓与 South 远端 CI 均绿色；
- South compatibility 记录精确 evidence，并发布 immutable tag；
- 社区仓最终依赖该 tag，不依赖 branch；
- 代码变更已执行本地 Desktop 安装与启动验收。

### 15.2 “社区生产接入完成”

除上面条件外，还必须满足：

- 九个 quota header 的 South contract 与等价测试完成；
- direct + buffered + Bearer eligibility 内的真实生产 attempt 走 South；
- routing、fallback、health、quota、receipt 和 user-visible error 与 legacy 等价；
- client disconnect、drain、deadline 能取消真实 reqwest I/O；
- 至少一个 upstream 通过显式 opt-in canary；
- `legacy` 回滚经过真实 App 验证；
- streaming、proxy、Header/OAuth 等未迁移路径继续有明确 legacy coverage。

### 15.3 仍不能宣称的事项

即使生产非流式 canary 完成，也不能宣称：

- 所有社区 provider 已迁移；
- streaming 已迁移；
- proxy 已迁移；
- 企业版已验证；
- South 已拥有 provider runtime/WIT；
- legacy ureq 可以删除。

## 16. 与旧方案的关系

本文件只收敛“South 已实现的 provider-call v1 如何接入社区宿主”。它取代旧
`2026-08-16-token-station-south.md` 和配套验收清单中以下已过时假设：

- 首切片使用 ureq；
- 一次迁移同时覆盖 GET/POST/stream；
- 宿主先解析 `(header, value)` 再交给 South；
- 生产 `try_upstream` 可在没有 quota metadata contract 时直接切换；
- South 仓仍是 public `GlimpseEngine` 地址。

旧文档关于长期 provider runtime、WIT、generation task 和企业版迁移的内容不由本文件
替代；它们必须在各自纵切中重新对照届时已经实现的 South contract，不能把历史草案当成
当前 API。

## 17. 复盘要求

每个实施 PR 完成后回写本文件：

- 实际 commit、tag、CI evidence；
- 与本计划的偏差及原因；
- 新发现的 compatibility 限制；
- 本地 Desktop 安装与真实验收结果；
- 尚未关闭的 P0/P1/P2；
- 下一阶段是否仍满足 go/no-go 门禁。

任何阶段发现请求被重复发送、secret 在诊断中泄漏、quota 数据丢失、proxy 被绕过或
取消后 I/O 继续运行，都直接判定为 no-go，不以“后续优化”名义带入下一阶段。
