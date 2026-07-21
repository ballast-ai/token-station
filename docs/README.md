# token-station 文档

本目录只放**面向用户**的文档。详细设计 / 规划 / research 在私有文档仓
`token-station-doc`，不在这里。

## 按角色导航

### 我要用它（使用者）

- [产品文档入口](product/README.md) —— 功能、CLI、配置、路由、插件
- 快速上手：[GLM-5.2 本地启动指南](guides/GLM-5.2-本地启动指南.md)
- 验证官方二进制：[可复现构建与发布验证](release/可复现构建与发布验证.md)

### 我要维护 / 开发 / 测试它（贡献者）

- [贡献者文档入口](contributing/README.md) —— 架构、开发环境、测试、贡献流程
- [桌面 App Agent 接入机制](contributing/桌面App-Agent接入机制.md)
- [Agent Connector 准入指南](contributing/Agent-Connector-准入指南.md)
- [Agent 兼容目录发布与回滚](release/Agent-兼容目录发布与回滚.md)

## 目录结构

```
docs/
  product/       产品功能文档（使用者）
  contributing/  贡献者文档（维护 / 开发 / 测试）
  guides/        端到端上手指南
  release/       发布验证与打包
  verification/  自动化、真实 E2E 与发布前验收证据
```
