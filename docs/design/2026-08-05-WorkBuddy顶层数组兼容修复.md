# WorkBuddy 顶层数组配置兼容修复

## 问题与目标

部分 Windows 和 macOS WorkBuddy 安装使用 `~/.workbuddy/models.json` 的原生数组格式，文件顶层直接是模型对象数组。Token Station 的通用 JSON 写入前复验只接受顶层对象，因此连接前会报“顶层不是对象”，即使文件内容本身是合法的 WorkBuddy 配置。

目标是让 WorkBuddy 连接器同时支持原生数组格式和既有的 `{ "models": [], "availableModels": [] }` 对象格式。连接时保留数组中的其他模型，断开时只删除 Token Station 自己写入的模型。

## 范围与非目标

本次只修改 WorkBuddy 配置解析、补丁应用和回归测试，不改变 Cursor、其他 Agent、路由协议或模型字段。不会把用户的数组文件自动重写成对象，也不会覆盖无关模型。

## 安全与数据红线

- 只允许修改 WorkBuddy 连接器声明的模型路径。
- 数组格式只能替换整个模型数组，替换值必须由原数组保留项加 Token Station 模型组成。
- 对象格式继续只修改 `models` 和 `availableModels`，保留其他顶层字段。
- 写入前继续执行语法解析、补丁所有权校验和投影复验。

## 用户可见行为与失败处理

合法的顶层数组应显示为可接入的 WorkBuddy 配置，不再提示“顶层不是对象”。非法 JSON、重复的 Token Station 模型、缺少适配器或模型字段不符合要求时继续拒绝写入，并保持原文件不变。

## 测试与验收

- 顶层数组连接会保留已有模型并追加 Token Station 模型。
- 顶层数组断开只删除 Token Station 模型。
- 对象格式现有连接、断开和无关字段保留测试继续通过。
- JSON 顶层标量仍被拒绝。
- 在目标 worktree 运行 agent_integration 相关 Rust 测试，再运行桌面构建验证。

## 实现落点与发布

实现位于 `config_codec.rs` 的 JSON 数组解析/根数组补丁兼容，以及 `connectors/workbuddy.rs` 的双格式分支。完成测试后在 `codex/cursor-compat-safe` 分支提交，供现有 PR #63 继续审查；不触碰实验 worktree。

## 原因与解决办法

原因是通用 `parse_rendered` 在连接器运行前把所有非对象 JSON 拒绝，而本机 WorkBuddy 的真实文件顶层是数组。解决办法是允许合法 JSON 数组进入补丁层，并让 WorkBuddy 根据根类型选择数组根替换或对象字段替换，同时保留所有权边界和回归测试。
