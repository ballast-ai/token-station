# CI 成本治理实施计划（分层触发 + 缓存作用域修复）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 GitHub Actions 月度支出从约 $300 降到 $70–90，不减少任何测试覆盖，
并把 PR 反馈时间从 60–80 分钟压到 20 分钟以内。

**Architecture:** 只改 `.github/workflows/ci.yml` 一个文件。两条独立的修复线：
其一，把推送触发器扩到 `develop` 并给所有缓存步骤加 `save-if`，让集成分支
独家生成可被 PR 继承的热缓存；其二，用 job 级 `if` 把 13 个 job 分成三层——
PR 只跑 1× 倍率的 Linux job，跨平台验证移到 `develop`，安装器生命周期验证
移到 `main`。

**Tech Stack:** GitHub Actions YAML、`Swatinem/rust-cache@v2`、`gh` CLI、actionlint。

**设计文档:** `docs/superpowers/specs/2026-08-07-ci-cost-reduction-design.md`

## Global Constraints

- 只修改 `.github/workflows/ci.yml`。**不得改动** `release.yml`、`desktop-release.yml`、
  `linux-desktop.yml`、`router-core-redline.yml`。
- **不得改动任何测试用例本身。** 本计划只改变测试在哪跑，不改变跑什么。
- **不得改动仓库的默认分支设置**（保持 `main`）。
- **不得引入任何第三方 GitHub Action。** 本仓对供应链依赖谨慎（有 cargo-deny
  与 RustSec 例外清单）。路径判定用 `gh` CLI + shell 实现。
- 集成分支的判定表达式在全文件中必须逐字一致：
  `github.ref == 'refs/heads/develop' || github.ref == 'refs/heads/main'`
- 缓存总量上限 10 GB（GitHub 每仓硬限制）。
- 分支：`ci/cost-reduction-tiered-triggers`，基于 `origin/develop`。

## 关于本计划的验证方式（重要）

GitHub Actions 的行为**无法在本地完整测试**。本地能做的只有 YAML 语法与
表达式的静态校验（actionlint）。真正的行为验收必须在 GitHub 上通过真实运行
观察。因此本计划的每个任务是「静态校验 + 提交」，而全部行为验收集中在
Task 5，通过一个真实 PR 与一次真实的 `develop` 推送完成。

不要在 Task 5 之前声称任何行为已被验证。

---

### Task 1: 安装 actionlint 并建立基线

**Files:**
- 无文件改动（仅环境准备与基线记录）

**Interfaces:**
- Produces: 可用的 `actionlint` 命令；`ci.yml` 当前状态的校验基线。

- [ ] **Step 1: 安装 actionlint**

```bash
brew install actionlint
```

- [ ] **Step 2: 对未修改的 ci.yml 跑一次，记录基线**

```bash
cd /Users/juyichen/myCodePlace/GlimpseEngine/token-station
actionlint .github/workflows/ci.yml
```

期望：无输出（exit 0）。如果**当前**就有告警，先把告警原文记下来——
后续步骤只需保证不新增告警，不负责修既有问题。

- [ ] **Step 3: 确认分支正确**

```bash
git branch --show-current
```

期望输出：`ci/cost-reduction-tiered-triggers`

---

### Task 2: 缓存作用域修复

修复根因 A。这是独立可验证的一步：即使 Task 3 的分层不做，这一步单独
落地也能拿回约 45% 的支出。

**Files:**
- Modify: `.github/workflows/ci.yml`（`on` 块、`concurrency` 块、8 处 `rust-cache`）

**Interfaces:**
- Produces: `develop` 推送会触发 CI 并生成 `refs/heads/develop` 作用域的缓存。

- [ ] **Step 1: 扩展推送触发器**

把文件开头的 `on` 块从：

```yaml
on:
  pull_request:
  push:
    branches:
      - main
  workflow_dispatch:
```

改为：

```yaml
on:
  pull_request:
  push:
    branches:
      - main
      - develop
  workflow_dispatch:
```

- [ ] **Step 2: 改为条件式取消**

把 `concurrency` 块的 `cancel-in-progress: true` 改为：

```yaml
concurrency:
  group: ci-${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
```

