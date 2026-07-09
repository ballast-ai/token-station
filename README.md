# token-station

token-station 是面向 AI Agent 工具和 LLM 模型供应商的智能模型路由器。

当前仓库是开源社区版与公共内核的开发主仓，目标是先落实 V2 计划中的公共地基：

- `router-core`：规则、hint、启发式和后续本地分类器的路由内核；
- `protocol`：Canonical IR、AgentHint 和基础协议类型；
- `plugin-api` / `plugin-runtime`：`agent-adapter` 与 `provider-adapter` 双端插件 ABI 与运行时；
- `conformance`：插件准入测试套件；
- `metrics`：本地指标与用量观测基础库；
- `apps/cli`：个人本地版命令行和本地代理入口。

## 开工依据

- [V2 开发计划](docs/planning/token-station-V2开发计划.md)
- [个人版（社区版客户端）开发计划](docs/planning/个人版开发计划.md)
- [V2 阶段需求清单](docs/planning/V2阶段需求清单.md)
- [代码仓库规划](docs/architecture/代码仓库规划.md)
- [双端 adapter 插件架构](docs/architecture/protocol-provider-adapter插件架构.md)
- [个人模式本地客户端概要设计](docs/architecture/个人模式本地客户端概要设计.md)
- [CLI 命令行交互设计](docs/architecture/cli命令行交互设计.md)
- [数据库设计 V1 基线与 V2 增量](docs/architecture/数据库设计-V1基线与V2增量.md)

## 本地检查

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
