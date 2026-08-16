# `token-station-south` 独立仓库建仓设计

> 状态：已批准执行建仓纵切；本文的依赖反转裁决优先于主设计和验收清单中的历史旧方案。
>
> 关联文档：`2026-08-16-token-station-south.md`、`2026-08-16-token-station-south-acceptance-checklist.md`
>
> 更新日期：2026-08-16

## 1. 问题与决策

企业版迁移到统一 provider 插件执行链的前置条件，是先有一个能被社区版和企业版独立消费的 south 仓。等待企业版先成为第二个消费者会形成循环依赖，因此本阶段直接建立公开独立仓，并用最小可验证纵切固定依赖方向和安全边界。

仓库决定如下：

- 仓库：`GlimpseEngine/token-station-south`，公开，Apache-2.0。
- 新仓的代码、注释、提交约定和全部文档使用英文；跨仓决策、迁移状态和中文说明保留在当前 `token-station` 仓。
- Rust profile 是“共享库 + 插件包/运行时”。团队规范以相邻仓 `rust-coding-standards-glimpse` 为权威输入。
- south 不依赖 `token-station` 或 `token-station-server`。两个宿主未来都只能依赖 south，不能形成双向 git 依赖。
- 首次提交建立可编译 workspace、安全契约纵切和 CI 门禁，但不伪装成已经可用于生产迁移。

## 2. 目标与范围

本次建仓目标：

1. 建立英文、公开、可独立构建的 Rust workspace。
2. 预留已批准的模块边界：`south-contracts`、`south-core`、两种 transport、provider API/runtime/conformance、testkit 和离线 migration 工具。
3. 用首个 TDD 纵切实现 host-owned 凭证边界：provider 普通 header 集合不能携带版本化 reserved header；认证声明、宿主解析和 transport 注入使用不同契约，首期不提前固化明文 auth 公开 API。
4. 用自动化门禁证明仓库不依赖宿主、数据库、缓存或宿主凭证实现。
5. 固定 Rust 工具链、格式化、lint、测试、依赖许可和安全审计基线。

本次不实现：

- 完整 `ProviderCall`、`ProviderTask`、WIT world 或 Wasmtime runtime。
- 真实 ureq/reqwest 网络调用、重试、fallback、routing、admission、billing、quota 或 audit persistence。
- token-station 或 token-station-server 的调用方迁移。
- crates.io 发布、正式稳定版本或生产可用性声明。
- UI、桌面 App 或用户交互变更；因此响应式、键盘和无障碍要求不适用。

## 3. Workspace 边界

初始 workspace 包含：

| crate | 本阶段职责 |
|---|---|
| `south-contracts` | provider-facing canonical types 与首个安全 header/auth 契约 |
| `south-core` | host-neutral orchestration 边界，占位但可编译 |
| `south-transport-ureq` | 同步 transport 边界，占位但不发网络请求 |
| `south-transport-reqwest` | 异步 transport 边界，占位但不发网络请求 |
| `south-provider-api` | provider ABI/WIT 所有权边界，占位 |
| `south-provider-runtime` | host runtime 所有权边界，占位 |
| `south-provider-conformance` | provider 契约验证边界，占位 |
| `south-testkit` | 对外契约测试边界，占位 |
| `south-migration` | 离线 fixture diff 工具边界；本阶段只建 library crate，暂不形成可执行发布物 |

这里刻意不先造九套 API。除 `south-contracts` 的首个安全纵切外，其余 crate 只用 crate-level 英文文档声明边界，后续每个行为都必须先有独立设计和失败测试。

## 4. 安全与数据红线

south 必须满足：

