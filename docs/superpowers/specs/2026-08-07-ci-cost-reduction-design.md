# CI 成本治理设计（分层触发 + 缓存作用域修复）

## 1. 目标

把 GitHub Actions 的月度支出从约 $300 降到 $70–90，同时**不减少任何测试覆盖**，
并把 PR 的反馈时间从 60–80 分钟压到 20 分钟以内。

本设计只覆盖阶段一。阶段二（是否进一步收窄 `windows-rust` 的测试范围、
`agent-platform-targeted` 的 macOS 分支是否真的需要 macOS runner）依赖本阶段
落地后的实测数据，届时另开设计。

## 2. 已验证基线

以下数据经 GitHub REST API 采集，窗口为 2026-07-09 至 08-05（28 天，178 次运行）。

- 仓库为私有仓，Actions 按分钟计费；Linux 1×、Windows 2×、macOS 10×。
- 单次完整 CI 约 $1.65：Windows 73.4 min／$1.17（71%），macOS 3.8 min／$0.30（18%），
  Linux 21.2 min／$0.17（10%）。
- 28 天内 161 次 CI，约 5.8 次／天，折合每月约 173 次。
- Actions 缓存占用 **10.43 GB**，超出每仓 10 GB 上限，正在持续 LRU 驱逐。
- 缓存条目全部挂在 `refs/pull/NN/merge` 作用域；`refs/heads/main` 与
  `refs/heads/develop` 各 **0 条**。
- `main` 自 2026-07-19 起未再有推送事件的 CI 运行。
- 仓库**未配置分支保护，也没有 ruleset**，因此不存在 required status check。
- 构建产物存储 0.67 GB，成本可忽略，本设计不处理。

### 2.1 根因

**根因 A —— 缓存作用域失效。** 分支模型是 git-flow：`develop` 是日常集成分支，
`main` 只在发正式大版本时进，打 tag 触发发版。但 `ci.yml` 的推送触发器只监听
`main`，而 `main` 已冻结，导致推送事件的 CI 自 7/19 起再未运行过。

GitHub 的缓存读取规则是：一次运行只能读取自身分支、基分支（PR 场景）、
以及默认分支的缓存。`main` 不跑、`develop` 也不跑 → 没有任何可共享的缓存 →
每个 PR 只能自建自用，用完即被下一个 PR 挤掉。这形成恶性循环：
冷启动全量重编译 → 产物更大 → 挤兑更严重。这正是 `windows-rust` 耗时在
13／29／38 分钟间剧烈波动的原因。

**根因 B —— 发布级验证跑在每个 PR 上。** `windows-msi` 需连编两遍 release 版
桌面应用（约 41.6 min／$0.67，占单次成本 40%）；`windows-rust` 跑的
`cargo clippy --workspace --all-targets` 与 `cargo test --workspace`
与 Linux 的 `rust` job 完全重复，只是价格翻倍。实测案例：纯文档 PR
`codex/docs-context-overflow-pr` 触发了一次 90 分钟 Windows 构建的完整 CI。

## 3. 方案选择

考虑过三种 PR 阶段策略：

| 方案 | PR 阶段 | 月度估算 | 结论 |
|---|---|---|---|
| A | 完全不跑 CI | ~$55 | 否决 |
| **B** | **跑 Linux 全量** | **~$70** | **采用** |
| C | Linux 全量 + Windows 冒烟 | ~$80 | 备选 |

否决 A 的理由：数据窗口内 24% 的运行以失败告终，且 PR 多来自 `codex/*`
自动生成分支。没有合并前拦截，这些失败会全部落到 `develop`，省下的钱
会以返工的形式还回去；仓库又没有分支保护，等同裸奔。

采用 B 的核心判断：**省钱的杠杆不是"少跑测试"，而是"把测试挪到便宜的
runner 上"。** Linux 是 1× 倍率，PR 上跑满 7 个 job 仅 $0.17，却能覆盖编译、
clippy、测试、格式、依赖审计、MSRV——绝大多数问题在这一层就能暴露。
真正昂贵的 Windows／macOS 在 PR 上跑的内容 90% 与 Linux 重复。

## 4. 设计

### 4.1 三层触发架构

不拆分工作流文件，在 `ci.yml` 内用 job 级 `if` 分层：单文件更易维护，
且共享同一个 concurrency group。

