# M3：社区 CLI 共享任务纵切

## 状态与范围

基线为 `develop / cfff584e4379e282ddd7feb90b5f8b8ffb001b60`，实现位于独立
`feature/target-architecture-tasks` 工作树。共享核心驱动的 CLI 与 SQLite 适配、独立审查及任务专项验收已完成；
桌面已按官方流程更新。后续独立修复已关闭安全／格式自动门，完整本地矩阵 33 项通过并再次更新 App；最新证据见[M3 验收收口](2026-09-20-m3-验收阻断收口.md)。界面目视检查受系统权限限制，未验证平台不能宣称通过。本记录不将其他未合并 South 工作树
视为当前已采用，不修改现有文本代理路径或桌面界面。

目标是 CLI 使用与 server 相同的任务组件和通用生命周期编排，至少以正式百炼
`task-adapter-v2` 包完成提交、跨进程观察与结果下载。MiniMax 已识别方言并沿同一组件合同装配，但本轮未取得其社区真实链验收。
共享核心负责状态推进和派发规则，宿主负责 SQLite 原子效果、凭证和媒体文件。
不实现企业余额、虚假预扣、上游取消 API、后台常驻调度或新桌面任务页面。

## 用户入口

命令保留既有 `--config`，任务另使用显式 `--task-config` 文件：

```text
token-station-cli --config token-station.json task --task-config tasks.json submit --provider bailian --request request.json --idempotency-key client-key
token-station-cli --config token-station.json task --task-config tasks.json inspect TASK_ID
token-station-cli --config token-station.json task --task-config tasks.json observe TASK_ID
token-station-cli --config token-station.json task --task-config tasks.json wait TASK_ID --timeout-seconds 30
token-station-cli --config token-station.json task --task-config tasks.json cancel TASK_ID
token-station-cli --config token-station.json task --task-config tasks.json fetch TASK_ID --output video.mp4
```

所有命令成功输出一份 JSON；诊断走 stderr，错误返回非零退出码，秘密不进入输出。
`submit` 返回宿主 ID，不等待视频完成；同幂等键和请求读取原任务，不重新派单。
`inspect` 只读元数据，缺组件和凭证时仍可读取。`observe` 执行一次观察；`wait`
只在指定有界窗口推进，超时不会把任务伪造成失败或取消。`cancel` 记录取消意图，
只有确定尚未发送时才允许停止派发；已发送时不宣称供应商已停止计算，仍可观察。
`fetch` 只交付确认成功的产物，通过原组件取得 URL 后由宿主限额下载，临时文件
完成后再原子发布，不能覆盖任意既有输出文件。状态由宿主对供应商事实投影；共享核心只约定推进与失权后读取规则，不定义数据库或 JSON 格式。

这是终端界面，不新增响应式布局；六个命令均提供可键盘使用的 `--help`，不要求
颜色识别或交互弹窗，stdin/路径读入请求与凭证，不把秘密放进进程参数。

## 配置与持久边界

任务配置独立于现有 `ClientConfig.upstreams`，避免 task-only 方言触发现有
provider-adapter 的启动校验。字段为 `state_dir`、`components_dir` 和
`providers` 具名表；每项明确 `endpoint`、`model`、`dialect`、完整 `pin` 与
`credential: {upstream, slot}`。不得用最新版本、目录顺序或同名包自动挑选。

`state_dir/tasks.sqlite` 不受可关闭的 metrics 开关控制；记录脱敏任务事实、请求
哈希、执行身份、原 endpoint/model/locator、原凭证引用和并发版本，不保存 prompt、
图片正文或凭证明文。原始请求只在当次提交内存中存在，受理不确定不自动重放。
同一 SQLite 事务实现共享核心要求的 prepare 与 apply 复合效果；无企业资金效果
显式报告 `billing_effect: not_applicable`，不能用金额 0 冒充已确认免费，也不能
把请求估算当供应商实际计量。公共 outbox/事件规则按共享核心合同实现，不复制
server 的余额表或引入 server crate。

初始凭证来自既有 `secrets::store_get(upstream, slot)` 的私有 store；现有 `key set`
继续从 stdin 输入。不为任务新增另一份明文 secret 文件。宿主在私有目录生成随机
HMAC key，任务保存凭证材料的 HMAC 和原槽引用，恢复前重新解析并验证；同槽换材料、
删除槽或 HMAC key 丢失时拒绝执行恢复，不能换用当前其他凭证。HMAC key 创建需
原子且并发安全。env/file 来源的完整恢复合同不在首纵切中悄悄加入。