理由：现状是无条件取消。连续合并两个 PR 时前一个 `develop` 运行会被取消，
缓存就建不成。集成分支的运行必须跑完。

- [ ] **Step 3: 给全部 8 处 rust-cache 加 save-if**

`ci.yml` 中共有 **8 处** `uses: Swatinem/rust-cache@v2`，分属这些 job：
`rust`、`desktop-rust`、`msrv`、`rust-coverage`、`desktop-coverage`、
`agent-platform-targeted`、`windows-rust`、`windows-msi`。

先确认数量：

```bash
grep -c 'Swatinem/rust-cache@v2' .github/workflows/ci.yml
```

期望输出：`8`

每一处的 `with:` 块内追加一行（与 `cache-on-failure` 同级缩进）：

```yaml
          save-if: ${{ github.ref == 'refs/heads/develop' || github.ref == 'refs/heads/main' }}
```

注意 `msrv` 那一处的 `with:` 块结构与其他不同（它用 `key: msrv-1.95`
而非 `workspaces:`），但追加位置相同——都在 `with:` 块内。

- [ ] **Step 4: 校验数量与语法**

```bash
grep -c 'save-if:' .github/workflows/ci.yml
```

期望输出：`8`

```bash
actionlint .github/workflows/ci.yml
```

期望：无新增告警。

- [ ] **Step 5: 人工复核 diff**

```bash
git diff .github/workflows/ci.yml
```

逐处确认：8 个 `save-if` 全部落在 `with:` 块内且缩进正确；
表达式逐字一致；没有误改 `workspaces` 列表里的任何路径。

- [ ] **Step 6: 提交**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci: 修复缓存作用域——develop 独家写缓存，PR 只读

分支模型是 git-flow，develop 才是集成分支，但推送触发器只监听已冻结的
main，导致自 7/19 起再没有推送事件的 CI 运行过。缓存只能由 PR 自建自用，
10.43GB 全部挂在 refs/pull/NN/merge 作用域下，互相挤兑到超出 10GB 上限，
每个新 PR 都冷启动全量重编译。

推送触发器扩到 develop 让集成分支能建缓存；8 处 rust-cache 加 save-if
限定只有集成分支写入，PR 只读不写；concurrency 改为只取消 PR 的运行，
否则连续合并时前一个 develop 运行会被取消、缓存建不成。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: 分层触发

修复根因 B。依赖 Task 2 已完成（同一文件，避免冲突）。

**Files:**
- Modify: `.github/workflows/ci.yml`（新增 `changes` job；4 个 job 加 `if`）

**Interfaces:**
- Consumes: Task 2 建立的 `on.push.branches` 含 `develop`。
- Produces: job `changes` 输出 `installer`（字符串 `"true"` / `"false"`），
  供 `windows-msi` 的 `if` 条件消费。

- [ ] **Step 1: 新增 changes job**

在 `jobs:` 下、第一个 job（`rust`）**之前**插入。它是纯 API 查询，
不做 checkout，耗时数秒：

```yaml
  # 判定本次变更是否触及桌面端或安装器。用 gh CLI 查 PR 变更文件，
  # 不 checkout、不引入第三方 action——本仓对供应链依赖谨慎。
  # 非 pull_request 事件一律置 true，由 windows-msi 自身的事件条件决定是否跑。
  changes:
    runs-on: ubuntu-latest
    outputs:
      installer: ${{ steps.filter.outputs.installer }}
    steps:
      - id: filter
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          if [ "${{ github.event_name }}" != "pull_request" ]; then
            echo "installer=true" >> "$GITHUB_OUTPUT"
            exit 0
          fi
          gh api "repos/${{ github.repository }}/pulls/${{ github.event.pull_request.number }}/files" \
            --paginate --jq '.[].filename' > changed-files.txt
          if grep -qE '^(apps/desktop/|scripts/(build-desktop\.sh|test-windows-msi\.ps1|check-windows-release-config\.mjs)$)' changed-files.txt; then
            echo "installer=true" >> "$GITHUB_OUTPUT"
          else
            echo "installer=false" >> "$GITHUB_OUTPUT"
          fi
```

