# Agent 接入提示移除与 Direct 行排序修复设计

- 日期：2026-08-13
- 状态：已实现、安装并完成自动化及可执行的真实 App 验收
- 范围：Desktop Agent 页面、Direct 单独路由供应商/模型行、前端依赖与样式
- 不涉及：后端路由契约、Provider/模型数据、Agent 接入事务、DMG 与远端发布

## 1. 问题

Agent 页面在代理停止且尚未接入时，会额外显示“点击一键接入后会自动启动代理，并等待代理
可达。”提示条。该文案重复解释按钮内部行为，占用首屏空间；移除提示不代表移除自动启动
能力。

Direct 单独路由列表虽然提供拖拽柄，但当前使用浏览器原生 HTML5 `dragStart/drop`：拖动时
其他行不会根据碰撞位置实时让位，只有落到目标行后才瞬间更新；指针移动阈值、取消态、键盘
拖拽语义和 WebView 一致性也没有统一处理。因此“上下重排”看得到入口，却缺少稳定反馈。

本次借鉴 cc-switch 在固定提交中的垂直排序结构：`DndContext`、Pointer/Keyboard Sensor、
`SortableContext`、稳定 Provider ID、`verticalListSortingStrategy` 和 transform/transition。
只借鉴交互机制，不继承 cc-switch 中排序会影响故障转移优先级的产品语义：

