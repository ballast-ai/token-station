# Token Station 工程约定

## 本地桌面 App 必须同步更新

凡是改动会影响 Token Station 可执行行为或界面的代码（包括 `apps/`、`crates/`、
`plugins/` 下的源代码及构建配置），在交付前必须完成本机桌面 App 更新，不能只运行
前端构建或测试后结束。

统一执行：

```bash
scripts/install-local-desktop.sh
```

该流程必须遵守以下顺序：

1. 使用官方 `scripts/build-desktop.sh --local` 流程构建并审计新 App。
2. 只有新 App 构建和审计全部成功后，才退出并删除旧 App。
3. 只允许替换 bundle id 为 `com.tokenstation.desktop` 的
   `/Applications/token-station.app`，禁止使用通配符或模糊匹配删除。
4. 安装新 App 后校验 bundle id 与代码签名，并启动 App 验证。
5. 任一步失败都必须如实报告；构建失败时必须保留当前已安装 App。

纯文档、注释或测试数据改动不要求重装 App，除非用户明确要求。