```text
第一层 · PR 与集成分支都跑（Linux，7 个 job，~$0.17）
  rust · desktop-rust · deny · audit · msrv · frontend · desktop-security
  → 保持无条件运行，不加任何条件

第二层 · 仅集成分支（if: github.event_name != 'pull_request'）
  windows-rust · agent-platform-targeted
  （rust-coverage / desktop-coverage 已有此条件，保持不变）

第三层 · 仅发布验证
  windows-msi
  → 推送到 main、手动触发、以及改动了桌面/安装器路径的 PR
```

第三层选用「推送到 `main`」而非 tag 作为发布侧触发点，理由有二：
其一，`ci.yml` 并不监听 tag，为此新增 tag 触发会与 `release.yml` 的
四平台构建整体重叠；其二，在本仓的 git-flow 下，代码进入 `main` 即意味着
准备发正式版本，这恰是运行安装器生命周期验证的正确时点。
`develop` 的日常合并不触发该层。

### 4.2 第三层的路径判定

新增一个前置 `changes` job，通过 GitHub API 查询 PR 的变更文件。
**不引入第三方 action**——本仓对供应链依赖谨慎（有 cargo-deny 与
RustSec 例外清单），不值得为路径过滤新增一个外部信任点。该做法
不需要 checkout，也不需要 `fetch-depth: 0`，耗时数秒。

监听的路径前缀：

- `apps/desktop/`
- `scripts/build-desktop.sh`
- `scripts/test-windows-msi.ps1`
- `scripts/check-windows-release-config.mjs`

非 `pull_request` 事件一律置为 `true`，交由 `windows-msi` 自身的
事件条件决定是否运行。

### 4.3 缓存作用域修复

三处改动：

```yaml
on:
  push:
    branches: [main, develop]      # develop 加入，让集成分支能建缓存
```

所有 `Swatinem/rust-cache` 步骤统一追加：

```yaml
save-if: ${{ github.ref == 'refs/heads/develop' || github.ref == 'refs/heads/main' }}
```

PR 只读不写，集成分支独家写。默认分支保持 `main` 不变——日常 PR 的基分支
都是 `develop`，按 2.1 所述的缓存读取规则（运行可读取基分支的缓存），
能够读到 `develop` 的缓存，无需改动仓库设置。

concurrency 改为条件式取消：

```yaml
concurrency:
  group: ci-${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
```

现状是无条件 `true`。连续合并两个 PR 时，前一个 `develop` 运行会被取消，
缓存就建不成。改为只取消 PR 的运行。

**容量核算。** `save-if` 生效后仅集成分支写缓存，预计约 8 GB（含两个覆盖率
job），低于 10 GB 上限。但现存的 10.43 GB 旧缓存需**手动清理一次**，否则新缓存
仍会被挤——旧的 PR 作用域缓存要 7 天不访问才自然过期。

### 4.4 附带优化

- 工作流级 `paths-ignore`：`docs/**`、`**.md`、`LICENSE`。
- Windows runner 上排除 Defender 实时扫描，Rust 构建通常快 20–40%。

### 4.5 明确不改动的部分

- `release.yml`、`desktop-release.yml`、`linux-desktop.yml` —— 均由 tag 触发，
  频次低，不是成本主项。
- `router-core-redline.yml` —— 安全门禁，通过 `pull_request_target` 在 PR 上运行，
  覆盖不受本设计影响。其 `push` 触发器同样只监听 `main`，但因 PR 层已覆盖，
  不构成缺口。
- 默认分支设置 —— 保持 `main`。
- 任何测试用例本身 —— 本设计只改变测试**在哪跑**，不改变**跑什么**。

## 5. 风险

**明确接受的风险：** Windows／macOS 特有的回归将延后到合并进 `develop` 后
才被发现，而非在 PR 阶段。

缓解措施有二：其一，第三层的路径判定确保真正碰安装器的 PR 仍会被拦截；
其二，`develop` 上的完整跨平台运行会在合并后数分钟内给出信号，
此时回滚或追加修复的成本远低于发版后。

