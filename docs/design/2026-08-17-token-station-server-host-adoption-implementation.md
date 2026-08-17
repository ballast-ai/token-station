# `token-station-server` 宿主接入实施记录

> 方案:`2026-08-17-token-station-server-host-adoption-plan.md`(裁决与验收标准以方案为准,本文只记事实)。
>
> 企业仓真相源:`token-station-server` 仓 `docs/product-review/34-south宿主接入-执行记录.md`。
>
> 状态:P0–P4 完成;P5 剩 wiring review(人工硬门禁)、`compatibility.json` 翻转、企业仓远端 CI 凭证、合入 `dev`。
>
> 更新日期:2026-08-17

## 1. 结论

`token-station-south` 的 minimal ProviderCall 已在企业网关的一条真实业务路径(`/v1/embeddings` OpenAI-compat 分支)上完成接入:纯 Bearer scope 内的 provider 经 south 的绑定校验、凭证解析编排与加固 transport 发出请求,sealed settlement 资金不变式全程保持。宿主以真实组装路径通过 `south.provider-call.v1` 全部 7 用例。

**这证明了 ProviderCall v1 形状的对接可行性,不等于 host verified**:south 定义的 verified 还差人工 wiring review 与 `compatibility.json` 翻转;也不改变能力边界——流式/multipart/GET/非 Bearer 认证仍在 south scope 之外。

## 2. 关键事实

| 事项 | 事实 |
|---|---|
| south 仓 P0 | PR #1 合入 `main`(`1355fa8`),远端 CI 全绿,tag `v0.0.1`(指 `3b6c91f`)已发布 |
| 企业仓分支 | `feat/south-host-adoption`,基于 `provider-crate-plan` |
| 纵切实现 | `d3ab9136`(adapter + embeddings 二分路由 + 验收测试腿) |
| 依赖切换 | `d54a774c`(path → `git+ssh…tag=v0.0.1`;`.cargo/config.toml` 开 `net.git-fetch-with-cli`;deny sources 白名单) |
| 立项/裁决记录 | 企业仓 34 号文档(`d079484c`):south 为企业仓第一个带 I/O 的外部 crate,显式立项;D1/D2/D3 按方案推荐口径执行 |
| 验收 | conformance 7/7;balance/quota 两腿 nextest 2488/2488;两档+config-center 档 clippy、fmt、deny、棘轮、doctest 全绿;PG 腿/长测 SKIP(无本地容器) |

## 3. 与方案的偏差

1. **"直接切换"落地为路由资格二分**:`/v1/embeddings` 同端点还服务非纯 Bearer 供应商(Azure 系等),全量切换会破坏它们。scope/URL 分解/slot 词法/api_key/transport 可用五项资格判定全部在 admit 之前,失败回落 legacy——能力边界,非运维开关。
2. **行为收紧两处**(D3 派生,已被测试钉住):south 路径不跟随 `HTTP_PROXY`;3xx 一律 `REDIRECT_DENIED` 不追随。302 pin 测试同时是"south 路径真实生效"的证明。

## 4. 剩余门禁(顺序)

1. wiring review:人工核对企业仓 `gateway/tests/south_adoption.rs` 的证据接线(方案 §5 P5.2)。
2. south 仓 PR:`compatibility.json` 的 `token-station-server` → verified。
3. 企业仓远端 CI 凭证:south 是 `ballast-ai` 下**私有仓**,CI 构建需 deploy key/token,未配置前 dev→main PR 的 CI 会在依赖抓取失败。
4. `feat/south-host-adoption` → `dev` 合并(先跑满 `run_local_matrix.sh`)。
5. 顺手项:south 仓 `Cargo.toml` 的 `repository` 字段误写 `GlimpseEngine/token-station-south`,实际是 `ballast-ai`,待修。

## 5. 对既有文档的影响

- 验收清单(`2026-08-16-token-station-south-acceptance-checklist.md`)§6.3 设想的 `post_json_attempt` 改造路线**不成立**:该系列 API 在企业仓已被源码棘轮封印弃用,且其返回裸 `reqwest::Response`(可流式)与 ProviderCall 的 buffered 契约不兼容。第一刀改落 embeddings,已记入方案 §8。
- `compatibility.json` 的 `token-station`(社区版宿主)仍为 `not_verified`,其接入是独立纵切,不受本次影响。
