# Token Station V1 设计原型

V1(cloud_ai_gateway)产品页面的静态 HTML 设计原型,**忠实还原现状**,作为 V2 设计工作的现状基线可视化。整理日期 2026-07-08。

## 使用

直接用浏览器打开 [index.html](./index.html),或在本目录起静态服务:

```bash
python3 -m http.server 8000
# 访问 http://localhost:8000/
```

页面间侧边栏可点击互跳;右上角可切换明暗主题(还原 V1 的主题系统,默认 dark)。

## 范围与口径

- **覆盖**:用户 Dashboard 9 页 + 管理后台 10 页(含用户/团队详情页),对应 [V1 需求清单](../../planning/V1需求清单与业务模块.md) 的 M10 控制台与 M8-3 管理看板。
- **不含**:营销页(intro/enterprise/router)、用户/管理登录页、多语言切换的实际生效(仅呈现语言控件)。
- **计费形态**:按「余额模式」(billing-balance)呈现,billing 页可见;admin「额度」页对应 billing-quota-credits 特性——真实 V1 中两者不会同时出现在一个构建里,原型为完整展示同时保留。

## 还原依据

| 内容 | 来源 |
|------|------|
| 视觉/设计系统 | 源码 `src/templates/dash.css` 原样复用(assets/dash.css) |
| 页面骨架(侧边栏/topbar/面包屑) | `src/templates/layout.rs` → `_shell-user.html` / `_shell-admin.html` |
| 各页区块/表格列/表单字段 | `src/templates/*_pages.rs` 渲染代码逐一对照 |
| 中文文案 | 源码 `src/i18n/zh-CN.json` 官方译文 |
| 图标 | `src/templates/assets.rs` 内联 SVG |

每个页面文件顶部注释标注了对应路由与还原依据的源码文件。所有数据为模拟数据。

## 目录结构

```
v1-prototype/
├── index.html          # 原型导航页(非 V1 产品页面)
├── README.md
├── _shell-user.html    # 用户侧页面骨架模板(维护用)
├── _shell-admin.html   # 管理侧页面骨架模板(维护用)
├── assets/
│   ├── dash.css        # V1 设计系统原样拷贝
│   └── app.js          # 侧边栏/主题切换脚本
├── user/               # 用户 Dashboard 9 页
└── admin/              # 管理后台 10 页
```
