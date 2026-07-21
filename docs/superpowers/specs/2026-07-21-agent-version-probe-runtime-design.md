# Agent 版本探测运行时修复设计

日期：2026-07-21

状态：等待用户确认书面方案

关联文档：

- [第一阶段：Agent 扫描缓存与按需复核设计](2026-07-20-agent-scan-cache-design.md)

## 1. 问题与证据

Token Station 能发现 Hermes 和 OpenClaw 的安装入口，但首次扫描时两者都进入了“只读保护”。2026-07-21 的只读诊断得到以下结果：

| Agent | 安装入口 | 版本命令 | 诊断结果 |
|---|---|---|---|
| Hermes | `~/.local/bin/hermes` | `hermes version` | 首次 App 扫描失败；手动重新扫描后成功识别 `0.18.0`，进入“可接入”状态 |
| OpenClaw | `~/.local/bin/openclaw` | `openclaw --version` | 每次 App 扫描都返回 `env: node: No such file or directory` |

Hermes 的入口会执行绝对路径 `~/.hermes/hermes-agent/venv/bin/hermes`。同一版本命令在隔离环境下稳定成功，耗时约 0.14 秒，低于当前 2 秒上限。首次失败没有留下足够的界面诊断，现有证据只能确认它是瞬时探测失败，不能把原因写成已证实的解释器错误。

OpenClaw 的入口是 `#!/usr/bin/env node` 脚本，本机 Node 位于 `~/.local/bin/node`。从 Finder 启动的桌面 App 不包含用户登录 Shell 的 PATH。Scanner 虽然通过 Registry 的已知位置找到了 OpenClaw，执行版本命令时却先清空环境，再使用桌面进程的 PATH，因此 `/usr/bin/env` 找不到 Node。

当前行为符合 fail closed 原则。版本无法确认时，兼容层返回 `INSTALLED_BROKEN`，前端显示“只读保护”并禁用接入。问题出在探测运行时和诊断信息，不在 Connector、配置事务或核心路由。

## 2. 目标

本次修改需要达到以下结果：

1. Finder 启动 Token Station 时，OpenClaw 仍能使用声明并验证过的 Node 解释器完成版本探测。
2. Scanner 不继承完整登录 Shell 环境，不执行 Shell 拼接命令，不扩大远程兼容目录权限。
3. Hermes 等 Agent 的版本探测若发生一次超时，可以进行一次有边界的重试。
4. 重试失败或其他错误继续进入只读保护，不能因为重试而放宽版本兼容准入。
5. Agents 页面展示脱敏后的具体失败原因，避免只显示“安装入口存在，但版本探测未成功”。
6. `crates/router-core/**` 保持零修改。

## 3. 非目标

- 不修改 Agent Connector 的配置投影。
- 不修改兼容目录中的已验证版本范围。
- 不自动安装、升级或修复 Node、Hermes、OpenClaw。
- 不读取 `.zshrc`、`.bashrc` 或其他登录 Shell 配置。
- 不把完整父进程环境传给版本命令。
- 不修改请求数据面、协议 Adapter、Gateway 或核心路由。
- 不为所有脚本实现通用 Shell 解释器。

## 4. 方案比较

### 4.1 继承登录 Shell PATH

桌面 App 可以启动登录 Shell，再读取其 PATH。该方案兼容 nvm、asdf 等版本管理器，但会执行用户 Shell 初始化文件，启动时延和副作用不可控，也会把大量用户目录加入命令搜索范围。本方案不采用。

### 4.2 把常见用户目录加入所有探测命令

可以将 `~/.local/bin`、Homebrew、nvm 等目录直接追加到 Scanner 的全局 PATH。实现简单，但任何使用 `/usr/bin/env <name>` 的脚本都可能命中这些目录中的同名程序，解释器选择不再受 Agent Descriptor 约束。本方案不采用。

### 4.3 Descriptor 声明解释器，Scanner 显式执行

推荐方案为版本探测增加可选的解释器声明。OpenClaw 明确声明 Node 候选和已知位置，Scanner 分别校验脚本与解释器的真实路径，然后直接执行：

```text
<canonical node> <canonical openclaw.mjs> --version
```

命令仍使用参数数组，不经过 Shell，不依赖 Finder PATH。没有解释器声明的 Agent 保持现有直接执行方式。

## 5. 设计

### 5.1 Registry 契约

`VersionProbe` 增加可选字段 `interpreter`：

```text
VersionProbe
  argv[]
  timeout_ms
  max_output_bytes
  output_matcher
  interpreter?
    executable_candidates[]
    known_install_locations[platform][]
```

约束如下：

- 字段只来自随 App 发布的内置 Registry，远程兼容目录不能修改它。
- 候选名称只允许普通文件名，不允许路径分隔符、空白和 Shell 元字符。
- 已知位置沿用现有绝对路径模板校验，只允许受支持的根变量。
- 解释器必须是可执行普通文件，执行前解析为 canonical path。
- Scanner 直接执行解释器，脚本 canonical path 作为第一个参数，Descriptor 的 `argv` 顺序追加。
- 如果多个解释器候选解析为不同 canonical path，探测失败并进入只读保护，不静默选择。

首期只有 OpenClaw 声明 Node 解释器。该结构可以供以后确实需要外部解释器的 Agent 使用，但不会自动推断任意 shebang，也不会扫描登录 Shell 配置。

### 5.2 运行环境

版本命令继续调用 `env_clear()`。子进程只获得当前允许列表中的最小环境，不增加 API Key、代理凭据或配置原文。

解释器模式不依赖 PATH 找 Node。保留 PATH 仅用于解释器自身确有需要的系统命令，来源仍是桌面进程中已有的绝对目录。Scanner 不把用户目录全局追加到 PATH。

