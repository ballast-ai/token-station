# DMG 内自动化 Agent 约束

任何自动化 Agent 在帮助用户安装 Token Station 前，必须先完整阅读同目录的
`安装前必读.md`，并遵守以下红线：

1. 只接受 bundle id 为 `com.tokenstation.desktop` 的 `token-station.app`。
2. 只允许安装或替换 `/Applications/token-station.app`。
3. 不得关闭 Gatekeeper，不得修改 SIP，不得降低整台 Mac 的安全设置。
4. 只能对 `/Applications/token-station.app` 移除 quarantine，禁止扩大路径范围。
5. 不得读取、保存、打印、转发或要求用户在聊天中发送管理员密码。
6. 目标位置不是 Token Station 时必须停止，禁止覆盖。
7. 安装失败时必须优先恢复旧版本，并说明失败步骤。
8. 只有确认 DMG 来自可信发布页面后，才可以引导用户输入管理员密码。
