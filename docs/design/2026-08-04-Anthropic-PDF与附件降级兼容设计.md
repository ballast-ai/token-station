# Anthropic PDF 与附件降级兼容设计

日期：2026-08-04
状态：已实现并通过真实 App 验收

## 1. 问题

Claude Code 和 Claude Desktop 的内置 `Read` 工具读取 PDF 后，会把文件放进用户消息的
`tool_result.content`，内容块类型为 Anthropic `document`。Token Station 的 Anthropic 入站适配器
无法把该类型写入当前 Canonical IR，因此在调用上游前返回 400：

```text
Anthropic tool-result content block `document` has no Canonical IR representation
```

图片已经有能力降级和单次重试，但 PDF、普通文件和音频没有同等保护。最危险的兼容方式是把
`document.source.data` 的 base64 序列化成普通文本，这既丢失文件语义，也可能消耗大量 token。

## 2. 目标、范围与非目标

本轮目标是让所有使用 Anthropic Messages 入站协议的 Agent 都能安全处理 `document`：

1. 路由到 Anthropic 原生上游时，继续原样透传文档，不改变已有能力。
2. 路由到当前 Canonical IR/OpenAI Chat 转换链时，PDF 有文字层就先在本机内存中提取文字，再交给模型。
3. PDF 没有可提取文字、文件类型不受支持或数据无效时，用与请求语言一致的附件占位说明继续请求，
   不把 base64 传给模型，也不再返回 Canonical IR 400。
4. 保留现有图片降级；音频继续使用当前明确的 capability 拒绝，不能伪装成模型已经听过音频。

本轮不新增 OCR，不联网下载 URL 文档，不解析 DOCX、表格、压缩包或可执行文件，也不把第三方模型
伪装成拥有它没有的视觉或文件能力。OpenAI Responses 和 Gemini 的原生文件映射需要新的上游方言，
不在此次 OpenAI Chat 修复中偷偷近似实现。

## 3. 安全与数据红线

1. PDF 解码和文字提取只在 Token Station 进程内存中完成，不写临时文件，不上传到额外服务。
2. 单个文档解码后最大 8 MiB；超过限制时不解析，只生成附件占位说明。
3. 注入模型的 PDF 文字最多 120,000 个 Unicode 字符，超出部分明确标记为截断。
4. 日志和请求收据只能记录替换数量与结果类型，禁止记录文件正文、base64、密钥或本地路径。
5. URL 文档不由 Token Station 主动抓取，避免把客户端可控 URL 变成新的服务器端请求出口。
6. 无论解析成功与否，base64 都不能进入 Canonical 文本或上游 OpenAI Chat 请求。

## 4. 用户可见行为与失败处理

PDF 提取成功后，模型会收到一个普通文本块，其中含“Token Station 已把附件转换为文字”、文件名、
媒体类型和提取出的正文。Claude 因此可以继续总结、检查或回答 PDF 内容。

PDF 没有文字层或解析失败时，模型会收到一条明确的附件占位说明：当前路由没有读取该附件的能力，
不能声称已经看过内容；如果用户想继续处理附件，应切换到支持文件或视觉/OCR 的模型。如果用户只想
继续文字对话，应编辑原消息并删除附件后重试。中文请求使用中文说明，其他请求使用英文说明。

不支持的普通文档类型和 URL 文档使用同一失败说明。图片仍使用现有图片占位和单次重试逻辑。

## 5. 响应式、键盘和可访问性

本轮不增加界面控件，不改变响应式布局、焦点顺序或键盘操作。错误与降级信息作为普通对话文本进入
现有可访问消息区域，不能只靠颜色表达状态。

## 6. 公开测试边界与验收标准

