# Hermes Agent 安全接入指南

Token Station 桌面端可以自动发现 NousResearch Hermes Agent，并只对经过本地验收的
`hermes-agent 0.18.0` 生成配置预览。该 package 版本对应官方 `v2026.7.1` 发布；当前官方
`v2026.7.7.2` 的 package 版本 `0.18.2` 仍显示为“未知”，不会写配置。

官方依据：

- [NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent)
- [v2026.7.1](https://github.com/NousResearch/hermes-agent/releases/tag/v2026.7.1)
- [配置说明](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/configuration.md)
- [Provider runtime](https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/provider-runtime.md)

## 1. 自动发现

桌面端只读检查：

- 可执行文件：`hermes`，版本命令 `hermes version`；
- 版本输出 `Hermes Agent v0.18.0 (2026.7.1)` 规范化为首个 package SemVer
  `0.18.0`；
- 环境覆盖：`HERMES_HOME/config.yaml`；
- 默认路径：macOS/Linux/WSL 为 `~/.hermes/config.yaml`，Windows 为
  `%LOCALAPPDATA%/hermes/config.yaml`；
- 配置格式：单文档 YAML。

扫描不执行 setup、update、doctor、migrate、repair，不安装或升级 Hermes，也不启动
Hermes gateway。多安装实例必须由用户选择唯一目标。

## 2. 接入前预览

`hermes-v1` 只拥有以下五个标量路径：

```text
/model/default
/model/provider
/model/base_url
/model/api_key
/model/api_mode
```

确认后的目标结构为：

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
`model.api_key` 声明为敏感路径：预览和 IPC 只显示脱敏值，原始配置先进入 OS keychain
保护的加密快照，新文件权限按安全写入器规范化。未来若实现多文件原子事务，再考虑迁移到
专用环境变量。

## 3. YAML 安全边界

Token Station 不会把 YAML 反序列化后整份重写。lossless CST 只修改 owned paths，并保留：

- 顶部、行内和无关字段注释；
- 未知字段、字段顺序、空白和未触碰的标量样式；
- 接入后用户对未拥有字段的修改。

以下情况写入前直接拒绝：非法 YAML、多个 YAML document、重复键、merge key、非对象根、
`model` 不是对象、owned path 父级类型冲突。错误信息不回显原配置行或虚拟 Key。

## 4. 确认、写入和恢复

只有版本精确命中、安装实例唯一、配置指纹未变化、`agent-openai` 就绪，且用户逐项确认
安装实例、目标文件和脱敏差异时，后端才会执行：

```text
加密快照 → revision 复验 → 同目录原子替换 → YAML 重解析 → Connector 自检 → ownership 提交
```

“断开”只恢复五个 owned paths；“恢复快照”可以精确恢复原始字节。owned values 被其他工具
修改后，旧计划会因 revision/ownership 冲突失效，必须重新扫描和预览。历史 `.bak` 只读
展示，不覆盖、不删除、不自动恢复。

## 5. Hermes 更新后的行为

`0.18.0` 以外版本均为 `DETECTED_UNKNOWN`，只能查看详情、重新扫描和导出诊断，不能预览
接入。扩大范围必须先对新官方 tag 重新核对 YAML schema、Provider runtime、版本输出、
lossless fixtures 和隔离 E2E；如果 owned paths 或协议发生变化，应新增 `hermes-v2`，不能
静默改变 `hermes-v1`。

Token Station 不会自动执行 `hermes update`，也不会为通过验收而修改
`crates/router-core/**`。cc-Switch 仅作为竞品参考，不是 Hermes 产品契约来源。
