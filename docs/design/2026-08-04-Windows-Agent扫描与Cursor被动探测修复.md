# Windows Agent 扫描与 Cursor 被动探测修复

日期：2026-08-04

状态：macOS 本地实现与验收完成，等待 Windows 权威 runner 复验

关联：PR #63，`docs/design/2026-07-30-Windows首发与跨协议完整修复设计.md`，
`docs/design/2026-08-04-Cursor兼容只读调研与分阶段设计.md`

## 1. 问题

Windows 实际运行暴露了一个会把两个现象叠在一起的故障。旧版创建的
`ownership-index.json` 可能仍带父目录继承 ACL；新版按 owner-only protected DACL
读取时返回“ownership 索引权限或类型无效”。Agent 页面生成每个安装卡片时又把这个
错误向上传播成整个 `scan_agents` 失败，前端最终看起来像所有 Agent 都没有被发现。

macOS 还存在一条独立但同属扫描副作用的问题：Cursor Registry 把 App 主可执行文件
配置成 `--version` 直接探测。重新扫描因此会启动 Cursor GUI，Dock 中出现 Cursor 后
再退出或保持运行。只读扫描不应改变用户的应用运行状态。

Windows 的 CLI 版本探测还会短暂创建可见终端窗口。探测本身使用直接 argv，没有执行
shell，但桌面 GUI 创建控制台子进程时没有设置 `CREATE_NO_WINDOW`，所以重新扫描会闪出
黑色终端窗口。

Cursor 一键接入只实现了 macOS 路径和进程检查，Windows 会错误读取 `HOME`，并尝试执行
不存在的 `pgrep`。用户确认本轮沿用现有直接写入方式，不增加新的凭据格式转换层；修复重点
是让同一套 Base URL、API Key、备份和回读逻辑在 Windows 使用正确的数据库路径。

PR #63 的 Windows 全量 Rust 门禁还误跑两条依赖 macOS 路径语义的测试。测试在
Windows 临时目录上构造 `C:\...` 路径，却要求它满足 macOS 绝对路径合同，因此失败
并不能证明生产 Windows 扫描逻辑错误。

## 2. 目标、范围和非目标

### 目标

1. Windows 可以安全读取并收紧旧版普通 ownership 索引的 ACL。
2. ownership 状态不可读时，Agent 的只读发现结果仍然显示；接入、恢复和断开保持禁用，
   不把未知状态误报为未接管。
3. Cursor 扫描只检查受信 Registry 指定的普通文件，不启动 Cursor 进程。
4. Windows 的所有只读版本探测都在无可见控制台窗口的进程中运行。
5. Cursor 在 macOS 和 Windows 都使用同一套一键接入：退出 Cursor 后备份原设置，并直接
   写入 TS Base URL 与虚拟 API Key。
6. macOS 专属路径测试不在 Windows runner 执行，Windows 生产测试继续完整运行。

### 范围

- `private-fs` 既有 owner-only DACL 收紧能力的复用。
- ownership 索引读取和 Agent View 的失败隔离。
- Registry 版本探测运行时增加明确的被动文件模式。
- Cursor descriptor 和对应 Rust 行为测试。
- PR #63 Windows 全量 Rust、Desktop、MSI 前置门禁。

### 非目标

- 不修改 Router、Provider 选择、fallback、额度或模型能力。
- 不自动删除、重建或清空 ownership、snapshot 或 Agent 配置。
- 不扫描未知目录，不执行 shell，不安装或升级任何 Agent。
- 不把 Cursor GUI 文件存在当成已接入代理，只作为安装发现证据。
- 不在 macOS 上模拟 Windows NTFS ACL 通过。

## 3. 安全与数据红线

1. ACL 兼容只允许处理已知 ownership 根目录下的精确索引文件。
2. 文件必须是普通文件且不是 symlink/reparse point；类型不对时继续失败关闭。
3. Windows 读取旧索引前可以调用现有 `harden_private_file` 收紧为当前用户 owner-only
   protected DACL，但收紧后仍必须重新验证 ACL、大小、JSON schema、记录字段和重复键。
4. ACL 收紧失败时不读取正文、不覆盖文件，也不生成空索引替代它。
5. Agent 页面可以降级为只读发现，但任何依赖 ownership 的写操作必须继续返回
   `ownership_repair_required` 或等价的可操作错误。
