# Gate D / T8 Provider 生命周期验收

> 日期：2026-07-22
> 状态：本地验证完成

## 验收结论

- 目录刷新保留下架模型并标记 `removed`。
- Provider 身份变更会使旧目录失效，删除后同名同 URL 不会继承旧账号的可信证据。
- tombstone 不可覆盖；同名新增必须先恢复再编辑。
- Provider 详情用量显示估算成本或成本未知，未定价/零值不声称免费。
- Provider 编辑与模型保存后的 save/apply 真实请求命中新 revision。

## 实际命令与结果

```text
cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml
  148 passed; 0 failed; 1 ignored
  yaml_scalar_regression: 3 passed; 0 failed

cargo clippy --manifest-path apps/desktop/src-tauri/Cargo.toml --all-targets -- -D warnings
  exit 0

cargo test -p token-station-cli
  unit/main/integration/doc tests 全部通过
  proxy: 49 passed; 0 failed; 1 ignored

npm --prefix apps/desktop run test -- --run
  9 files passed; 85 tests passed

npm --prefix apps/desktop run build
  TypeScript 与 Vite 生产构建成功

cargo fmt --check
  exit 0
```

## 边界

- 未执行真实付费 Provider 凭据请求。
- Agent Skill 用量不在 T8 范围，本次未新增相关采集或展示。