- 不直接读写数据库，不引入 `sqlx`、`rusqlite`、`diesel`、Redis client 或迁移文件。
- 不读取环境变量、配置文件、系统 keychain、Vault 或宿主 secret store。
- 普通 header 集合拒绝一份明确、版本化的 reserved-header 集合；首版覆盖认证头，以及 `host`、`content-length`、`transfer-encoding`、`connection` 等 framing/hop-by-hop 头。这不是“识别所有凭证”的黑名单；未知认证形态必须通过独立 `AuthDeclaration` 扩展，而不能塞进普通 header。
- 普通 header 集合限制为最多 64 个、名称 256 字节、单值 16 KiB、总计 64 KiB；累加使用 checked arithmetic。错误和 `Debug` 均不得输出不可信 header 名或值，防止插件把内容编码进合法名称后借日志外泄。
- 不记录或格式化 secret、token、API key、请求正文或响应正文。
- 不包含 dispatch、跨 upstream retry、租户、账单、配额账本、任务持久化和审计落库。
- 不依赖 `token-station-*` 或 `token-station-server-*` package。
- 插件默认无网络、文件系统和密钥权限；将来新增 WIT/runtime 时必须有 import allowlist、fixture、conformance、checksum/signature 和 SBOM 门禁。
- 生产代码禁止 `unwrap`/`expect`，错误使用 `thiserror` typed enum，开发者错误消息使用英文。
- 默认禁止 `unsafe`；任何例外必须隔离、写 `// SAFETY:`，并单独批准。

宿主负责：解析 `SecretRef`、构造受控 auth material、授权 endpoint、持久化、计费、配额、审计、全局 tracing 初始化和取消根节点。south 只消费显式注入的能力。

## 5. Rust 工程规范落地

依据 `rust-coding-standards-glimpse`，建仓时落地：

- edition 2024、MSRV 1.96、`rust-toolchain.toml` 精确固定 1.96.0。
- `rustfmt.toml`：edition 2024、100 列。团队规范建议的 `imports_granularity` / `group_imports` 在 Rust 1.96 stable 仍是 unstable，与同一规范“生产禁止 nightly”冲突，因此以 stable toolchain 为高优先级约束，明确不启用这两项。
- workspace lint：Clippy correctness/all/suspicious deny，pedantic/nursery warn；CI 使用 `-D warnings`。
- `resolver = "2"`，共享依赖集中在 `[workspace.dependencies]`，版本不使用通配符。
- 根 workspace 首期全是 library crate，因此不提交 `/Cargo.lock`；一旦 migration 形成 binary，必须改为提交根 lockfile。
- 独立的 fuzz binary workspace 必须提交 `fuzz/Cargo.lock`，并在 PR 中单独执行 compile、deny、audit、machete 和 boundary 检查；根 library workspace 仍不提交 `/Cargo.lock`。
- feature 必须 additive；CI 同时验证 all-features 与 no-default-features。
- `cargo nextest`、doctest、`cargo deny`、`cargo audit`、`cargo machete` 进入 CI；对不稳定外部 advisory 服务的依赖不得削弱本地 fmt/clippy/test 硬门禁。
- 纯 library 不初始化 tracing、不读取配置；后续 transport/runtime 在 operational boundary 发出结构化英文事件，subscriber 由宿主安装。
- 异步 transport 使用 Tokio 时，所有长任务必须显式接受 cancellation/deadline；禁止锁跨 `.await`、丢弃 `JoinHandle` 和无界 channel。
- 产品构建和常规 CI 只使用 1.96.0 stable；定时 `cargo-fuzz` 因 sanitizer 的 `-Z` 参数单独固定 `nightly-2026-08-15`，不改变库的 stable MSRV，也不允许 nightly 特性进入产品代码。

## 6. 首个公开行为测试

先写 `south-contracts` 集成测试并确认 RED，再实现最小代码使其 GREEN：

1. 普通、规范化后的非敏感 header 可以进入 `SafeHeaders`。
2. header 名大小写不影响 reserved-header 检测。
3. reserved header 被拒绝，并返回可穷尽匹配的 typed error code；错误内容不得包含 header value。
4. header 数量、名称、单值与总字节超限均失败关闭，错误和 `Debug` 不包含输入名称或值。
5. 字段保持私有，不提供未经校验的 `FromIterator`/`Extend`/反序列化入口。
6. 首期只承诺内存行为，不开放 serde wire schema；至少用属性测试覆盖 header 名大小写变体与“敏感值不出现在错误中”的不变量。

本阶段不写真实 HTTP mock，因为还没有 transport 行为；用空测试假装网络层已完成属于错误验收。

## 7. CI、发布与验收

