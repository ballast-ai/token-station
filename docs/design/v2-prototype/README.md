# Token Station V2 设计原型

V2(token-station 全量需求)产品页面的静态 HTML 设计原型,按 **P4 终态一次画全、区块标注首次交付阶段**。口径源为 [V2 页面详细设计](../V2页面详细设计.md)(本原型是其可视化;区块变更须先改详设再改原型)。整理日期 2026-07-09。

## 使用

直接用浏览器打开 [index.html](./index.html),或在本目录起静态服务:

```bash
python3 -m http.server 8000
# 访问 http://localhost:8000/
```

页面间侧边栏可点击互跳;右上角切换明暗主题。与 [v1-prototype](../v1-prototype/) 并排打开即可对照看增量。

## 范围与口径

- **覆盖**:用户 Dashboard 16 页 + 管理后台 16 页,对应 [V2 阶段需求清单](../../planning/V2阶段需求清单.md) P1–P4 与 C2 服务端配套的全部 UI 落点(追溯矩阵见详设 §四)。
- **呈现形态**:平台 SaaS + 余额计费(与 v1-prototype 同口径);Team/Enterprise 差异用 `Ent` 徽章与置灰引导态标注,不单独出页(详设 §0.1)。
- **阶段徽章**:`V1` 保留 / `P1`–`P4` 各期新增 / `C2` 客户端配套 / `Ent` license 门槛(详设 §0.2)。开发某期时,晚于该期的区块整体不渲染。
- **不含**:营销页、登录页、本地客户端界面(客户端无 GUI,CLI 是唯一本地管理面;其云端控制面 = user/devices.html)。

## 视觉依据

- 设计系统:`assets/dash.css` 原样复用 v1-prototype(即 V1 源码 `src/templates/dash.css`),明暗双主题。
- V2 新组件只增不改,收敛在 `assets/v2.css`(阶段徽章、决策时间线、成本旋钮、开关矩阵、Ent 置灰)。
- 页面骨架:`_shell-user.html` / `_shell-admin.html`(侧边栏按详设 §一 重排)。

每个页面文件顶部注释标注对应产品路由、详设小节号与主体阶段。所有数据为模拟数据。

## 目录结构

```
v2-prototype/
├── index.html          # 原型导航页(非产品页面,含阶段图例)
├── README.md
├── _shell-user.html    # 用户侧骨架模板(维护用)
├── _shell-admin.html   # 管理侧骨架模板(维护用)
├── assets/
│   ├── dash.css        # V1 设计系统原样拷贝
│   ├── v2.css          # V2 新增组件
│   └── app.js          # 侧边栏/主题切换脚本
├── user/               # 用户 Dashboard 16 页
└── admin/              # 管理后台 16 页
```