1. Anthropic `tool_result` 中的 base64 PDF 可通过 OpenAI Chat 转换路由，返回 200，上游收到提取文字。
2. 上游请求不含原始 PDF base64。
3. 顶层用户 `document` 与工具结果中的 `document` 使用相同处理。
4. 损坏 PDF、空文字 PDF、超限 PDF、非 PDF 和 URL 文档不崩溃、不联网、不泄漏数据，改用本地化说明。
5. Anthropic 原生上游仍收到原始 `document`，不会被提前转成文字。
6. 现有图片、普通工具结果、server-tool history、音频拒绝和认证行为不回归。
7. Rust 定向测试、CLI 全量测试、workspace 测试、格式和静态检查通过。
8. 执行 `scripts/install-local-desktop.sh`，确认本机 App 的 bundle id 和签名，再启动验证。
9. 用真实 Claude Code 或 Claude Desktop 的 `Read` 工具读取一份带文字层 PDF；请求到达上游，回复依据
   PDF 正文，而不是返回 Canonical IR 400 或声称读取了实际未读取的附件。
10. Claude assistant 只有 `tool_use`、没有文字正文时，OpenAI Chat 输出仍显式包含 `content: null`；
    DeepSeek 等严格实现不能因为字段缺失把后续 `tool_result` 判为 malformed。
11. DeepSeek 推理模型的 assistant 工具历史必须包含 `reasoning_content`。有 Anthropic thinking 历史时
    还原真实文字；没有时写入 DeepSeek 接受的空占位，不伪造推理过程，也不把该私有字段发给其他模型。

## 7. 实现落点与发布要求

主机侧在 `apps/cli/src/gateway.rs` 的 Anthropic 非原生路径预处理原始 JSON。主机侧负责受限 base64
解码和 PDF 文字提取；WASM Anthropic adapter 继续只接收可由 Canonical IR 表达的内容。这样原生
Anthropic 透传不会被破坏，也不会把 PDF 解析器塞进每个协议插件。

`plugins/official/provider-openai-compatible/src/lib.rs` 对只有工具调用的 assistant 消息写入
`content: null`。这与 OpenAI Chat 的合法形状一致，也兼容要求该字段必须存在的严格上游。
同一插件只对模型名以 `deepseek` 开头的工具历史补齐 `reasoning_content`；其他 OpenAI-compatible
模型保持原请求形状，避免向严格的非 DeepSeek 接口发送厂商私有字段。

公开行为回归放在 `apps/cli/tests/proxy.rs`。完成后必须回写本节状态、真实 App 验收结果和遗留项，
随后使用中文提交说明创建本地 commit。

## 8. 当前验收状态

已完成：

1. 公开回归先复现了 `document has no Canonical IR representation` 400，实现后改为
   200；上游能看到 PDF 文字和文件名，请求中不含原始 base64。
2. 损坏 PDF 和 URL 文档回归通过：网关使用请求语言的说明继续对话，不抓取 URL，
   不把原数据交给模型。文本模型的图片降级与原有行为保持一致。
3. OpenAI-compatible 插件已为纯 `tool_use` assistant 历史写入 `content: null`，并只对
   DeepSeek 模型补齐工具续轮要求的 `reasoning_content`。直连验证确认，缺少该字段会
   返回 400，空占位能正常续轮。
4. `scripts/check-rust-format.sh`、`cargo clippy --workspace --all-targets -- -D warnings`、
   `cargo test --workspace` 全部通过。Proxy 回归为 `68 passed, 1 ignored`；桌面端为
   `226 passed, 1 ignored`，安装器与 YAML 回归额外 5 项全部通过。
5. `scripts/install-local-desktop.sh` 已完成桌面构建、产物审计、签名校验、精确替换和启动；
   真实 App 显示代理运行中，revision `113` 已应用。
6. 真实验收使用 7 页、903 KB 的
   `PHYS200_LabReport4_LuXiaorui_3806592.pdf`，通过 Claude Code Anthropic `tool_result`
   路径发送。DeepSeek 路由返回 200，正确回答报告标题 `Lab 4: Hooke's Law` 和
   `g = 9.81 m/s²`；收据显示只请求一次上游。请求日志中没有文件名或 PDF base64 头。

已知遗留项：扫描件或只有图片的 PDF 没有 OCR；DOCX、表格、音频等格式不会被伪装成
已读取，而是继续使用明确的本地化能力说明或现有 capability 拒绝。