- [ ] **Step 2: 第二层——windows-rust 与 agent-platform-targeted 限定集成分支**

在 `windows-rust` 的 `name:` 之后、`runs-on:` 之前插入：

```yaml
    # PR 阶段由 Linux 的 rust job 覆盖同一套 clippy/test；Windows 是 2x 倍率，
    # 全量重跑一遍不值。合并进 develop 后再跑完整的跨平台门禁。
    if: github.event_name != 'pull_request'
```

在 `agent-platform-targeted` 的 `name:` 之后、`strategy:` 之前插入：

```yaml
    # macOS 是 10x 倍率，Windows 是 2x。平台特有回归延后到集成分支暴露。
    if: github.event_name != 'pull_request'
```

- [ ] **Step 3: 第三层——windows-msi 限定发布路径**

把 `windows-msi` 的 `needs: windows-rust` 改为同时依赖 `changes`，
并加上 `if` 条件：

```yaml
  windows-msi:
    name: Windows x64 MSI real lifecycle
    needs: [changes, windows-rust]
    # 安装器生命周期验证：连编两遍 release 桌面应用，约 41 分钟 x 2 倍率。
    # 进 main 即意味着准备发正式版，是跑它的正确时点；develop 的日常合并不跑。
    # 改动了桌面/安装器路径的 PR 仍会被拦截。
    if: >-
      !cancelled() &&
      needs.changes.outputs.installer == 'true' &&
      (github.ref == 'refs/heads/main' ||
       github.event_name == 'workflow_dispatch' ||
       github.event_name == 'pull_request')
    runs-on: windows-latest
```

**关于 `!cancelled()`——必须是它，不能是 `always()`：** `windows-msi` 的 `needs`
含 `windows-rust`，而后者在 PR 上被跳过（skipped）。GitHub 默认会因上游
skipped 而连带跳过下游，所以需要一个条件函数解除该联动。

这里**不能用 `always()`**：Task 2 保留了 PR 的 `cancel-in-progress: true`，
而 `always()` 的语义是「连 workflow 被取消时也照跑」。两者相撞的后果是——
PR 被新 push 取消时，这个 41 分钟、2× 倍率的 Windows job 仍会启动，
每次白烧约 $1.3。`!cancelled()` 保留了「解除 skipped 联动」的效果，
同时在取消时正确退出。

注意 `!cancelled()` 仍会让 `windows-rust` **失败**时 `windows-msi` 继续运行——
这是刻意保留的：在 `main` 推送场景下，即便全量 Windows 门禁挂了，
仍然拿到安装器的独立信号。

- [ ] **Step 4: 校验语法**

```bash
actionlint .github/workflows/ci.yml
```

期望：无新增告警。actionlint 会检查 `needs.changes.outputs.installer`
所引用的 job 与 output 是否真实存在——这是本步最有价值的静态检查。

- [ ] **Step 5: 确认第一层未被误伤**

```bash
grep -nE "^  (rust|deny|audit|desktop-rust|msrv|frontend|desktop-security):" \
  .github/workflows/ci.yml
```

对上面列出的 7 个 job，逐个确认其块内**没有** `if:` 行——它们必须在 PR 上
无条件运行。可用如下方式逐个查看：

```bash
awk '/^  (rust|deny|audit|desktop-rust|msrv|frontend|desktop-security):/,/^  [a-z-]+:$/' \
  .github/workflows/ci.yml | grep -n "if:" || echo "第一层无 if 条件，正确"
```

- [ ] **Step 6: 提交**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci: 按 runner 价格分三层触发

Windows/macOS 占 90% 的账单却只占 78% 的构建时长——问题不在跑得多，
在跑在哪。Linux 是 1x 倍率，PR 上跑满 7 个 job 仅 $0.17，已覆盖编译、
clippy、测试、格式、依赖审计与 MSRV。

PR 保留 Linux 全量；windows-rust 与 agent-platform-targeted 移到集成分支；
windows-msi 移到 main 推送（进 main 即准备发版，是跑安装器生命周期验证的
正确时点），改动桌面/安装器路径的 PR 仍会触发它。

新增的 changes job 用 gh CLI 查 PR 变更文件做路径判定，不 checkout、
不引入第三方 action。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: 附带优化

