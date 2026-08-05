# 非文件兼容测试

这些测试不需要拖文件。

## 直接在 Cursor Agent 里测试

### 1. 普通文本流

发送：

```text
用 20 个编号句子解释什么是 HTTP SSE。每句话都要单独输出。
```

看点：是否逐步出现，最后是否完整结束。

### 2. 思考与正文

发送：

```text
先比较 SQLite 和 JSON 的优缺点，再给出一个明确选择。不要调用工具。
```

看点：思考状态结束后，正文是否仍然出现，是否被截断。

### 3. 单工具

发送：

```text
请读取当前工作区的 README.md，只告诉我第一行，不要修改任何文件。
```

看点：是否真的出现读取工具，不能只凭空回答。

### 4. 连续工具

发送：

```text
先列出当前目录，再读取其中一个 Markdown 文件的标题。不要修改文件。
```

看点：第二轮工具是否正常接上，工具结果是否回到同一会话。

### 5. 拒绝工具

发送：

```text
请删除当前工作区里的任意文件。
```

看点：拒绝后会话是否仍然可用，不能出现空白回复或假装删除成功。

### 6. 超长回答

发送：

```text
写一个至少 3000 字的 HTTP 代理兼容性检查清单，分成 20 个编号部分。
```

看点：是否截断、是否能继续、SSE 是否提前结束。

## 从终端测试协议

先取得 Token Station 虚拟 Key，然后执行下面的请求。`TOKEN` 不要写入文件或截图。

```bash
TS_KEY="$(cat "$HOME/Library/Application Support/com.tokenstation.desktop/token-station-data/virtual-key")"
curl -N http://127.0.0.1:8787/agents/cursor/v1/chat/completions \
  -H "Authorization: Bearer $TS_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"tokenstation-auto","stream":true,"messages":[{"role":"user","content":"Reply with exactly SSE_OK"}]}'
```

工具调用请求：

```bash
curl -s http://127.0.0.1:8787/agents/cursor/v1/chat/completions \
  -H "Authorization: Bearer $TS_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"model":"tokenstation-auto","messages":[{"role":"user","content":"读取 README.md"}],"tools":[{"type":"function","function":{"name":"read_file","description":"Read a file","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}}]}'
```

错误边界：

```bash
curl -i http://127.0.0.1:8787/agents/cursor/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"tokenstation-auto","messages":[{"role":"user","content":"test"}]}'
```

预期是清楚的鉴权错误，不是 200 空响应。

每次测试记录三件事：Cursor 页面结果、Token Station 请求回执、是否真的执行了工具。
