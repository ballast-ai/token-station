import { describe, expect, it } from "vitest";
import { humanizeAppError } from "./errors";

describe("humanizeAppError", () => {
  it("explains a concurrent configuration update in the selected language", () => {
    const raw = "apply_in_progress: 已有配置正在应用";

    expect(humanizeAppError(raw, "en")).toBe(
      "Another configuration update is still running. Wait a moment, then try again.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "另一项配置更新仍在进行。请稍等片刻，然后重试。",
    );
  });

  it("turns the production no-pools error into startup guidance", () => {
    const raw = "a router with no pools can route nothing";

    expect(humanizeAppError(raw, "en")).toBe(
      "Routing is not configured yet. Select a provider and model for at least one route, then save before starting Token Station.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "路由尚未配置。请至少为一个路由选择供应商和模型，保存后再启动 Token Station。",
    );
  });

  it("preserves the pool name in the production empty-pool error", () => {
    const raw = "pool `tier_high` has no members";

    expect(humanizeAppError(raw, "en")).toBe(
      "Route pool `tier_high` is empty. Add a provider and model to this pool, then save again.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "路由池 `tier_high` 为空。请为该路由池添加供应商和模型，然后重新保存。",
    );
  });

  it("preserves the rule and pool names in the production unknown-pool error", () => {
    const raw = "rule `long-context` routes to pool `missing`, which does not exist";

    expect(humanizeAppError(raw, "en")).toBe(
      "Rule `long-context` points to missing route pool `missing`. Choose an existing pool for this rule, then save again.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "规则 `long-context` 指向不存在的路由池 `missing`。请为该规则选择现有路由池，然后重新保存。",
    );
  });

  it("explains that an update candidate changed without exposing its internal code", () => {
    const raw = "update_version_changed: confirmed 1.1.3 but latest is now 1.1.4";

    expect(humanizeAppError(raw, "en")).toBe(
      "A newer update became available. Check again before installing.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "可用更新已经发生变化。请重新检查后再确认安装。",
    );
  });

  it("does not expose an unknown backend string", () => {
    const raw = "secret internal transaction detail /Users/example/config.json";

    expect(humanizeAppError(raw, "en")).toBe(
      "The operation could not be completed. Try again. If it still fails, open the local logs from Recovery mode.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。",
    );
    expect(humanizeAppError(raw, "en")).not.toContain("/Users/example");
  });
});