组件清单能力、真实摘要与授权必须检查；Wasm 不获得网络、文件系统和凭证明文。
所有 descriptor 都做目的地与 SecretRef 授权；签名媒体下载不携带供应商认证头。
已持久化任务固定原组件完整 pin；缺包、不兼容 runtime 或未知 schema 拒绝恢复，
纯元数据读取仍可用。部署不能假定旧版本宿主理解新任务格式。

## 依赖与兼容

接入前社区 MSRV 为 1.95，South 0.31 要求 1.96；本轮已同步到 1.96。本纵切明确使用 `cargo +1.96.0` 验证，
正式落依赖时同步 workspace 与相关 package 的 MSRV、README/构建门，不能只在本机
偷用较新 stable。新工作树使用独立 `CARGO_TARGET_DIR`，不改原树缓存。

社区本地 protocol 与 South 的 kernel Git protocol 同名同版本但来源不同，任务
适配模块显式使用对应 kernel 类型；不把两者当同一种 Rust 类型、不全仓改文本 IR。
现有 Wasmtime 46 与 South 48 的共同构建、旧插件兼容由实际构建证明；不可仅凭
manifest 宣称通过。共享 task-core 是实际调用依赖，不是只增加 Cargo 条目。
正式 South 版本与提交由主线程统一固定，临时 path 覆盖不得冒充发行采用。

## 验证与交付

1. 先在旧 CLI 上运行真实进程测试，证明缺少任务入口，失败必须是行为断言而非编译错误。
2. 用正式百炼 Wasm、真实 loopback HTTP 和 SQLite，从 CLI submit 产生任务，再启动
   新 CLI 进程 inspect/observe/fetch；测试不手工种原任务，不依赖桌面或真实用户数据。
3. 验同键重放零额外 HTTP、受理回包丢失不重发、缺包/错包拒绝、槽材料改变拒绝、
   未知状态保留、取消意图与派发互斥、并发 CAS、事务故障回滚、媒体无凭证与大小上限。
4. server 与社区分别实现公共原子效果履约套件；社区明确无资金效果，仍必须原子提交
   任务事实。不能用参考宿主或只调用纯组件方法替代真实采用。
5. 单个进程事故场景总窗口不超过 55 秒；冷编译单独记录。完整 Rust、旧文本插件、
   桌面编译/测试按仓级规则执行，现有行为不可因加入任务配置被破坏。
6. 桌面 UI 不变，但行为代码仍须最终通过 `scripts/install-local-desktop.sh` 的真实
   App 验收。主线程统筹该阶段；本子任务不自行重装、提交、发布，不提前标交付完成。

## 本轮实现与边界

- `south-task-core` 与 `south-task-conformance` 为独立 `0.1.0` 包，固定 South Git
  `b29d6810cc66e64b9a399f9d76e228093749b260`；任务组件和运行时仍使用正式
  `v0.31.0 / 8077743c213602e375c0ee17d08c051abb0bcd2c`，不重解释旧组件身份。
- `apps/cli/src/tasks/workflow.rs` 真实调用共享 submit / observe / wait / cancel；
  `store.rs` 负责本地 SQLite 效果，`executor.rs` 负责完整 pin、能力、授权、异步 HTTP。
- 网络请求使用实际异步 reqwest Future，10 秒单请求上限、无重定向、4 MiB 响应上限；
  共享 wait 到期会丢弃在飞请求。Wasm 包校验和编译在 wait 时钟起点前完成，并在该次
  CLI 进程内复用；不创建脱离等待者的 HTTP 后台任务。
- 包目录、数据库和 guard 文件拒绝符号链接；缺原包、换槽材料、缺原 guard 都不使用
  当前替代来源。缺 guard 时纯 inspect / 同键重放仍可用，observe 保留未知事实。
- 获取观察栅栏的 UPDATE 与返回行读取处于同一 Immediate 事务。非终态观察 CAS
  失败时清除本次临时产物；已成功任务的刷新是独立只读分支，不重复终态事件。
- 当前只从既有私有 SecretStore 槽取原凭证；任务 HTTP 不承诺沿用文本网关的代理、
  自定义 CA、路由池或健康调度政策。若后续纳入这些策略，必须显式扩展任务配置与测试。
