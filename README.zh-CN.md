# token-station

[English](README.md)

本地回环的 LLM 代理 + 本地路由。IDE、Agent 或任何 OpenAI 兼容客户端指向
`127.0.0.1`，token-station 按你写的规则把每个请求路由到合适的上游——你自己的
API key、你本机的模型——路由决策全部发生在你的机器上，prompt 的任何内容都
不会离开它。

```
IDE / Agent ──▶ 127.0.0.1:8787 ──▶ 规则 → hint → 启发式 → 默认池
                                        │
                     ┌──────────────────┼──────────────────┐
                     ▼                  ▼                  ▼
               api.openai.com     api.groq.com      localhost:11434
               （你的 key）        （你的 key）       （你的 Ollama）
```

## 为什么做这个

模型路由通常意味着一个能看到你流量的云端网关。token-station 押注相反的方向：
**路由是本地决策**。路由器、规则、指标、密钥全部在本机。代理的外联目的地
就是你配置的上游——外加一个匿名版本检查，且只在你运行 `upgrade` 时发生，
永不后台。

- **内容到不了磁盘。** 请求日志和指标库构建自一个字段全是数字、闭集枚举或
  你配置里名字的记录类型——没有一个列能装下 prompt；测试用金丝雀直接 grep
  数据库原始字节来证明这一点。
- **key 在 OS 钥匙串里**，按请求解析、注入一个 header、永不进日志。粘进
  URL 的 key 在配置加载时就被拒绝。
- **插件是沙箱 WASM。** provider adapter 只做协议翻译，没有网络、拿不到
  明文密钥；外泄闸门在凭据解析前核对每个出站请求是否指向你配置的端点。
- **默认开鉴权。** 本地虚拟 key 先于端口存在；回环是对网络的边界，不是对
  本机其他进程的。

## 快速开始

```bash
git clone https://github.com/ballast-ai/token-station
cd token-station
cargo build --release -p token-station-cli

cp apps/cli/example-config.json token-station.json
./target/release/token-station-cli upstream add openai_personal \
  --provider openai-compatible --base-url https://api.openai.com/v1 \
  --model "gpt-5.5,tool,vision,json-schema,ctx=400000" \
  --auth keyring --pool sota
./target/release/token-station-cli key set openai_personal provider_api_key
./target/release/token-station-cli serve
```

客户端指向 `http://127.0.0.1:8787/v1`，带上 `serve` 打印的虚拟 key。请求
`auto` 由路由器决策；请求具体模型就给你那个模型。

管理面在同一个二进制里：`upstream list/add/remove/test`、`rule list`、
`config set/edit`、`stats`（用量/错误/延迟/token，读本地指标库）。

## 验证官方二进制

官方发布的设计目标是可复现且有签名：Ed25519 签名的 manifest 证明发布者，在
发布 tag 上重建证明源码——`scripts/verify-release.sh` 替你做两道比对。

签名私钥离线保管。**注意（预发布）**：本构建尚未内置发布公钥
（`OFFICIAL_RELEASE_PUBKEY_HEX` 为空），因此 `upgrade` 会拒绝下载，不信任未
经验证的二进制。在经审查的发布构建注入公钥之前，请按
[docs/release/可复现构建与发布验证.md](docs/release/可复现构建与发布验证.md)
手动验证官方二进制。注入公钥是发布的前置条件，不是可选步骤。

## 状态

C1（最小可用客户端）已完成：流式代理、四层路由、上游健康摘除、OS 钥匙串
保管、指标库、CLI 管理面与发布工程。OpenAI 兼容上游（含 Ollama、vLLM 及
多数 BYOK 供应商）今天即可用；Anthropic / Gemini 原生 adapter 按社区反馈
排期。

面向用户的文档在 [docs/](docs/)：产品功能见 [docs/product/](docs/product/)，
维护 / 开发 / 测试见 [docs/contributing/](docs/contributing/)，上手指南见
[docs/guides/](docs/guides/)，发布验证与打包见 [docs/release/](docs/release/)。

## 许可

Apache-2.0。路由内核、插件 ABI 与本客户端是开放内核；托管平台（账号、
**仅元数据**的云同步、账单对账）基于同一批 crate 构建中。
