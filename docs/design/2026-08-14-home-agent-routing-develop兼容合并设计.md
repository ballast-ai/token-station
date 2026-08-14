# Home、Agent、请求日志与模型目录增量兼容合并设计

- 日期：2026-08-14
- 状态：已实施，待远端 CI 与合并
- 目标分支：`develop`
- 集成分支：`codex/home-agent-routing-develop`
- 远端基线：`b2458f76927a0f138470c2f9b024a0c9b14c4e71`
- 原始增量：`7d854d06639c3b75a0ced3a2df0de990b9383b3d..3614d4ddaaa0e2233e5dce39d8d049aa11dc5269`

## 问题、目标、范围和非目标

远端 `develop` 已通过 PR #86 兼容移植原分支截至 `7d854d0` 的 11 个提交，但原分支随后继续产生
13 个提交。直接合并原分支会重复引入已移植历史，并与远端 Agent 生命周期、网关、模型目录、桌面
Shell、前端状态及公开设计文档产生冲突。

本次目标是在最新 `develop` 的安全与兼容契约上，只移植 `7d854d0` 之后的增量，保留请求日志明文
查看与清理、请求语义解析、Provider 与额度交互修复、主题和语言同步、新手引导、Agent 显示与重新扫描、
状态栏快捷操作、Claude Desktop 扫描及 adaptive thinking 兼容等用户价值。

范围包括冲突解析、必要兼容修复、公开行为测试、全量构建测试、本机桌面 App 更新、真实界面验收、
集成分支推送和 PR 合入。非目标包括版本号变更、Release、DMG/MSI 打包或发布、读取真实凭据或请求正文、
清理原工作区、重写已经合入的 PR #86 历史，以及无关架构重构。

## 安全与数据红线

- 原工作区的修改与未跟踪文件属于用户数据，不得 stash、clean、移动、覆盖或纳入本次提交。
- 远端已有的 Agent revision、所有权、脱敏、原子写入、私有备份和失败闭合契约优先，不得用旧实现回退。
- 网关继续只监听 loopback，保持本地鉴权、目标与凭据绑定、未知路由失败闭合。
- 请求正文只在本机、用户主动操作且已有权限边界内展示或清理；不得写入日志、诊断或远端服务。
- 清理脚本必须显式选择目标，限制为可证明的请求正文缓存，不得使用宽泛路径、通配删除或跟随符号链接。
- Claude Desktop 与插件转换不得泄露 thinking 正文、凭据、用户路径或第三方未受管配置。
- 本地安装只允许替换 bundle id 为 `com.tokenstation.desktop` 的 `/Applications/token-station.app`；失败必须保留旧 App。
- 不直接推送覆盖 `develop`，不绕过 CI；通过集成分支 PR 合入。

## 用户可见交互、状态变化和失败处理

- 请求日志可区分原始请求、语义解析结果和正文不可用状态；清理成功、部分失败和权限失败必须可行动。
- Provider、凭据和额度操作保持选择值、应用门禁与错误提示一致，不能因刷新或目录失败丢失用户选择。
- 主题跟随系统与显式选择保持单一状态源；首次语言自动识别后允许用户覆盖，切换不要求重启。
- 未检测 Agent 的显示开关、主页状态、手动重新扫描和新手引导入口保持可达；扫描失败不伪装为未安装。
- 状态栏菜单的 Agent 快切、重新扫描、启动、停止和打开页面必须使用当前 generation/state，失败不得静默。
- Claude Desktop 扫描兼容受支持布局；adaptive thinking 转换保持类型、预算与流式事件顺序，未知结构失败闭合。
- 后台常驻、关闭重开、代理启动停止和单实例行为继续服从远端 `develop` 的生命周期实现。

## 响应式、键盘操作和可访问性

- 900×600 及常用桌面尺寸下，请求详情、Agent 页面、新手引导、Provider 与额度面板不得遮挡关键操作或横向溢出。
- 新增按钮、菜单和开关需保留可访问名称、可见焦点、禁用态和状态关系，不只用颜色表达结果。
- Tab、Enter、Space、Escape 与现有对话框/菜单契约一致；状态栏导航映射必须有纯函数测试。
- 主题和语言变化不得造成焦点丢失、重复 toast 或不可恢复的中间状态。

## 公开测试边界、验收标准和真实 App 检查项

测试只观察 CLI、IPC、DOM、公开纯函数、隔离文件和进程状态，不读取真实第三方配置、凭据或请求正文。

1. 每个冲突都追溯远端与原提交意图；远端安全契约优先，同时为保留的新增行为保留或补充回归测试。
2. `git diff --check`、Rust fmt/Clippy/test/doc、Desktop Rust 门禁、前端测试与生产构建通过。
3. 请求正文清理在隔离目录验证选择范围、缺失文件、重复执行、符号链接、非法路径和部分失败。
4. Agent 扫描、显示偏好、重新扫描、状态栏导航、Claude Desktop 与 adaptive thinking 覆盖成功及失败边界。
5. `scripts/install-local-desktop.sh` 完成官方构建、审计、bundle id、签名、固定路径替换和启动。
6. 真实 App 检查 Home、Agent、请求日志、Provider/额度、Settings、主题、语言、新手引导和状态栏入口；
   写入行为只使用隔离测试数据，不触碰真实 Agent 配置和请求正文。
7. 推送后等待 GitHub CI 与平台门禁完成；任何失败必须区分产品、测试夹具和环境层，修复后重跑。

## 实现落点、已知遗留项和发布要求

