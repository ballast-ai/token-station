# CLI 命令行交互设计

> 场景：个人本地版没有本地 Web 管理面，CLI 是唯一的本地管理面。本文定义 `token-station` CLI 的命令结构、交互风格、输出约定和 C1-C3 分期，作为 [个人模式本地客户端概要设计](./个人模式本地客户端概要设计.md) 的命令行细化。
>
> 整理日期 2026-07-09。核心原则：**默认本地、显式联网、secret 不回显、机器可读输出可选、所有会影响数据面行为的变更都可审计和回滚。**

---

## 一、设计目标

1. 让用户用 CLI 完成个人版全部本地管理：启动代理、管理 key、配置上游、规则、指标、同步和对账。
2. 让 IDE / Agent 接入成本最低：首次启动自动生成本地虚拟 key，并清晰输出 `base_url` 与示例环境变量。
3. 保持隐私承诺可验证：关闭云同步和远程配置时，CLI 能说明当前会访问哪些网络目的地。
4. 支持脚本化：所有查询命令都支持 `--json`，所有变更命令支持 `--yes` 跳过交互确认。
5. 避免魔法：不在后台自动上传内容，不自动启用云端功能，不自动升级。

---

## 二、全局约定

### 2.1 命令形态

```bash
token-station <command> [subcommand] [flags]
ts <command> [subcommand] [flags]   # 可选短别名，安装器创建
```

全局 flags：

| Flag | 说明 |
|------|------|
| `--profile <name>` | 指定本地 profile，默认 `default` |
| `--config <path>` | 指定配置文件路径 |
| `--json` | 输出机器可读 JSON |
| `--no-color` | 禁用彩色输出 |
| `--yes` / `-y` | 跳过确认，仅用于变更命令 |
| `--verbose` / `-v` | 输出调试级本地日志，不输出 secret |

### 2.2 输出规则

- 人类输出默认简洁，成功只显示关键结果和下一步。
- `--json` 输出稳定字段，供脚本和 UI 包装器消费。
- secret 默认永不回显；新增 key 只在创建瞬间显示一次，之后只显示前后缀摘要。
- 会产生联网行为的命令必须在执行前说明目标域名和用途，除非用户传 `--yes`。
- 命令失败必须返回非 0 exit code，并输出稳定错误码。

### 2.3 Exit Code

| Code | 含义 |
|------|------|
| `0` | 成功 |
| `1` | 通用失败 |
| `2` | 参数错误 |
| `3` | 配置错误 |
| `4` | 鉴权/授权失败 |
| `5` | 上游不可用 |
| `6` | 本地代理未运行 |
| `7` | 隐私/安全策略阻止 |

---

## 三、命令总览

| 命令 | C1 | C2 | C3 | 说明 |
|------|----|----|----|------|
| `doctor` | ✅ | ✅ | ✅ | 环境和配置诊断 |
| `init` | ✅ | ✅ | ✅ | 初始化本地配置、虚拟 key、指标库 |
| `serve` | ✅ | ✅ | ✅ | 启动 `127.0.0.1` 本地代理 |
| `status` | ✅ | ✅ | ✅ | 查看代理、profile、上游、同步状态 |
| `key` | ✅ | ✅ | ✅ | 管理本地虚拟 key |
| `upstream` | ✅ | ✅ | ✅ | 管理 BYOK、本地模型、平台账户上游 |
| `rule` | ✅ | ✅ | ✅ | 管理本地路由规则 |
| `model` | ✅ | ✅ | ✅ | 查看聚合模型目录 |
| `usage` | ✅ | ✅ | ✅ | 本地指标库统计 |
| `metrics` | ✅ | ✅ | ✅ | 指标库开关、导出、清理 |
| `auth` |  | ✅ | ✅ | OAuth 设备码授权 |
| `sync` |  | ✅ | ✅ | 云同步开关和状态 |
| `config` | ✅ | ✅ | ✅ | 本地/远程 profile 管理 |
| `upgrade` | ✅ | ✅ | ✅ | 匿名检查版本、显式升级 |
| `audit` |  |  | ✅ | 本地隐私审计输出 |
| `reconcile` |  |  | ✅ | 本地指标库与平台账单对账 |

---

## 四、C1 命令

### 4.1 `init`

初始化本地运行所需文件：

```bash
token-station init
```

行为：

- 创建默认 profile；
- 首启生成本地虚拟 key；
- 初始化指标库；
- 不绑定账号；
- 不启用云同步；
- 不上传任何内容。

示例输出：

```text
Created profile: default
Created local virtual key: ts_local_xxx...abcd
Created metrics database: ~/Library/Application Support/token-station/metrics.sqlite

Use it from OpenAI-compatible clients:
  export OPENAI_BASE_URL=http://127.0.0.1:4317/v1
  export OPENAI_API_KEY=ts_local_xxx...abcd
```

