# Token Station 与 CC Switch 3.19 特殊接口兼容差距修改建议

## 文档定位与红线

这是一份测试版协议审计和修改建议，不是实现记录。本文只回答一个问题：Token Station
目前是否已经像 CC Switch 一样，能正确处理 Claude Code、Codex 和其他 Agent 带来的特殊
工具、专属端点及多轮状态。答案是：普通文本、图片和 function tool 主链已经能用，但离
CC Switch 3.19.0 当前覆盖的特殊接口还有一层明显差距。

本轮不更新 Token Station，不拉取或合并线上版本，不修改业务代码，不替换本机正在运行
的 App，也不提交或推送。CC Switch 只作为只读竞品样本，临时源码副本不进入项目。

审计基线如下：

- 当前项目工作树：`4322dafcf416c51f708a2763c51027273e351a8b`。
- 实际运行 App 来自 `token-station-develop` 工作树：
  `76ff43c78b8875e992b753023dd0900f2de13049`。
- 两个工作树中的 `agent-anthropic` 和 `agent-openai-responses` 文件 SHA-256 完全一致，
  因此本文对核心入站适配器的结论也适用于当前运行 App。
- Claude Code：`2.1.173`。
- 本机 CC Switch：`3.19.0`，对照源码为上游 `v3.19.0` 对应提交
  `c0ff89b9b208c092d6ef40b155403dcf290e5767`。

同日更窄的 GLM/WebFetch/WebSearch 复现记录见
[Claude Code 接入 GLM 的 Web 与推理能力修改建议](./2026-07-30-Claude-Code接入GLM的Web与推理能力修改建议.md)。
本文不重复代理环境和 WebFetch 域名检查问题，而是向外扩展到整套特殊协议面。

## 先给判断

Token Station 现在缺的不是一个“搜索接口”，而是一层能判断工具由谁执行、目标上游是否
拥有它、协议转换会不会损失语义的能力路由。当前 Canonical IR 把工具基本压成
`name + description + parameters`，这对普通 function tool 很合适，但遇到下面这些东西
就不够了：

- Anthropic 的 `web_search_*`、`web_fetch_*`、`code_execution_*`、`tool_search_*`。
- Anthropic 定义 schema、但要求客户端执行的 `bash_*`、`text_editor_*`、`memory_*`、
  `computer_*`。
- OpenAI Responses 的 `web_search`、`file_search`、`code_interpreter`、`mcp`、
  `local_shell`、`computer`、`image_generation`。
- Codex 的 `custom`、`namespace`、`tool_search` 和对应 call/output/SSE 事件。
- `/v1/responses/compact`、`previous_response_id`、加密 reasoning、引用和审批事件。

最危险的现象已经不是“明确不支持”，而是 OpenAI Responses 适配器会把非 function 工具
直接删掉，再把剩下的请求继续路由。模型收到的工具集合和客户端声明的不一样，但请求仍
可能返回 200。说白了，客户端以为桌上有搜索、Shell 和插件，模型面前实际只剩一个普通
函数。这种“成功”比 400 更难排查。

正式版本最小可落地形态应是“能力感知的协议桥”，不是在 Token Station 里急着自建一个
搜索引擎：

1. 先识别工具类型和执行方，禁止静默删除关键语义。
2. 对原生同协议上游走受控透传，对跨协议路线走明确转换。
3. 不能转换时，在访问上游前返回可操作的 capability 错误，或在 Agent 配置阶段明确
   关闭该工具。
4. 再补 Codex 的 compact、namespace、custom tool、tool search 和状态回放。
5. 最后才考虑 Token Station 自己托管搜索、代码沙箱或浏览器。

本轮不建议做三件事：不要把 Anthropic WebSearch 改名后假装成 OpenAI Web Search，不要
让 Claude Code 转去调用用户的 Codex 账号，不要靠一张不断增长的模型名白名单声称“全
兼容”。托管工具属于提供它的服务，跨供应商只能原生透传、显式转换、客户端执行或关闭，
不能凭名字继承。

