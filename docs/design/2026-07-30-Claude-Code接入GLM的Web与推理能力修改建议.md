# Claude Code 接入 GLM 的 Web 与推理能力修改建议

## 文档定位

这是一份测试版问题分析和修改建议，不是实现记录。本文不修改当前 Token Station
正式分支，不替换本机正在运行的 App，也不把本地实验结论标记为已发布能力。

测试环境为 Claude Code 2.1.173、Token Station 本地 `8787` Agent 入口，以及通过
OpenAI-compatible provider 路由的 GLM 5.2。结论来自附带终端记录、Claude Code debug
日志、Token Station 进程和请求记录、协议复现请求，以及公开网页的真实抓取测试。
测试期间使用隔离 worktree，虚拟 Key 和上游 Key 均未写入本文。

## 先给结论

当前 GLM 对话和 Claude Code 的本地工具闭环可以继续使用。所谓“Web 都坏了”其实是
三个不同问题叠在一起：Claude Code 内置 `WebSearch` 是 Anthropic 服务端工具，GLM
网关不能直接继承；`WebFetch` 是本机工具，但抓取前的域名安全检查被本机代理拖死；
Claude Code 的推理强度则停在 Anthropic 入站字段里，没有转换成 GLM 上游认识的
`reasoning_effort`。

因此不建议在 Token Station 里伪造 Anthropic WebSearch。正式版本应先补协议转换和
清楚的能力错误，再把联网搜索作为独立的本地工具或 MCP 能力接入。WebFetch 的代理
问题属于启动环境，应在诊断页或接入指南里暴露，不应归咎于 GLM。

## 请求实际怎么走

普通对话的主链如下：

```text
Claude Code
  -> /agents/claude-code/v1/messages
  -> agent-anthropic 将 Anthropic 请求规范化
  -> Token Station 按 virtual key 和 tier 选路由
  -> provider-openai-compatible 渲染 OpenAI 请求
  -> WeCoding / GLM 5.2
  -> provider 解析响应
  -> agent-anthropic 渲染回 Claude Messages 响应
  -> Claude Code
```

`Read`、`Bash`、`Edit` 等 client tool 由 Claude Code 在本机执行。模型先返回工具调用，
Claude Code 执行后把结果放进下一轮 `/v1/messages`，所以这些工具仍会经过上面的模型
主链，但文件读取或命令执行本身不发生在 WeCoding。

Web 相关请求分成两条支线：

```text
WebFetch:
Claude Code -> Anthropic 公共 domain_info 安全检查 -> 本机抓网页
            -> 把网页内容送回当前 Token Station 模型主链整理

WebSearch:
Claude Code -> 当前 ANTHROPIC_BASE_URL 请求 Anthropic server tool
            -> Token Station 没有 Anthropic 搜索执行服务 -> 失败
```

这也解释了为什么只看 `requests.log` 里有没有 `WebFetch` 字样会得出错误结论。当前日志
不记录 prompt、工具名和请求体，搜索不到字符串不能证明请求绕过了 Token Station。

## 已确认的问题

### P1：内置 WebSearch 被当成普通工具解析

真实复现中，Claude Code 发送 `source=web_search_tool` 请求，请求里的工具使用
`web_search_20260209` 一类版本化 `type`，没有普通 client tool 必需的
`input_schema`。当前 `agent-anthropic` 因而返回：

```text
tool declares no input_schema
```

这个报错把“网关没有 Anthropic server tool 执行能力”说成了“请求 schema 写错”，会
诱导后续排查。即使放宽 schema 校验，OpenAI-compatible 上游也不会自动替 Anthropic
完成搜索，因此单纯透传仍然没有可执行闭环。

建议正式版本识别无 `input_schema` 且带版本化 `type` 的 Anthropic server tool，在
路由前返回明确的 `capability` 错误。错误应告诉用户改用本地/MCP 搜索工具，且该请求
不应访问上游。不要把 server tool 静默改造成普通 function tool。

### P1：WebFetch 被强制代理卡在域名检查

Claude Code 抓网页前会访问：

```text
https://api.anthropic.com/api/web/domain_info?domain=...
```

在启动脚本强制使用 `127.0.0.1:7890` 时，该请求 15 秒超时；清除代理环境变量后，同一
接口约 1 秒返回 `can_fetch: true`，真实 Claude Code 随后成功取得 Example Domain。
这说明失败发生在目标网页请求之前，与 GLM 生成能力、Token Station 鉴权或 provider
转换无关。

建议不要在 Claude Code 启动脚本里默认为所有流量强塞代理。若产品要保留代理，应让
用户显式选择，并在 Agent 诊断里分别探测 Token Station、`domain_info` 和目标网页。
错误提示应写清楚失败的是域名安全检查还是网页本身。

### P1：推理强度没有传到 GLM

Claude Code 2.1.173 的 `--effort` 会发送 Anthropic `output_config.effort`。当前
`agent-anthropic` 把 `output_config` 留作未知扩展，但没有生成 provider adapter 已能
渲染的 `reasoning_effort`，所以 Claude Code 界面显示 low/high/max 不等于 GLM 上游
真的收到了相同强度。

建议在 Anthropic adapter 中只接受可表达的 `output_config.effort`，将其写入 Canonical
扩展 `reasoning_effort`，再由 OpenAI-compatible provider 发给上游。未知
`output_config` 字段应返回 capability 错误，不能静默丢弃。GLM 5.2 原生路线优先验证
`high` 和 `max`；其他值是否可用要以实际 WeCoding 接口为准。

