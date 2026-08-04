# macOS Release Agent 扫描与密钥错误修复设计

日期：2026-08-04

状态：已实现并完成本机 Release App 验收

## 1. 问题与证据

队友把 Release 版 Token Station 安装到 `/Applications` 后，从 Finder 启动应用。电脑上已经安装 Claude Code、Gemini CLI、OpenCode、OpenClaw 和 Hermes，但 Agents 页面连续重新扫描仍显示未发现。Codex 可以被发现，因为它有固定的 App Bundle 内置路径，不依赖 Finder 进程的 `PATH`。

当前 Scanner 只检查 Agent Registry 中的精确路径和桌面进程继承到的 `PATH`。Finder 启动的应用通常只有很窄的系统 `PATH`。内置 Registry 对 macOS 用户级包管理目录覆盖不完整，例如 OpenCode 在 Linux 已声明 `~/.opencode/bin/opencode`，macOS 却只有 Homebrew 路径；npm 自定义 `prefix`、pnpm、Bun 和 Volta 的常见目录也没有进入候选集合。本机现有 OpenCode 和 OpenClaw 位于 `~/local/npm-global/bin`，该目录由 `~/.npmrc` 的 npm `prefix` 决定，当前 Release Scanner 无法看到。

同一台电脑上的 Codex 已经接入 Token Station，请求也确实到达 `http://127.0.0.1:8787/agents/codex/v1/responses`，但 Gateway 返回 401：`secret provider_api_key: not in the local store (re-enter the key)`。这说明 Agent Connector 和本地代理地址已经生效，失败点是路由选中的 upstream 在本地密钥库中没有对应密钥。当前错误只显示通用 slot 名 `provider_api_key`，没有显示 upstream 名。用户无法判断应该重填 DeepSeek、OpenAI 还是其他 provider。

## 2. 目标与范围

本次修复达到以下结果：

1. macOS Release App 从 Finder 启动且 `PATH` 不含用户目录时，仍检查受支持的常见用户级 CLI 安装目录。
2. Scanner 读取用户级 `~/.npmrc` 中唯一生效的绝对 `prefix`，并检查其 `bin` 目录中的 Node CLI，不执行 Shell 或 npm 命令。
3. Claude Code、Gemini CLI、OpenCode 和 OpenClaw 共享同一套声明式候选生成规则；OpenCode 同时覆盖官方 `~/.opencode/bin`。Hermes 保持 Python 用户级 `~/.local/bin` 路径，并增加常见的 `~/Library/Python/*/bin` 受限枚举。
4. 密钥缺失、环境变量缺失、密钥文件不可读和空值错误都明确显示 upstream 与 slot，且永远不包含密钥值。
5. 不修改 Connector 投影、路由选择、密钥存储格式、第三方 Agent 配置和核心路由。

## 3. 非目标

- 不读取或执行 `.zshrc`、`.bashrc` 等 Shell 初始化文件。
- 不调用 `npm config`、`which`、`find` 或包管理器安装命令。
- 不递归扫描整个用户目录，也不自动修复或安装 CLI。
- 不把“配置文件存在”当成“CLI 已安装”。
- 不迁移、猜测或自动生成缺失的 provider API Key。用户仍需在对应 provider 中重新输入真实密钥。
- 不改变多个真实安装并存时的冲突保护。
- 不修改 `crates/router-core/**`。

## 4. 安全与数据红线

Scanner 只生成候选路径，后续仍使用现有规则验证候选必须是可执行普通文件、解析 canonical path、限制版本探测时间与输出，并在异常时 fail closed。`~/.npmrc` 只按文本读取 `prefix`：拒绝相对路径、变量展开、命令替换、URL 和含 NUL 的值；最后一个有效的 `prefix` 与 npm 配置覆盖语义一致。读取失败等同于没有额外候选，不影响完整扫描。

对 Python 用户目录只枚举 `~/Library/Python` 下一层版本目录，并检查固定的 `bin/hermes`，不递归进入其他位置。目录项数量设置上限，避免异常目录拖慢扫描。

所有密钥错误只能包含配置中的 upstream 名、slot 名和来源位置。错误不得包含解析出的值、Authorization Header、请求正文或密钥库内容。

## 5. 用户可见行为与失败处理

用户点击“重新扫描”后，常见用户级目录中的 Agent 会像 PATH 或内置路径中的安装一样显示。若同一 Agent 在多个目录解析为同一个 canonical 文件，现有归并逻辑去重；若解析为不同安装，继续显示多安装冲突并要求用户选择，不静默挑一个。

若 `~/.npmrc` 不存在、格式异常或 `prefix` 不安全，扫描继续执行其他已知路径，不弹出阻塞错误。CLI 存在但版本探测失败时，继续显示现有只读保护与脱敏诊断。

请求因 provider 密钥缺失而失败时，错误改为类似：

```text
upstream `deepseek` secret `provider_api_key`: not in the local store (re-enter the key)
```

用户由此可以回到 DeepSeek provider 重新输入密钥。HTTP 状态仍为 401，协议错误结构不变。

## 6. 响应式、键盘与可访问性

本次不改变页面结构、控件、焦点顺序或快捷键。新增信息只进入现有错误文本区域，必须保持可复制、可由屏幕阅读器读取，并允许在窄窗口中自然换行。

