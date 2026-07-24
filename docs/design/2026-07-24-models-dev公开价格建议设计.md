# models.dev 公开价格建议设计

> 日期：2026-07-24
> 状态：已实现；专项自动化通过，存在合入前阻塞项
> 范围：Desktop 定价编辑器的公开价格查询、缓存、模型匹配与表单预填
> 核心边界：价格目录只给建议；用户点击“保存新版本”前，不写配置、不改变运行态、不生成价格版本

## 1. 背景

现有版本化定价编辑器要求用户手工录入五类 token 价格。新增模型时，如果本地价格表没有该模型，成本保持 unknown；这保证了统计不把“未知”误报为“免费”，但用户需要自行查价和换算。

CC Switch 一类工具的通用做法是维护内置价格或导入公共模型目录，再用规范化模型 ID 匹配 token 用量。本设计采用相同方向，但不把外部价格静默写入 Token Station：Desktop 按需读取 [models.dev](https://models.dev/) 的公开目录，仅预填编辑器，最终仍走已有版本化保存路径。

本功能是对《版本化价格编辑器与用量过滤设计》中“不从外部 Provider 自动同步价格”的受控扩展：

- 仍不自动同步有效价格表；
- 不在应用启动时批量联网；
- 不根据外部目录直接改变运行代理；
- 只有用户显式保存才产生 `PriceTable.version + 1`。

## 2. 目标与非目标

### 2.1 目标

1. 用户输入一个尚未定价的模型 ID 后，Desktop 可按需查找公开标价。
2. 支持精确模型 ID、已知官方 Provider 前缀，以及有限的安全别名规范化。
3. 把美元 / 1M tokens 换算为现有微单位字段，并预填输入、输出、缓存读、缓存写和可选推理价。
4. 显示数据源、命中的 Provider 和模型名称，要求用户核对并显式保存。
5. 使用 24 小时本地缓存；联网失败时允许使用最后一次有效缓存。
6. 遵循现有 egress 配置、代理凭据解析、超时、禁止重定向和响应体上限。
7. 不发送 Provider API Key、prompt、response、token 用量或历史 Receipt。

### 2.2 非目标

- 不声明 models.dev 覆盖“所有”模型或价格绝对准确。
- 不抓取各厂商价格网页，不调用带账户身份的计费 API。
- 不计算阶梯价、批处理价、地区税费、渠道折扣或账户实际账单。
- 不给同一模型持久化多套 Provider 价格；当前价格表仍以模型 ID 为键。
- 不改成本计算、Receipt、路由选择、插件 ABI、Canonical IR 或 metrics schema。
- 不在后台定时刷新，也不自动覆盖用户已经填写的金额。

## 3. 红线与改动边界

本功能限定在 Desktop 控制面：

- 新增 `apps/desktop/src-tauri/src/pricing_catalog.rs`；
- 扩展 `apps/desktop/src-tauri/src/lib.rs` 的只读建议命令；
- 扩展 `apps/desktop/src/api.ts`；
- 扩展 `apps/desktop/src/components/PricingEditor.tsx`；
- 对应增加 Rust、API 和 React 测试。

以下路径不得修改：

- `crates/protocol/**`（Canonical IR）；
- `crates/router-core/**`；
- `crates/plugin-api/**`；
- `crates/plugin-runtime/**`；
- `plugins/official/**`；
- `crates/metrics/**`。

现有 `set_model_price(expected_version, ...)` 仍是唯一写路径。建议命令没有配置写入、SQLite 写入和运行态应用能力。

## 4. 数据源与价格语义

数据源为：

```text
GET https://models.dev/api.json
Accept: application/json
```

models.dev 是社区维护的开源模型数据库，仓库采用 MIT License。其 API 把模型成本表示为 USD / 1M tokens，字段包括 `input`、`output` 以及可选的 `cache_read`、`cache_write`、`reasoning`。

映射规则：

| models.dev | Token Station | 规则 |
|---|---|---|
| `cost.input` | `input_per_mtok` | 必须存在，美元乘 `1_000_000` 后四舍五入为微单位 |
| `cost.output` | `output_per_mtok` | 必须存在，同上 |
| `cost.cache_read` | `cache_read_per_mtok` | 缺失按 0 预填 |
| `cost.cache_write` | `cache_write_per_mtok` | 缺失按 0 预填 |
| `cost.reasoning` | `reasoning_per_mtok` | 缺失保持 `null`，沿用现有“跟随输出”语义 |

金额必须有限且非负；越界目录数据被拒绝。界面明确标注“公开美元标价”，它是成本估算基线，不等于用户账户的真实发票。

## 5. 模型匹配

匹配优先级：

1. 调用方给出 Provider 时，在该 Provider 内精确匹配模型 ID；
2. 模型 ID 带已知 Provider 前缀时，在该 Provider 内匹配；
3. 无 Provider 时，按已知模型族推断官方 Provider，例如 GPT→OpenAI、Claude→Anthropic、Gemini→Google；
4. 无法推断时，只在全目录唯一命中时返回建议；
5. 跨 Provider 出现多个同名模型时返回 `None`，不猜价格。

允许的安全规范化：

- 移除与目标 Provider 一致的 `<provider>/` 前缀；
- 移除 `@high` 一类推理强度后缀；
- 移除 `:free` 一类渠道后缀；
- Claude 等型号中的点号/连字符变体；
- 末尾 `-YYYYMMDD` 或 `-YYYY-MM-DD` 日期快照。

未知命名空间被视为显式边界。例如 `unknown/gpt-5` 不会退化成 OpenAI 的 `gpt-5`。

## 6. 缓存与网络边界

缓存文件：

```text
<data.dir>/catalogs/models-dev-api.json
```

行为：

- 只在用户输入未定价模型后按需查询；
- 350 ms 防抖，不在应用启动时请求；
- 缓存 TTL 为 24 小时；
- 新鲜缓存直接使用；
- 缓存过期后尝试联网，失败则使用旧缓存；
- 损坏或超过 8 MiB 的缓存被忽略；
- 成功响应经临时文件写入后重命名；
- 缓存只包含 models.dev 的公开 JSON，不含密钥、请求头、错误正文或用户数据。

网络请求复用当前 `EgressConfig`：

- 6 秒全局超时；
- 最大响应 8 MiB；
- `max_redirects = 0`；
- 支持现有 HTTP/HTTPS/SOCKS 代理和 `no_proxy`；
- 如代理本身需要鉴权，只从现有 SecretStore 解析代理凭据；
- 请求不携带任何模型 Provider API Key。

## 7. IPC 与交互

只读命令：

```text
suggest_model_price(provider_id?: string, model_id: string)
  -> Option<ModelPriceSuggestionView>
```

返回命中模型、Provider、来源、获取时间和五类价格。命令不接收价格版本，也不能保存价格。

交互流程：

```text
输入未定价模型 ID
  -> 350 ms 防抖
  -> Tauri 按需读取缓存 / models.dev
  -> 安全匹配并换算金额
  -> 表单预填并展示来源
  -> 用户核对
  -> 点击“保存新版本”
  -> 现有 set_model_price(expected_version)
  -> 生成新的 PriceTable 版本
```

保护规则：

- 已在当前价格表中的模型不触发建议；
- 用户开始编辑任一金额后，不再发起或应用自动建议；
- 旧模型请求返回时如果输入已变化，结果被丢弃；
- 查不到、离线或目录错误不阻塞手工输入；
- 建议成功也不自动调用 `set_model_price`。

## 8. 成本与历史不变量

本功能不改变成本计算链：

1. 用户保存后，新价格仍通过现有 `PriceTable` 版本化。
2. 正在运行的代理仍需按现有流程重新应用配置。
3. 新请求按生效时的价格版本计算。
4. 历史 Receipt 已固化的 `cost_micros / cost_kind / price_version` 不重算。
5. 未命中且用户未保存时，模型成本继续保持 unknown，而不是 0。

## 9. 测试与验收

### 9.1 Rust

- Provider 精确匹配与 USD→微单位换算；
- 官方 Provider 推断和安全别名规范化；
- 未知命名空间不越界；
- 跨 Provider 歧义不猜测；
- 请求超时、禁止重定向、响应体上限；
- 缓存新鲜命中、过期回退和损坏降级。

### 9.2 React / IPC

- API 参数映射使用 `providerId / modelId`；
- 建议只预填，不自动保存；
- 用户确认后复用 `setModelPrice(..., expectedVersion)`；
- 用户已经开始输入时不查询、不覆盖；
- 已配置模型不触发建议；
- 查询失败时仍可手工保存。

### 9.3 工程门禁

- Desktop Rust 全量测试；
- Desktop Vitest 全量测试；
- TypeScript / Vite 构建；
- clippy 与 fmt；
- `git diff` 验证红线路径零改动。

## 10. 已知限制与后续方向

当前 `PriceTable` 以模型 ID 而不是 `(provider, model)` 为键，所以第一阶段优先采用官方 Provider 的公开价格；代理商、云平台或聚合渠道的同名模型可能价格不同。若未来需要按渠道精确计费，应单独设计 Provider-aware pricing key、迁移策略和 Receipt 兼容性，不能在本功能中绕过现有版本契约。

models.dev 是社区目录，新增模型能否自动预填取决于目录是否已收录且模型 ID 可安全匹配。未收录、歧义或特殊计价模型仍需手工填写。

深度审查确认以下问题必须在本 Draft PR 合入前处理：

1. 从已有模型切换到另一个模型时，表单的 `priceTouched` 和旧价格未与模型 ID
   一起重置，可能把前一个模型的价格保存到新模型。
2. 只读建议命令依赖整份草稿 `materialize()`；其他配置区域处于合法编辑中间态时，
   建议会被无关校验阻断，且前端当前会静默吞掉错误。
3. 过期缓存存在时，如果联网成功但响应 JSON 无效，当前不会回退到最后一次有效缓存。
4. 建议请求只有前端结果取消，没有后端 singleflight、并发上限或解析缓存；大量并发
   IPC 调用可能重复下载或解析最多 8 MiB 的目录。

非 ASCII 模型 ID 触发日期后缀检测 panic 的问题已在本 PR 中修复，并增加专项回归测试。

## 11. 回滚

删除 Desktop 建议命令、前端预填逻辑和公开目录缓存即可回滚。由于没有配置 schema、IR、Receipt 或 metrics 迁移，回滚不会修改已有价格版本和历史成本。

## 12. 验收结果

2026-07-24 本地 fresh verification：

- Desktop Rust：186 passed，0 failed，1 ignored；
- 定价目录专项 Rust：6 passed；
- Desktop Vitest：156 passed；
- TypeScript / Vite 生产构建：通过；
- clippy `--all-targets -- -D warnings`：通过；
- `git diff --check`：通过；
- 定价功能自身未修改 Canonical IR、router-core、plugin-api、plugin-runtime、
  official plugins 或 metrics；本汇总 PR 中的 Claude Desktop Thinking 变更属于独立改动。

仓库级 `cargo fmt --check` 仍会报告任务开始前已存在的
`apps/desktop/src-tauri/src/agent_integration/commands.rs` 格式差异。本次未修改该文件，
新增 `pricing_catalog.rs` 已单独通过 rustfmt，避免把无关格式化混入功能差异。