- 无上游取消能力时，accepted/submitting 的 cancel 仅记录意图。CLI 退出、等待超时、
  提交回包丢失均不重发未知任务；prepared 被取消后原子阻止派发。

定向证据：旧 CLI 六个帮助入口及真实包生命周期均先行为红；修后正式包真实 CLI
14/14 通过（含缺包、换材料、未知不重发、取消、慢 HTTP 截止、HTTP barrier CAS）；
真实 SQLite 公共 11 场景和额外双连接栅栏交错通过。完整检查和安装结果见下节；
定向通过本身不等于发布就绪或桌面 UI 已目视验收。



任务专腿不得静默跳过缺包，执行方式为：

```bash
TOKEN_STATION_TASK_BAILIAN_PACKAGE_DIR=/绝对路径/task-bailian-v2 \
  bash scripts/test-task-cli.sh
```

脚本运行全部真实 CLI 案例以及正式共享契约下的 SQLite 履约测试；普通 workspace
测试仍执行无包的 CLI 入口与 SQLite 单测，需正式包的案例只由这条明确的专腿点名。


## 最终验收记录

工作树固定在 `feature/target-architecture-tasks`，所有临时数据、loopback HTTP 和构建
缓存都在独立目录，不修改原社区分支。生产源码在最后两项并发修复后保持冻结；此后
只增强 CLI 失败时的脱敏诊断，并为已固定的 South/kernel 来源增加两份 deny 精准
白名单。来源策略仍拒绝其他 Git 仓和 registry，不新增漏洞忽略。

- 正式来源：`/tmp/target-architecture-community-metadata.json` 已核六个实际 South
  包各自仅一种来源；四个组件/运行时包固定 `.31`，两个共享任务包固定 `b29d6810`。
  两个同名 protocol 来源有意分开：旧文本使用本地 IR，任务仅使用 South 的 kernel
  来源；未把它们冒充同一类型。
- 独立复核发现的 claim/read 事务间隙与 CAS 败者产物交付问题均先真实红，再最小修复。
  证据分别为 `community-task-fence-red.log`、`community-task-cas-delivery-red.log`，
  以及修后真实双连接/HTTP barrier 测试。日志统一位于 `/tmp/target-architecture-` 前缀。
- 任务等待从同步 HTTP 改成可取消异步请求，先证明 1 秒预算误等完 4 秒 HTTP，后验证
  到期保留原任务；缺 credential guard 不重造并保持历史读取。早期编译错误不计行为红。
- 新代码格式通过；仓级格式门在基线即因 `apps/cli/src/config.rs`、`gateway.rs`、
  `tls_trust.rs` 与 `apps/cli/tests/tls_trust.rs` 失败。已从 `cfff584` 提取原文到独立
  临时目录，以同一 rustfmt 复现，未修改无关文件。
- Node 22.23.1 下完整前端 36 文件 / 414 测试通过，行覆盖 86.02%，构建通过。
  系统 Node 26 首跑与并发较高时两条 UI 失败均保留；基线 App 71/71 与当前树 App
  71/71 后，再完成低并发全量，不把先前失败冒充已证明的基线缺陷。
- 旧插件全仓首跑受错误继承 `CARGO_TARGET_DIR` 影响，内嵌 cargo 产物与测试硬编码读取
  路径不同。保留该失败轮；随后以命令参数指定独立 target、取消环境继承并预热 guest，
  正式 workspace nextest 609/609 通过。9 个 ignored 中 8 个任务正式包案例由专腿另跑；另 1 个是由既有父测试主动拉起的代理环境子进程，不是遗漏功能测试。
- 官方安装由主线程执行：Rust 1.97 构建、签名、产物审计、内嵌插件自检和启动通过，
  已存在 1040×720 的真实窗口。窗口截图与系统事件目视检查被 macOS 权限拒绝，未调整
  权限，不声称完成截图或目视 UI 验收。宿主 MSRV 和任务门另用 Rust 1.96 验证。
- 完整安全门在当前树及 `cfff584` 纯归档基线均报告 rustls 0.23.41 的
  RUSTSEC-2026-0285、旧 Wasmtime 46.0.2 的 RUSTSEC-2026-0268/0269，以及 chacha20
  0.10.1 被撤回。来源、许可与版本重复策略通过；不将安全门写成通过，未搭车升级或
  加忽略。最终处置由主线程单独明确。