## 7. 实现落点

- `apps/desktop/src-tauri/src/agent_integration/platform.rs`：增加 macOS 常见用户 bin 候选、受限 npm prefix 读取和 Hermes Python 用户目录枚举。
- `apps/desktop/src-tauri/agent-registry/builtin-agents.json`：补齐静态、可审计的 macOS 安装路径。
- `apps/desktop/src-tauri/src/agent_integration/registry.rs` 与 Scanner 测试：锁定五个 Agent 在 Finder 瘦 PATH 下的候选行为。
- `apps/cli/src/secrets.rs`：统一密钥解析错误上下文。
- `apps/cli/tests/proxy.rs`：通过真实 Gateway 路径验证 401 包含 upstream，不泄漏密钥。

## 8. 公开测试边界与验收标准

自动化测试必须覆盖：

1. macOS 环境的 `PATH` 为空时，四个 Node CLI 可以从静态用户目录和 `~/.npmrc` 绝对 prefix 生成候选；Hermes 可以从 `~/.local/bin` 与受限 Python 用户目录生成候选。
2. 相对 prefix、变量、命令替换、URL 和过量 Python 目录不能扩大扫描范围。
3. 候选来源仍标记为已知路径，不伪装成用户 `PATH` 命中。
4. 缺失本地密钥的 `/agents/codex/v1/responses` 请求返回 401，错误同时包含选中的 upstream 名与 `provider_api_key`，且不包含任何测试密钥值。
5. 原有 Registry、Scanner、Gateway、协议适配和前端测试全部通过。

真实 App 验收必须从 Finder 启动 `/Applications/token-station.app`，确认重新扫描能识别本机实际路径；启动代理并发一条 Codex 请求，确认缺密钥时能指出 provider，补回密钥后请求可以到达 provider。纯扫描前后不应改变第三方配置文件。

## 9. 发布、回滚与遗留项

实现完成后运行全量测试与桌面构建，再用 `scripts/install-local-desktop.sh` 更新本机 App。安装和真实界面验收完成后回写本文档状态，并使用中文说明创建本地 commit，不推送远端。

回滚只需恢复候选生成、Registry 路径和错误文案，不涉及数据迁移。任意自定义安装目录无法从安全的静态目录或 npm prefix 推导时，仍需后续设计手动选择入口功能；本次不通过无边界磁盘搜索覆盖它。

## 10. 实现与验收结果

2026-08-04 已完成以下实现：

- macOS Registry 补齐 `~/.npm-global/bin`、pnpm、Bun、Volta、OpenCode 官方目录和对应 Node 运行时位置。
- Scanner 以 64 KiB 上限只读解析 `~/.npmrc` 的绝对 `prefix`，为 Node package Agent 增加 `<prefix>/bin/<agent>` 候选；危险或相对值不生效。
- Scanner 受限枚举 `~/Library/Python/<数字版本>/bin/hermes`，最多检查 32 个版本目录。
- SecretStore 的 store、env、file 和空值错误统一显示 upstream 与 slot，不改变 401 协议结构，不包含密钥值。

自动化验证结果：

- `cargo test --workspace --all-targets`：通过。
- 桌面端 Vitest：27 个测试文件、240 条测试全部通过。
- `npm run build`：通过；保留现有大 chunk 警告，本次没有扩大前端 bundle。
- 真实 Gateway 测试：缺失 store key 的 Codex 请求返回 401，正文包含 `deepseek_release` 和 `provider_api_key`，provider 收到 0 次请求。
- `crates/router-core/**`：零修改。

本机 App 验收结果：

- `scripts/install-local-desktop.sh` 成功，`/Applications/token-station.app` 版本为 1.1.0，bundle id 为 `com.tokenstation.desktop`，代码签名校验通过，启动健康检查通过。
- 从真实安装版重新扫描后，Claude Code 被识别为多安装并要求选择，OpenCode 与 Hermes 显示“可接入”，OpenClaw 显示“安装入口存在”而不再是“未检测”。
- 本机没有 Gemini CLI 可执行文件，因此真实 App 仍显示“未检测”；Finder 瘦 PATH 下的 Gemini npm/pnpm/Bun/Volta 候选由自动化测试覆盖。
- 本机 OpenClaw 位于 `~/local/npm-global/bin/openclaw`，Scanner 已通过 npm prefix 找到它。实际 `openclaw --version` 报告 Node.js 22.19+ 才受支持，而本机 Node.js 为 22.14.0，因此 App 正确保持只读保护。这是本机运行时版本问题，不是继续漏扫。
- 使用已安装 App 启动本地代理后，请求 `/agents/codex/v1/responses` 得到 401：`upstream wec secret provider_api_key: not in the local store (re-enter the key)`。本机 `wec/provider_api_key` 确实缺失，请求未外发；验收后已停止代理。
- 真实重新扫描前后，Claude、OpenCode、OpenClaw 和 Hermes 配置文件 SHA-256 完全一致。Gemini 配置文件在本机不存在。

仍保留一个明确边界：安全路径表与 npm prefix 都无法推导的任意自定义目录不会自动发现。后续若需要覆盖此类安装，应设计“手动选择可执行入口”，不应改成递归扫描整个用户目录。
