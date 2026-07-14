# CLI 命令参考

所有功能都在同一个二进制 `token-station-cli` 里。命令来源以源码
[apps/cli/src/main.rs](../../apps/cli/src/main.rs) 的 clap 定义为准。

## 全局选项

| 选项 | 默认 | 说明 |
|---|---|---|
| `--config <PATH>` | `token-station.json` | 每个命令都读它；编辑类命令原子重写它。所有子命令通用。 |
| `--version` | | 打印版本。 |
| `--help` | | 任意层级都可加 `--help` 看子命令用法。 |

## 命令一览

| 命令 | 作用 |
|---|---|
| `serve` | 启动回环代理。 |
| `key set / remove` | 在 OS 钥匙串里存 / 删上游凭证。 |
| `upstream list / add / remove / test` | 列出 / 增删 / 探活上游。 |
| `config set / edit` | 翻开关，或在校验保护下编辑整份配置。 |
| `plugin list / install / remove / info / new / build / test` | 插件的安装面与开发面。 |
| `rule list` | 查看路由表（只读）。 |
| `stats` | 聚合本地指标库。 |
| `upgrade` | 匿名检查新版，签名验证通过后才更新。 |

---

## serve

```bash
token-station-cli serve
```

启动回环代理，监听配置里 `server.listen`（默认 `127.0.0.1:8787`）。首次启动
会打印一次本地虚拟 key（`ts-...`），立即保存——客户端用它做鉴权。停止用
`Ctrl-C`。

## key

凭证值从 **stdin** 读取，永不进 argv（不会落进 shell 历史 / 进程列表）。

```bash
token-station-cli key set <upstream> <slot>      # 存，值从 stdin 读
token-station-cli key remove <upstream> <slot>   # 删
```

`<slot>` 是 provider adapter 解析的凭证槽名，一般是 `provider_api_key`。

## upstream

```bash
token-station-cli upstream list

token-station-cli upstream add <name> \
  --provider <dialect> \
  --base-url <url> \
  --model "<model>[,tool][,vision][,json-schema][,ctx=N]" \
  [--auth keyring | env:<VAR> | file:<PATH>] \
  [--slot provider_api_key] \
  [--pool <pool>]

token-station-cli upstream remove <name>
token-station-cli upstream test <name> [--model <model>]
```

| 参数 | 说明 |
|---|---|
| `<name>` | 上游引用名，池会引用它，如 `openai_personal`。 |
| `--provider` | provider 方言；必须由一个已发现的插件包提供（`plugin list` 显示有哪些）。 |
| `--base-url` | 基址，**不能夹带凭证**，否则被拒。 |
| `--model` | 可重复；`,tool` `,vision` `,json-schema` `,ctx=N` 声明能力。 |
| `--auth` | 凭证位置：`keyring` / `env:<VAR>` / `file:<PATH>`。开放上游（如本地 Ollama）省略。 |
| `--slot` | 凭证槽名，默认 `provider_api_key`。 |
| `--pool` | 同时把这些模型追加到该池，让路由能到达它们。 |

- `upstream remove`：当仍有池路由到它时被拒。
- `upstream test`：对每个声明的模型发一次**真实**的最小 completion，会消耗额度。

## config

```bash
token-station-cli config set <switch> <on|off>   # switch: server.auth | data.metrics
token-station-cli config edit                    # 用 $VISUAL/$EDITOR 打开
```

`config edit` 打开整份配置文件编辑，**只有校验通过才应用**；被拒的编辑让文件
保持逐字节不变。

## plugin

安装面与开发面在同一组命令下。开发链细节见 [插件体系.md](插件体系.md)。

```bash
# 安装面
token-station-cli plugin list                    # 已发现的包 + upstream add 查的方言表
token-station-cli plugin install <path>          # 跑 conformance，通过才纳入
token-station-cli plugin remove <name>           # 有上游经它路由时被拒
token-station-cli plugin info <name>             # 身份 / 信任 / 方言 / 声明的密钥

# 开发链（第三方 adapter 作者）
token-station-cli plugin new <name> [--dialect <d>] [--dir <parent>]
token-station-cli plugin build [<path>]          # cargo build --target wasm32-wasip2 并就位
token-station-cli plugin test [<path>]           # 跑 install 时同一套 conformance
```

- `install <path>`：路径下需有 `manifest.json`、`adapter.wasm`、`fixtures/`；跑
  conformance 套件，通过后复制进插件目录并记录批准。
- `new`：脚手架不是空壳，而是把官方 OpenAI 兼容 adapter 改名——一上来就能编译、
  能过 conformance。

## rule

```bash
token-station-cli rule list
```

按求值顺序逐层打印路由表（只读）。四层的含义见 [路由机制.md](路由机制.md)。

## stats

```bash
token-station-cli stats [--since <window>] [--by <dimension>]
```

| 参数 | 取值 | 默认 |
|---|---|---|
| `--since` | `all` / `<N>h` / `<N>d` | `24h` |
| `--by` | `upstream` / `model` / `pool` / `status` | 无（只出总计） |

以**只读**方式打开 `serve` 写的同一个 SQLite 指标库，聚合用量、错误、延迟、
token。只读打开保证与运行中的 `serve` 不争用、也不会误改历史。

## upgrade

```bash
token-station-cli upgrade [--yes] [--check-only]
```

匿名检查是否有新版；**仅在签名 manifest 验证通过后**才下载并保留。

| 参数 | 说明 |
|---|---|
| `--yes` | 跳过确认提示（仍会先验证再保留）。 |
| `--check-only` | 只检查上报，从不下载（与 `--yes` 互斥）。 |

验证原理见 [../release/可复现构建与发布验证.md](../release/可复现构建与发布验证.md)。
