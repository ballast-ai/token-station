# 贡献者文档

面向要**维护、开发、测试** token-station 的人。建议按顺序读前两篇。

- [架构总览.md](架构总览.md) —— 仓库布局、各 crate 职责、一次请求的数据流、
  沙箱边界、改动落点。**先读这篇。**
- [开发环境.md](开发环境.md) —— 工具链、系统依赖、构建、起本地实例、本地门禁。
- [测试指南.md](测试指南.md) —— 测试分层、WASM guest、CI 三个 job。
- [贡献流程.md](贡献流程.md) —— 分支 / 提交 / PR 惯例、代码风格硬约束、依赖
  策略、安全相关改动的要求。
- [入站适配器-需求与实现路线.md](入站适配器-需求与实现路线.md) —— 入站侧工作流:
  agent 适配器需求、ABI 契约、实现路线。
- [桌面app-设计与交接.md](桌面app-设计与交接.md) —— `apps/desktop`(Tauri GUI)的
  架构、三档面板、v1 子页面、多入站编排与 CC 安全闸。

## 30 秒速览

- Rust workspace，`edition 2024`，MSRV 1.85，`unsafe_code = forbid`。
- 一个回环代理二进制（`apps/cli`）跑在共享路由内核（`router-core`）+ 沙箱
  WASM adapter（`plugin-runtime`）之上。
- **内容不离开本机**是硬约束：凭证不进日志、指标里没有能装 prompt 的列、
  插件无网络无密钥值。改到这些面要在 PR 里论证仍然安全。
- 公共地基改动 **upstream-first**：先落这个开源仓。

产品视角见 [../product/README.md](../product/README.md)。