- [cc-switch `ProviderList.tsx`](https://github.com/farion1231/cc-switch/blob/1f38c83826a8bca3c1a7a18d9629f05a914718fd/src/components/providers/ProviderList.tsx)
- [cc-switch `useDragSort.ts`](https://github.com/farion1231/cc-switch/blob/1f38c83826a8bca3c1a7a18d9629f05a914718fd/src/hooks/useDragSort.ts)

## 2. 目标、范围与非目标

### 2.1 目标

1. Agent 页面不再渲染自动启动代理的说明条。
2. 代理停止时“一键接入”仍可点击，内部继续执行
   `ensure_serve_running → plan_agent_connection → apply_agent_plan → cached refresh`。
3. Direct 行只能从左侧拖拽柄启动排序；拖动超过小阈值后，行随指针移动，目标行实时让位，
   松手后固定新顺序。
4. 保留无需进入拖拽模式的 ArrowUp/ArrowDown 直接移动；同时支持标准键盘 sortable 交互。
5. 排序刷新后仍从 localStorage 恢复；选择态和每家 Provider 的模型草稿不随位置串行。

### 2.2 范围

- `AgentRoutePage` 的提示渲染。
- `DirectRoutePanel` 的排序传感器、sortable row 和状态提交。
- Direct 行拖动态样式与 reduced-motion 降级。
- `@dnd-kit/core`、`@dnd-kit/sortable`、`@dnd-kit/utilities` 直接依赖声明。
- 组件公开行为测试、前端全量测试/构建、本机 App 安装与真实界面验收。

### 2.3 非目标

- 不修改一键接入成功、失败、超时或实例 generation 语义。
- 不改变 Provider、模型、Direct target 或 Router 配置格式。
- 不增加 Provider 自动选择、模型自动替换或拖拽后自动应用。
- 不把展示顺序升级为故障转移、额度优先或请求路由顺序。
- 不在本次重做 Direct 行的响应式布局或模型下拉菜单定位。
- 不制作、上传或发布 DMG，不推送远端分支。

## 3. 安全与数据红线

1. 排序 ID 必须使用 Provider 稳定 `name`；禁止使用数组下标作为身份。
2. 拖动和键盘排序只修改 `providerOrder`，只能写入
   `token-station-direct-provider-order-v1`，不得调用 `onApply` 或任何 Tauri 写命令。
3. 已选 Direct target 继续由 `selectedProvider + modelByProvider[selectedProvider]` 产生，
   不得从第一行、目标索引或视觉位置推断。
4. Provider 移动前后必须携带自己的模型选项和模型草稿；不能把 Kimi 的模型写给 DeepSeek。
5. localStorage 写入失败时继续显示现有错误 toast，本次会话中的排序仍可使用。
6. API Key、Base URL、请求内容和 Agent 配置不得进入拖拽数据、日志或 localStorage。
7. 本地 App 构建失败时保留现有 `/Applications/token-station.app`；安装脚本只允许替换
   bundle id 为 `com.tokenstation.desktop` 的明确目标。

## 4. 用户可见交互、状态与失败处理

### 4.1 Agent 接入区

- 删除代理停止且未接入时的橙色说明条，不用其他 toast、tooltip 或空白占位替代。
- “一键接入”按钮的启用条件、处理中状态和错误提示保持不变。
- 自动启动失败、超时或接入失败继续进入现有错误 toast；移除说明不吞掉失败反馈。

### 4.2 Direct 指针排序

1. 用户在拖拽柄按下并移动不足 8px：视为点击，不启动排序。
2. 移动达到 8px：当前行进入 dragging 态并随指针垂直移动。
3. 指针越过其他行中心：其他行使用 transform/transition 实时让位。
4. 松手在有效目标上：按稳定 Provider ID 计算新顺序并持久化。
5. 松手时无目标、按 Escape 取消、首尾未发生位置变化：顺序保持不变。
6. 模型下拉、radio、品牌区和整行点击都不能误启动拖拽。

### 4.3 键盘排序

- 拖拽柄保持可聚焦和明确的中英文可访问名称。
- 直接按 ArrowUp/ArrowDown 仍各移动一行；首行 ArrowUp、末行 ArrowDown 不循环。
- 标准 sortable 键盘模式保留 Space/Enter 拾取、方向键移动、Space/Enter 放下、Escape 取消。
- 排序后焦点留在该 Provider 的拖拽柄，不能跳到原索引上的另一家 Provider。

### 4.4 状态与失败

- `providerOrder`：纯展示偏好；Provider 集合变化时去重、删除失效项、追加新项。
- `selectedProvider`：稳定 Provider name；排序不改值。
- `modelByProvider`：以 Provider name 为 key；排序不改映射。
- `target`：后端已应用值；排序不改值，也不触发应用。
- localStorage 不可用：保留当前内存顺序并复用现有可关闭 toast。

## 5. 响应式、动画与可访问性

- 1040×720 默认窗口与 900×600 最小窗口均通过同一个垂直排序上下文工作。
- sortable transform 只作用于被注册的 Direct 行，不改变模型下拉固定浮层定位。
- 指针传感器只绑定拖拽柄，并设置 `touch-action: none`；行内其余控件保持原交互。
- dragging 态使用边框/阴影/透明度共同表达，不只依赖颜色。
- `prefers-reduced-motion: reduce` 时取消排序过渡，仍保留即时位置反馈。
- radio、combobox、应用按钮、选中勾选和焦点轮廓的现有语义全部保留。

## 6. 公开测试边界与验收标准

### 6.1 组件测试

1. `serveRunning=false` 且未接入时，不出现自动启动说明，但“一键接入”仍可用且完整调用
   `ensure → plan → apply → cached`。
2. Direct 行按 Provider 稳定 name 注册为 sortable item，拖拽柄是唯一激活入口。
3. 键盘 ArrowUp/ArrowDown 能重排；首尾越界保持不变。
4. 上下移动后 Provider、品牌、模型 combobox 和模型草稿仍属于同一行。
5. 排序不改变 radio 选择、不调用 `onApply`；只有点击“应用”才提交稳定
   `(upstream, model)`。
6. localStorage 成功保存新顺序；写入失败仍可选择模型和应用路由。
7. reduced-motion 样式明确取消 Direct 行的排序 transition。

### 6.2 自动化门禁

- 定向 Vitest：`AgentRoutePage`、`DirectRoutePanel`、主题样式。
- Desktop 前端全量 Vitest、TypeScript 与 Vite production build。
- `git diff --check`。
- 仓库全量 Rust format、Clippy、test 与 build；本次不应引入 Rust 行为差异。
- `scripts/install-local-desktop.sh` 完成本地构建、审计、签名、安装与启动。

### 6.3 真实 App 检查

1. Agent 页面不再显示橙色自动启动说明；停止态“一键接入”仍为可用按钮。
2. Direct 行从拖拽柄移动时有连续位移和让位反馈，不再只在 drop 后瞬移。
3. DeepSeek/Kimi 互换后，各自品牌、模型下拉和选中态不串行；不点“应用”不改路由。
4. 拖拽不足阈值不误移动，模型下拉仍可正常打开和选择。
5. ArrowUp/ArrowDown、标准键盘拾取/放下、Escape 取消与焦点保持正常。
6. 1040×720、900×600、深浅主题和 reduced-motion 下无横向溢出或残留浮层。

## 7. 实现落点、遗留项与发布要求

### 7.1 计划实现落点

- `apps/desktop/src/pages/AgentRoutePage.tsx`：移除说明条。
- `apps/desktop/src/components/DirectRoutePanel.tsx`：接入 dnd-kit 垂直 sortable。
- `apps/desktop/src/App.css`：dragging/transform/reduced-motion 样式。
- `apps/desktop/src/pages/AgentRoutePage.test.tsx`、
  `apps/desktop/src/components/DirectRoutePanel.test.tsx`、主题样式测试：公开行为回归。
- `apps/desktop/package.json`、`package-lock.json`：把已选 dnd-kit 包声明为直接依赖。

### 7.2 已知遗留项

- 排序仍是当前桌面 profile 的 localStorage 偏好，不跨设备同步。
- Provider name 仍兼具 Direct 展示顺序身份；未来若允许安全重命名，需要独立迁移稳定 ID。
- JSDOM 不能证明 macOS WebView 的实际指针手感，真实 App 拖拽验收不可省略。

### 7.3 发布要求

- 影响桌面可执行行为，交付前必须执行 `scripts/install-local-desktop.sh` 并验收已安装 App。
- 本任务不获准创建 DMG、上传 Release 或推送远端。
- 验收后回写本文状态、自动化结果、真实 App 结果与遗留项，并使用中文说明创建本地 commit。

## 8. 实现与验收记录

### 8.1 实现结果

- 已移除 Agent 页面中“点击一键接入后会自动启动代理，并等待代理可达。”说明条；
  `ensure → plan → apply → cached refresh` 接入链路未改动。
- Direct 行已由 HTML5 drag 迁移到 dnd-kit 垂直 sortable：稳定 Provider name、8px 指针阈值、
  实时让位、列表外 drop/`Escape` 取消、专用拖拽柄、Space/Enter 键盘模式及中文读屏播报。
- 展示顺序仍只写入原 localStorage key；Provider 选择、模型草稿和已应用 route 均继续按
  Provider name 绑定，排序不会调用 `onApply`。
- 拖动态的 z-index 落在承载 transform 的 wrapper；drop 后不保留 layout transform，避免影响
  固定定位的模型下拉；reduced-motion 同时关闭 wrapper 与行的排序过渡。

### 8.2 自动化结果

- 公开测试先行：新断言在旧实现上分别暴露说明条仍存在、缺少 sortable 语义及缺少拖动态样式；
  实现后定向 Vitest 为 3 个文件、31 项通过。
- Desktop 前端全量 Vitest：35 个文件、350 项通过。
- TypeScript 与 Vite production build：通过；仅保留仓库既有的大 chunk 提示。
- 任务文件 `git diff --check`：通过。
- 隔离工作树中的 `cargo test --workspace --all-targets --locked`：在提供仓库忽略的
  `plugins-dist` 测试产物后通过；`cargo build --workspace --locked`：通过。
- Rust format 仍被当前 HEAD 中未改动的 `agent_integration/commands.rs` 与
  `model_catalog.rs` 格式差异拦截；Clippy `-D warnings` 仍被未改动的
  `crates/protocol/src/capability.rs:63` 基线告警拦截。本任务不扩大范围修改这些文件。

### 8.3 本机 App 验收

- 在已合并状态栏任务提交 `ac2340e` 的代码上叠加本次前端改动，执行
  `scripts/install-local-desktop.sh` 通过：官方 bundled-plugin gate、release build、artifact
  audit、精确替换、bundle id、临时签名和启动健康检查均成功；已安装并启动
  `/Applications/token-station.app`。
- 真实 App 1040×720 默认窗口中，将代理停止并选中未接入的 Claude Code：
  橙色“点击一键接入后…”说明不存在，“一键接入”仍为可用按钮。
- 在真实 App 通过拖拽柄的 ArrowUp 将 DeepSeek 从第二行移回首行：焦点留在
  DeepSeek 拖拽柄，中文状态播报位置正确，DeepSeek/Kimi 的品牌、模型和选中态
  均没有串行，已应用路由仍为 `deepseek / deepseek-v4-flash`，且未点击“应用”。
- 重排后的模型下拉可正常打开，默认深色与浅色主题下未见遮挡、残留浮层或
  横向溢出；验收后已恢复深色主题、DeepSeek 首行和代理停止态。
- Computer Use 的单次拖拽能在真实 WebView 触发拾取态、拖动视觉反馈和中文播报，
  `Escape` 取消后顺序不变；但该自动化手势未能稳定合成“跨行后在有效目标内松手”，
  因此实际连续指针手感、900×600 窗口和系统 reduced-motion 还需人工补验。指针
  8px 阈值、目标行实时让位、有效 drop、列表外 drop 及取消已由公开行为测试覆盖。
