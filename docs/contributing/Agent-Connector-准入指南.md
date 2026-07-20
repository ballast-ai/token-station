# Agent Connector 准入指南

本指南用于新增 Agent、适配 Agent 新版本或升级已有 Connector。目标是让 Agent 安全接入本机代理，不改变核心路由，不接管 Agent 生命周期。

## 1. 不可跨越的边界

- 不修改 `crates/router-core/**` 的路由算法、规则匹配、评分、选池、排序及 `RouterConfig`/`Decision` 契约。
- 不安装、升级、启动、修复或卸载第三方 Agent。
- 不读取或写入开发者真实全局配置做测试；使用临时 HOME、fixtures 和 mock endpoint。
- 不接受 renderer 提交的目标路径、patch、配置 bytes 或命令。
- 不让远程兼容目录携带代码、脚本、配置路径、路由字段或未知 Connector ID。

## 2. 分层准入

```text
Descriptor（只读发现）
→ 版本与配置契约证据
→ Adapter 协议能力
→ Connector owned paths
→ 事务/恢复测试
→ 真实 E2E
→ 兼容目录精确放行
```

新增 Agent 首先以 `discovery_only` 进入 Registry。只有以下证据齐全，才可改为 `supported`。

## 3. Descriptor 要求

Descriptor 是纯数据，必须包含稳定 kebab-case `agent_id`、展示信息、可执行候选、无 shell 的版本探测 argv、超时/输出上限、平台安装位置、配置位置、环境覆盖和结构指纹规则。

- macOS、Linux、Windows、WSL 都要有路径 fixture；缺平台事实时保持空，而不是猜路径。
- 环境变量只能来自 Registry allowlist，展开结果必须是绝对路径。
- 相同 canonical path 去重，不同路径形成多安装冲突组；用户必须选择唯一实例。
- 版本探测仅只读运行 `--version` 等固定 argv，不经 shell，不执行 update/install/doctor。

## 4. Adapter 与 Connector

Adapter 负责 HTTP 协议到 Canonical IR 的转换；Connector 只负责目标 Agent 配置投影。复用现有 OpenAI Chat Completions、Responses 或 Anthropic Adapter 时，不因 Agent 名称增加路由特判。

每个 Connector 必须：

1. 使用版本化 ID，例如 `agent-name-v1`；
2. 只声明最小 owned paths；
3. 校验运行态 Adapter 已就绪；
4. 对缺失配置安全生成，对非法/歧义配置 fail closed；
5. 保留未知字段、注释、顺序和非归属值；
6. 支持 connect、disconnect、snapshot restore 三种投影；
7. 自检目标 endpoint、认证来源和协议模式；
8. 不把 Key、配置原文或 patch 暴露给 IPC、日志和 Debug。

配置格式优先使用 round-trip 编辑器。普通 serializer 会破坏 JSON5/YAML/TOML 注释或排版时不得准入。重复键、merge key、多文档 YAML、错误父节点类型等歧义输入一律只读保护。

## 5. owned paths 与配置事务

owned paths 是 Connector 唯一可写范围，同时也是 HMAC 归属检查、断开和恢复的边界。禁止把整个配置文件或宽泛父对象声明为 owned，除非上游官方契约证明没有用户字段且经过专项评审。

所有写入必须进入统一事务：

```text
服务端预览
→ 用户确认
→ 复核安装/版本/目录/文件 revision
→ OS keychain 主密钥 + AES-256-GCM 快照
→ 同目录原子替换
→ 写后解析与 Connector 自检
→ ownership revision 提交
→ 失败时按快照恢复
```

用户改动受管值后，Connector 必须拒绝覆盖并要求重新预览；非归属字段必须逐字节或语义保留。

## 6. 版本矩阵

每个准入版本记录：官方 release/tag、package/CLI SemVer、版本输出样例、配置 schema/路径、协议 endpoint、验证日期和证据链接。

- `verified`：只放真实验证的精确版本或有充分证据的窄范围。
- `inferred`：仅限同 minor 的稳定补丁范围，同时绑定已验证 baseline 和小写 SHA-256 配置指纹；仍需额外确认。
- `blocked`：已知破坏版本，优先级高于 verified/inferred。
- 未命中：始终 `DETECTED_UNKNOWN`，零写入。

Agent 更新时先只读探测。Connector 契约未变可通过更高 sequence 签名目录扩范围；契约变化则新增 Connector 版本，不能让旧 Connector 猜新格式。

## 7. 必测矩阵

### 发现

- PATH、known location、env override、重复 canonical path、多实例；
- 多行/非 UTF-8/超长/超时/非零版本输出；
- macOS/Linux/Windows/WSL 路径与 shim；
- 扫描前后目录树 hash 不变。

### 配置与恢复

- 文件缺失、空文件、合法复杂文件、未知字段、注释、重复键、非法 UTF-8/语法；
- connect 幂等边界、预览后并发修改、写前/写后各阶段故障；
- encrypted snapshot、权限、随机 nonce、篡改、缺钥匙、保留与 pinned；
- disconnect/restore 只改 owned paths，保留后续用户字段；
- 恢复失败单独报告 `repair-required`，不得吞掉主错误。

### 协议与真实 E2E

- 文本、流式、工具调用、认证失败、429/5xx 和客户端错误可见性；
- 使用临时配置根和只监听回环的 mock upstream；
- 记录真实 Agent 版本，不安装或升级 Agent；
- 请求确实经过目标 Adapter/Provider 管线，而不只是配置文件看起来正确。

## 8. CI 与评审门

```bash
cargo fmt --manifest-path apps/desktop/src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
cargo llvm-cov --manifest-path apps/desktop/src-tauri/Cargo.toml \
  --lcov --output-path target/desktop-agent.lcov --fail-under-lines 90
node scripts/check-coverage-thresholds.mjs \
  target/desktop-agent.lcov apps/desktop/src-tauri/src/agent_integration 90
bash scripts/check-router-core-redline.sh "$(git merge-base HEAD main)" HEAD
```

评审必须核对 Descriptor、Connector、owned paths、版本矩阵、平台 fixtures、真实 E2E、依赖审计和 `router-core` 零 diff。远程目录规则不能替代代码评审或 E2E。

## 9. 完成清单

- [ ] 官方配置与版本证据已归档；
- [ ] Descriptor 先以 discovery-only 验证；
- [ ] Adapter 协议能力明确；
- [ ] Connector ID 与 owned paths 最小且稳定；
- [ ] 四平台 fixtures 和负向扫描通过；
- [ ] 配置保真、事务、快照、回滚和并发测试通过；
- [ ] 真实 Agent E2E 在隔离目录通过；
- [ ] 兼容目录仅放行已验证范围；
- [ ] desktop 与 Agent 模块覆盖率均达到 90%；
- [ ] `crates/router-core/**` 与冻结基线一致；
- [ ] 未执行任何 Agent 自动安装或升级。