`--json` 示例：

```json
{
  "profile": "default",
  "base_url": "http://127.0.0.1:4317/v1",
  "local_key_id": "default",
  "metrics_enabled": true,
  "cloud_sync_enabled": false
}
```

### 4.2 `serve`

启动本地代理：

```bash
token-station serve
token-station serve --port 4317
token-station serve --profile work
```

约束：

- 默认只监听 `127.0.0.1`；
- 监听 `0.0.0.0` 必须显式传 `--listen 0.0.0.0` 并二次确认；
- 启动时打印当前外联策略摘要。

示例输出：

```text
token-station local proxy is running
  base_url: http://127.0.0.1:4317/v1
  profile: default
  upstreams: openai-personal, ollama-local
  cloud_sync: off
  remote_config: off
```

### 4.3 `status`

```bash
token-station status
token-station status --json
```

输出内容：

- 代理是否运行；
- 当前 profile；
- 本地虚拟 key 数量；
- 上游健康状态；
- 指标库状态；
- 云同步/远程配置状态；
- 最近一次外联目的地摘要。

### 4.4 `key`

本地虚拟 key 是 IDE / Agent 连接 `127.0.0.1` 的凭证。

```bash
token-station key list
token-station key create --name cursor
token-station key revoke cursor
token-station key rotate cursor
```

输出规则：

- `create` 只在创建瞬间显示明文；
- `list` 只显示摘要和创建时间；
- `revoke` / `rotate` 需要确认，除非传 `--yes`。

### 4.5 `upstream`

管理三类当前上游：BYOK 直连、本地模型、平台账户。

#### BYOK 直连

```bash
token-station upstream add openai \
  --type byok \
  --provider openai \
  --key-env OPENAI_API_KEY

token-station upstream add anthropic \
  --type byok \
  --provider anthropic \
  --key-prompt
```

约束：

- `--key-env` 从环境变量读取后写入本地密钥保管；
- `--key-prompt` 交互式输入，终端不回显；
- 不支持把 key 写进普通配置文件。

#### 本地模型

```bash
token-station upstream add ollama \
  --type local \
  --base-url http://127.0.0.1:11434/v1
```

#### 平台账户上游（C2）

```bash
token-station upstream add platform --type platform
```

若未授权，触发 `auth device`。

#### 通用操作

```bash
token-station upstream list
token-station upstream test openai
token-station upstream disable openai
token-station upstream remove openai
```

### 4.6 `model`

```bash
token-station model list
token-station model list --provider openai
token-station model refresh
```

模型目录来源：

- 本地配置；
- 上游 `/models`；
- 平台下发定价表（C2）；
- 本地缓存。

### 4.7 `rule`

管理本地路由规则：

```bash
token-station rule list
token-station rule add code-review \
  --when "has_tool == true || code_blocks >= 1" \
  --then "model=claude-sonnet,provider=anthropic"
token-station rule disable code-review
token-station rule remove code-review
token-station rule test --file ./fixtures/request.json
```

C1 只要求本地文件配置源；C2 之后可从远程 profile 拉取非机密规则。

### 4.8 `usage`

读取本地指标库：

```bash
token-station usage today
token-station usage month
token-station usage range --from 2026-07-01 --to 2026-07-09
token-station usage top models
token-station usage errors
```

默认展示：

- 请求数；
- input/output token；
- 估算成本；
- 平均延迟；
- 错误数；
- 路由命中规则。

### 4.9 `metrics`

```bash
token-station metrics status
token-station metrics disable
token-station metrics enable
token-station metrics export --format json --out usage.json
token-station metrics purge --before 2026-01-01
```

约束：

- 指标库默认开启；
- 关闭后仍保留文件日志；
- purge 需要确认；
- export 不包含 prompt/response。

### 4.10 `doctor`

```bash
token-station doctor
```

检查项：

- 配置文件可解析；
- 密钥保管可用；
- 指标库可读写；
- 本地端口可绑定；
- 上游健康；
- 当前隐私模式下的外联目的地。

---

## 五、C2 命令

### 5.1 `auth`

设备码授权：

```bash
token-station auth device
token-station auth status
token-station auth logout
```

示例输出：

```text
Open this URL:
  https://token-station.ai/device

Enter code:
  AB12-CD34

Waiting for authorization...
Authorized as lv@example.com
Scopes: platform-upstream, usage-read, sync-write, release-download
```

### 5.2 `sync`

云同步默认关闭：

```bash
token-station sync status
token-station sync enable
token-station sync disable
token-station sync push-now
```

开启前确认文案必须列出字段白名单：