普通 CLI 生命周期在一次专腿中出现过 13/14、observe 返回未知。增强测试诊断后
12 次独立诊断均通过，但原轮没有保存足够请求证据，根因仍未证实；不能把重跑称作
修复。最终专腿和全矩阵单独记录，未知状态本身仍保持不重发、不交付的边界。

最终统一命令、逐腿退出码、耗时、日志与前后源码摘要见
`/tmp/target-architecture-community-final-result.json`。最终 17 腿：12 通过、5 失败；
失败为根/桌面完整 deny、audit、audit -D unsound、RustSec 例外期限门，并非五个
独立新代码缺陷。除上列四项既有依赖阻断外，`RUSTSEC-2024-0429` 的既有例外已于
2026-09-18 过期，亦在 `cfff584` 纯归档实际复现，未延长例外。

最终实际结果：

| 检查 | 结果 |
| --- | --- |
| Rust 1.96 全 workspace clippy / all-targets / -D warnings | 通过 |
| nextest 0.9.145，全 workspace，单例 55 秒失败界限 | 609/609，200.760 秒；9 个点名隔离案例按上述方式覆盖 |
| 正式百炼包 CLI 专腿 | 14/14，11.03 秒；随后真实 SQLite 两测试通过，含公共 11 场景与额外双连接栅栏 |
| workspace doctest / build / rustdoc -D warnings | 通过 |
| 根/桌面依赖 sources 精准白名单门 | 通过；完整 deny 仍因上列既有安全项失败 |
| CI 触发、scaffold 预取、DMG 打包规则、release-readiness | 四项通过，触发器未改变 |
| 根/桌面 metadata | 均为可解析 JSON、固定正式来源；分别六/五个唯一 South 包 |
| Node 22.23.1 前端 coverage / build | 414/414、行覆盖 86.02%；构建通过 |
| 全仓格式 | 基线四文件差异，已复现；新代码与 diff-check 通过 |
| 完整安全门及例外期限门 | 明确失败，保留基线复现；不能宣称全门全绿或发布就绪 |

最终前后均为 611 项非 Markdown 源码，摘要
`bd14e4336274495de8e0ad040b0d4e5d41da180a5557ab9bc62993a27bafde63`，
`source_unchanged=true`。运行时、依赖锁与测试源在这一轮没有被中途修改。
桌面独立 Rust 1.96 clippy 全 targets、严格 doc、doctest 通过；最终 nextest
**390/390**，2 个既有 ignored（本机 Agent smoke／显式 stress），40.697 秒，最长
单例 16.502 秒，零 FAIL／TIMEOUT／LEAK。使用 `--no-fail-fast --test-threads 2`，
每例 55 秒及 200ms 泄漏失败规则未改变。首轮并行验证的 1 失败／8 超时保留于
`/tmp/target-architecture-community-desktop-tests.log`，最终日志为同名前缀 `-v2.log`；
低负载复验通过不等于已定位首轮所有超时根因。桌面格式另有旧 `lib.rs` 一处差异，
已从 `cfff584e` 原文以相同 rustfmt 复现。Rust 覆盖率及 Windows/Linux 平台作业不属于本轮
已取得的本机执行证据，不能用本地 App 构建替代。

本轮完成的是社区任务域的真实共享采用，不是整个文本栈迁移，也不把 T08 全部完成。
T08 尚含文本采用与未知族试片，留给后续 M4。本轮只形成隔离分支本地提交，不推送或发布；完整安全门修复不搭车
混入此次任务采用。

提交前仅更正 `apps/cli/Cargo.toml` 一行过时的“候选依赖”注释，所有非注释内容
逐行相同；未修改运行时代码、依赖选择或锁文件。验证轮摘要与交付摘要分开保留于
`/tmp/target-architecture-community-comment-only-finalization.json`，不将注释更正
前后的字节摘要冒充相同。


## 后续收口说明

上列安全／格式失败和旧二进制摘要保留为 `9e699fe` 采用轮的历史记录。后续的兼容安全补丁、glib 官方两行回补、完整检查与新 App 证据以[M3 验收收口](2026-09-20-m3-验收阻断收口.md)为准，不再把这些已修复的代码问题列为当前阻断。