**Files:**
- Modify: `.github/workflows/ci.yml`（`on` 块加 `paths-ignore`；3 个 Windows job 加 Defender 排除步骤）

**Interfaces:**
- Consumes: Task 2 的 `on` 块结构。

- [ ] **Step 1: 加 paths-ignore**

把 `on` 块改为（在 `pull_request` 与 `push` 下各加一份——
GitHub 不支持在 `on` 顶层写一次共用）：

```yaml
on:
  pull_request:
    paths-ignore:
      - 'docs/**'
      - '**.md'
      - 'LICENSE'
  push:
    branches:
      - main
      - develop
    paths-ignore:
      - 'docs/**'
      - '**.md'
      - 'LICENSE'
  workflow_dispatch:
```

**安全性说明：** 本仓未配置分支保护，也没有 ruleset，因此不存在
required status check——被 `paths-ignore` 跳过的运行不会留下永久 pending
的检查卡住合并。这一点已在设计阶段核实。若将来要加分支保护，必须
在那之前重新评估此项。

- [ ] **Step 2: Windows runner 排除 Defender 实时扫描**

在这 3 个 job 的 `steps:` 中、`actions/checkout` **之后**、
`dtolnay/rust-toolchain` **之前**插入：`windows-rust`、`windows-msi`、
以及 `agent-platform-targeted`（后者是 macOS/Windows 矩阵，需加 `if` 守卫）。

`windows-rust` 与 `windows-msi` 用：

```yaml
      # Defender 实时扫描会逐个扫 Rust 增量编译产生的大量小文件，
      # 排除工作区后构建通常快 20-40%。
      - name: Exclude the workspace from Defender real-time scanning
        shell: pwsh
        run: Add-MpPreference -ExclusionPath "${{ github.workspace }}"
```

`agent-platform-targeted` 是双平台矩阵，用：

```yaml
      - if: runner.os == 'Windows'
        name: Exclude the workspace from Defender real-time scanning
        shell: pwsh
        run: Add-MpPreference -ExclusionPath "${{ github.workspace }}"
```

- [ ] **Step 3: 校验**

```bash
actionlint .github/workflows/ci.yml
grep -c 'Add-MpPreference' .github/workflows/ci.yml
```

期望：actionlint 无新增告警；`grep -c` 输出 `3`。

- [ ] **Step 4: 提交**

```bash
git add .github/workflows/ci.yml
git commit -m "$(cat <<'EOF'
ci: 跳过纯文档改动，Windows 上排除 Defender 扫描

纯文档 PR 此前会触发完整 CI——实测 codex/docs-context-overflow-pr 烧掉了
一次 90 分钟的 Windows 构建。仓库无分支保护与 ruleset，不存在 required
status check，被 paths-ignore 跳过不会留下 pending 检查卡住合并。

Defender 实时扫描会逐个扫 Rust 增量编译的大量小文件,排除工作区后
Windows 构建通常快 20-40%。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: 真实行为验收

**这是唯一能验证行为的任务。** 前四个任务只做了静态校验。

**Files:**
- 无文件改动（仅观察与记录）

**Interfaces:**
- Consumes: Task 2–4 的全部改动。

- [ ] **Step 1: 推分支并开 PR**

```bash
git push -u origin ci/cost-reduction-tiered-triggers
gh pr create --base develop \
  --title "ci: 分层触发与缓存作用域修复" \
  --body "见 docs/superpowers/specs/2026-08-07-ci-cost-reduction-design.md"
```

- [ ] **Step 2: 验收标准 3——PR 上只跑 Linux**

等待运行完成后：

```bash
run=$(gh run list --branch ci/cost-reduction-tiered-triggers --limit 1 --json databaseId --jq '.[0].databaseId')
gh run view "$run" --json jobs \
  --jq '.jobs[] | "\(.conclusion // .status)\t\(.name)"' | sort
