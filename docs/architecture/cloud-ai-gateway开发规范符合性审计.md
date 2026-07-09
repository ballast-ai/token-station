# V1（cloud_ai_gateway）开发规范符合性审计与改造清单

> 场景：token-station V2 开发拍板按 [GlimpseEngine/rust-coding-standards](https://github.com/GlimpseEngine/rust-coding-standards)（Axum + SQLx + DDD 的 Rust 服务规范，15 个 skill）执行。本文对 **V1**（cloud_ai_gateway 现有代码，117 个 .rs 文件，约 9.1 万行）做逐项符合性审计，产出改造清单并编排进 [升级差异分析与业务方案](./cloud-ai-gateway升级差异分析与业务方案.md) 与 [V2 开发计划](../planning/token-station-V2开发计划.md) 的分期路线。
>
> 审计日期 2026-07-08。§一–§六为结论收口；**附录 A 为逐规范详细审计记录**（核心要求 / 行级证据 / 判定 / 工作量），供改造执行时对照。

---

## 零、总体结论

- **15 项规范：1 项完全不适用**（messaging-kafka，无消息队列）、**1 项大部分不适用**（service-contracts，单体无内部服务边界）、**13 项适用**。
- 适用项中**没有一项完全符合**，但偏离性质分三类：
  1. **高危缺陷（必须立即修，多数天级）**：认证安全多项 Must 级缺失、SSE 断连计费丢失、无优雅停机、CI 无自动门禁；
  2. **结构性债务（编排进升级分期，周级）**：无 Service 层 + 七个 2600–4100 行巨型文件、错误目录体系缺失、集成测试全是 bash；
  3. **合理取舍（不强行对齐，记录理由）**：SQLite 选型、单实例内存态、OpenAI 兼容错误 shape。
- **加分项**：单元测试文化扎实（1075 个 `#[test]`）、顶级 AppError + exhaustive match 思路正确、跨 await 持锁纪律良好、基本无 println。

---

## 一、符合性总表

| # | 规范 | 符合度 | 最严重偏离 | 严重度 | 工作量 |
|---|------|--------|-----------|--------|--------|
| 1 | rust-tooling | 偏离 | CI 仅手动触发（`ci.yml:4`）；rustfmt/clippy/deny/toolchain 四配置全缺；edition 2021；计费无 overflow-checks | 中高 | 天级 |
| 2 | rust-architecture | 偏离 | 无 Service/domain 层，handler 直连 db；巨型文件：proxy_audio 4108 / token_counter 4023 / proxy_video 3888 / admin 3774 / streaming 3364 / config 2971 / dispatch 2612 行 | 高 | 周级 |
| 3 | rust-error-handling | 部分符合 | 顶级枚举 + exhaustive match ✅；但未用 thiserror（638 处手工 map_err）；`Database`/`Internal` 变体把内部错误回显给客户端；1373 处 `.unwrap()` | 中高 | 天级+ |
| 4 | rust-logging | 偏离 | `fmt().init()` 反模式（`main.rs:242`），无 JSON/OTel/config 分支；全库仅 132 处 tracing 调用、几乎无 `#[instrument]`；无 secrecy 脱敏 | 中 | 天级 |
| 5 | rust-config | 偏离 | 秘密全是明文 `String`（Stripe/Resend/admin 密码，`config.rs:315/156/185`），Debug 可泄露；无 deny_unknown_fields（TOML 拼错被静默吞）；无分层加载 | 中高 | 天级 |
| 6 | rust-database | 选型偏离 | rusqlite（运行时字符串 SQL）非 SQLx（无编译期校验）；money 用 INTEGER 微美元（SQLite 下可辩护）；无 updated_at 触发器 | 中（见 §四） | 见 §四 |
| 7 | rust-testing | 混合 | 单元测试 ✅；集成测试全是 shell（含 449KB 的 test_integration.sh），无 Rust `tests/`；**计费无 proptest 守恒不变量**（规范 mandatory）；无 mock/snapshot | 集成高 | 天—周级 |
| 8 | rust-auth-security | **高危** | 管理员密码 SHA-256 非 Argon2id 且 `==` 比较（timing 侧信道，`admin.rs:44`）；session token 明文存库（`schema.sql:14`）；cookie 缺 `Secure`/`__Host-`；**零 CSRF** | 高 | 天级 |
| 9 | rust-async-concurrency | 良好 | 持锁纪律 ✅；但 rusqlite 同步调用从未包 spawn_blocking（`busy_timeout=5s` 可阻塞执行器）；后台 loop 无取消 | 中 | 1–4 人日 |
| 10 | rust-api-design | 部分符合 | **SSE 断连不记账**（计费结算在 stream 循环之后，客户端断连即丢账，`streaming.rs:646/946`）；无 SSE 心跳与 idle/total 双超时；错误响应缺 request_id；无 Retry-After | **高** | 4–7 人日 |
| 11 | rust-microservice-runtime | 部分符合 | **无优雅停机**（无 signal/CancellationToken/with_graceful_shutdown，SIGTERM 硬切在途流与 DB 写）；无 /livez /readyz；failover 无退避 jitter；熔断只有 cooldown 近似无 half-open | 高 | 6–10 人日 |
| 12 | rust-service-contracts | 大部分 N/A | workspace 组织合理；唯一疑点：profile-analyzer 共享网关 SQLite 文件（若视为独立服务则违规，视为离线批处理工具则可接受） | 低 | 不建议改 |
| 13 | rust-error-catalog | 偏离 | 无集中错误码 catalog；响应缺稳定 `code`/`request_id`/`trace_id`；API 错误未本地化（UI i18n 质量高但不覆盖错误）；locale 用 task_local（规范反模式） | 高 | 3–6 人日 |
| 14 | rust-redis-state | 单实例合理 | RPM/轮询/熔断冷却全在进程内存，重启丢失、多副本不共享；固定窗口限流有 2× burst 边界缺陷 | 中（仅扩展时） | 单实例 0；扩展 4–6 人日 |
| 15 | rust-messaging-kafka | N/A | 无任何消息队列 | — | 0 |

---

## 二、P0 高危清单（先于一切功能开发，合计约 2–3 周）

按风险 × 成本排序，全部是天级修复：

| # | 问题 | 修法 | 来源规范 |
|---|------|------|---------|
| 1 | **SSE 断连计费丢失**——流式请求客户端断连时，`deduct_balance_and_record` 位于 stream 循环之后永不执行，已产出 token 不入账 | 结算移入独立 spawned task 或 stream drop guard | api-design |
| 2 | **认证安全五连**：admin 密码 SHA-256+`==` 比较、session token 明文存库、cookie 缺 Secure/`__Host-`、零 CSRF、无 constant-time 比较 | Argon2id + spawn_blocking；session token 哈希存储；补 cookie flags；CSRF 中间件；`subtle::ConstantTimeEq` | auth-security |
| 3 | **CI 无自动门禁**——仅 workflow_dispatch 手动触发，回归裸奔 | PR/push 触发 + fmt→clippy→test→`cargo deny`→`cargo audit` 链；补 rustfmt.toml/clippy 配置/deny.toml/rust-toolchain.toml | tooling |
| 4 | **无优雅停机**——SIGTERM 硬切在途 SSE 流与 DB 写 | `tokio::signal` + CancellationToken 根 + `with_graceful_shutdown`，health_probe loop 接线；顺手补 `/livez` `/readyz` | microservice-runtime |
| 5 | **秘密明文**——Stripe/Resend/admin 密码裸 `String`，Debug 派生可泄露 | `secrecy::Secret` 包裹 + config 补 `deny_unknown_fields` | config |
| 6 | **错误体回显内部错误**——`Database(e)` 把 rusqlite 原始错误发给客户端 | 变体改为记日志 + 返回通用消息；引入 thiserror + `#[from]` 顺带消灭 638 处 map_err | error-handling |
| 7 | **计费无 overflow-checks 与守恒测试** | `[profile.release] overflow-checks = true`；db/billing.rs 补 proptest 守恒不变量 | tooling / testing |

> P0 与功能无关、纯加固，且第 1、7 条直接关系 token-station 的**计费可信度叙事**——一个宣称「路由透明、账单可对账」的产品，自己的流式计费先丢账，是产品级矛盾。

---

## 三、编排进升级分期（与业务方案 P1–P4 合并）

| 升级期 | 搭车的规范改造 | 理由 |
|--------|--------------|------|
| **P0（新增加固期）** | 上表 7 项 | 先于功能，独立可交付 |
| **P1（决策链路 + BYOK）** | ① 错误响应补稳定 `code` + `request_id`/`trace_id`（error-catalog 最小改造）——与决策链路是**同一条纵切**，响应头 `X-Routed-*` 与 request_id 一次埋好；② 日志重写 `init_tracing`（JSON/config/OTel guard），关键路径补 `#[instrument]`——decision trace 的可观测底座 | 同一刀切两件事，避免二次开膛 |
| **P2（质量侧路由）** | ① 路由改造必然重写 routing.rs/dispatch.rs/retry.rs → 顺势引入 Service 层雏形（routing domain 先行示范 DDD 切片）；② failover 补退避 + jitter，熔断升级为 open/half-open 状态机（语义化 fallback 本来就要区分失败类型）；③ rusqlite 热路径 spawn_blocking 评估 | 功能重写与规范重构重叠度最高的一期 |
| **P3（多租户 + Enterprise）** | ① **rusqlite → SQLx 迁移**（见 §四，多租户 schema 大改是最佳迁移窗口）；② Redis 外置（RPM 滑动窗口/全局轮询/共享熔断）——本来就是 Enterprise 多副本的必做项；③ 巨型文件拆分随多租户触及逐个执行（admin.rs 必拆）；④ 集成测试逐步 bash → Rust `tests/`（license 分档需要矩阵化测试，bash 撑不住） | 规范要求与 Enterprise 档需求完全同向 |
| **P4（Preset + 学习型路由）** | 错误目录 catalog 完整化 + API 错误 Fluent 本地化（五语言产品的收尾一致性） | 低优先收尾 |

---

## 四、三个不强行对齐项（拍板记录）

1. **SQLite 不是错误，但 rusqlite 要换成 SQLx**。规范预设 PG+SQLx；网关用 SQLite 与「单二进制静态部署」的 Team 档定位一致，保留。但 **SQLx 本身支持 SQLite 后端**：迁移到 SQLx 可同时拿到编译期 SQL 校验（规范核心诉求）、原生 async（解决 spawn_blocking 问题）、以及 **P3 Enterprise 档 Postgres 支持的同一套代码路径**——一次迁移满足三个需求。建议 P3 执行，迁移窗口与多租户 schema 改造合并。
2. **bash 集成测试不全废**。449KB 的 test_integration.sh 是既有资产，覆盖真实上游对账等 Rust 难以复现的场景；口径：新功能一律写 Rust `tests/`（testcontainers），存量 bash 只迁移计费与路由核心断言，其余维持到自然淘汰。
3. **OpenAI 兼容错误 shape 保留**。不改 `{"error":{...}}` 外形（客户端 SDK 依赖它），按规范「边缘兼容例外」在对象内**加**稳定 `code` + `request_id`，不做整体替换。

## 五、两个待拍板问题

1. **profile-analyzer 的定位**：若按升级方案 §三「平台形态默认下线、私有部署自用」执行，共享 DB 文件可接受（离线批处理工具）；若未来升级为独立服务，必须改走查询接口，禁止直读网关库。
2. **spawn_blocking 的范围**：P3 若确定迁 SQLx（async），P2 只需对已知重写路径（billing 结算）做 spawn_blocking，不值得全量包裹后又推翻。

---

## 六、与既有文档的同步

| 文档 | 同步内容 | 状态 |
|------|---------|------|
| [升级差异分析与业务方案](./cloud-ai-gateway升级差异分析与业务方案.md) | 分期路线新增 P0 加固期；P1–P4 各期并入本文 §三的搭车项 | 已回写（2026-07-08） |
| [V2 开发计划](../planning/token-station-V2开发计划.md) | 重构项的排期编排与待拍板收口 | 已并入（2026-07-08） |
| [cloud-ai-gateway 功能清单](../research/cloud-ai-gateway-自研网关功能清单.md) | 能力现状依据，不变 | 无需同步 |

---

# 附录 A：逐规范详细审计记录

> 以下 15 项按规范逐条给出：核心要求、V1 现状（源文件行级证据）、判定与严重度、改造工作量。行号基于审计时（2026-07-08）的代码树。

## A.1 rust-tooling（工具链）

**核心要求**：edition 2024；binary crate 提交 Cargo.lock；提交 rustfmt.toml / clippy 配置 / deny.toml / rust-toolchain.toml（钉死精确版本）；CI 顺序 fmt → clippy(-D warnings) → test → cargo deny → cargo audit 且 fail-fast；计费/计量代码 `overflow-checks = true`；应用代码零 unsafe。

**V1 现状**：
- `Cargo.toml:4` `edition = "2021"`（落后一代）；Cargo.lock 已提交 ✅
- rust-toolchain.toml / rustfmt.toml / clippy.toml / deny.toml **全部不存在**；`.cargo/config.toml` 为空
- `.github/workflows/ci.yml:4` `on: workflow_dispatch` —— **CI 仅手动触发，PR 不自动跑**；有 fmt/clippy(-D warnings)/test 三步，无 cargo deny / cargo audit / nextest
- 工具链用 `dtolnay/rust-toolchain@stable`（版本漂移风险）
- 无 `[workspace.lints]`；无 release overflow-checks（计费代码 db/billing.rs 存在）
- wasmedge-sdk 走 git tag 依赖（`Cargo.toml:71`），供应链审计缺失下风险放大

**判定：偏离，中高。工作量：天级**（四配置 + CI 改造 + `cargo fix --edition` 迁移）。

## A.2 rust-architecture（架构分层）

**核心要求**：DDD 分层 `core/ infra/ shared/ modules/<name>/{domain,service,repo,handler}` + `server/state.rs`；Handler 必须过 Service 层不得直连 Repo；业务代码禁 `std::env::var()`；函数签名约定（&str vs String、4+ 参数用 builder）。

**V1 现状**：
- 实际为按技术类型的扁平分层：`routes/`（handler）+ `proxy/` + `db/` + `middleware/`；**无 Service/domain 层**，handler 直连 db（如 `routes/auth.rs:73` `state.db.get_user_by_session_token`）
- 巨型文件：`routes/proxy_audio.rs` 4108 行、`proxy/token_counter.rs` 4023 行、`routes/proxy_video.rs` 3888 行、`routes/admin.rs` 3774 行、`proxy/streaming.rs` 3364 行、`config.rs` 2971 行、`proxy/dispatch.rs` 2612 行
- `std::env::var` 业务代码 11 处（config 层集中读取基本合规；`db/bench.rs` 违规）
- 无 `#[from]` 自动转换 → 全库 638 处手工 `map_err`

**判定：偏离，高（结构性债务）。工作量：周级**（Service 层 + 拆巨型文件；全面 DDD 化为月级，不建议一步到位）。

## A.3 rust-error-handling（错误处理）

**核心要求**：全层 thiserror，顶级 AppError 枚举；`IntoResponse` 单个 exhaustive match；`#[from]` 生成转换；不向客户端暴露内部错误；生产代码禁 unwrap。

**V1 现状**：
- `src/error.rs` 手写 enum AppError（20 变体）+ 手写 Display + 手写 IntoResponse；**未用 thiserror**（Cargo.toml 无 thiserror/anyhow）
- `error.rs:85-188` IntoResponse 是单个 exhaustive match ✅（编译期强制新变体补映射），日志分级有单测（`error.rs:221-335`）✅
- **`AppError::Database(e)` 把数据库原始错误放进响应体**（`error.rs:146-150`）；`Internal`/`UpstreamError.body` 同样回显 —— 违反「不暴露内部错误」
- 生产代码 1373 处 `.unwrap()` + 228 处 `.expect()`

**判定：部分符合，中高。工作量：天级+**（thiserror + `#[from]` 消灭 638 处 map_err；unwrap 清理为持续性工作）。

## A.4 rust-logging（日志）

**核心要求**：tracing + tracing-subscriber + OTel；prod JSON / dev pretty 由 config 选择；单一 init_tracing 返回 guard；禁 println/eprintln；Service 方法 `#[instrument(skip(self), err)]`；敏感数据 secrecy 脱敏。

**V1 现状**：
- `main.rs:242-247` `tracing_subscriber::fmt().with_env_filter(...).init()` —— 规范明确点名的反模式：无 JSON 分支、无 config 选择、无 OTel、无 guard
- tracing 调用全库仅 132 处（9.1 万行代码，覆盖面薄）；println/eprintln 仅 9 处 ✅
- 几乎无 `#[tracing::instrument]`；多为字符串插值而非结构化字段（`error.rs:175`）
- 无 secrecy 依赖，秘密字段全明文 String；无 trace_id/request_id 注入

**判定：偏离，中。工作量：天级**（重写 init_tracing；instrument 铺开为持续性工作）。

## A.5 rust-config（配置）

**核心要求**：四层加载（defaults → TOML 按 APP_ENV → env → CLI）；sub-config 派生 Validate + `#[serde(deny_unknown_fields)]`；秘密用 Secret<String> 不入日志、Debug 不泄露；业务代码零 env::var。

**V1 现状**：
- `config.rs:1362` `AppConfig::load(path)` 只读单个 TOML + `resolve_env_vars()` + 手写 `validate()`（`config.rs:1413`，功能上可接受）；无分层合并 / APP_ENV / CLI override
- **无 deny_unknown_fields**——TOML 拼写错误被静默忽略
- **秘密明文**：`StripeConfig.secret_key: String`（`config.rs:315`）、`webhook_secret`、`EmailConfig.resend_api_key`（`config.rs:156`）、`AdminConfig.password`（`config.rs:185`），`#[derive(Debug)]` 可泄露
- 正向：env 读取集中在 config 层，业务代码基本不散读

**判定：偏离，中高。工作量：天级**（secrecy 包裹 + deny_unknown_fields 为要害项；完整分层加载偏上）。

## A.6 rust-database（数据库）

**核心要求**：PostgreSQL 15+ + SQLx 编译期校验 + SQLx CLI 迁移；snake_case 复数表名、`idx_/uniq_` 索引；money 用 NUMERIC（BIGINT cents 仅限单币种永久+无 FX）；每表 created_at/updated_at（trigger 维护）；ledger 表 append-only（REVOKE 强制）。

**V1 现状**：
- **技术栈整体偏离**：rusqlite（SQLite）+ r2d2（`Cargo.toml:40`），运行时字符串 SQL 无编译期校验（如 `db/users.rs:106`）
- 迁移：`db/migrations.rs` 以 `include_str!` 内嵌 57 个迁移，序号前缀（非时间戳）——自洽的等价做法，可接受
- **money 全 INTEGER 微美元**（`schema.sql:10` balance、`:48` total_cost、`:64` amount）；SQLite 无 NUMERIC，微美元定点在此约束下可辩护，但严格对照仍是偏离
- 命名合规 ✅（复数表名、idx_ 前缀）；无 updated_at 触发器；`balance_transactions` ledger 无法在 SQLite 上 REVOKE 强制 append-only

**判定：选型层面高偏离 / SQLite 内评估为中。工作量**：迁 PG+SQLx 为月级（与 V2 P3 多租户窗口合并执行，SQLite 后端先行）；SQLite 内改进（updated_at 触发器、money 精度审计）为天级。

## A.7 rust-testing（测试）

**核心要求**：单元测试在源文件 `#[cfg(test)]`；集成测试为 Rust `tests/` + testcontainers；HTTP 用 wiremock、trait 用 mockall、快照 insta；money 函数强制 proptest 守恒不变量；时间用 start_paused；coverage 用 llvm-cov。

**V1 现状**：
- **单元测试扎实**：1075 处 `#[test]` + 95 处 `#[tokio::test]` + 77 个 `#[cfg(test)] mod`，覆盖 error 映射、config 校验、credential round-trip —— 最亮眼的合规面 ✅
- **集成测试全是 shell**：tests/ 目录零 .rs 文件，只有 test_billing.sh / test_audio.sh / test_cli_agents.sh；根目录 **test_integration.sh 449KB** 的黑盒 bash，不进 cargo test、无类型安全
- 无 mockall/wiremock/insta/proptest（dev-deps 仅 tempfile 等）；**计费无 proptest 守恒测试**（规范 mandatory）；无 coverage 配置

**判定：单元测试符合（低）/ 集成与工具链高偏离。工作量：天—周级**（核心断言迁 Rust + 计费 proptest 先行；bash 全迁为周级，不建议全废）。

## A.8 rust-auth-security（认证安全）——最高危

**核心要求**：密码 Argon2id + PHC + spawn_blocking；API key/token ≥256bit OsRng + 前缀 + SHA-256 存储 + constant-time 比较；session opaque + 哈希存储 + 轮换 + 双过期；cookie `__Host-` + Secure + HttpOnly + SameSite；cookie 认证必须 CSRF；防枚举。

**V1 现状**：
- API key SHA-256 哈希存储 ✅（`middleware/auth.rs:7,34`）；但查询走 SQL 等值匹配，无 constant-time（无 subtle 依赖）
- **管理员密码**：`admin.rs:28` cookie 值 = sha256(admin_password)，密码明文存 config；登录比较 `admin.rs:44` 用 `==`（**timing 侧信道**）；**SHA-256 非 Argon2id**
- **session token 明文存库**（`schema.sql:14` session_token TEXT；`db/users.rs:251` `WHERE session_token = ?1`）——DB 泄露即会话全盗
- **cookie 缺 Secure / `__Host-`**（`auth.rs:216` 仅 Path/HttpOnly/SameSite=Lax/Max-Age）
- **全库 CSRF 命中 0 处**——cookie 认证的 dashboard/admin 状态变更端点无 CSRF 保护
- 无设备码流（V2 需求，P3 新建）；无登录锁定/防枚举

**判定：偏离，高危。工作量：天级**（P0 全部修复）。

## A.9 rust-async-concurrency（异步并发）

**核心要求**：禁跨 .await 持锁；spawn 必须可控（JoinSet/abort/记名 fire-and-forget）；CancellationToken 根植 main；通道有界；同步 DB 驱动 / >1ms 阻塞用 spawn_blocking。

**V1 现状**：
- **持锁纪律良好** ✅：`rpm_tracker.rs:38,53` tokio Mutex 临界区无 await（注释 L14-25 自陈权衡）；`routing.rs:159-164` 克隆 Arc 后锁外 fetch_add（规范推荐模式）；`credential_store.rs:74,193-200` 同步 RwLock + 防中毒封装 + snapshot 克隆
- 生产裸 spawn 极少（health_probe、token_refresh.rs:1078），`health_probe.rs:71-99` fire-and-forget 有界并发（for_each_concurrent=8）、错误被吞不扩散 ✅；**但无 CancellationToken，无法优雅停机**
- **rusqlite 同步调用从未包 spawn_blocking**（全库 0 处）：`db.rs:126` conn() 同步 checkout 直接在 async handler 调用；`busy_timeout=5000`（db.rs:81-88）意味着写争用时最长阻塞执行器线程 5 秒

**判定：良好中带一处实质偏离（spawn_blocking），中。工作量：1–4 人日**（P3 SQLx 原生 async 根治；P2 仅 billing 热路径过渡处理）。

## A.10 rust-api-design（API 设计）

**核心要求**：/v1/ 版本前缀、正确状态码、列表游标分页、统一错误 shape（code/request_id/trace_id）、SSE 心跳保活 + 断连取消上游并记录已产出用量 + idle/total 双超时 + 显式终结事件、OpenAPI。

**V1 现状**：
- URL/版本/命名 ✅（OpenAI/Anthropic 兼容 wire 属规范允许的边缘兼容例外）
- 状态码较细致（400/402/403/401/404/429/503/504 分明），但 ModelNotFound→400（`error.rs:122-126`）、429 无 Retry-After、无 409/412、POST 无 Location
- 错误 shape `{"error":{"type","message"}}`（`error.rs:180-187`）**无 request_id/trace_id/稳定 code**
- **SSE 三连缺陷**：无网关自身心跳 keep_alive；**计费结算位于 stream 循环之后（`streaming.rs:646-656`、`946-956`），客户端断连 → future 被 drop → 已产出 token 永不入账**；无 idle/total 双超时（仅 client read_timeout 120s，`main.rs:298`）
- 请求体上限全局 100MB（`mod.rs:41`）偏大；无 OpenAPI/utoipa

**判定：SSE 断连不记账为高危（计费正确性）；其余中低。工作量：4–7 人日**（SSE 修复进 P0，request_id/code 进 P1 决策链路同刀）。

## A.11 rust-microservice-runtime（运行时）

**核心要求**：出站全超时；重试仅幂等 + 指数退避 jitter + 预算封顶；每上游熔断器（open/half-open）；SIGTERM 优雅停机全链路；/livez /readyz；出站错误 typed 映射；写端点 Idempotency-Key。

**V1 现状**：
- **出站超时覆盖完善** ✅：非流式 upstream_total_timeout（dispatch.rs:462,495；retry.rs:267），流式 connect 10s/read 120s（main.rs:295-299），设计注释详尽（main.rs:281-294）
- 重试/failover：按 channel 链 + max_retries 封顶 ✅；**但失败后立即打下一个 channel，无退避无 jitter**（retry.rs:425-446，grep 无 sleep/backoff）——failover 换目标语义下危害低于同目标重打，仍属偏离
- 熔断：`routing.rs:147` cooldown map 是轻量近似，**无 open/half-open 状态机与错误率阈值**
- **无优雅停机**：`main.rs:341-352` 直接 serve().await，无 signal/CancellationToken/with_graceful_shutdown——SIGTERM 硬切在途 SSE 与 DB 写
- **无 /livez /readyz**；无 API 层 Idempotency-Key（Stripe 内部幂等除外）
- 错误映射集中（`error.rs:198-208` timeout→504、传输错→503）✅；Outbox/Saga/mTLS 等分布式条款对单体**不适用**

**判定：无优雅停机为高；退避/熔断/健康端点/幂等键为中。工作量：6–10 人日**（停机+健康端点进 P0；退避+熔断状态机进 P2 容灾升级）。

## A.12 rust-service-contracts（服务契约）

**核心要求**：workspace 多 crate 角色分离；跨服务经生成客户端；schema 源控 + CI 破坏性检测；URL/package 级版本化；标准请求头传播；契约测试。

**V1 现状**：单体网关无内部服务间 RPC 边界，绝大部分条款**不适用**。workspace 组织（根 crate + profile-analyzer + wasm_modules/gateway_sdk）边界清晰、注释详尽，符合精神。唯一疑点：**profile-analyzer 直读网关 SQLite 文件**（其 Cargo.toml 注释自陈 "shares the gateway's DB FILE"）——若定位为独立服务则违反「禁止直读他服务库」，若定位为单体的离线批处理工具则可接受。

**判定：符合精神，低。工作量：当前不改**；若 profile-analyzer 升级为独立服务，需改查询接口（中偏高）。

## A.13 rust-error-catalog（错误目录与 i18n）

**核心要求**：集中错误码注册（`<DOMAIN>_<REASON>` 全库唯一 + CI 检查）；统一响应 `{code, message, request_id, trace_id, details}`；Fluent i18n、message key 匹配 error code、每 locale 覆盖 CI；locale 边缘解析一次入 RequestContext。

**V1 现状**：
- 无 catalog 注册表、无唯一性检查、无 code→status→message_key 单表；错误分类仅十几个 `error_type` 字符串（`error.rs:88-171`）
- 响应缺 code/request_id/trace_id/details；无 debug_message prod 剥离
- i18n（`src/i18n/mod.rs`）为「源字符串即 key」的 UI 文案本地化（zh-CN/zh-TW/ja/ko + 完备性测试），工程质量高 ✅，**但 API 错误响应完全未本地化**；locale 用 `tokio::task_local!`（规范反模式，对服务端渲染 UI 影响小）

**判定：catalog 与响应 shape 偏离高（受 OpenAI 兼容 shape 约束，属边缘例外但例外要求的 code+request_id 也未满足）。工作量**：最小改造（对象内补 code/request_id）1–2 人日进 P1；完整 catalog + Fluent 3–6 人日进 P4。

## A.14 rust-redis-state（Redis 运行时状态）

**核心要求**：Redis 为可重建镜像非真相源；限流滑动窗口/桶计数 + limit/remaining/reset 返回；key TTL 注册表；Lua 原子。

**V1 现状**（无 Redis，评估内存态）：
- RPM tracker（`rpm_tracker.rs:40-53`）：进程内**固定 60s 窗口**（边界 2× burst 缺陷，L119-123 整窗重置），注释 L27-32 自陈单实例取舍
- round-robin 计数（`routing.rs:135`）与熔断冷却（`routing.rs:147`）均进程内——多副本不共享（一个副本探测到上游挂，其他副本照打）
- 真相源正确 ✅：余额/用量在 SQLite，未用内存态存钱；限流响应无 limit/remaining/reset 头

**判定：单实例架构下合理取舍，非硬偏离；水平扩展时成为硬伤（中）。工作量**：单实例 0；Enterprise 多副本时 4–6 人日（P3 #6）。

## A.15 rust-messaging-kafka（消息队列）

全库 grep kafka/rdkafka/nats/rabbitmq/amqp/outbox/broker **零命中**——无任何消息队列。**完全不适用（N/A）**，无改造工作量。