CC Switch 3.19.0 本身也没有完成“Claude hosted WebSearch 转 Codex Responses”。当前
release 源码仍会把 Anthropic `tools[]` 一律转成 Responses function tool，修复 PR
[#5856](https://github.com/farion1231/cc-switch/pull/5856) 已关闭且未合并，替代 PR
[#5681](https://github.com/farion1231/cc-switch/pull/5681) 仍开放。因此本文借鉴的是它的
compact、状态续接和 Codex 特殊工具转换，不把它写成万能搜索兼容层。

### 源码追踪结论：CC Switch 没有执行这次搜索

当时能搜索的完整链路是“Claude Code 嵌套 WebSearch 请求 → CC Switch 原样转发
→ DeepSeek Anthropic 兼容服务”。CC Switch 没有注入搜索 prompt，没有从缓存伪造
tool result，也没有在本地调用外部搜索 API。它之所以能跑，关键正是它在
同协议路线上“少做了一层”，没有像 Token Station 一样先把每个工具强制解析成
`name + input_schema`。

源码里的决定性路径如下：

1. Claude provider 先读取 `meta.apiFormat`。值为 `anthropic` 时，不启动
   OpenAI Chat、Responses 或 Gemini 协议转换。代码还直接注释为“直接透传，
   无需转换”。参见 [`claude.rs:38-101`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/claude.rs#L38-L101)
   和 [`claude.rs:968-990`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/claude.rs#L968-L990)。
2. Anthropic 同协议路线仅做两类小范围兼容处理：修整带 `tool_use` 的 thinking
   历史，以及在 DeepSeek 关闭 thinking 时删掉冲突的 effort 字段。这个函数没有
   读取或改写 `tools[]`。参见
   [`claude.rs:200-244`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/claude.rs#L200-L244)。
3. Forwarder 判定 `needs_transform == false` 后，直接把完成模型映射的
   `mapped_body` 作为上游请求体。因此 `type: web_search_20250305` 虽然没有
   `input_schema`，仍会留在请求里。参见
   [`forwarder.rs:1326-1376`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/forwarder.rs#L1326-L1376)
   和 [`forwarder.rs:1417-1522`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/forwarder.rs#L1417-L1522)。
4. 上游响应回来后，非流式响应按字节返回，SSE 流也只增加用量和超时统计，
   没有搜索结果合成器。参见
   [`response_processor.rs:300-333`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/response_processor.rs#L300-L333)
   和 [`response_processor.rs:677-710`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/response_processor.rs#L677-L710)。

对 `v3.19.0` 全源码搜索也没有找到 `web_search_20250305`、`server_tool_use`
或 `web_search_tool_result` 的执行逻辑。之前从二进制串里看到的
`web_search_tool_type: text_and_image` 位于
[`gpt5_5_template.json`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/resources/gpt5_5_template.json#L51-L56)，
它是 Codex 模型目录字段，不是 Claude/DeepSeek 路线的搜索执行器。

因此，“搜索究竟是谁做的”的精确答案是：搜索发生在 CC Switch 之后的
DeepSeek Anthropic 兼容服务层，或由该服务层继续委派给它的搜索后端。DeepSeek
内部如何实现没有公开源码，现有证据不能再向里区分“模型原生联网”和
“兼容网关调用搜索服务”。能确定的是 CC Switch 本身没有执行它。后来出现的
`402 Insufficient Balance` 也与这条路径一致：嵌套请求已经越过 CC Switch，
到了 DeepSeek 计费层。

这个结论不与上面的跨协议缺口冲突。Claude → DeepSeek 当时走的是
Anthropic 同协议透传；Claude → Codex OAuth 走的是 Anthropic → Responses 转换。
后一条路线在 `v3.19.0` 仍会把每个 Anthropic tool 重建成 function，缺少
hosted WebSearch 桥接。参见
[`transform_responses.rs:357-377`](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform_responses.rs#L357-L377)。
对 Token Station 有用的借鉴点不是“也什么都盲透传”，而是先分流：同协议原生
路线做有安全边界的 opaque pass-through，跨协议路线才进入显式能力转换和拒绝逻辑。

本文的优先级是“兼容声明和后续实现门禁”，不是线上事故等级：

| 优先级 | 先处理什么 | 为什么 |
| --- | --- | --- |
| P0 | 禁止静默删工具，补执行方分类，保留 beta header/body 对，原样回传 Claude Code 上游错误 | 不先修会继续出现假成功、错误重试和难以解释的 400/401 |
| P1 | server-tool 历史、signed thinking、compact、custom/namespace/tool-search、Responses 状态与事件 | 决定 Codex/Claude 长任务能否跨工具继续 |
| P2 | structured output、Gemini hosted tools、工具计费、旧版 attribution 和 model discovery | 有实际价值，但不应挡住普通 GLM 对话和 function tool 主链 |

## 先把五种“工具”分清楚

“工具调用”在界面上看着都像模型调用了一个名字，执行位置却完全不同。Token Station
后续的协议设计至少要区分下表五类：

| 类型 | 例子 | 谁真正执行 | Token Station 应做什么 |
| --- | --- | --- | --- |
| 用户定义 client tool | Claude Code 的 Read/Edit、普通 function tool | Claude Code、Codex 或调用方 | 翻译声明和 call/result，保持 ID 与 schema |
| 厂商 schema client tool | Anthropic `bash_*`、`text_editor_*`、`memory_*`、`computer_*` | 调用方按 Anthropic schema 执行 | 识别版本和 schema，不能要求它提供普通 `input_schema` |
| 厂商 server tool | Anthropic `web_search_*`、OpenAI `web_search`、`file_search` | 对应厂商服务端 | 只在目标上游原生拥有该能力时透传，否则关闭或明确失败 |
| Agent 本机工具 | Claude Code 本机 WebFetch、浏览器扩展、local shell | Agent 进程或本机扩展 | 继续走 client tool 闭环，不包装成厂商托管工具 |
| 网关自有工具 | Token Station 将来提供的 MCP、搜索或沙箱 | Token Station 或它管理的执行器 | 使用自己的能力 ID、权限、计费和引用协议，不冒充厂商工具 |

这张表也回答了“换成 GLM、DeepSeek 后能不能继续用官方 deep research”。普通本机工具可以
继续用，Anthropic 或 OpenAI 服务器执行的 deep research、web search、file search 并不
随模型代理自动转移。若要保持研究工作流，只能给 Agent 配本地/MCP 搜索，或者把请求送到
真实支持相应 hosted tool 的上游。

## 当前能力矩阵

“支持”只表示本文找到完整输入、输出和状态闭环，单纯接受字段或返回文本不算支持。

| 协议面 | Token Station 当前行为 | CC Switch 3.19.0 对照 | 判断 |
| --- | --- | --- | --- |
| Anthropic 普通 user tool | 可转成 Canonical `ToolDef`，普通工具闭环已有测试 | 支持并能桥接到 Chat/Responses | 已有基础能力 |
| Anthropic 提供的 client/server tools | 全部先按普通 `name + input_schema` 解析，无 `input_schema` 时 400 | 区分原生透传、转换和禁用，并不声称所有跨协议 hosted tool 都能执行 | Token Station 缺工具分类 |
| Anthropic `output_config.effort` | 进入未知扩展，但 OpenAI-compatible provider 不会渲染 | 显式映射到 `reasoning_effort`，`max -> xhigh` | Token Station 语义丢失 |
| Anthropic beta header/body 配对 | `anthropic-beta` 只保留 header 名，值到不了 adapter，未知 body 字段又未必能渲染 | 有较多定向字段转换，但也不是完整开放透传 | Token Station 会拆散能力对 |
| Claude Code 上游错误恢复 | provider 把原错误包成 Canonical 错误并改写 message | 本轮不将此列为竞品结论，Claude 官方要求原样转发 | Token Station 会破坏客户端自愈 |
| OpenAI Responses function tool | 支持一个实用子集 | 支持原生和多种跨协议转换 | 已有基础能力 |
| Responses hosted/built-in tools | 所有非 `function` 类型被静默过滤 | 原生 Responses 路线尽量保留，按 provider profile 过滤或关闭，xAI 等严格上游有定向清洗 | Token Station 当前最严重差距 |
| Codex `custom` / `namespace` / `tool_search` | 作为非 function 工具被删除，相应 item/event 也无法闭环 | 有展开、扁平化、还原、SSE 和历史回放代码及测试 | 直接缺失 |
| `/v1/responses/compact` | adapter 只匹配精确 `/v1/responses` | 注册多种 compact 路径并按目标协议处理 | 直接缺失 |
| `previous_response_id` | 明确 capability 失败 | 可用历史存储恢复 function/custom/tool-search call 与 output | 直接缺失 |
| Responses structured output | 明确 capability 失败 | 原生路线可透传，跨协议能力取决于目标路线 | 明确缺失，但比静默删除安全 |
| Responses 引用、审批、容器、文件和 hosted-tool 事件 | Canonical stream/item 没有对应类型 | 多条原生或定向转换路径覆盖其中一部分 | IR 级缺口 |
| Anthropic server-tool 历史与 signed thinking | 未知 content block 直接 capability，Responses 不能渲染 encrypted reasoning | 有 signed/redacted thinking 封装与工具历史恢复 | 跨工具多轮会中断 |
| `/v1/messages/count_tokens` | Anthropic adapter 不匹配，现有文档也标记未实现 | 有独立协议/路由处理思路 | 非阻断，但上下文估算不精确 |
| 根 `/v1/models` | 网关已有显式端点 | 已有 | 不能笼统说 Token Station 没有 models API |
| Claude Code gateway model discovery | 返回通用上游 model id，未做 Claude `id/display_name` 投影 | 有客户端配置和 model catalog 专门逻辑 | 端点存在不等于 `/model` 能发现 |
| Gemini Google Search、URL Context、Code Execution | 只接受 `functionDeclarations`，其他工具明确 capability 失败 | 本轮没有证明 CC Switch 对每个 Gemini hosted tool 都完整闭环 | Token Station 的下一组同类缺口 |

## 已确认的差距

### P0 门禁：Responses 内建工具被静默删除，且测试把它固定成了预期行为

`agent-openai-responses` 的 `tools_of` 只保留 `type == "function"`。注释明确列出
`web_search`、`file_search`、`local_shell`、`computer`、`code_interpreter`、`mcp`、
`namespace` 等类型，然后在入站边界直接 `return None`。代码位置：

- `plugins/official/agent-openai-responses/src/lib.rs:367-401`。

现有 fixture 还专门输入一个 `namespace` 和一个 `local_shell`，期望输出里只剩
`read_marker` function：

- `plugins/official/agent-openai-responses/fixtures/agent.normalize.codex-drops-builtin-tools.input.json:65-90`。
- `plugins/official/agent-openai-responses/fixtures/agent.normalize.codex-drops-builtin-tools.expected.json:46-70`。

这和项目自己的 IR 变更请求冲突。该文档写明，不能表达的 item/tool 必须返回 Capability
或 InvalidRequest，“不能丢弃后继续路由”：

- `docs/contributing/入站适配器-IR变更请求-openai-responses.md:27-40`。

本轮运行 `cargo test -p token-station-plugin-runtime --test official_plugins`，16 个测试全部
通过。这里的“通过”反而证明静默删除已经被夹具当作正式行为，不是偶发 bug。

建议把这条设为扩展兼容声明的 P0 门禁：只要请求含无法保持的关键工具，默认不能继续
路由。若产品确实允许降级，必须由路由策略显式写出 `disabled_by_route`，返回到诊断记录，
并保证客户端不会继续以为该工具可用。

### P0 架构根因：Canonical IR 不知道工具类型和执行方

当前 `ToolDef` 只有 `name`、`description`、`parameters`：

- `crates/protocol/src/chat.rs:112-120`。

它无法表达以下决定：

- 工具是 function、custom、namespace 还是 hosted tool。
- 工具由 Agent、Token Station 还是上游服务执行。
- 是否需要 `strict`、`cache_control`、`defer_loading`、`allowed_callers`。
- 是否允许原生透传，还是必须跨协议转换。
- 工具被关闭后，`tool_choice` 和历史 item 应如何同步变化。

这不是给 `ToolDef` 再塞一个可选 `type` 就能完全解决。当前所有请求最终都变成
`ChatRequest`，而 Responses 的核心是有类型、有生命周期的 item 流。正式设计应先拆开
“同协议原生通道”和“Canonical Chat 转换通道”，再决定哪些字段进入下一版 IR。

### P0：上游错误被改写，Claude Code 的自动降级可能失效

`provider-openai-compatible` 会先把上游状态映射到 Canonical `ErrorCode`，再把 message
改成 `the upstream refused the request as malformed` 等固定文本。原 provider message
只有不超过 256 个字符时才放进附加字段：

- `plugins/official/provider-openai-compatible/src/lib.rs:568-623`。

这套错误规范化有利于 Token Station 统一统计，却和 Claude Code 当前官方网关契约冲突。
Claude Code 会根据上游原始错误文字识别 thinking 字段、thinking signature 和会话中 system
message 被拒绝，然后自动关闭该能力并重试。官方明确要求错误 body 原样转发，只保留状态码
或再包一层 envelope 都会破坏恢复路径。参考
[Claude Code Gateway protocol：Automatic retry and error forwarding](https://code.claude.com/docs/en/llm-gateway-protocol#automatic-retry-and-error-forwarding)。

建议把“客户端可见错误”和“内部归一化错误”分开。Token Station 可以在内部记录统一 code，
但 Anthropic gateway 路线应按经过安全审查的规则把原始 status、body 和必要 header 返回给
Claude Code。日志仍只记脱敏摘要，不能因为要原样回给调用方就把错误正文写入长期日志。

### P0：`anthropic-beta` 值丢失，header/body 能力对会被拆开

`HeaderDigest` 只允许 `content-type`、`anthropic-version` 和 `x-agent-step` 保留值，其他
header 只保留名字：

- `crates/protocol/src/envelope.rs:18-80`。

所以 adapter 能知道 `anthropic-beta` 来过，却读不到它的值，更无法按官方要求原样转发。
与此同时，`context_management`、`strict`、`defer_loading`、`output_config` 等 body 字段可能
进入 extensions 后在 provider 端消失，最终形成三种坏状态：header 丢了但 body 还在时
上游 400，两者都丢时能力悄悄关闭，订阅 OAuth capability 被剥离时甚至可能 401。

Claude Code 官方把 `anthropic-beta` 和 `anthropic-version` 列为 Anthropic Messages 网关
必须原样转发的开放列表，并要求 beta header 和对应 body 字段成组处理。参考
[Claude Code Gateway protocol](https://code.claude.com/docs/en/llm-gateway-protocol#forward-as-open-lists)。

这里不能粗暴地把所有请求 header 暴露给 WASM plugin。`HeaderDigest` 的密钥隔离设计是对的。
建议由可信 host 层增加“允许原生转发、但不向任意 plugin 暴露”的 opaque header carrier，
只对目标 Anthropic wire route 生效，跨协议路线则由 adapter 成组消费或拒绝这些能力。

### P1：Anthropic server tool 被误报为 schema 错误

`agent-anthropic` 对 `tools[]` 中每个对象都强制读取 `name` 和 `input_schema`：

- `plugins/official/agent-anthropic/src/lib.rs:394-416`。

Anthropic 官方当前把工具分成两类：服务端执行的 Web Search、Web Fetch、Code
Execution、Advisor、Tool Search、MCP，以及 Anthropic 定义 schema、客户端执行的
Memory、Bash、Text Editor、Computer。它们和用户自定义工具一起出现在 `tools` 数组，
但并不都使用普通 user tool 的 `input_schema` 形状。参考
[Anthropic Tool reference](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-reference)。

真实 Claude Code deep-research 日志快照中，共识别到 160 个唯一 WebSearch tool call，
至少 153 个返回同一条 `400 tool declares no input_schema`，对应 usage 中
`server_tool_use.web_search_requests` 合计为 0。数量来自仍在增长的工作流快照，真正重要
的是执行次数始终为 0：搜索没有被 GLM 或 Token Station 执行，模型随后改用 WebFetch
抓搜索引擎页面兜底。

正式行为应改成“识别后分类”：

- server tool 且目标上游不拥有它：返回 capability，upstream hit 必须为 0。
- Anthropic-schema client tool：按明确版本决定支持或不支持，不能报缺 `input_schema`。
- 普通 user tool：维持当前闭环。
- 混合请求：不能只删掉不支持的部分后继续。

### P1：server-tool 历史块和 signed thinking 无法重放

`agent-anthropic::parse_plain_block` 当前只接受 text 和 image，thinking/redacted thinking 明确
capability，其他类型统一报 `unsupported Anthropic content block`：

- `plugins/official/agent-anthropic/src/lib.rs:137-189`。

这意味着 `server_tool_use`、`web_search_tool_result`、document、search_result 等块无法进入
下一轮。即便未来第一轮能由原生上游完成搜索，带着 server-tool 结果继续对话仍会在入站
解析阶段失败。

跨 Anthropic/Responses 的工具回合还有一层签名问题。Token Station 的 Responses renderer
会拒绝 `RedactedThinking`，并且 visible thinking 的 signature 没有 Responses wire slot：

- `plugins/official/agent-openai-responses/src/lib.rs:420-442`。

CC Switch 会把 signed thinking/redacted thinking 封装到
`reasoning.encrypted_content`，下一轮再恢复原 Anthropic block。这个方案可以作为互操作
参考，但 Token Station 必须先定义 opaque reasoning 的保存期限、日志红线和 provider 切换
语义，不能把密文当普通可展示 reasoning。

### P1：Token Station 没有 Responses compact 子协议

Responses adapter 的路径匹配是精确 `POST /v1/responses`：

- `plugins/official/agent-openai-responses/src/lib.rs:737-751`。

它不会接管 `/v1/responses/compact`。项目早期调研已经注意到 Grok Build/Codex 会用这个
端点，但当前仍没有实现。OpenAI 官方把 standalone compact 定义为独立、无状态端点，
返回的压缩窗口必须原样进入下一轮，不能先压成普通 chat message。参考
[OpenAI Compaction](https://developers.openai.com/api/docs/guides/compaction)。

CC Switch 3.19.0 已注册 `/responses/compact`、`/v1/responses/compact`、带 Codex 前缀和
Grok Build 前缀的路由，并让它们进入相同的 provider 选择与转换流程：

- [server.rs:320-356](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/server.rs#L320-L356)。
- [handlers.rs:900-1025](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/handlers.rs#L900-L1025)。

Token Station 不应只新增一个 URL 别名。compact 的输入输出包含普通消息、工具历史和
opaque compaction item，必须先确定原生透传、跨协议拒绝和日志脱敏三种行为。

### P1：Codex 的 custom、namespace 和 tool search 没有 item 闭环

当前 Responses adapter 会删掉 `custom`、`namespace` 和 `tool_search` 工具，Canonical
stream 也只有文本、普通 tool-call 参数、usage、thinking、done 和 error：

- `crates/protocol/src/stream.rs:15-61`。

因此即使放宽入站校验，返回的 `custom_tool_call`、`tool_search_call`、
`custom_tool_call_output`、`tool_search_output`，以及
`response.custom_tool_call_input.delta/done` 仍无处安放。

CC Switch 的处理值得借鉴，但不是简单“把 namespace 删掉”：

- 它把 namespace 子工具展平成稳定名字，响应时再恢复 `{name, namespace}`，若名字碰撞，
  明确失败，不静默选一个。
- 它把 custom tool 和 tool search 映射到目标 Chat/Anthropic 路线可表达的 call，再把结果
  还原成 Codex 认识的 item。
- 它保存上一轮 function/custom/tool-search call，处理下一轮只有 output 和
  `previous_response_id` 的情况。

源码证据：

- [transform_codex_chat.rs:118-253](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform_codex_chat.rs#L118-L253)。
- [transform_codex_responses_namespace.rs:1-221](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform_codex_responses_namespace.rs#L1-L221)。
- [codex_chat_history.rs:443-480](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/codex_chat_history.rs#L443-L480)。

### P1：`previous_response_id` 和 reasoning 状态被截断

Token Station 会直接拒绝任何非空 `previous_response_id`：

- `plugins/official/agent-openai-responses/src/lib.rs:103-111`。

OpenAI 官方把它定义为 Responses 的多轮状态链，参考
[Conversation state](https://developers.openai.com/api/docs/guides/conversation-state)。对 Codex
来说，这不只影响聊天历史，还影响工具 call/output、reasoning item、encrypted content
和缓存前缀。简单把 ID 删除后重发当前消息，会让工具结果找不到原调用，也会让长任务突然
失忆。

建议先支持两种明确模式：

1. 原生 stateful 上游：透传 `previous_response_id`，不在 Token Station 解引用。
2. 跨协议无状态上游：只有在 Token Station 已保存并能完整重放所需 item 时才转换，否则
   capability 失败。

不要把 `previous_response_id` 当作稳定的 Token Station session ID。它属于具体 Responses
上游，切换 provider 后通常不能继续使用。

### P1：未知请求字段被“收进扩展”，但没有真正送到上游

Anthropic adapter 会把 `output_config` 等未知字段放进 `extensions`，Responses adapter
也保留一批非直接字段。问题出在末端：`provider-openai-compatible::body_of` 只渲染普通
Chat Completions 字段和 function tools，`build_http_request` 额外只处理
`reasoning_effort`：

- `plugins/official/provider-openai-compatible/src/lib.rs:181-244`。
- `plugins/official/provider-openai-compatible/src/lib.rs:400-425`。

因此“adapter 接受了字段”不等于语义被保留。`output_config`、`include`、`store`、
`service_tier`、hosted-tool 配置、metadata、缓存控制等都需要逐项决定：映射、原生透传、
明确拒绝或仅作本地观测。不能继续依赖扁平 extensions 产生自动兼容的错觉。

其中 `output_config.effort` 已有直接竞品实现可参考。CC Switch 先读取显式 effort，再按目标
模型映射到 `reasoning_effort`，并把 `max` 映射为 `xhigh`：

- [transform.rs:64-124](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform.rs#L64-L124)。

Token Station 正式实现前仍要用实际 GLM/DeepSeek 网关验证允许值，不能因为 CC Switch
这样映射就默认所有 OpenAI-compatible provider 都接受 `xhigh`。

### P1：返回内容和流事件只覆盖普通聊天子集

OpenAI Web Search 返回 `web_search_call` item、action、sources/results 和消息引用，界面
还必须让引用可见且可点击。参考
[OpenAI Web search](https://developers.openai.com/api/docs/guides/tools-web-search)。File Search、
Code Interpreter、MCP、Computer Use 还有各自的 call、状态、容器、审批和结果事件。

Anthropic 的 server tool 会返回 `server_tool_use` 和不同结果块，普通 tool result 也能含
text、image、document、search_result。Token Station 的 `ContentPart::Unknown` 能保留部分
未知对象是个好基础，但当前 agent adapter 和 provider renderer 没有端到端闭环，stream
枚举对未知事件还会直接反序列化失败。

建议下一版 item/event 设计优先覆盖“身份、状态、执行方、原始 payload、可展示引用”五个
要素。不要把引用拼进普通文本后丢掉结构，也不要把审批事件当成模型回答。

### P2：结构化输出已经安全拒绝，但仍是实际兼容缺口

Responses 的 `text.format=json_schema/json_object` 当前返回 capability，而 Canonical IR
和 provider 其实已有一部分 `ResponseFormat` 表达。这比静默降级成 text 正确，但说明
Responses adapter、Canonical IR 和 provider 的能力没有接通。建议在 hosted tool 和状态
门禁完成后，再补同协议透传与明确的跨协议映射，不要把它抢到第一阶段。

### P2：Gemini 的同类 hosted tool 还没有纳入能力模型

`agent-gemini` 只接受 `functionDeclarations`，遇到 Google Search grounding、URL Context、
Code Execution 等工具会返回 `only Gemini functionDeclarations tools are supported`：

- `plugins/official/agent-gemini/src/lib.rs:200-225`。

这条至少是明确失败，没有 Responses 静默删除的问题。正式能力模型一旦建立，应把 Gemini
纳入同一套工具执行方分类，而不是再做一套互不相干的 if/else。

### P2：usage 只统计 token，无法解释 hosted tool 的执行和成本

当前 `Usage` 记录 input/output、缓存和 reasoning token：

- `crates/protocol/src/usage.rs:10-49`。

它无法记录 server-tool 调用次数、搜索/容器费用、审批状态或引用数量。于是即使未来原生
透传成功，Token Station 仍可能显示“请求成功、工具调用 0”，也无法区分模型 token 成本
和搜索调用成本。建议把 hosted-tool usage 放进独立、可扩展的计量项，不要继续往固定 token
结构里塞布尔值。

### P2：Claude Code 2.1.173 的 attribution 前缀会降低第三方缓存命中

当前实测客户端是 2.1.173。Claude Code 官方说明，在 2.1.181 以前，自定义 Base URL 下的
system attribution block 含每次请求都会变化的 token。若 Token Station 或第三方上游按
完整请求前缀做 prompt cache，同一会话也可能持续 miss。

用户已经明确本轮不更新，所以这里不建议升级客户端。正式接入指南应记录一个旧版兼容
选项：当网关会重排 system 或上游按请求体缓存时，由用户显式设置
`CLAUDE_CODE_ATTRIBUTION_HEADER=0`。Token Station 不应私自删或合并该 system block。
参考 [Claude Code Gateway protocol：System prompt attribution block](https://code.claude.com/docs/en/llm-gateway-protocol#system-prompt-attribution-block)。

### P2：`/v1/models` 已存在，但 Claude Code discovery 契约还需专门投影

Token Station 的根路由和 agent-scoped fallback 都能返回 `/v1/models`，响应来自通用上游
catalog：

- `apps/cli/src/server.rs:202-208,310-322,556-570`。
- `apps/cli/src/gateway.rs:1074-1119`。

Claude Code 的 gateway discovery 是另一层契约：需要显式开启，调用
`GET /v1/models?limit=1000`，只接受 `id` 以 `claude` 或 `anthropic` 开头的条目，并可读取
`display_name`。GLM、DeepSeek 或 `auto` 这类通用 ID 即使 HTTP 200，也不会自动出现在
Claude Code `/model` 选择器。参考
[Claude Code Gateway protocol：Model discovery](https://code.claude.com/docs/en/llm-gateway-protocol#model-discovery)。

建议给 Claude Code connector 增加显式的 discovery projection：公开稳定的 Claude 形状
alias，响应里再说明真实 resolved provider/model。不要改通用 `/v1/models` 的全局语义，也
不要让模型选择器里的 alias 冒充上游真实模型名。

## CC Switch 真正值得学的部分

CC Switch 并没有把所有 hosted tool 神奇地翻译成任意模型都能执行。它当前的核心做法有
四种：目标上游本来支持时原生透传，目标协议可表达时定向转换，已知不支持时在配置阶段
关闭，严格网关有已验证差异时做 provider-specific 清洗。这比“统一转成 Chat
Completions”更接近真实协议世界。

但它在 Claude hosted WebSearch 上同样还有断口。v3.19.0 的 Claude→Responses 转换仍将
每个 Anthropic tool 包成普通 function，`web_search_*` 的执行归属和结果 item 没有恢复：

- [transform_responses.rs:357-376](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform_responses.rs#L357-L376)。

截至本轮审计，专门修复 Claude hosted WebSearch 的 PR #5856 已关闭未合并，#5681 仍开放。
为遗漏的 tool_search 注入 deferred desktop tools 的 PR
[#5799](https://github.com/farion1231/cc-switch/pull/5799) 也仍开放。因此正确的竞品结论是：
CC Switch 在 compact、Codex custom/namespace/tool_search 和进程内历史续接上领先，Hosted
WebSearch 跨厂商桥接仍未完成。

Token Station 值得借鉴的是：

1. 端点和 wire API 是 provider capability 的一部分，不只是一段 base URL。
2. Codex 的 custom、namespace、tool search 需要成对的请求变换和响应还原。
3. `previous_response_id` 与工具历史要一起处理。
4. 不支持的 hosted web search 应在 Agent 配置阶段关闭，避免呈现一个死工具。
5. 对 provider-specific 转换写真实 fixture 和 SSE 测试，而不是只测普通 function call。
6. 媒体和工具结果要保持结构，不能把一大段 base64 当普通文本反复回放。

也有三点不应照搬：

- CC Switch 某些跨协议路径仍会过滤 hosted tool。它用配置同步降低暴露概率，但这不等于
  转换本身已经无损，Token Station 应把“已关闭”和“被转换层删除”都写进决策记录。
- 供应商黑名单适合快速止血，不适合作为长期能力系统。长期应由 provider/model/endpoint
  capability manifest 驱动，并保留探测证据和日期。
- CC Switch 还承担客户端配置管理。Token Station 不需要为了追平特殊接口，把它的所有
  provider 管理界面和同步功能一起复制过来。

## 建议的最小架构

### 1. 在路由前建立工具能力分类

建议增加一层 protocol preflight，至少输出以下信息：

```text
tool_kind: function | provider_client | provider_hosted | custom | namespace | tool_search
execution_owner: agent | token_station | upstream_provider
wire_protocol: anthropic_messages | openai_responses | openai_chat | gemini
disposition: native_passthrough | canonical_translate | client_execute | explicit_disable | capability_error
```

这些字段不一定全部进入现有 `ToolDef`。第一阶段可以在 adapter 路由决策中形成独立 manifest，
先解决错误行为，若要跨多轮保存 item，再评审 Canonical IR v2。

### 2. 拆出原生协议通道

同协议 provider 不应被强制压成 ChatRequest 再还原。例如 Codex Responses 送到真正支持
Responses 的上游时，可以在鉴权、模型映射、限流和安全审计后保留原生 body/item/SSE。
opaque passthrough 只能用于同一协议和通过 allowlist 的字段，不能拿来绕过跨协议校验。

跨协议路线继续使用 Canonical IR，但必须 fail closed：无法保持的关键语义在访问上游前
失败，不允许“尽量转一点”。

### 3. 建立 endpoint capability registry

建议把端点作为版本化能力管理，而不是靠 adapter 的字符串匹配散落在代码里。第一批：

- Anthropic：`/v1/messages`、`/v1/messages/count_tokens`。
- OpenAI/Codex：`/v1/responses`、`/v1/responses/compact`、`/v1/models`。
- 后续按需要加入 Files、Containers、MCP 或 Gemini countTokens，不因发现二进制字符串就
  自动宣称支持。

每个端点需要声明 `native`、`translated`、`local` 或 `unsupported`，并能被诊断页和
测试读取。

### 4. 为 Responses 增加可扩展 item/event 层

最小 item 层至少要区分 message、reasoning、function/custom/namespace/tool-search call
与 output、hosted-tool call、approval、compaction 和 unknown-opaque。事件层要保留 item ID、
call ID、状态、输出索引和原始事件类型。

如果 ABI 不能原地扩展，应按项目已有 IR 变更请求发布新版本，旧 adapter 继续只声明普通
聊天子集。不要为了兼容而让旧枚举悄悄吞掉新事件。

### 5. 把“关闭能力”做成可观测的一等结果

正式路由记录建议同时显示：

- 客户端声明的工具。
- Token Station 识别出的类型和执行方。
- 目标 provider/model 声明的能力。
- 最终 disposition。
- 被关闭或拒绝的具体原因。
- upstream hit count 和 server-tool usage。

日志继续禁止记录 prompt、工具参数里的密钥、Cookie、文件正文和搜索结果全文。诊断需要
工具类型和状态，不需要泄露内容。

## 建议实施顺序

### 阶段 A：先修“不能撒谎”的门禁

1. 把 Responses 非 function 工具的静默删除改成显式能力判定。
2. 修改现有 `codex-drops-builtin-tools` fixture，使混合请求不再假装完整成功。
3. Anthropic parser 先识别 versioned tool type，再决定普通 user tool 校验。
4. 将 Claude Code 客户端错误原样回传与内部 ErrorCode 记录拆开。
5. 给每次降级写 route decision，默认不访问上游。
6. 普通文本、图片、function tool 和现有 streaming 全量回归。

这一步不实现任何搜索执行器，但会立刻把现在的误导性 200 和 schema 错误变成可诊断行为。

### 阶段 B：补 Codex/Responses 当前必需接口

1. 设计并实现 `/v1/responses/compact` 的原生透传和明确拒绝路线。
2. 加 custom tool、namespace、tool search 的请求/响应/SSE 成对转换。
3. 支持原生 `previous_response_id`，再评估跨协议历史回放。
4. 原生 Responses provider 保留 `include`、`store`、reasoning 和允许的 item。
5. provider 不支持 hosted web search 时，在 Codex 配置或 capability handshake 中关闭。

### 阶段 C：补 Anthropic 特殊字段与结果块

1. 由可信 host 层成组保留 `anthropic-beta`、`anthropic-version` 和对应 body 字段。
2. `output_config.effort -> reasoning_effort`，按 provider 允许值验证。
3. 区分 Anthropic schema client tool 与 server tool。
4. 支持 server_tool_use、document/search_result/server-tool result 及引用结构。
5. 设计 signed/redacted thinking 与 Responses encrypted reasoning 的 opaque 往返。
6. 增加 `/v1/messages/count_tokens`，或返回明确的本地估算标识。
7. 检查 cache_control、strict、defer_loading、allowed_callers 的保留策略。

### 阶段 D：再评估统一搜索和其他厂商工具

先用本地 skill 或 MCP 搜索恢复 GLM/DeepSeek 下的研究工作流。若 Token Station 以后要自有
hosted search，必须单独设计权限、引用、地区、内容安全、日志脱敏、供应商、限流和计费，
并使用 Token Station 自己的 tool kind。Gemini Google Search、URL Context 和 Code
Execution 也在这一阶段纳入同一能力模型。

## 正式版本公开验收矩阵

| 编号 | 场景 | 观察证据 | 通过标准 |
| --- | --- | --- | --- |
| CAP-001 | Responses 只有 `web_search` | adapter 错误、route decision、upstream hit | 不静默删除，不支持路线 upstream 为 0 |
| CAP-002 | function + `web_search` 混合 | 上游收到的 tools、客户端错误 | 不能只保留 function 后返回伪成功 |
| CAP-003 | `namespace` 含两个子工具 | 扁平名和响应恢复 | call/output 往返后 namespace 与 name 不变 |
| CAP-004 | namespace 名字碰撞 | preflight 错误 | 明确失败，不覆盖或随机选一个 |
| CAP-005 | custom tool 流式参数 | 原始 SSE 与回传 SSE | delta/done 顺序、ID、input 均可闭环 |
| CAP-006 | tool search 动态加载 namespace | tool_search call/output 和下一轮工具集 | 加载的工具能在下一轮被调用 |
| CAP-007 | 非空 `previous_response_id` | 原生上游请求或历史回放 | 不删除 ID，工具 output 能找到原 call |
| CAP-008 | `/v1/responses/compact` | 路由、上游 body、返回 item | 原生路线透传，跨协议路线明确 capability |
| CAP-009 | Anthropic `web_search_20260209` | 工具分类和 server-tool usage | 不再报缺 `input_schema`，无能力时不访问上游 |
| CAP-010 | Anthropic `bash_20250124` | 工具分类和客户端执行回合 | 与 server tool 分开处理，版本不支持时错误准确 |
| CAP-011 | server_tool_use + search result 历史 | 第二轮入站和最终回答 | 不因未知 block 400，call/result 身份保持 |
| CAP-012 | `anthropic-beta` + `output_config` | mock 上游 header/body | 原生路线成组原样转发，跨协议路线成组转换或拒绝 |
| CAP-013 | 上游 thinking/signature 400 | 客户端实际收到的 error body | 文字与原 body 不变，Claude Code 能按官方逻辑自愈 |
| CAP-014 | `output_config.effort=max` 到 GLM | mock/真实上游请求 | 只在 provider 允许时映射，诊断显示最终值 |
| CAP-015 | signed/redacted thinking 跨协议往返 | encrypted payload 和下一轮回放 | 签名可恢复，日志不写 opaque 内容 |
| CAP-016 | tool result 含图片/PDF/search_result | 往返 payload 和日志 | 内容结构不丢失，日志不写正文或 base64 |
| CAP-017 | OpenAI Web Search 引用 | item、annotations、界面链接 | 引用可见可点击，不被拼成无结构纯文本 |
| CAP-018 | server-tool usage 与费用 | usage 和请求详情 | token 与工具调用分开计量，不显示假 0 |
| CAP-019 | Gemini Google Search tool | capability decision | 明确 native/unsupported，不误当 functionDeclarations |
| CAP-020 | Claude Code gateway model discovery | `/v1/models?limit=1000` 与 `/model` | 合法 alias 可发现，并显示真实 resolved model |
| CAP-021 | Claude Code 2.1.173 重复长前缀 | 两轮 cache key/usage | attribution 处理策略明确，不由网关偷偷改 system |
| REG-001 | 普通文本与图片 | 现有公开测试和真实 App | 现有主链不回归 |
| REG-002 | 普通 function tool | 两轮 call/result | Claude Code、Codex、OpenCode 原闭环不回归 |
| REG-003 | 流中断与重试 | settle outcome、请求记录 | 部分输出仍不记成功，不跨工具副作用重放 |

真实 App 验收时，还要同时检查 requested model、resolved provider/model、工具 disposition 和
实际进程/端口。只跑 cargo test 或只看到界面按钮不算完成。

## 证据索引

### Token Station 本地源码与测试

- `plugins/official/agent-anthropic/src/lib.rs:26-38,68-119,394-430,691-762`
- `plugins/official/agent-openai-responses/src/lib.rs:22-35,74-124,367-410,737-814`
- `plugins/official/provider-openai-compatible/src/lib.rs:181-244,400-425`
- `plugins/official/agent-gemini/src/lib.rs:200-225,396-410`
- `crates/protocol/src/envelope.rs:18-80`
- `crates/protocol/src/chat.rs:17-45,112-217`
- `crates/protocol/src/stream.rs:15-61,169-174`
- `crates/protocol/src/usage.rs:10-49`
- `docs/contributing/入站适配器-IR变更请求-openai-responses.md:27-49`
- `docs/contributing/入站适配器-协议盘点.md:39-52`
- `crates/plugin-runtime/tests/official_plugins.rs`

### CC Switch 3.19.0

- [Release v3.19.0](https://github.com/farion1231/cc-switch/releases/tag/v3.19.0)
- [Responses/compact 路由](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/server.rs#L320-L356)
- [Codex tool context 与转换](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform_codex_chat.rs#L118-L253)
- [namespace 展平与恢复](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform_codex_responses_namespace.rs#L1-L221)
- [previous_response_id 工具历史](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/codex_chat_history.rs#L443-L480)
- [xAI Responses 工具类型清洗](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform_codex_responses_xai_sanitize.rs#L1-L101)
- [effort 映射](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform.rs#L64-L124)
- [Claude hosted WebSearch 尚未正确分类的转换](https://github.com/farion1231/cc-switch/blob/c0ff89b9b208c092d6ef40b155403dcf290e5767/src-tauri/src/proxy/providers/transform_responses.rs#L357-L376)
- [PR #5681：仍开放的 hosted WebSearch 兼容修复](https://github.com/farion1231/cc-switch/pull/5681)
- [PR #5799：仍开放的 deferred tool-search 修复](https://github.com/farion1231/cc-switch/pull/5799)

### 官方协议资料

- [Anthropic Tool reference](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-reference)
- [Anthropic Server tools](https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools)
- [Anthropic Token counting](https://platform.claude.com/docs/en/build-with-claude/token-counting)
- [Claude Code Gateway protocol](https://code.claude.com/docs/en/llm-gateway-protocol)
- [Claude Code Tools reference](https://code.claude.com/docs/en/tools-reference)
- [OpenAI Web search](https://developers.openai.com/api/docs/guides/tools-web-search)
- [OpenAI File search](https://developers.openai.com/api/docs/guides/tools-file-search)
- [OpenAI Code Interpreter](https://developers.openai.com/api/docs/guides/tools-code-interpreter)
- [OpenAI MCP and Connectors](https://developers.openai.com/api/docs/guides/tools-connectors-mcp)
- [OpenAI Conversation state](https://developers.openai.com/api/docs/guides/conversation-state)
- [OpenAI Compaction](https://developers.openai.com/api/docs/guides/compaction)

## 未证实项与边界

1. 本文没有用真实付费账号逐个调用 Anthropic/OpenAI hosted tool。厂商能力来自当前官方
   文档，CC Switch 行为来自 v3.19.0 源码、测试和 release notes，不能据此宣称所有第三方
   provider 都已在真实环境成功。
2. Claude Code 2.1.173 二进制中能发现 `/v1/agents`、`/v1/files`、`/v1/skills`、
   `agent_toolset_20260401` 等字符串，但字符串只能作为后续探测候选，不证明普通 Claude
   Code 会经 Token Station 调用 Managed Agents API。本轮不把它们列为高优先级缺陷。
3. 深度研究工作流仍可能继续增长，所以 WebSearch 数量只代表审计快照。错误形状、零
   server-tool usage 和 adapter 代码路径已经相互印证，不依赖最终次数。
4. `/v1/models` 在 Token Station 根网关存在，当前 server 源码也会把 agent-scoped GET
   规范化到该响应，本轮没有携带真实虚拟 Key 重做 discovery，所以只确认响应投影存在
   风险，不沿用旧验收文档里的 502 结论。
5. 本文没有判断 Token Station 应不应该经营搜索、浏览器或代码沙箱。那是产品和安全
   决策，当前先把协议能力说真、路由行为做可观测。

## 当前状态

本轮只新增本文。没有修改 adapter、Canonical IR、provider、路由配置或测试 fixture，没有
执行 App 安装脚本，没有替换 `/Applications`，没有切换当前运行进程，也没有更新 CC
Switch、Claude Code、Agent Reach 或任何依赖。只读审计中运行了现有 official plugin 测试，
结果为 16 passed、0 failed，这个结果用于确认当前行为基线，不表示上文缺口已修复。