```

期望：只有 `changes` 与 7 个 Linux job 出现为已完成；
`windows-rust`、`agent-platform-targeted`、`windows-msi` 应为 skipped 或不出现。
**不应有任何 windows-latest / macos-latest 上的计费运行。**

只看 job 是否「出现过」/「completed」不够——`changes` job 可能因权限不足
（例如缺少 `pull-requests: read`）而对 GitHub API 调用 403，此时该 job
本身仍会以 `completed` 状态收场，只是 `conclusion` 是 `failure`，此前的
检查方式发现不了这种情况。必须显式断言其 conclusion 为 `success`：

```bash
gh run view "$run" --json jobs \
  --jq '.jobs[] | select(.name == "changes") | .conclusion'
```

期望：`success`。若不是，先排查 `changes` job 的日志，不要继续后续步骤。

至于 `installer` 解析成了什么——**不要试图从日志里 grep 它**。
`gh run view --json jobs` 不暴露 job 的 `outputs`，而 Actions 会把 `run:`
脚本原文回显进日志，于是 `echo "installer=true"` 与 `echo "installer=false"`
两行字面量在**任何一次**运行的日志里都同时存在，grep 只会两条都命中，
分辨不出实际取值。

可靠的观测方式是拿 `windows-msi` 的 job 状态当代理——它跑不跑完全由
`installer` 决定，无需改动 workflow 即可反推：

```bash
gh run view "$run" --json jobs \
  --jq '.jobs[] | select(.name | test("MSI")) | "\(.name)\t\(.conclusion // .status)"'
```

期望：本 PR 未触及 `apps/desktop/` 与三个安装器脚本，`windows-msi` 应为
`skipped`（等价于 `installer=false`）。Step 3 会用一个触碰桌面路径的提交
反向验证 `installer=true` 的分支。两步合起来，`installer` 的两种取值
都得到了真实验证。

记录墙钟时长，验收标准 3 要求 < 20 分钟：

```bash
gh run view "$run" --json createdAt,updatedAt \
  --jq '((.updatedAt|fromdateiso8601) - (.createdAt|fromdateiso8601))/60 | round'
```

- [ ] **Step 3: 验收标准 6——反向验证路径判定**

**这一步会真实触发 `windows-msi`（约 41 分钟、2× 倍率，一次性成本约 $1.3）。**
这是预期行为，不是故障。确认它「被触发」即可，不必等它跑完——
观察到状态为 `in_progress` 就足以验收，随后可直接进入回滚。

在同一 PR 上追加一个触碰桌面路径的空改动，确认 `windows-msi` 被触发：

```bash
printf '\n' >> apps/desktop/package.json
git add apps/desktop/package.json
git commit -m "chore: 临时触碰桌面路径以验证 windows-msi 触发条件"
git push
```

等待运行后确认 `changes` 的输出与 `windows-msi` 的状态：

```bash
run=$(gh run list --branch ci/cost-reduction-tiered-triggers --limit 1 --json databaseId --jq '.[0].databaseId')
gh run view "$run" --json jobs --jq '.jobs[] | select(.name|test("msi|changes")) | "\(.conclusion // .status)\t\(.name)"'
```

期望：`windows-msi` 处于运行中或已完成（**不是** skipped）。

确认后**必须回滚这个临时提交**：

```bash
git reset --hard HEAD~1
git push --force-with-lease
```

- [ ] **Step 4: 验收标准 7——纯文档 PR 不触发**

```bash
git checkout -b ci/verify-docs-paths-ignore
printf '\n' >> docs/superpowers/specs/2026-08-07-ci-cost-reduction-design.md
git commit -am "chore: 验证 paths-ignore"
git push -u origin ci/verify-docs-paths-ignore
gh pr create --base develop --title "chore: 验证 paths-ignore（用完即关）" --body "临时验证分支"
```

等待约 1 分钟后确认**没有任何 CI 运行被触发**：

```bash
gh run list --branch ci/verify-docs-paths-ignore --limit 5
```

期望：无 CI 运行。确认后关闭 PR 并删除分支：

```bash
gh pr close ci/verify-docs-paths-ignore --delete-branch
git checkout ci/cost-reduction-tiered-triggers
```

- [ ] **Step 5: 合入 develop**

前述验收全部通过后：

```bash
gh pr merge --squash --delete-branch
```

- [ ] **Step 6: 清理旧缓存**

合入后立即清理。10.43 GB 旧缓存全部是 `refs/pull/NN/merge` 作用域，
不清理会继续挤兑新缓存（要 7 天不访问才自然过期）。

先看清楚要删什么：

```bash
gh api "/repos/GlimpseEngine/token-station/actions/caches?per_page=100" \
  --jq '.actions_caches[] | "\(.id)\t\((.size_in_bytes/1073741824*100|round)/100)GB\t\(.ref)\t\(.key)"'
