# glib 0.18.5 精确安全回补

状态：精确回补、反篡改门、实际依赖核验和定向回归已通过；全仓最终矩阵由主线程统一验收。目的为消除 RUSTSEC-2024-0429 的实际缺陷，不延长已过期风险例外。

## 来源与限定范围

使用 crates.io 官方 glib 0.18.5 完整发布包（121 文件，压缩 267679 字节）。原始归档保存在 `upstream/glib-0.18.5.crate`，SHA256 为 `233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5`，与原 Cargo.lock 校验值一致。

唯一生产代码变化是上游提交 `b5a4071e439bef2b5eea76c3aa25e5ae84839e34` 的两行：`let p` 改为 `let mut p`；FFI 输出参数 `&p` 改为 `&mut p`。保留版本 0.18.5、API、依赖与许可证，不伪装为 0.20。官方 0.18 分支尚未包含该修复，Tauri 的 GTK3 依赖仍使用 0.18。

- 上游补丁：https://github.com/gtk-rs/gtk-rs-core/commit/b5a4071e439bef2b5eea76c3aa25e5ae84839e34
- 公告：https://rustsec.org/advisories/RUSTSEC-2024-0429.html
- 原始发布包：https://static.crates.io/crates/glib/glib-0.18.5.crate

## 验证与审计边界

`vendor/glib-0.18.5` 必须与已校验官方归档逐文件一致，仅允许上述两行差异；拒绝增删文件、符号链接和其他内容变化。完整 desktop Cargo metadata 必须仅有一个 glib 0.18.5，实际清单路径必须为该 vendor，依赖解析图必须真正消费它。

cargo-audit 0.22.2 会跳过 path 包：只改来源即可从告警变绿，不构成修复证据。因此新审计入口先核完整内容与实际依赖，再将锁文件中该 glib 投影回原 registry 身份，执行完整审计；仅在证据通过后允许豁免已回补的 0429，其他漏洞及 unsound 按既有策略失败，unmaintained 仍显示警告。投影仅写临时文件，不改真实 lock。

原例外保留为历史记录，不修改原 owner/approved_by/审批日期或到期日；活跃例外中删除该条。任何单独 audit 的绿都不能代替补丁证据门。

## 验证记录与平台边界

1. 完整性门的真实行为红测为 13 项中 12 项失败，实施后 13 项通过；增加真实 cargo-audit 隔离公告库后，审计壳的 16 项中 2 项失败，实施后 16 项全部通过。负例覆盖丢补丁、额外修改、增删文件、符号链接、归档替换、registry 来源、错误路径、重复依赖、图中未消费，以及新增 glib 漏洞不得被 0429 的限定允许规则一起放行。
2. 独立小型 probe 复用官方三个字符串迭代器测试，并补非空 `next_back`。Rust 1.96 release 优化模式下，原始版本真实发生 SIGSEGV；两行修复后 4 项通过，执行窗口 0.00 秒。脚本将构建与测试分开，测试进程硬上限为 55 秒。独立 fixture 的 `cargo fmt --check` 已通过；不重排或修改官方 vendor 源码。
3. 实际 desktop metadata 已证明唯一 glib 0.18.5 来自指定 vendor，并由 desktop 解析图消费。实际审计投影扫描 746 项依赖后通过，保留 7 项既有 unmaintained 警告；本次不改变该告警政策。
4. 本机 macOS、GLib 2.88.0 已执行优化 FFI 回归；Linux GTK 实际构建和平台回归尚未在本机验证，不能以该结果冒称 Linux 桌面已验证。完整官方源码保留其现有编译警告，不为清除警告扩大回补。

本轮日志保存在执行环境的 `/tmp`，不属于可移植发布工件：

- 反篡改门红／绿：`/tmp/target-architecture-glib-gate-{red,green}.log`。
- 审计策略红／绿：`/tmp/target-architecture-glib-audit-gate-{red,green}.log`。
- 优化 FFI 红／绿：`/tmp/target-architecture-glib-ffi-{red,green}.log`；有界 runner：`/tmp/target-architecture-glib-ffi-runner-green.log`。
- 格式检查：`/tmp/target-architecture-glib-probe-fmt.log`。
- 实际依赖门／审计：`/tmp/target-architecture-glib-actual-metadata-gate.log`、`/tmp/target-architecture-glib-actual-audit-green.log`。

可重跑入口为 `node --test tests/glib-backport.mjs`、`node scripts/check-rustsec-exceptions.mjs`、`node scripts/audit-desktop.mjs` 和 `scripts/run-glib-regression.sh`。审计 CLI 不接受额外忽略参数，单独运行也必须先通过补丁与实际依赖证据门。
