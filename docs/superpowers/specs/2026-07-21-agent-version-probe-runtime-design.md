# Agent 版本探测运行时修复设计

日期：2026-07-21

状态：用户已确认，已按方案实现，等待最终 App 验收

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

### 1.1 cc-Switch 对照证据

2026-07-21 对 cc-Switch 主分支和公开问题进行只读调研，结论如下：

- cc-Switch 的[版本检测源码](https://github.com/farion1231/cc-switch/blob/613fef70bc7d5e35299b4131935f738c85765b35/src-tauri/src/commands/misc.rs#L1498-L1737)会枚举 `~/.local/bin`、Homebrew、nvm、fnm、mise、Volta、Python Scripts 等常见目录，并在执行候选 CLI 时把当前候选目录放到子进程 PATH 最前。这能让 npm 入口脚本通过 `#!/usr/bin/env node` 找到同目录 Node。
- cc-Switch 同时保留候选入口和 canonical 真身，用 canonical path 去重多入口，并标识 PATH 默认安装和安装来源。这一模型适合发现多版本冲突。
- cc-Switch 的版本检测仍继承 GUI 进程的其他环境，且当前实现没有 Token Station 已有的显式进程组超时和输出上限。因此不能原样复制。
- cc-Switch 当前的[应用类型仍是闭合集合](https://github.com/farion1231/cc-switch/blob/613fef70bc7d5e35299b4131935f738c85765b35/src-tauri/src/app_config.rs#L367-L447)，配置管理和本地版本检测也是两套能力。Token Station 不退回 Agent 名称硬编码，也不把“发现到 CLI”直接等同于“版本已验证可接入”；新增 Agent、版本探测和兼容准入继续分别由 Registry、Probe Runtime 与兼容目录声明。
- cc-Switch 的安装/升级是另一条执行链。公开的 [#4162](https://github.com/farion1231/cc-switch/issues/4162)、[#5252](https://github.com/farion1231/cc-switch/issues/5252) 和 [#5370](https://github.com/farion1231/cc-switch/issues/5370) 表明，Finder/Dock 启动后，该链路仍可能因瘦 PATH 返回 `env: node: No such file or directory`。这证明运行时解析需要成为可复用基础能力，不能只在单个按钮内补 PATH。
- Windows npm 会在同一目录生成裸名脚本、`.cmd` 和 `.exe`。cc-Switch 的 [#4610](https://github.com/farion1231/cc-switch/issues/4610) 与 [PR #4782](https://github.com/farion1231/cc-switch/pull/4782) 说明，应在候选生成阶段优先可运行的原生入口，避免把同一安装误报为多实例。
- 配置写入方面，cc-Switch 的 [OpenClaw 配置模块](https://github.com/farion1231/cc-switch/blob/613fef70bc7d5e35299b4131935f738c85765b35/src-tauri/src/openclaw_config.rs)和 [Hermes 配置模块](https://github.com/farion1231/cc-switch/blob/613fef70bc7d5e35299b4131935f738c85765b35/src-tauri/src/hermes_config.rs)已采用锁、写前备份、原子替换、未知字段保留和部分写后健康检查；Hermes 还会修复历史重复顶层键。其 [#3851](https://github.com/farion1231/cc-switch/issues/3851) 也说明，整段追加或不完整重写会破坏 Agent 配置。

Token Station 已有 `env_clear()`、进程组超时、输出上限、revision 绑定计划、加密快照、原子写入和恢复，这些约束继续保留。cc-Switch 调研只用于补齐“安装上下文中的运行时解析”、多入口归一化测试和配置前向兼容回归，不降低现有安全边界。

## 2. 目标

本次修改需要达到以下结果：

1. Finder 启动 Token Station 时，OpenClaw 仍能使用声明并验证过的 Node 解释器完成版本探测。
2. Scanner 不继承完整登录 Shell 环境，不执行 Shell 拼接命令，不扩大远程兼容目录权限。
3. Hermes 等 Agent 的版本探测若发生一次超时，可以进行一次有边界的重试。
4. 重试失败或其他错误继续进入只读保护，不能因为重试而放宽版本兼容准入。
5. Agents 页面展示脱敏后的具体失败原因，避免只显示“安装入口存在，但版本探测未成功”。
6. 运行时解析成为 Scanner 内可复用的统一能力，后续新增声明式 Agent 时不需要编写 Agent 专属分支。
7. `crates/router-core/**` 保持零修改。

## 3. 非目标

- 不修改 Agent Connector 的配置投影。
- 不修改兼容目录中的已验证版本范围。
- 不自动安装、升级或修复 Node、Hermes、OpenClaw。
- 不在本阶段修改 Agent 的安装、升级或自更新执行链；若以后增加该能力，必须复用同一运行时解析器并另写 Spec。
- 不读取 `.zshrc`、`.bashrc` 或其他登录 Shell 配置。
- 不把完整父进程环境传给版本命令。
- 不修改请求数据面、协议 Adapter、Gateway 或核心路由。
- 不为所有脚本实现通用 Shell 解释器。

## 4. 方案比较

### 4.1 继承登录 Shell PATH

桌面 App 可以启动登录 Shell，再读取其 PATH。该方案兼容 nvm、asdf 等版本管理器，但会执行用户 Shell 初始化文件，启动时延和副作用不可控，也会把大量用户目录加入命令搜索范围。本方案不采用。

### 4.2 把常见用户目录加入所有探测命令

cc-Switch 的版本检测采用了接近这一思路的做法：扫描常见安装目录，并把当前候选目录放在继承 PATH 前。它覆盖面广，能修复很多 Finder PATH 问题，但任何使用 `/usr/bin/env <name>` 的脚本都可能命中继承目录中的同名程序，解释器选择不再完全受 Agent Descriptor 约束；同时登录 Shell、版本管理器和 GUI 进程可能看到不同默认版本。本方案不原样采用。

### 4.3 Descriptor 声明 Probe Runtime，Scanner 显式执行

推荐方案为版本探测增加可选的运行时声明。OpenClaw 明确声明 `env_shebang`、Node 候选、解析来源和已知位置。Scanner 同时保留“用户实际命中的入口”和“canonical 真身”，校验脚本与解释器后直接执行：

```text
<canonical node> <canonical openclaw.mjs> --version
```

命令仍使用参数数组，不经过 Shell，不依赖 Finder PATH。没有运行时声明的 Agent 保持现有直接执行方式。后续若 Claude Code、Codex 或其他 Agent 的发行形式同时包含原生二进制和 Node 脚本，可以通过 Descriptor 声明允许的 launcher 类型扩展，不在 Scanner 中增加 Agent 名称判断。

## 5. 设计

### 5.1 Registry 契约

`VersionProbe` 增加可选字段 `runtime`：

```text
VersionProbe
  argv[]
  timeout_ms
  max_output_bytes
  output_matcher
  retry_on_timeout
  runtime?
    kind: direct | env_shebang
    interpreter_candidates[]
    resolution_sources[]
      - observed_entry_sibling
      - known_install_locations
    known_install_locations[platform][]
```

约束如下：

- 字段只来自随 App 发布的内置 Registry，远程兼容目录不能修改它。
- `runtime` 缺省为 `direct`；首期只实现 `direct` 和受声明约束的 `env_shebang`，不自动执行未知脚本类型。
- 解释器候选名称只允许普通文件名，不允许路径分隔符、空白和 Shell 元字符。
- 已知位置沿用现有绝对路径模板校验，只允许受支持的根变量。
- `env_shebang` 只读取 canonical 脚本开头的有限字节，并要求 shebang 与 Descriptor 声明完全匹配；不解析或执行脚本正文。
- 解释器必须是可执行普通文件，执行前解析为 canonical path，并校验为 Mach-O、ELF 或 PE 原生可执行格式；文本脚本和 shebang shim 不能冒充解释器。
- Scanner 先检查实际被选择的 observed entry 同目录中的解释器。这吸收 cc-Switch 的“安装目录携带运行时”经验，并确保 npm、nvm、Homebrew 的入口优先使用同一安装上下文中的 Node。
- observed entry 同目录无有效解释器时，才检查 Descriptor 的精确已知位置；不读取登录 Shell PATH，不递归遍历整个版本管理器目录。
- 同一优先级若有多个候选解析为不同 canonical path，探测失败并进入只读保护；低优先级发现其他 Node 不否定已经验证的同目录 Node。
- Scanner 直接执行 canonical 解释器，canonical 脚本作为第一个参数，Descriptor 的 `argv` 顺序追加。
- 构造命令前再次 canonicalize Agent 入口与解释器，并比较文件身份；若从解析到启动前已发生替换则 fail closed。该复验不宣称消除操作系统级全部竞态，但不会在已观察到路径变化时继续执行。
- 运行时解析结果只在单次扫描内使用，不持久化，不自动改变系统 PATH，也不用于安装或升级。

首期只有 OpenClaw 声明 Node 运行时。该结构可以供以后确实需要外部解释器的 Agent 使用，但不会自动信任任意 shebang，也不会扫描登录 Shell 配置。

### 5.2 安装入口选择与多实例归一化

现有 `Installation` 已同时保存 `canonical_path`、`observed_probe_path` 和全部 evidence。本次明确以下选择规则：

1. 先按 canonical Agent 真身归并软链接入口。
2. 在同一安装内选择确定的 probe entry：PATH 默认入口优先，其次内置已知位置，最后按规范化路径排序。
3. Probe Runtime 的 `observed_entry_sibling` 只相对于该 probe entry 解析，不把其他安装目录混入搜索。
4. 不同 canonical Agent 真身仍作为多实例展示，不能因为版本相同而合并。
5. Windows 继续只执行现有允许的原生 `.exe`/`.com`；`.cmd`、`.bat`、`.ps1` 保持只读保护。候选生成不得加入同目录裸名 Unix shim。未来如支持 Windows npm shim，必须单独设计安全执行器并覆盖 cc-Switch [#4610](https://github.com/farion1231/cc-switch/issues/4610)、[#3332](https://github.com/farion1231/cc-switch/issues/3332) 类回归，不能通过 `cmd /C` 临时放宽。

该顺序避免“canonical 脚本已经移动到 node_modules 后丢失原安装目录”，也避免用户机器上存在另一个无关 Node 时产生错误冲突。

### 5.3 运行环境

版本命令继续调用 `env_clear()`。子进程只获得当前允许列表中的最小环境，不增加 API Key、代理凭据或配置原文。

解释器模式不依赖 PATH 找 Node。子进程 PATH 仅包含 canonical 解释器所在目录和固定系统允许目录；不会恢复 GUI 进程中的其他用户目录，也不会加入其他候选安装目录。Scanner 不修改全局 PATH，也不把用户登录 Shell 的完整 PATH 注入所有探测。

进程退出或被终止后，stdout/stderr reader 会收到停止信号，并通过 Unix `FIONREAD` 或 Windows `PeekNamedPipe` 进行可取消的非阻塞轮询；即使异常后代进程仍持有管道，也不会留下永久阻塞的读取线程。Scanner 最多等待 100 毫秒回收输出，超过期限即返回截断诊断。

子进程 PATH 必须由 canonical 解释器目录和固定系统目录成功构造。注入最小环境时始终忽略父环境中的 PATH；若路径包含平台无法编码的分隔符等异常导致 PATH 构造失败，探测直接 fail closed，绝不回落到继承 PATH。

### 5.4 有边界的重试

只有内置 Registry 明确设置 `retry_on_timeout: true` 的版本命令，才在 `VersionProbeTimeout` 时重试一次，间隔 100 毫秒；每次继续使用 Descriptor 规定的独立超时和输出上限。首期仅 Hermes 开启，其他 Agent 不默认重复执行外部程序。

以下情况不重试：

- 进程返回非零状态；
- 输出无法解析为 SemVer；
- 解释器缺失、冲突或不是可执行普通文件；
- 配置读取或结构指纹失败。

重试成功后可以正常进入兼容判断，同时保留一条不含原始输出的诊断，说明首次探测超时且第二次成功。两次都超时则维持 `INSTALLED_BROKEN`。

该策略不保证解释 2026-07-21 的 Hermes 首次失败。由于原界面没有展示具体 `reason_code`，文档只记录已观察到的现象。后续若同类问题再次出现，界面诊断应提供可验证证据。

### 5.5 界面诊断

`AgentDiscoveryView` 已包含脱敏后的 `diagnostics`。失败或超时时，后端直接把 `version_raw` 置空，不让原始 stderr 通过 IPC 返回；Agent 卡片在 `INSTALLED_BROKEN` 状态下优先展示与兼容结论 `reason_code` 匹配的诊断，只有找不到匹配项时才回退到首条诊断，且不展示配置原文或环境变量。

展示示例：

```text
安装入口存在，但版本探测未成功
VERSION_PROBE_EXIT_FAILURE：版本探测以非零状态退出（code=127）
```

成功重扫后，旧错误随扫描结果原子替换，不继续显示。

### 5.6 配置写入安全保持项

cc-Switch 的 OpenClaw 写入链包含写锁、写前内容冲突检测、无变化不备份、时间戳备份、原子替换和写后健康扫描；Hermes 写入链会保留未知字段，并对历史重复顶层键做兼容读取。Token Station 本阶段不改 Connector，但必须保留并回归验证现有更严格能力：

- 预览计划绑定扫描 revision，写前重新验证，防止外部配置在预览后发生变化；
- 只投影 Token Station 拥有的字段，保留未知字段、注释和无关段落；
- 写前创建加密快照，原子写入，写后校验，失败恢复；
- 扫描、版本探测和“预览接入”不得修改第三方配置；
- 不采用整段追加、不完整整体重写或“解析失败仍覆盖”的降级路径。

## 6. 影响面地图

| 维度 | 文件或模块 | 动作 | 验证 | 状态 |
|---|---|---|---|---|
| Registry 契约 | `agent_integration/types.rs`、`registry.rs` | 增加并校验可选 Probe Runtime 声明 | Registry 正负向单测 | 必须修改 |
| 内置描述符 | `agent-registry/builtin-agents.json` | 为 OpenClaw 声明 `env_shebang`、Node 候选、解析来源与已知位置 | 内置 Registry 加载测试 | 必须修改 |
| 发现运行时 | `agent_integration/platform.rs`、`discovery.rs` | 确定 probe entry、解析运行时、构造显式命令、增加超时单次重试 | Unix 隔离 PATH、同目录运行时、超时与冲突测试 | 必须修改 |
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
- 构造 observed entry 位于 `bin/`、canonical 脚本位于 `lib/node_modules/`、Node 位于 observed entry 同目录的 OpenClaw 安装，证明 Scanner 不依赖 Finder PATH 也能得到版本。
- 构造 npm、Homebrew 和 nvm 风格的入口软链接夹具，证明运行时始终与被选 probe entry 属于同一安装上下文。
- 同目录 Node 缺失时允许命中唯一的 Descriptor 已知位置；同一优先级存在多个不同 canonical Node 时 fail closed。
- shebang 与声明不符、解释器缺失、路径为目录、无执行权限或解析路径被替换时全部 fail closed。
- 文本脚本不能伪装成 Node 解释器；解释器必须命中受支持的原生可执行格式。
- Descriptor 包含相对路径、Shell 元字符或未知字段时拒绝加载。
- 同一 canonical Agent 的多个软链接入口正确归并；不同 canonical Agent 真身不因版本相同而合并。
- Windows 候选不把 npm 裸名 Unix shim 误报为独立安装，现有脚本 shim 仍不执行。
- 首次超时、第二次成功时记录诊断并返回版本。
- 连续两次超时仍为不可运行。
- 非零退出不重试。
- 未声明 `retry_on_timeout` 的 Agent 即使超时也不重试；连续两次超时在第二次后停止。
- 输出 reader 超过硬回收截止时必须返回，不得卡住完整扫描。
- 继承 PATH 中的用户可写目录不能进入 probe 子进程 PATH。
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

实现提交不得包含 `crates/router-core/**` 的任何内容、文件名或权限变化。若运行时解析失败，系统继续只读，不回退到继承完整环境、调用登录 Shell 或执行系统包管理器。

代码回滚只需恢复 Agent Registry、探测运行时和前端诊断改动，不需要迁移用户数据。扫描和版本探测不写第三方 Agent 配置，也不产生新的持久化状态。

完成标准如下：

- OpenClaw 在 Finder 启动环境中稳定识别为已验证版本。
- Hermes 初次超时最多重试一次，失败仍安全阻断。
- 用户能看到脱敏后的具体探测失败原因。
- 新增 Agent 若需要外部解释器，只需扩展受校验的 Registry 描述和兼容夹具，不增加 Agent 专属执行分支。
- 自动化测试、桌面构建和本机只读验收通过。
- 扫描前后第三方 Agent 配置字节一致。
- `crates/router-core/**` 零修改，红线脚本通过。
