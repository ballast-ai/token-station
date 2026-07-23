# 第二批协作任务：T5 EgressPolicy 验收

> 日期：2026-07-22
> 范围：#5 直连、HTTP/SOCKS 代理、`no_proxy`、认证槽与出口解释
> 结论：本地实现与自动化验收通过

## 1. 交付结论

- `ClientConfig.egress` 支持 `direct`、`http`、`socks5`，严格校验 URL scheme、userinfo、路径、`no_proxy` 和认证来源。
- Gateway 的 Provider 请求、模型目录、健康/能力探测使用同一显式 policy；Desktop 模型目录复用同一配置和 secret store。
- 代理认证从 `egress-proxy/<slot>` 按请求解析，UI 和公开配置不保存密码。
- `/admin/egress` 返回运行中 Gateway 对 `provider_request`、`model_catalog`、`health_probe` 的实际出口；更新检查标记为固定直连。
- 所有 client 明确设置 `max_redirects(0)`；环境代理不参与解析，任何 3xx 都不会产生携带原 Authorization 的第二跳。

## 2. 关键证据

- `http_proxy_is_used_for_a_real_request_without_resolving_the_target_locally`：真实 TCP 代理收到 CONNECT/绝对 URI 请求，目标使用不可解析域名仍完成响应。
- `http_and_socks_policies_are_explicit_and_apply_no_proxy`：HTTP 与 SOCKS5h 均建立显式代理，实际 matcher 区分 bypass/proxy 目标。
- `proxy_auth_resolves_from_a_slot_and_never_from_url_userinfo`：密码仅从 slot 来源解析，公开 URL 无 userinfo。
- `redirects_and_environment_proxies_are_disabled_explicitly`：direct agent 明确无代理并拒绝重定向。
- `redirects_to_other_hosts_loopback_and_metadata_never_receive_a_second_hop`：外部主机、回环地址和 metadata 目标均收不到第二跳。
- `admin_data_plane_serves_the_running_views`：受虚拟 Key 保护的 `/admin/egress` 返回实际请求类别路由和固定 direct 类别。

## 3. Fresh verification

```text
cargo test -p token-station-cli
结果：lib 126 passed, 1 ignored；main 1；devchain 1；install 4；proxy 50 passed, 1 ignored；upgrade 4；doc tests 通过

cargo test --test proxy admin_data_plane_serves_the_running_views -- --exact
结果：1 passed

cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
结果：lib 158 passed, 1 ignored；YAML regression 3 passed；doc tests 通过

npm test -- --run
结果：11 files passed，109 tests passed

npm run build
结果：TypeScript 与 Vite production build 通过

cargo clippy --workspace --all-targets -- -D warnings
结果：通过，0 warning

cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
结果：通过，0 warning

cargo fmt --all -- --check
结果：通过
```

## 4. 通过条件对照

| 条件 | 结果 |
|---|---|
| direct / HTTP CONNECT / SOCKS5 可配置 | 通过 |
| `no_proxy` 实际影响请求出口 | 通过 |
| 代理认证使用 credential slot | 通过 |
| 每类请求出口可解释 | 通过 |
| 环境代理不覆盖显式策略 | 通过 |
| 每个 3xx 停止，不产生跨主机鉴权第二跳 | 通过 |
| 更新检查固定直连 | 通过 |
| UI 不读取或展示代理密码 | 通过 |