### 5.3 有边界的重试

版本命令只在 `VersionProbeTimeout` 时重试一次，间隔 100 毫秒，每次继续使用 Descriptor 规定的独立超时和输出上限。

以下情况不重试：

- 进程返回非零状态；
- 输出无法解析为 SemVer；
- 解释器缺失、冲突或不是可执行普通文件；
- 配置读取或结构指纹失败。

重试成功后可以正常进入兼容判断，同时保留一条不含原始输出的诊断，说明首次探测超时且第二次成功。两次都超时则维持 `INSTALLED_BROKEN`。

该策略不保证解释 2026-07-21 的 Hermes 首次失败。由于原界面没有展示具体 `reason_code`，文档只记录已观察到的现象。后续若同类问题再次出现，界面诊断应提供可验证证据。

### 5.4 界面诊断

`AgentDiscoveryView` 已包含脱敏后的 `diagnostics`。Agent 卡片在 `INSTALLED_BROKEN` 状态下展示第一条诊断的 `message` 和 `reason_code`，但不展示未截断的 stdout、stderr、配置原文或环境变量。

展示示例：

```text
安装入口存在，但版本探测未成功
VERSION_PROBE_EXIT_FAILURE：版本探测以非零状态退出（code=127）
```

成功重扫后，旧错误随扫描结果原子替换，不继续显示。

## 6. 影响面地图

| 维度 | 文件或模块 | 动作 | 验证 | 状态 |
|---|---|---|---|---|
| Registry 契约 | `agent_integration/types.rs`、`registry.rs` | 增加并校验可选解释器声明 | Registry 正负向单测 | 必须修改 |
| 内置描述符 | `agent-registry/builtin-agents.json` | 为 OpenClaw 声明 Node 候选与已知位置 | 内置 Registry 加载测试 | 必须修改 |
| 发现运行时 | `agent_integration/platform.rs`、`discovery.rs` | 解析解释器，构造显式命令，增加超时单次重试 | Unix 隔离 PATH 测试、超时与冲突测试 | 必须修改 |
| 前端展示 | `components/AgentCard.tsx` | 展示脱敏诊断 | React 组件测试 | 必须修改 |
| IPC 类型 | `apps/desktop/src/api.ts` | 现有诊断类型可复用，预计无需变更 | TypeScript 编译 | 已检查，预计不修改 |
| Connector 与事务 | `connectors.rs`、`plan.rs`、`transaction.rs` | 不改变配置计划和写入逻辑 | 既有 Agent 集成测试 | 已检查，不修改 |
| 兼容目录 | `builtin-compatibility.json`、远程目录 | 不改变版本范围 | 目录选择测试 | 已检查，不修改 |
| 缓存行为 | `App.tsx`、扫描缓存 Spec | 保持每进程一次自动扫描和手动重扫；单次探测内部超时重试不等于重复完整扫描 | App 扫描次数测试 | 文档澄清，代码不修改 |
| 请求数据面 | Gateway、协议 Adapter、Provider Adapter | 与版本探测无调用关系 | 变更路径审计 | 不在范围 |
| 核心路由 | `crates/router-core/**` | 禁止修改 | 红线脚本与 Git diff | 绝对红线 |

## 7. 测试与验收

### 7.1 Rust 自动化测试

- 构造 PATH 只有 `/usr/bin:/bin:/usr/sbin:/sbin` 的 macOS 扫描环境。
- 构造 OpenClaw 脚本和位于已知位置的假 Node，证明 Scanner 显式执行解释器并得到版本。
- 解释器缺失、路径为目录、无执行权限、多个 canonical path 冲突时全部 fail closed。
- Descriptor 包含相对路径、Shell 元字符或未知字段时拒绝加载。
- 首次超时、第二次成功时记录诊断并返回版本。
- 连续两次超时仍为不可运行。
- 非零退出不重试。
- 版本输出、stderr 和诊断继续满足长度与脱敏约束。
- 现有五类 Agent 发现、版本归一化、兼容判断和计划前强制复核测试全部通过。

### 7.2 前端自动化测试

- `INSTALLED_BROKEN` 展示安全诊断和 reason code。
- 正常、未知版本、多安装和已接入状态不增加错误文案。
- 手动重扫成功后清除旧诊断。
- 页面切换和重新扫描次数保持第一阶段缓存契约。

### 7.3 本机验收

1. 从 Finder 启动最新 `/Applications/token-station.app`。
2. 打开 Agents 页面，OpenClaw 应显示 `2026.6.11`，状态为“可接入”。
3. Hermes 应显示 `0.18.0`，状态为“可接入”；若首次超时，应自动重试一次。
4. 只执行“预览接入”，确认差异与恢复点正确，不直接写入外部 Agent 配置。
5. 验收前后比较 Hermes 和 OpenClaw 配置文件哈希，纯扫描过程必须保持字节不变。
6. 检查本机只保留一个 `/Applications/token-station.app`。

## 8. 红线、回滚与完成标准

实现提交不得包含 `crates/router-core/**` 的任何内容、文件名或权限变化。若解释器解析失败，系统继续只读，不回退到继承完整环境或调用登录 Shell。

代码回滚只需恢复 Agent Registry、探测运行时和前端诊断改动，不需要迁移用户数据。扫描和版本探测不写第三方 Agent 配置，也不产生新的持久化状态。

完成标准如下：

- OpenClaw 在 Finder 启动环境中稳定识别为已验证版本。
- Hermes 初次超时最多重试一次，失败仍安全阻断。
- 用户能看到脱敏后的具体探测失败原因。
- 自动化测试、桌面构建和本机只读验收通过。
- 扫描前后第三方 Agent 配置字节一致。
- `crates/router-core/**` 零修改，红线脚本通过。