主要落点为 `apps/cli/src/bodylog.rs`、`apps/cli/src/gateway.rs`、Desktop Agent integration、
`desktop_shell.rs`、`model_catalog.rs`、请求日志与设置相关 React 组件、Router Core、Anthropic/OpenAI-compatible
插件，以及 `scripts/cleanup-request-bodies.sh`。冲突解析不得恢复远端已删除的内部设计快照；稳定规则写入本记录，
功能专项设计随对应增量移植。

实施后回写实际冲突、兼容取舍、测试、构建、App 安装、真实界面、提交、PR 和 CI 结果。未完成上述门槛前，
不得声明已合入 `develop`。本次不创建或发布 DMG/MSI/Release；可执行行为在下一正式发布前仅存在于源码和本地 App。

## 实施与验收结果

### 兼容移植与冲突取舍

- 基于 `b2458f76927a0f138470c2f9b024a0c9b14c4e71` 建立独立 worktree，只移植原分支
  `7d854d0..3614d4d` 的 13 个新增提交；PR #86 已移植的 11 个提交未重复引入。
- 共解析 34 处文本冲突。Agent 状态、revision、所有权、OpenCode、代理生命周期和动态模型文档继续采用
  `develop` 实现；在其上叠加主页 Agent 显示/扫描、状态栏导航、请求正文、主题语言、新手引导和 Provider 增量。
- 远端已经删除的 Gate D 内部设计快照保持删除，没有借冲突恢复；功能专项设计与公开测试随增量保留。
- Claude Desktop adaptive thinking 测试显式声明 `reasoning_effort` 能力：已声明模型继续传递，未声明模型按
  安全门禁降级，避免测试夹具误把旧行为当成公开契约。
- 请求正文快照读取新增私有文件校验，拒绝符号链接和非私有文件；请求 ID 限制为生成器实际使用的小写十六进制，
  打开日志失败不再泄露绝对目录。补充非法大写 ID 与符号链接回归测试。
- Router Core 的额度优先空候选失败闭合是本次设计行为，冻结树哈希更新为
  `7119bdc4618af62452ceadbe8d72b9e5a9aaea61`，未放宽格式/越界检查。

### 自动化验证

- `git diff --check`、`scripts/check-rust-format.sh` 通过。
- 根工作区 `cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`、
  `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps` 全部通过；Rust 1.95
  `cargo +1.95.0 check --workspace --all-targets` 通过。
- Desktop Rust 严格 Clippy、rustdoc 和测试通过：主 crate 353 项通过、2 项既有忽略；集成测试
  1 + 10 + 2 + 9 项通过。
- 前端 `npm ci` 无漏洞，`npm run test:coverage` 36 个文件、404 项测试通过，行覆盖率 86.09%；
  `npm run build` 通过，仅保留既有大 chunk 提示。
- 请求正文清理脚本在隔离临时目录验证：2 个合法候选中只删除过期文件；近期文件、大写 ID、无关文件、
  嵌套文件、符号链接及其目标均保留。
- MSI、scaffold prefetch、macOS DMG 策略、release assets、公钥、Desktop release 依赖、版本一致性、
  build-desktop 输出和 updater release 门禁全部通过。
- `scripts/test-router-core-redline.sh` 指向当前仓库不存在的既有
  `scripts/check-router-core-redline.sh`，因此无法运行；该入口在基线 `develop` 同样缺失，且不属于当前 CI。

### 本地 App 与真实界面

- `scripts/install-local-desktop.sh` 成功：官方插件、前端生产构建、release App、审计、ad-hoc 签名、
  固定路径安装和启动全部完成；`/Applications/token-station.app` 的 bundle id 为
  `com.tokenstation.desktop`，严格签名校验通过，进程正常运行。
- 通过 macOS 辅助功能接口只读验收：主页展示 7 个本机 Agent、重新扫描与路由配置可达；设置页 Agent
  显示开关完整；添加供应商目录展示 42 个常规供应商，DeepSeek 配置和高级凭据来源可展开；用量页与请求日志
  切换正常，日志详情可展开并提供解析视图/原文切换。代理全程保持 `127.0.0.1:8787` 运行。
- 验收未显示、复制或修改虚拟 Key，未修改供应商、Agent、额度、主题、语言或新手引导状态，也未输出请求正文。
- Computer Use 无法读取 `SystemUIServer`，状态栏菜单未进行真实点击；其 Agent 快切、重扫、启动/停止和页面
  映射由桌面 Rust/纯函数自动化测试覆盖。这是验收工具边界，不作为产品通过的替代证据。

### 远端状态

- 已推送 PR #89。首轮 `frontend` 在较慢 Linux runner 上暴露测试夹具竞态：手动重扫用例覆盖 IPC mock 时
  漏掉 `get_runtime_state` 轮询和 `get_request_receipts`，后台错误与预期重扫错误同时产生多个 alert；产品行为、
  覆盖率阈值及其余前端测试未失败。
- 测试夹具已补全两类后台调用，并将断言限定在通知容器；修复后重新执行本地前端覆盖率并触发 CI。
- 首轮 `desktop-rust` 同时发现状态栏 Agent 菜单投影方法只在 macOS 调用、却未声明平台编译边界；Linux
  `-D warnings` 将其判为 dead code。方法现与实际调用统一使用 `cfg(target_os = "macos")`，不改变 macOS 行为。
- 必须等待 PR #89 的 GitHub CI 全绿再合入 `develop`；最终合并状态以 PR 记录为准。
