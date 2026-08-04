# 路由操作产生 `null` 并污染内存草稿

- **复测版本**：`e752c24`
- **优先级**：P1
- **状态**：回归，未解决
- **影响位置**：Agent 独立路由、主页路由、供应商保存

## 现象

路由配置失败后，返回主页仍显示：

```text
配置结构不合法: invalid type: null, expected a sequence
```

主页上、中、下三档全部变成“未选择”，用户无法继续正常配置。此前同类问题还会显示：

```text
配置结构不合法: invalid type: null, expected struct AgentRouteTarget
```

## 定位结论

磁盘上的 `token-station.json` 仍然合法，主页三档也完整。因此损坏发生在当前 App 进程的内存草稿，不是配置文件或 Ollama 服务损坏。

相关代码：

- `apps/desktop/src-tauri/src/lib.rs:1355-1373`：主页档位为空时，Agent 独立路由会生成 `null`。
- `apps/desktop/src-tauri/src/lib.rs:2477-2547`：主页和 Agent 路由命令直接修改全局 `inner.draft`，失败后没有恢复完整旧草稿。
- `apps/desktop/src-tauri/src/config_state.rs:148-169`：`observe_draft()` 只记录指纹和 revision，不执行完整配置校验。
- `apps/desktop/src-tauri/src/lib.rs:1101-1115`：非法结构直到生成页面状态、保存或启动时才暴露。

本机两个 Token Station 版本还曾同时运行，并共用 `~/Library/Application Support/com.tokenstation.desktop`，会进一步放大陈旧草稿和覆盖风险。

## 建议修复

1. 所有路由修改必须是事务性的：操作前保存完整草稿，校验失败后同时恢复草稿和 revision。
2. 未完成的 Agent 路由应保存在独立 UI 草稿中，不能进入全局可保存配置。
3. 错误提示应指出具体 Agent、档位和 JSON 字段，不直接显示 `expected a sequence`。
4. 同一配置目录只允许一个 Token Station 实例编辑，或必须提供 revision 冲突检测。

## 验收标准

1. 任意路由操作失败后，主页和其他页面状态与操作前完全一致。
2. 未完成的 Agent 独立路由不会阻断主页配置、添加供应商、保存或启动。
3. 重启前后显示一致，不需要通过重启清理非法内存草稿。
4. 自动化测试覆盖“空主页 → 独立路由 → 返回主页 → 配置并保存”的完整流程。
