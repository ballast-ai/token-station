# Agent 安装选择器实施计划

## 目标

按已确认的
[Agent 接入区与安装选择器界面设计](../specs/2026-07-21-agent-installation-picker-ui-design.md)
统一五个 Agent 的接入区，隐藏完整路径，并在多安装时提供短名选择器。

## 实施步骤

1. 在 `AgentRoutePage.tsx` 提取跨 macOS/Linux/Windows 的可执行文件短名 helper，使用版本和稳定序号生成无路径标签。
2. 删除 Agent 标题下的 `canonical_path` 文本。
3. 用“选择安装”按钮和可访问列表替代原生路径 `select`；只在多安装时渲染。
4. 在 `App.css` 固定主操作按钮单行和最小尺寸，增加弹出列表的浅/深色、焦点和窄窗口样式。
5. 在 `App.test.tsx` 增加单安装路径隐藏、多安装短名〉切换和完整内部路径 plan 参数测试。

## 验证

```bash
cd apps/desktop && npm test -- --run
cd apps/desktop && npm run build
cargo fmt --all -- --check
bash scripts/check-router-core-redline.sh e0d727b HEAD
git diff --check
```

使用生产组件做一次深色与窄窗口视觉检查，确认“一键接入”不换行且完整路径不可见。

## 红线

不修改 `apps/cli/**`、`apps/desktop/src-tauri/**`、`crates/protocol/**`、`crates/plugin-api/**` 或 `crates/router-core/**`。
