# macOS DMG 安装提示与打包要求

> **必读范围：** 任何准备创建、修改、测试、上传或发布 Token Station macOS DMG 的人和 Agent，都必须先完整阅读本文。除非当前任务明确要求打包，否则只遵守和维护本文，不得自行开始构建或生成 DMG。

## 1. 目的

Token Station 的未公证 macOS 构建可能被 Gatekeeper 阻止首次启动。以后制作 DMG 时，安装包必须给普通用户一个能看懂、能双击执行、失败后也知道怎么处理的安装入口。同时，安装流程不能为了放行 Token Station 而降低整台 Mac 的安全级别。

## 2. DMG 必须包含的内容

以后生成的 DMG 根目录至少包含以下内容：

1. `token-station.app`
2. 指向 `/Applications` 的快捷方式
3. `安装前必读.md`
4. `安装 Token Station.command`
5. `AGENTS.md`

DMG 内的 `AGENTS.md` 必须要求自动化 Agent 先阅读 `安装前必读.md`，并重复本文的安全红线。文件名应直接可见，不能藏在 App bundle 内部或多层目录里。

## 3. 禁止全局关闭 Gatekeeper

不得在说明、脚本、自动化流程或客服回复中要求用户执行 `sudo spctl --master-disable`。这个命令会全局关闭 Gatekeeper，让 macOS 停止检查其他来源不明的 App；它并不会关闭 System Integrity Protection（SIP）。为了安装一个未公证 App 而关闭整台机器的来源验证，安全代价过大。

如果以后有人再次提出这个命令，打包者或 Agent 必须说明上述区别，并改用只作用于 Token Station 的命令：

```bash
sudo xattr -dr com.apple.quarantine /Applications/token-station.app
```

这个命令只能在已经确认 App 来源、安装路径和 bundle id 后执行。禁止对 `/Applications`、用户主目录或其他宽泛目录递归移除 quarantine。

## 4. 双击安装脚本的要求

`安装 Token Station.command` 必须是可执行的 shell 脚本。用户双击后，终端应先用中文显示将要执行的动作，再等待用户明确确认。脚本可以通过 `sudo` 让 macOS 请求管理员密码，但不能自行读取、保存、打印或转发密码。

脚本必须按以下顺序执行：

1. 找到同一 DMG 中的 `token-station.app`。
2. 读取并验证源 App 的 bundle id 必须为 `com.tokenstation.desktop`。
3. 使用 `codesign --verify --deep --strict` 验证源 App。
4. 告知用户安装目标是 `/Applications/token-station.app`，并说明只会放行这个 App。
5. 等待用户输入明确的 `y` 或 `Y`；其他输入均取消。
6. 通过 `sudo` 请求管理员权限。终端必须提示用户：输入密码时不会显示字符，这是正常现象。
7. 如果目标位置已有 App，先验证它的 bundle id。目标不是 Token Station 时必须停止，禁止覆盖。
8. 先把新 App 复制到 `/Applications` 下带固定 Token Station 前缀的临时目录，完成 bundle id 和代码签名验证后再替换旧版本。
9. 替换旧版本前创建可恢复备份。任何后续步骤失败时恢复旧版本。
10. 只对 `/Applications/token-station.app` 执行 `xattr -dr com.apple.quarantine`。
11. 再次验证 bundle id 和代码签名，然后启动 App。
12. 成功后删除临时文件和备份；失败时保留明确错误并等待用户按回车关闭终端窗口。

脚本不得使用通配符定位、删除或覆盖 App，不得对不确定路径执行递归删除，不得关闭 Gatekeeper，不得修改 SIP。

## 5. `安装前必读.md` 的用户文案要求

面向用户的说明必须使用正常、直接的中文，至少讲清楚以下内容：

- 这个版本是否完成 Developer ID 签名和 Apple 公证。
- 推荐方式是双击 `安装 Token Station.command`。
- 终端为什么会要求管理员密码，以及输入密码时为什么看不到字符。
- 脚本只安装并放行 Token Station，不会关闭系统全局安全检查。
- 不运行脚本时，可以把 App 拖入 Applications，然后右键点击 App，选择“打开”，并在 macOS 提示中再次点击“打开”。
- 如果右键打开仍失败，可以执行只针对 Token Station 的 `xattr` 命令。
- 只有从可信发布页面下载的 DMG 才可以输入管理员密码；来源不明时必须停止。

不要把“关闭 Gatekeeper”和“关闭 SIP”写成同一件事。不要告诉用户执行全局安全降级后再点击“打开”。

## 6. Agent 行为要求

Agent 看到本文件后，只能把它当作未来发布约束。用户没有明确说“现在打包”时，不得因为本文存在而运行构建、创建 DMG、重装 App、提交或上传产物。

当用户明确要求打包时，Agent 必须先检查当前分支、远端差异、工作区未提交文件、实际运行实例和现有发布脚本。Agent 必须保留不属于当前任务的改动，并按项目根目录 `AGENTS.md` 的设计、测试、构建、真实 App 验收和中文本地提交顺序执行。

管理员密码只能由用户直接输入 macOS 的系统终端或 `sudo` 提示。Agent 不得让用户把密码发到聊天中，也不得通过命令参数、日志、环境变量或临时文件传递密码。

## 7. 打包验收标准

发布者必须挂载最终 DMG，而不是只检查暂存目录。验收至少包括：

1. 五个必需入口全部存在，安装脚本具有可执行权限。
2. DMG 内 App 的 bundle id 是 `com.tokenstation.desktop`。
3. App 代码签名验证通过。
4. 安装说明和脚本不包含全局关闭 Gatekeeper 的执行步骤。
5. 安装脚本只对 `/Applications/token-station.app` 移除 quarantine。
6. 目标不是 Token Station 时，脚本拒绝覆盖。
7. 复制、签名验证或启动失败时，旧版本可以恢复。
8. 在一台具有 quarantine 标记的测试环境中完成真实首次启动验证。
9. 最终 DMG 的文件名、版本号、架构和校验值与发布页面一致。

任何一项失败，都不能把 DMG 标记为可发布。失败发生在哪一步，发布记录就必须如实写明哪一步。

## 8. 当前状态

本文只记录未来打包要求。创建本文时没有生成新 DMG，没有修改现有打包脚本，也没有重新安装 Token Station。后续实现必须另开明确的打包任务，并先更新对应设计文档和公开测试。