**未缓解的残余风险：** 分层之后，PR 阶段**不再有任何 job 编译 Windows 目标**——
`#[cfg(windows)]` 代码块在 PR 上完全得不到类型检查，这不是「某些边缘场景
覆盖不到」的程度，而是整条 PR 流水线对 Windows 条件编译零覆盖。`changes`
job 的路径判定只看 `apps/desktop/` 与三个安装器脚本，`crates/private-fs/Cargo.toml`
中的 `[target.'cfg(windows)'.dependencies]` 块不在其内；一个只改动
`crates/private-fs/`（或任何其他 crate 里的 Windows 条件编译代码）的 PR 会
在 Linux 上全绿通过，却可能把一个硬性的 Windows 构建错误直接带进 `develop`，
要等集成分支的完整跨平台运行才会暴露。此事实已向项目负责人说明，负责人
在权衡后选择维持当前设计、接受这一缺口，而非额外增加一个 Windows
`cargo check` 冒烟 job——这是经过确认的取舍，不是遗漏。

## 6. 验收标准

以下均须实测确认，不接受推断：

1. `develop` 上出现 `refs/heads/develop` 作用域的缓存条目。
2. 缓存总量回落至 10 GB 以下。
3. PR 上仅 Linux job 运行，墙钟 < 20 分钟。
4. `develop` 推送上 Windows job 正常运行且通过。
5. 记录 Windows 任务在热缓存下的新耗时基线，供阶段二决策使用。
6. 改动了 `apps/desktop/` 的 PR 上，`windows-msi` 确实被触发。
7. 纯文档 PR 不触发任何 job。

## 7. 阶段二 · 第一项：`agent-platform-targeted` 的冗余

阶段一落地后的第一项后续。这一项**不依赖热缓存耗时数据**——它是覆盖冗余问题，
不是性能问题，因此先于候选二（`windows-rust` 是否需要在 `develop` 上跑全量）处理。

### 7.1 调查结论

`agent-platform-targeted` 是 macOS + Windows 矩阵，跑四个指定测试。经核查，
**四个测试全部与宿主平台无关**：

| 测试 | 平台无关的依据 |
|---|---|
| `discovery_platform_templates_expand_without_touching_the_filesystem` | 平台以 `Platform::Macos/Linux/Windows/Wsl` 枚举作参数传入，fixture 驱动的字符串展开 |
| `discovery_windows_path_enumerates_native_files_and_reports_shell_shims` | 同样构造 `Platform::Windows`，且主动 `replace('\\', "/")` 抹平分隔符差异 |
| `snapshot_validation_rejects_malformed_index_envelope_and_hex` | hex / envelope 校验，纯逻辑 |
| `transaction_snapshot_and_pre_replace_failures_leave_target_untouched` | 用 `FailBeforeReplaceWriter` 注入 `AtomicWriteStage::Permission` **模拟**失败时序，不触碰 `#[cfg(windows)]` 的真实 `apply_windows_owner_dacl` |

四者均为裸 `#[test]`，无 `cfg` 守卫。作为对照，本仓确实会给平台相关测试加守卫——
`#[cfg(unix)]` 守卫的测试有 22 个。

决定性证据：这四个测试在 1× 倍率的 Linux `desktop-rust` job 中**已经全部跑过并通过**
（运行 31163807366 的日志逐条可查）。以 10×／2× 的价格重跑一遍不产生任何额外覆盖。

### 7.2 一个附带发现：它选错了测试

全仓**唯一**带 `#[cfg(windows)]` 守卫、即真正只能在 Windows 上编译运行的测试是
`ownership.rs` 的 `ownership_store_hardens_a_legacy_regular_index_before_reading`。
它**不在** `agent-platform-targeted` 的指定清单里。也就是说这个 job 精心挑选了四个
不需要特定平台的测试，却漏掉了唯一真正需要 Windows 的那个——后者由 `windows-rust`
的全量 `cargo test` 覆盖。

### 7.3 决定

Windows 一侧整条移除：`windows-rust` 在 `develop` 上跑全量 `cargo test`，
已覆盖上述四个测试与那个 `#[cfg(windows)]` 专属测试。

macOS 一侧保留但降级为 `cargo check --all-targets`（新 job `macos-compile-check`）。
保留的理由不是测试覆盖，而是**编译覆盖**：`#[cfg(target_os = "macos")]` 的代码
（如 `verified_workbuddy_bundle`，要调 `/usr/bin/codesign`）需要一个 macOS 宿主
才能被类型检查。这部分代码目前没有任何测试覆盖，删掉整个 job 会让它连编译检查
都失去，直到发版打 tag 才暴露。`--all-targets` 让测试代码一并参与编译。

**明确不做的：** 不为 `#[cfg(target_os = "macos")]` 补测试——那是产品测试范围，
不属于 CI 成本治理。
