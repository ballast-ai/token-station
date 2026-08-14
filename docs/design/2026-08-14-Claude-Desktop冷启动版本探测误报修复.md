# Claude Desktop 冷启动版本探测误报修复

## 问题

Token Station 1.1.3 扫描 Claude Desktop 时执行
`/Applications/Claude.app/Contents/MacOS/Claude --version`，Registry 给该进程的预算只有
2 秒。真实机器上的首次冷启动实测需要 3.38 秒，后续热启动约 0.13 秒，因此首次扫描会把
健康安装误判为 `VERSION_PROBE_TIMEOUT` / `INSTALLED_BROKEN`。前端又通过通用
`timeout` 正则把该本地探测错误展示成“请检查网络连接”，造成错误归因。

## 目标

1. 扫描 Claude Desktop 时不启动其 GUI 可执行文件，不再受冷启动时间影响。
2. 保留可执行文件存在性、真实路径与普通文件检查；不降低配置只读预检、Connector 绑定、
   版本黑名单之外的安全准入边界。
3. 即使其他 Agent 以后仍发生 `VERSION_PROBE_TIMEOUT`，也展示准确的本地版本检测提示，
   不再误报网络故障。
4. 用公开行为测试锁定 Registry 契约、无进程探测和中文错误文案。

## 范围与非目标

本次把 Claude Desktop 的 `version_probe` 改为已有的 `passive_file` 运行时。该运行时只验证
扫描到的入口与复核入口解析为同一普通文件，不启动进程，并返回“版本未知”。当前兼容目录对
Claude Desktop 没有版本封禁规则，`evaluate_discovery` 允许版本未知但通过其他只读预检的安装
进入 `DETECTED_VERIFIED`，因此不会扩大当前可接入版本范围。

本次不新增 macOS `Info.plist` / Windows PE Version Resource 解析器，也不修改 Claude Desktop
3P 配置写入与恢复逻辑。如果未来需要按 Claude Desktop 版本执行封禁，应先新增无进程的跨平台
应用元数据探测，再启用对应规则，不能恢复执行 GUI `--version`。

## 安全与数据红线

- 扫描只读，不写 Claude Desktop、Claude-3p 或 Token Station 配置。
- 不启动、关闭或重启 Claude Desktop，不读取用户会话内容。
- 被动探测仍复核 canonical path 与普通文件类型，入口变化时失败关闭。
- 不修改虚拟 Key、Provider、路由、ownership、快照与恢复语义。

## 用户可见行为与失败处理

- 健康的 Claude Desktop 在首次冷扫描时直接显示“可接入”，不再短暂显示“暂不可接入”。
- 扫描期间不会拉起 Claude Desktop，也不会因其启动速度阻塞 Agent 页面。
- 其他 Agent 若确实发生 `VERSION_PROBE_TIMEOUT`，中文提示为“Agent 版本检测超时。请重新扫描；
  如果仍然失败，请检查该 Agent 的安装是否完整。”英文显示等价信息。
- 真正的网络、证书、Base URL 和代理错误仍沿用现有网络提示。

## 响应式、键盘与可访问性

本次不改变布局、焦点顺序、按钮结构或键盘交互。状态标签和详情文本继续使用现有语义化元素，
窄窗口行为不变。

## 测试与验收

1. Registry 测试断言 Claude Desktop 使用空参数、`SUCCESS_ONLY` 和 `passive_file`。
2. Discovery 测试使用会写 marker 的假 GUI，断言扫描成功且 marker 不存在。
3. 前端错误测试断言 `VERSION_PROBE_TIMEOUT` 优先命中本地版本探测文案，而不是网络文案。
4. 运行桌面端 Rust 全量测试、前端全量测试和正式构建。
5. 通过官方本地安装脚本替换 `/Applications/token-station.app`，在真实 App 中重新扫描 Claude
   Desktop；确认状态可接入，且扫描期间没有新 Claude 进程。

## 实现落点

- `apps/desktop/src-tauri/agent-registry/builtin-agents.json`
- `apps/desktop/src-tauri/src/agent_integration/registry.rs`
- `apps/desktop/src-tauri/src/agent_integration/discovery.rs`
- `apps/desktop/src/errors.ts`
- `apps/desktop/src/errors.test.ts`

## 实现状态

已实现并完成本机验收：

- Claude Desktop Registry 已切换为 `passive_file`，扫描不再执行 GUI `--version`。
- 新增真实 descriptor + 假慢 GUI 的回归测试；修复前稳定得到
  `VERSION_PROBE_TIMEOUT`，修复后扫描约 0.01 秒完成且 marker 未生成。
- `VERSION_PROBE_TIMEOUT` 已按稳定错误码优先展示本地版本检测提示。
- 桌面 Rust 全量测试通过：351 passed、2 ignored；前端测试与覆盖率通过：397 passed，
  statements 83.3%、branches 78.11%、functions 80.38%、lines 86.04%；前端正式构建通过。
- `scripts/install-local-desktop.sh` 完成隔离 Release 构建、bundle 签名校验、产物审计、
  事务性替换和启动健康检查。
- 真实 App 点击“重新扫描”后 Claude Desktop 显示“可接入”，扫描前后均未发现
  `/Applications/Claude.app/Contents/MacOS/Claude` 进程；随后用户在界面完成接入，状态正常显示
  “已接入，请求已通过 Token Station”。

仓库级 `cargo fmt --check` 与 Clippy 仍被本次任务开始前已有的未提交代码阻塞：格式差异位于
`commands.rs`、`lib.rs`、`serve_lifecycle.rs` 等非本次修改文件；Clippy 报告位于
`desktop_shell.rs` 与 `lib.rs` 的 4 个既有告警。本次没有覆盖或顺带修复这些用户改动。