### P2：模型名与真实路由容易混淆

Claude Code 请求里的 `auto` 或 Claude 模型名只是 Agent 协议入口的标识。当前路由关闭
了 exact model honor，而且各 tier 都指向 WeCoding 的 GLM 5.2，因此 `/model` 中看到的
名字不代表上游真的切换了模型。

建议在 Agent 请求详情中同时显示 `requested model`、`resolved upstream` 和
`resolved model`。如果 exact model 被策略覆盖，应显示“已由路由策略覆盖”，不要让
用户靠模型自报身份猜测。

### P2：本地 macOS 安装流程误打 DMG

本次只为验证候选代码运行了统一安装脚本。新 App 的编译和临时签名成功，但
`scripts/build-desktop.sh --local` 仍进入 DMG 打包并在 `bundle_dmg.sh` 失败，所以安装
脚本没有替换任何 App，原测试版进程继续运行。

建议本地模式只构建 `.app` bundle，生产模式继续按发布配置生成 DMG，并增加脚本测试
固定两种模式的 Tauri 参数。这是发布链问题，与 Claude Web 故障无直接关系，建议另开
任务处理。

## 后续能力应该怎么接

### 联网搜索

短期路线是给 Claude Code 配一个本地 skill 或 MCP 搜索工具：本机执行搜索，返回标题、
URL 和摘要；需要正文时再交给已经打通的 WebFetch。这样搜索执行权清楚，也不依赖
Anthropic 托管服务。搜索实现需要独立处理供应商、限流、引用、隐私和失败回退。

中期如果 Token Station 要提供统一 hosted search，应设计自己的工具协议和状态机，
不能只复用 `web_search_*` 名字冒充 Anthropic。至少要定义工具输入、搜索结果引用、
多轮 tool result、计费归属、日志脱敏和 provider 不支持时的行为。

### 浏览器操作

WebSearch 只解决“找到信息”，不解决登录态、点击、表单和动态页面。需要真实浏览器时，
应走 Claude Code 的浏览器扩展或浏览器 MCP。这条链与 Token Station 的模型路由并行：
浏览器工具在本机执行，结果再回传给 GLM 继续判断。

### 推理与 thinking 展示

第一阶段只保证 effort 到上游，不急着承诺完整思维链显示。验收时应查看 mock upstream
收到的 `reasoning_effort`，再核对真实 provider 响应里是否有 reasoning token 或
reasoning content。若供应商不返回这些字段，只能确认“参数已送达且请求成功”，不能
宣传“已显示完整 thinking”。

### Token 计数、缓存与其他 Anthropic 能力

当前 `/v1/messages/count_tokens` 未实现时，Claude Code 会退回本地估算。这不会阻断
对话，但上下文余量可能不够精确。建议后续单独实现并做跨协议误差测试。

一次响应里 cache count 为 0 不能证明缓存失效。是否支持 prompt caching 取决于
Claude Code 请求、adapter 字段保留和 WeCoding 上游能力，必须用两次相同长前缀请求及
上游计费字段验证。Anthropic 账号侧的托管能力也不应默认出现在第三方 GLM 网关上。

## 建议的正式实施顺序

1. 先补公开行为测试：effort 转换、未知 output_config 拒绝、server tool capability
   错误，并确认普通 client tool 不受影响。
2. 再实现 `output_config.effort -> reasoning_effort` 和 server tool 分类错误，不加入搜索
   执行逻辑。
3. 单独修启动代理和 Agent 诊断，真实验证 WebFetch 的域名检查与网页抓取。
4. 把本地搜索 skill 或 MCP 作为可选能力设计，明确引用、隐私和失败回退。
5. 修复本地 `.app` 与生产 DMG 的构建目标分离后，再安装测试版做真实界面验收。
6. 最后才评估 hosted search、精确 token 计数和缓存指标，这些不应阻塞主链修复。

## 正式版本验收表

| 场景 | 需要观察的证据 | 通过标准 |
| --- | --- | --- |
| 普通对话 | resolved upstream/model、200 响应 | GLM 5.2 返回有效内容 |
| 本地工具 | 两轮 Agent 请求和 tool result | Read/Bash 后得到正确最终答案 |
| WebFetch | domain_info、目标网页、web_fetch_apply | 无代理时可取公开网页并总结 |
| 内置 WebSearch | adapter 错误和 upstream hit count | 返回明确 capability，upstream 为 0 |
| 本地/MCP 搜索 | 搜索工具结果、URL、后续抓取 | 有引用且不依赖 Anthropic server tool |
| effort | mock upstream 请求体 | high/max 被渲染为 reasoning_effort |
| 路由说明 | requested/resolved 两组字段 | 用户能看出模型名是否被覆盖 |
| 日志安全 | requests、debug、错误响应 | 不含 prompt、response、Key 或 Cookie |
| 桌面安装 | bundle id、签名、进程、8787 PID | 新构建全部通过后才替换旧 App |

## 当前状态

本文记录的问题已经复现，候选转换逻辑也只在隔离 worktree 中做过针对性测试。根据测试
版边界，候选代码、本机启动脚本和临时搜索 skill 已撤回；没有更新 `/Applications`，
没有替换当前运行 App，没有提交 commit，也没有推送远端。正式实现需要另行授权。
