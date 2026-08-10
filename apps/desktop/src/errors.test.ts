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

  it("turns an incomplete route into an actionable message", () => {
    const raw = "你还没有设置路由池。必须先设置至少一个路由池，才能启动 Token Station。";

    expect(humanizeAppError(raw, "en")).toBe(
      "Routing is not configured yet. Select a provider and model for at least one route, then save and apply.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "路由尚未配置。请至少为一个路由选择供应商和模型，然后保存并应用。",
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
