# WorkBuddy 国内版与海外版合并检测

日期：2026-08-04

状态：本地实现、测试和真实 App 验收完成，未推送远端

## 1. 问题、目标和范围

本机同时安装 `/Applications/WorkBuddy.app` 5.3.8 和
`/Applications/WorkBuddy AI.app` 5.2.7。两者由同一个腾讯 Team ID 签名，包内都使用
`@genie/agent-cli` 和 `cli/bin/codebuddy`，属于同一 WorkBuddy 产品家族。海外版使用独立
bundle id `com.workbuddy.workbuddy-ai`，并在 `product.json` 声明数据目录
`.workbuddy-ai`；国内版使用 `.workbuddy`。

目标是在 Token Station 中继续只显示一个 WorkBuddy 卡片。两份 App 作为同一个
`agent_id = workbuddy` 的多个安装实例出现，用户选择精确安装后执行同一个
`workbuddy-v1` Connector。国内版写 `~/.workbuddy/models.json`，海外版写
`~/.workbuddy-ai/models.json`。

本轮不增加 WorkBuddy AI 卡片、Agent ID、路由命名空间、图标或 Connector，也不修改
模型格式、图片降级、工具调用和路由策略。

## 2. 安全与数据红线

1. 扫描只执行 Registry 已声明的精确 CLI 路径和既有 `--version` 参数，不启动 GUI。
2. 安装实例必须绑定自己的配置路径。选择海外版时不能写国内版配置，反之亦然。
3. 多份安装继续进入冲突选择状态，未选择精确安装前不能生成写入计划。
4. 配置目标仍由服务端扫描结果决定，前端不能提交任意文件路径。
5. 备份、ownership、恢复和断开继续绑定安装路径与精确配置文件。
6. 不读取或输出 models.json 中的 API Key。

## 3. 用户可见行为和失败处理

只安装任一版本时，Agent 页面显示一个 WorkBuddy，并直接使用该版本对应的配置目录。
同时安装两个版本时，仍只显示一个 WorkBuddy 卡片，卡片要求用户从两份安装中选择一份。
选择后的一键接入、恢复和断开只操作该安装对应的 models.json。

如果海外版 CLI 无法通过版本探测，它仍按现有规则显示为已发现但不可接入，国内版不受
影响。配置路径条件无法匹配时使用 WorkBuddy 既有国内版默认目录，不能猜测其他目录。

界面结构、键盘顺序、响应式和可访问性不变；安装选择继续使用现有可聚焦控件和文字状态。

## 4. 最小架构

1. WorkBuddy `known_install_locations.macos` 同时声明两个 App 内的 `codebuddy`。
2. `ConfigLocation` 增加可选的安装路径条件。存在匹配条件时只使用匹配的配置位置；没有
   匹配项时使用没有条件的默认位置。
3. WorkBuddy 国内版保留无条件默认 `~/.workbuddy/models.json`，海外版增加条件位置
   `~/.workbuddy-ai/models.json`，条件只匹配 `WorkBuddy AI.app` 的精确 CLI 路径。
4. 扫描器按每个安装实例分别解析、检查和计算配置指纹，不能把两份 models.json 的状态
   混成一个指纹。
5. Connector 不新增分支；计划继续取该安装记录的第一个服务端配置候选。

## 5. 测试和验收

公开行为测试必须证明：

1. Registry 仍只有一个 `workbuddy`，但包含两个 macOS 安装路径。
2. 国内版记录的首个配置候选是 `~/.workbuddy/models.json`。
3. 海外版记录的首个配置候选是 `~/.workbuddy-ai/models.json`。
4. 两版同时存在时生成两份同 Agent 安装记录，并进入现有精确安装选择状态。
5. 修改海外版 models.json 只改变海外版记录的配置指纹。
6. 安装路径条件包含相对路径、遍历、未知变量或与平台不一致时 Registry 拒绝加载。
7. 既有 WorkBuddy Connector 的保留字段、接入、恢复和断开测试继续通过。

本地门禁包括 Registry、Discovery、Connector 和 Agent 页面测试，Desktop Rust 全量测试、
Clippy、前端测试与生产构建。完成后执行 `scripts/install-local-desktop.sh`，在真实 Agent 页面
确认只有一个 WorkBuddy 卡片，同时能看到两份安装并分别指向正确配置目录。macOS 验收不能
替代未来 Windows 版本的真实路径验证。

## 6. 实现和发布

实现落点预计为：

- `apps/desktop/src-tauri/agent-registry/builtin-agents.json`
- `apps/desktop/src-tauri/src/agent_integration/types.rs`
- `apps/desktop/src-tauri/src/agent_integration/registry.rs`
- `apps/desktop/src-tauri/src/agent_integration/platform.rs`
- `apps/desktop/src-tauri/src/agent_integration/discovery.rs`

本轮在独立本地分支 `codex/workbuddy-ai-variant` 完成，不更新 PR #63。测试和真实 App 验收
结束后回写本节状态；是否提交新 PR 由用户另行决定。

## 7. 实现与验收记录

已完成：

- Registry 仍只有一个 `workbuddy`，并声明国内版与海外版两份 macOS CLI 安装路径。
- 配置位置支持按已验证安装路径选择；匹配海外版时只返回
  `~/.workbuddy-ai/models.json`，其他 WorkBuddy 安装继续使用
  `~/.workbuddy/models.json`。
- 扫描器按安装实例分别检查配置并计算指纹，ownership、计划和恢复继续绑定精确安装与
  精确配置文件。
- Registry 拒绝把安装条件指向未知路径或没有对应配置默认值的平台。

2026-08-04 本地验收：

- Desktop Rust：238 通过，1 个既有真机探测测试按标记忽略；安装器测试 2 通过，YAML
  回归测试 3 通过。
- Desktop Clippy `--all-targets -- -D warnings` 通过。
- Rust 格式和冻结 Router tree 检查通过。
- `scripts/install-local-desktop.sh` 完成生产前端构建、Desktop 构建、artifact 审计、签名
  检查、安装和启动。
- 真实 Agent 页面仍显示 9 张 Agent 卡片，其中只有一张 WorkBuddy。该卡片显示“多实例”，
  安装列表同时出现海外版 `WorkBuddy AI.app` CLI v2.106.4 和国内版 `WorkBuddy.app` CLI
  v2.115.0。
- 本轮没有点击一键接入，没有修改 `~/.workbuddy/models.json` 或
  `~/.workbuddy-ai/models.json`。

本地分支为 `codex/workbuddy-ai-variant`。没有推送远端，也没有更新 PR #63。