6. 错误信息不得包含 ownership 内容、虚拟 Key、绝对用户路径或 Agent 配置正文。
7. 被动探测只允许 Registry 明确声明；不能根据文件名或 GUI 外观临时猜测。
8. 本轮不增加新的凭据格式转换或系统密钥链依赖；保持当前直接写入合同，失败时必须恢复
   Base URL 与 API Key 两项原值。

## 4. 用户可见行为和失败处理

正常升级时，Token Station 会在首次读取 Windows ownership 索引前收紧其 ACL，然后
继续显示真实 Agent 安装和既有接管状态。这个过程不删除备份，不修改 Agent 配置。

如果索引是目录、reparse point、损坏 JSON、未知 schema 或无法收紧权限，Agent 页面仍
显示扫描到的安装，但卡片进入只读保护状态，并明确说明接管索引需要修复。此时不能一键
接入、恢复或断开，避免在不知道旧 ownership 的情况下覆盖配置。

Cursor 重新扫描只检查 `/Applications/Cursor.app/Contents/MacOS/Cursor` 或 Windows
已声明安装路径的文件事实。扫描不执行 `Cursor --version`，不启动 GUI，也不关闭正在
运行的 Cursor。版本暂时显示未知是可接受的，因为 Cursor 当前走 discovery-only 和专用
接入流程，文件存在不能冒充版本兼容证据。

Windows 仍会探测其他 CLI 的版本，但创建子进程时必须使用 `CREATE_NO_WINDOW`。现有
stdout/stderr 有界读取、超时和进程组回收合同不变。

Cursor 运行时不写 SQLite，也不会强制关闭 Cursor。用户退出 Cursor 后点击一键接入，
Token Station 在一笔事务中写入 `applicationUser.openAIBaseUrl` 和 OpenAI API Key，回读
两项都匹配后才报告成功。写入或验证失败时恢复两项原值。

本轮不改变页面结构、焦点、键盘顺序或响应式布局。保护状态继续用文字呈现，不能只靠
颜色。

## 5. 最小架构

1. `ProbeRuntime` 增加 `passive_file`。该模式要求空 argv，并返回“普通文件存在且可解析
   真实路径”的可运行发现结果，不创建子进程。
2. Cursor descriptor 改用 `passive_file` 和 `SUCCESS_ONLY`，其他 Agent 的直接、Node、
   shebang 探测保持不变。
3. `FileOwnershipStore::read_index` 在 Windows 对现有普通文件先执行既有 ACL hardening，
   再执行当前严格验证和结构校验。Unix/macOS 保持原有只验证不自动 chmod 的行为。
4. `AgentCommandState::views` 隔离 ownership 读取失败：保留 discovery evidence，附加
   `ReadOnlyPreflightFailed` 诊断并生成不可接入的卡片，不让一个状态文件错误清空整页。
5. Windows `SystemProbeRunner` 在启动探测子进程前设置 `CREATE_NO_WINDOW`；其他平台不
   传 Windows creation flags。
6. 计划、应用、恢复、断开和 drift 仍直接读取 ownership，并在失败时停止，不能复用 UI
   降级结果绕过安全门。
7. Cursor 数据库路径按平台解析：macOS 使用 Application Support，Windows 使用
   `%APPDATA%\Cursor\User\globalStorage`。进程检查不启动 shell，Windows 子进程隐藏窗口。
8. Cursor 备份保存两条需要修改的 SQLite 行，不复制可能超过 1 GB 的整个数据库；备份
   文件写入 Token Station 私有目录并经过 owner-only 验证。

## 6. 公开测试和验收标准

### Rust 行为测试

1. `passive_file` 对可执行 fixture 返回发现成功，但 marker 证明从未启动该文件。
2. Registry 拒绝 `passive_file` 携带 argv，也拒绝普通 runtime 使用空 argv。
3. Cursor 内置 descriptor 必须声明 `passive_file`，不能回退到 `direct`。
4. Windows 创建带继承 ACL 的普通 ownership 索引后，首次读取会收紧并通过
   `verify_private_file`；非普通文件和 reparse point 仍失败。
5. ownership store 返回错误时，`scan_agents` 仍返回已发现安装，状态为只读保护且不包含
   `PreviewConnect`。