本地和 GitHub Actions 至少执行：

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo nextest run --workspace --all-features
cargo test --workspace --doc --all-features
cargo test --workspace --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
rustup run 1.96.0 cargo check --workspace --all-targets
cargo deny check
cargo audit
cargo machete
boundary dependency check
```

建仓验收标准：

- 所有文件和公开 API 文档为英文。
- 首个契约测试有可追溯的 RED/GREEN 记录，最终全部通过。
- dependency boundary 脚本拒绝宿主、数据库、缓存依赖。
- README 明示当前是 bootstrap、尚未生产就绪。
- 本地所有可用门禁通过；因本机缺少工具或远端 advisory 网络导致的未执行项必须如实记录。
- 创建本地英文初始提交，推送公开 GitHub 仓并核对默认分支和 CI。
- 当前 `token-station` 仓回写中文实现状态和验收结果，并按本仓约定使用中文提交说明单独提交；不得把用户已有的无关改动混入。

## 8. 已知遗留与下一纵切

下一阶段不是直接搬运现有 `apps/cli` 代码，而是先定稿：

1. `south-contracts` 的 Canonical IR、HTTP、Auth、Stream 与 Error 版本策略。
2. transport 的同步/异步能力对齐、deadline/cancellation、响应大小和 egress 授权契约。
3. provider WIT 的二进制兼容策略与公开 conformance fixtures。
4. 两个真实宿主分别编译消费 south 的 release gate，防止 Cargo package identity 和发布顺序再次形成环。

只有这些纵切通过后，才开始 token-station 和企业版的真实迁移。

## 9. 实施状态与验收结果

状态：2026-08-16 已完成建仓 bootstrap，但尚未进入真实 transport、provider runtime 或宿主迁移阶段。

- 公开仓库：`https://github.com/GlimpseEngine/token-station-south`
- bootstrap 提交：`0321b84a5f03c439bd643b3cfa0f5ca5cf142dff`
- 远端 CI：`https://github.com/GlimpseEngine/token-station-south/actions/runs/31953189715`，`quality` job 全部通过；定时 fuzz job 在普通 push 上按设计跳过。
- `main` 已启用严格分支保护：必须通过 `quality`，要求 1 个 CODEOWNER 审批、最后一次推送后重新审批、解决所有对话并保持线性历史；禁止强推和删除。管理员不强制，以保留新仓配置故障时的恢复通道。
- south 仓全部代码和文档均为英文；中文决策、实施记录和验收结果保留在当前仓。

实际交付包括九个 workspace crate、架构和安全文档、版本化兼容性清单、依赖边界门禁、GitHub Actions、Dependabot、CODEOWNERS、根 workspace 与 fuzz workspace 的供应链检查，以及首个可执行的 `SafeHeaders` 契约。除 `south-contracts` 的 bootstrap 契约外，其余 crate 仍是明确标记的占位骨架，不得宣称生产可用。

测试按 TDD 执行：先观察到 `SafeHeaders` 和 typed error 尚不存在导致的 RED，再完成最小实现得到 GREEN；安全评审补充资源上限、transport-owned header 拒绝、无不可信输入的错误与 `Debug` 输出后，再次先观察新增测试 RED，最终 12 个测试全部通过。最终本地验证结果：

- fmt、Clippy `-D warnings`、nextest、doctest、no-default-features、rustdoc 和 MSRV 检查通过。
- 根 workspace 与 fuzz workspace 的 `cargo deny`、`cargo audit`、`cargo machete` 均通过。
- dependency boundary 自测和真实 workspace 检查通过，覆盖直接、重命名、路径、git source、分隔符变体和传递依赖绕过。
- fuzz target 在最终版本上运行 8,417,873 次，无 crash；普通 CI 只做 stable compile，定时任务才使用固定日期 nightly 运行 fuzz。
- actionlint、shellcheck、无浮动 GitHub Action、无中文字符和 `git diff --check` 均通过。

与团队规范的已知偏差只有一处：规范同时要求 stable 生产工具链和 stable 尚不支持的 `imports_granularity` / `group_imports`。本次以“生产禁止 nightly”为更高优先级，没有启用两个 unstable rustfmt 选项；fuzz 所需 nightly 被隔离在定时任务中。另因本机 Homebrew `cargo` 不支持 `cargo +toolchain` 语法，MSRV 验证使用等价的 `rustup run 1.96.0 cargo ...`。

当前仓仅修改中文文档，south 是独立 library 仓，没有改变 Token Station 桌面 App 的可执行行为，因此本阶段不运行 `scripts/install-local-desktop.sh`。两个真实宿主的兼容状态仍为 `not_verified`；下一阶段必须以宿主编译消费和公开 conformance fixture 作为迁移门禁。