```text
Cloud sync uploads metadata only:
  device_id, time_bucket, upstream_ref, model, token counts,
  latency, status_code, error_code, route_rule_id, estimated_cost, client_version

It never uploads prompt, response, request headers, or provider keys.
```

### 5.3 `config`

```bash
token-station config profile list
token-station config profile use default
token-station config profile export --out profile.json
token-station config remote status
token-station config remote enable
token-station config remote pull
token-station config remote rollback --version 12
```

约束：

- 远程配置只包含非机密 profile；
- 云端 profile 不能包含 provider key 明文；
- 云端不可达时使用最后一次成功缓存；
- 关闭远程配置后回到本地 profile。

### 5.4 `upgrade`

```bash
token-station upgrade check
token-station upgrade install
```

约束：

- `check` 可匿名；
- `install` 需要用户显式确认；
- 安装包签名必须校验；
- 不做静默自动更新。

---

## 六、C3 命令

### 6.1 `reconcile`

本地对账：

```bash
token-station reconcile platform --from 2026-07-01 --to 2026-07-09
token-station reconcile platform --json
```

流程：

1. 拉取平台只读账单；
2. 读取本地指标库；
3. 按 request_id / time window / model / token 口径匹配；
4. 输出差异报告。

示例输出：

```text
Compared 1,248 local records with platform billing.

Matched: 1,241
Missing locally: 0
Missing remotely: 3
Token mismatch: 4
Cost mismatch: 2

Report: ./reconcile-2026-07-01_2026-07-09.json
```

### 6.2 `audit`

本地隐私审计输出：

```bash
token-station audit local
token-station audit network --last 24h
token-station audit export --out audit.json
```

输出内容：

- 当前 profile 摘要；
- 启用的云端功能；
- 同步字段白名单版本；
- 最近 N 条路由决策；
- 最近 N 条外联目的地与用途；
- 本地 key 摘要，不含明文；
- 指标库状态。

---

## 七、典型交互流程

### 7.1 纯 BYOK 用户首次使用

```bash
token-station init
token-station upstream add openai --type byok --provider openai --key-prompt
token-station serve
```

然后在 IDE / Agent 中配置：

```bash
OPENAI_BASE_URL=http://127.0.0.1:4317/v1
OPENAI_API_KEY=<local virtual key>
```

### 7.2 本地模型 + BYOK 混合路由

```bash
token-station upstream add ollama --type local --base-url http://127.0.0.1:11434/v1
token-station upstream add openai --type byok --provider openai --key-prompt
token-station rule add local-cheap \
  --when "tokens < 2000 && has_tool == false" \
  --then "provider=ollama,model=llama3.1"
token-station rule add hard-code \
  --when "code_blocks >= 1 || has_tool == true" \
  --then "provider=openai,model=gpt-4.1"
```

### 7.3 开启多设备用量观测

```bash
token-station auth device
token-station sync enable
token-station sync push-now
```

### 7.4 做本地对账

```bash
token-station reconcile platform --from 2026-07-01 --to 2026-07-09
```

---

## 八、配置与数据位置

默认路径按平台约定：

| 数据 | macOS | Linux | Windows |
|------|-------|-------|---------|
| 配置 | `~/Library/Application Support/token-station/config.toml` | `~/.config/token-station/config.toml` | `%APPDATA%\token-station\config.toml` |
| 指标库 | `~/Library/Application Support/token-station/metrics.sqlite` | `~/.local/share/token-station/metrics.sqlite` | `%LOCALAPPDATA%\token-station\metrics.sqlite` |
| 日志 | `~/Library/Logs/token-station/` | `~/.local/state/token-station/logs/` | `%LOCALAPPDATA%\token-station\logs\` |
| 密钥 | OS keychain 优先 | Secret Service/libsecret 优先 | Windows Credential Manager 优先 |

---

## 九、隐私与安全交互规则

- CLI 不接受 `--key <plaintext>`，只接受 `--key-env` 或 `--key-prompt`。
- `status` / `doctor` / `audit` 不输出 secret 明文。
- `sync enable` 必须显示字段白名单并要求确认。
- `config remote enable` 必须说明云端配置不包含 key。
- `serve --listen 0.0.0.0` 必须二次确认，并提示局域网暴露风险。
- 所有删除/清空/撤销命令默认二次确认。

---

## 十、命令到里程碑映射

| 里程碑 | 必须完成的命令 |
|--------|----------------|
| C1 | `init`、`serve`、`status`、`doctor`、`key`、`upstream`（BYOK/local）、`model`、`rule`、`usage`、`metrics`、`upgrade check` |
| C2 | `auth`、`sync`、`config remote`、`upstream add platform`、`upgrade install` |
| C3 | `reconcile`、`audit` |