6. 正常 ownership 的 managed/connected 行为保持不变。
7. 两条 macOS 路径测试仅在非 Windows 目标编译运行。
8. Windows 探测进程统一使用 `CREATE_NO_WINDOW`；真机重新扫描不出现终端窗口。
9. Cursor 配置测试证明 Base URL 与 API Key 会同时写入，任一项验证失败都会恢复两项原值。
10. macOS/Windows 路径 fixture、运行中拒绝和关闭后两项更新分别覆盖。

### 本地门禁

- `scripts/check-rust-format.sh`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `scripts/prepare-desktop-test-plugins.sh`
- Desktop `cargo clippy --all-targets -- -D warnings` 与全量测试
- 前端测试和生产构建
- `scripts/install-local-desktop.sh`

### Windows 权威验收

1. Windows full Rust and desktop gates 全绿。
2. 从旧版宽 ACL ownership 索引升级后，Agent 页面能列出真实已安装 Agent。
3. ownership ACL 读回为 protected owner-only，索引内容和 revision 不变。
4. 损坏/类型错误索引不会清空 Agent 列表，也不会允许任何配置写入。
5. 重新扫描不会启动 Cursor；Windows 和 macOS 各观察一次进程列表。
6. Windows 重新扫描不会弹出终端窗口，CLI 版本和可运行状态仍能正常回读。
7. Windows Cursor 页面出现真实 Cursor 图标和配置按钮；运行中不写库，退出后一键写入
   Base URL 与 API Key，重启 Cursor 后请求进入 `/agents/cursor/v1`。
8. Windows MSI 真实生命周期任务能继续进入构建、安装、升级和卸载阶段。

## 7. 实现落点、遗留项和发布要求

实现落点：

- `crates/private-fs/src/lib.rs`
- `apps/desktop/src-tauri/src/agent_integration/ownership.rs`
- `apps/desktop/src-tauri/src/agent_integration/commands.rs`
- `apps/desktop/src-tauri/src/agent_integration/types.rs`
- `apps/desktop/src-tauri/src/agent_integration/registry.rs`
- `apps/desktop/src-tauri/src/agent_integration/discovery.rs`
- `apps/desktop/src-tauri/agent-registry/builtin-agents.json`

macOS 本地测试只能证明非 Windows 回归和 Cursor 不启动，不能代替 NTFS ACL 真机证据。
所有本地门禁和 App 安装通过后才能推 PR；推送后必须等待 Windows full Rust、Agent
platform checks 和 MSI lifecycle。若 Windows runner 再暴露生产问题，继续在同一 PR
更新，不合并、不发布。

## 8. 实现与验收记录

已完成实现：

- Cursor Registry 改为被动文件探测，重新扫描不再执行 Cursor 主程序。
- Windows 子进程探测统一使用 `CREATE_NO_WINDOW`，Cursor 运行状态检查也隐藏终端窗口。
- Windows ownership 索引先收紧 ACL 再读取；读取失败时保留 Agent 发现结果，并禁用写操作。
- Cursor 数据库路径按 macOS `HOME` 和 Windows `APPDATA` 分流。一键接入会备份并直接写入
  Base URL 与 API Key，两项回读匹配后才返回成功。
- Cursor 卡片在 ownership 不可读时显示“接管状态不可用”，不能误点一键接入。

2026-08-04 本地验收结果：

- Desktop Rust 全量测试：236 通过，1 个既有真机探测测试按标记忽略；安装器测试 2 通过，
  YAML 回归测试 3 通过。
- Desktop Clippy `--all-targets -- -D warnings` 通过。
- Workspace Rust 全量测试通过。
- 前端测试 27 个文件、242 个测试全部通过；生产构建通过。
- 五个 Desktop 测试插件准备成功。
- `scripts/install-local-desktop.sh` 完成构建、artifact 审计、bundle id 与签名检查，并安装启动
  `/Applications/token-station.app`。
- 真实 Agent 页面显示 Cursor。点击“重新扫描”后 Cursor 仍可发现，进程检查结果为
  `CURSOR_NOT_STARTED`，确认扫描没有启动 Cursor。

macOS 没有 Windows NTFS ACL、控制台窗口和 MSI 环境。Windows full Rust、Desktop、Agent
platform checks 与 MSI lifecycle 仍以 PR 的 Windows runner 为最终验收，不把本地模拟写成
Windows 真机通过。