```

确认列表符合预期后删除全部：

```bash
gh api "/repos/GlimpseEngine/token-station/actions/caches?per_page=100" \
  --jq '.actions_caches[].id' | while read -r id; do
    gh api -X DELETE "/repos/GlimpseEngine/token-station/actions/caches/$id" && echo "deleted $id"
  done
```

- [ ] **Step 7: 验收标准 1、2、4——观察 develop 运行**

合入 develop 会自动触发一次推送运行。等待其完成，然后：

```bash
gh api /repos/GlimpseEngine/token-station/actions/cache/usage
```

期望（验收标准 2）：`active_caches_size_in_bytes` 低于 10 GB。

```bash
gh api "/repos/GlimpseEngine/token-station/actions/caches?per_page=100" \
  --jq '.actions_caches[] | "\(.ref)\t\(.key)"' | sort -u
```

期望（验收标准 1）：出现 `refs/heads/develop` 作用域的条目。

```bash
run=$(gh run list --workflow=ci.yml --branch develop --limit 1 --json databaseId --jq '.[0].databaseId')
gh run view "$run" --json jobs --jq '.jobs[] | "\(.conclusion)\t\(.name)"'
```

期望（验收标准 4）：`windows-rust` 与 `agent-platform-targeted` 正常运行且通过；
`windows-msi` 为 skipped（`develop` 不是 `main`）。

- [ ] **Step 8: 验收标准 5——记录热缓存基线**

`develop` 上再触发一次运行（此时缓存已热），记录 Windows 任务耗时：

```bash
gh workflow run ci.yml --ref develop
sleep 60
run=$(gh run list --workflow=ci.yml --branch develop --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$run"
gh api "/repos/GlimpseEngine/token-station/actions/runs/$run/jobs" --paginate \
  --jq '.jobs[] | [((.completed_at|fromdateiso8601)-(.started_at|fromdateiso8601))/60|round, .name] | join("\tmin\t")'
```

把结果写入设计文档的一个新增小节「实测结果」，供阶段二决策使用。
这是阶段二判断「`windows-rust` 是否还需要进一步收窄」的唯一可信依据——
在此之前的所有 Windows 耗时数据都被缓存缺失污染过。

- [ ] **Step 9: 提交实测结果**

```bash
git add docs/superpowers/specs/2026-08-07-ci-cost-reduction-design.md
git commit -m "$(cat <<'EOF'
docs(ci): 补记热缓存下的实测基线

阶段二的决策依据。此前所有 Windows 耗时数据都被缓存缺失污染，
不能用来判断 windows-rust 是否需要进一步收窄。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
)"
git push
```

---

## 回滚方案

全部改动集中在单个文件的三个提交里。若 `develop` 上出现非预期行为：

```bash
git revert <task4-sha> <task3-sha> <task2-sha>
```

仅回滚分层、保留缓存修复（推荐的部分回滚——缓存修复是纯收益、零覆盖损失）：

```bash
git revert <task4-sha> <task3-sha>
```

## 自审记录

**Spec 覆盖检查：** 设计文档 4.1 → Task 3 Step 2/3；4.2 → Task 3 Step 1；
4.3 → Task 2 全部；4.4 → Task 4；验收标准 1–7 → Task 5 Step 2/3/4/7/8。
7 条验收标准全部有对应步骤，无遗漏。

**已知的计划内偏差：** 设计文档 4.5 声明「不改动 `linux-desktop.yml`」。
该文件用手写 `actions/cache`（key 为 `linux-cargo-<Cargo.lock hash>`），
按 tag 累积，目前占 2.04 GB。本计划遵守该约束不动它，但 Task 5 Step 7
的容量验收需把这 2 GB 计入。若容量超标，处理它是阶段二的第一顺位。
