# `token-station-south` 最小 Provider Call 纵切实施记录

> 状态：south 独立仓的 library slice 已在本地 feature 分支实现并完成审查；远端 CI、PR 合入和真实宿主接入尚未完成。
>
> 英文设计：`token-station-south/docs/design/2026-08-16-minimal-provider-call.md`
>
> 更新日期：2026-08-17

## 1. 本次交付结论

`token-station-south` 已不再只是建仓骨架。当前纵切实现了一条可运行但尚未接入真实宿主的南向调用链：宿主绑定 provider endpoint 和 credential slot，provider 只能提交受限相对路径、普通 header 和有界 JSON body；south 在绑定校验通过后解析宿主凭证，再通过加固的异步 reqwest transport 发出一次 buffered JSON POST。

这仍然只是 library slice，不代表企业版已经可以迁移，也不代表生产就绪。`token-station` 和 `token-station-server` 的兼容状态继续保持 `not_verified`。只有真实宿主 adapter 编译通过、接入真实调用点并运行公开 assembled-executor conformance suite 后，才能改变对应状态。

## 2. 已实现范围

| 层 | 已实现能力 |
|---|---|
| `south-contracts` | HTTP/Auth/Error Rust 契约 v1；endpoint、相对路径、credential slot、JSON body、请求 header、响应 body 和两项响应元数据的显式上限；stream 版本仍为 `null` |
| `south-core` | endpoint 与唯一 credential slot 绑定；异步凭证解析能力；零化并脱敏的 secret；sealed prepared request；覆盖 resolver 与 transport 的绝对 deadline 和 cancellation |
| `south-transport-reqwest` | reqwest `=0.12.28`；关闭默认 feature；只启用 `rustls-tls` 与 `stream`；禁用隐式代理、重定向、重试、压缩、Cookie 和 Referer；有界流式响应读取与稳定错误分类 |
| `south-provider-conformance` | 固定的 `south.provider-call.v1` 七用例 fixture，覆盖成功、非法路径、credential slot 不匹配、重定向拒绝、响应超限、取消和 deadline |
| `south-testkit` | 面向 assembled executor 的公开 runner、完整 mismatch 报告和基于真实 `south-core` 的 reference executor |
| 工程门禁 | parser property tests 与 fuzz target；reqwest 所属 crate、精确版本和 feature 集合门禁；宿主、数据库、缓存和迁移依赖门禁 |

本阶段仍不实现 streaming、SSE、multipart、provider WIT、Wasmtime runtime、ureq transport、retry/fallback/routing、计费、配额、审计、任务持久化或真实宿主 adapter。

## 3. 安全与数据边界

- south 不读写数据库，也不引入数据库、缓存或 migration 依赖。数据库仍然完全属于宿主。
- south 不读取环境变量、配置文件、keychain、Vault 或宿主 secret store。宿主通过显式 `CredentialResolver` 能力提供凭证。
- provider 只能选择宿主已绑定 endpoint 下的受限相对路径，不能提交 absolute URL、userinfo、query、fragment、dot segment、encoded separator 或越过 base path 的路径。
- credential slot 校验和 URL containment 校验发生在 secret resolution 之前；失败时 resolver 和 transport 都不得被调用。
- `SecretValue` 自身分配在 drop 时清零，不实现 `Clone`、`Display` 或 serde，并使用脱敏 `Debug`。reqwest transport 对其持有的 Authorization header owner 也执行清零；该保证不虚假延伸到 TLS、内核或上游基础设施缓冲区。
- 请求和响应 body 上限均为 32 MiB。JSON 验证不构建完整 DOM；请求体通过共享 owner 交给 reqwest，避免再深拷贝一份 32 MiB 数据。
- 所有 3xx 都返回 `REDIRECT_DENIED`，不会发出第二次请求；4xx 和 5xx 保留为正常 transport outcome，供未来 provider adapter 解释。
- URL 校验不能单独解决 DNS rebinding 或私网 SSRF。生产宿主仍必须提供 endpoint 授权、网络 egress policy 或受控代理。

## 4. TDD、审查与本地验收

实现按设计、公开行为测试、代码、全量门禁的顺序推进。关键 RED 包括缺失契约 API、空 userinfo 和重复 slash 绕过、规范化后 endpoint 超限、3xx 可被构造成成功响应、缺失 core 编排接口、reqwest transport 未实现、依赖门禁可绕过，以及 conformance deadline 通知竞态。

最终本地结果：

- workspace `nextest`：99/99 通过。
- fmt、Clippy `-D warnings`、doctest、all-features、no-default-features、rustdoc `-D warnings` 和 Rust 1.96.0 MSRV 检查通过。
- 根 workspace 与 fuzz workspace 的 deny、audit、machete 通过。
- fuzz locked compile、boundary self-test、真实 dependency boundary 和 actionlint 通过。
- contracts、core、reqwest transport、conformance/testkit 均完成独立规格审查和代码质量审查，最终无未关闭 P0/P1/P2。
- south feature 分支的英文状态同步提交为 `3a8c525`；尚未 push，远端 CI 尚未运行。

## 5. 与当前 Token Station 仓的关系

本次只改 south 独立 library 仓及本中文实施记录，没有改变 Token Station 的可执行行为、UI、状态模型或发布行为，因此不运行 `scripts/install-local-desktop.sh`。

当前仓已有的 south 总方案和验收清单仍是迁移规划输入，但其中若有“宿主先解析明文 header/value 再交给 south”或“transport 尚未实现”等历史描述，应以 south 英文最小纵切设计和本实施记录为准。当前实际边界是宿主注入 async credential resolver，south 只在绑定校验后解析 secret，并在 transport 最后一刻构造 Bearer header。

## 6. 下一步门禁

1. 对 south feature 分支运行最终全量门禁和独立全 diff 审查。
2. push feature 分支并创建 PR，等待远端 CI 通过；不能绕过 `main` 分支保护。
3. 单独启动企业版 host-adoption 纵切，选择一个真实 Bearer JSON POST 调用点，实现宿主 adapter，并运行同一 `south.provider-call.v1` suite。
4. 只有企业 adapter 的真实编译、运行和 wiring 审查通过后，才把 `token-station-server` 从 `not_verified` 改为已验证；社区版同理独立验收。
