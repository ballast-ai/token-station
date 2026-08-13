# Hermes Agent 安全接入指南

Token Station 桌面端可以自动发现并接入 NousResearch Hermes Agent。当前内置兼容目录
没有版本 blocklist，因此不会只按 `hermes-agent 0.18.0` 阻断其他版本；这不代表任意未来
版本都经过真实验收，配置结构、适配器就绪和内部计划复核仍会在写入前失败关闭。

官方依据：

- [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent)
- [v2026.7.1](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.7.1)
- [配置说明](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/configuration.md)
- [Provider runtime](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/provider-runtime.md)

## 1. 自动发现

桌面端只读检查：

- 可执行文件：`hermes`，版本命令 `hermes version`；
- 版本输出规范化为首个 package SemVer；
- 环境覆盖：`HERMES_HOME/config.yaml`；
- 默认路径：macOS/Linux/WSL 为 `~/.hermes/config.yaml`，Windows 为
  `%LOCALAPPDATA%/hermes/config.yaml`；
- 配置格式：单文档 YAML。

扫描不执行 setup、update、doctor、migrate、repair，不安装或升级 Hermes，也不启动
Hermes gateway。多安装实例必须由用户选择唯一目标。

## 2. 接入内容

`hermes-v1` 只拥有以下五个标量路径：

```text
/model/default
/model/provider
/model/base_url
/model/api_key
/model/api_mode
```

点击“一键接入”后，桌面端生成内部计划并立即写入；首次接入会在写入完成后展示关键改动。
目标结构为：

```yaml
model:
  default: auto
  provider: custom
  base_url: http://127.0.0.1:8787/v1
  api_key: <本地虚拟 Key>
  api_mode: chat_completions
```

这对应 Hermes 官方的 custom OpenAI-compatible endpoint 契约，请求进入 Token Station
`/v1/chat/completions` 并复用 `agent-openai`。`display`、terminal、gateway、tools、skills、
memory、其他 Provider，以及 `model` 下未声明的字段均不属于 Token Station。

Hermes 官方建议常规长期密钥放在 `.env`；本 Connector 写入的是 Token Station 本机虚拟
Key。为避免 `config.yaml` 与 `.env` 两文件非原子更新，当前只管理一个 YAML 事务，并把
`model.api_key` 声明为敏感路径：IPC 和写入后的改动摘要只显示脱敏值，原始配置先进入本地私有
`snapshot-master.key` 保护的加密快照，新文件权限按安全写入器规范化。未来若实现多文件原子事务，再考虑迁移到
专用环境变量。

## 3. YAML 安全边界

Token Station 不会把 YAML 反序列化后整份重写。lossless CST 只修改 owned paths，并保留：

- 顶部、行内和无关字段注释；
- 未知字段、字段顺序、空白和未触碰的标量样式；
- 接入后用户对未拥有字段的修改。

以下情况写入前直接拒绝：非法 YAML、多个 YAML document、重复键、merge key、非对象根、
`model` 是字符串/数组/非空 flow mapping 或含 anchor/tag/alias，以及其他 owned path 父级类型
冲突。`model:`、`model: null`、`model: ~` 和 `model: {}` 是安全空表示，新版 Token Station
会保留注释并展开为块映射，不再要求用户手工修文件。错误信息不回显原配置行或虚拟 Key。

## 4. 写入和断开

只有 Agent descriptor 已准入且没有命中 blocklist、安装实例唯一、配置指纹未变化、
`agent-openai` 就绪、内部计划和确认令牌仍有效时，后端才会执行：

```text
加密快照 → revision 复验 → 同目录原子替换 → YAML 重解析 → Connector 自检 → ownership 提交
```

点击“一键接入”即代表同意写入。“恢复官方配置并断开”只移除五个 owned paths，并在写盘前
验证删除结果仍可再次接入。owned
values 被其他工具修改后，旧计划会因 revision/ownership 冲突失效，必须重新扫描。历史 `.bak` 只读
展示，不覆盖、不删除、不自动恢复。

## 5. Hermes 更新后的行为

Hermes 更新后，Token Station 会重新扫描版本、路径和配置结构。当前空 blocklist 不会仅因
版本号变化自动禁止接入，但签名兼容目录可以增加明确的阻断范围；YAML schema 或 owned
paths 不兼容时，Connector 仍会在写入前拒绝。扩大真实验收范围时仍应核对官方 tag、
Provider runtime、版本输出、lossless fixtures 和隔离 E2E；如果 owned paths 或协议发生
变化，应新增 `hermes-v2`，不能静默改变 `hermes-v1`。

Token Station 不会自动执行 `hermes update`，也不会为通过验收而修改
`crates/router-core/**`。cc-Switch 仅作为竞品参考，不是 Hermes 产品契约来源。
