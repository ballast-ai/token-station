import { describe, expect, it, vi } from "vitest";
import { humanizeAppError, humanizeErrorCode } from "./errors";
import { setActiveLanguage } from "./i18n";

describe("humanizeAppError", () => {
  it.each([
    ["zh-TW", "驗證 · Key", "上游拒絕此憑證。"],
    ["ja", "認証 · Key", "アップストリームが認証情報を拒否しました。"],
  ] as const)("localizes receipt errors for %s", (language, layer, message) => {
    expect(humanizeErrorCode("auth", language)).toMatchObject({ layer, message });
  });

  it.each([
    ["zh-TW", "操作未能完成。請重試。"],
    ["ja", "操作を完了できませんでした。もう一度お試しください。"],
  ] as const)("does not fall back to English App errors for %s", (language, expected) => {
    expect(humanizeAppError("secret internal detail", language)).toContain(expected);
  });

  it("uses a persisted new locale when the caller omits the language", () => {
    window.localStorage.setItem("token-station-language", "ja");
    expect(humanizeAppError("secret internal detail")).toContain("操作を完了できませんでした");
  });

  it("explains that Cursor must be quit before its local database can be configured", () => {
    const structured = {
      code: "cursor_running",
      message: "Cursor 正在运行。请手动退出 Cursor 后再点一键接入。",
    };

    expect(humanizeAppError(structured, "en")).toBe(
      "Cursor is still running. Quit Cursor completely, then click Connect again. Token Station will not close it for you.",
    );
    expect(humanizeAppError(structured, "zh-CN")).toBe(
      "Cursor 仍在运行。请彻底退出 Cursor 后再点一次一键接入。Token Station 不会强制关闭它。",
    );
    expect(humanizeAppError("cursor_running", "zh-CN")).toBe(
      "Cursor 仍在运行。请彻底退出 Cursor 后再点一次一键接入。Token Station 不会强制关闭它。",
    );
    expect(humanizeAppError("cursor_running", "zh-TW")).toContain("完全退出 Cursor");
    expect(humanizeAppError("cursor_running", "ja")).toContain("Cursor を完全に終了");
  });

  it("uses the active session language when locale storage is unavailable", () => {
    setActiveLanguage("ja");
    const storageSpy = vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("storage unavailable");
    });
    expect(humanizeAppError("cursor_running")).toContain("Cursor を完全に終了");
    storageSpy.mockRestore();
  });

  it("explains Cursor tunnel failures without falling back to a generic error", () => {
    const raw = "读取 cloudflared 下载包失败：the response body is larger than request limit";

    expect(humanizeAppError(raw, "en")).toBe(
      "Token Station could not establish the Cursor HTTPS tunnel. Check the network, then try again. The temporary endpoint has been closed.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "Token Station 无法建立 Cursor HTTPS 隧道。请检查网络后重试。本次临时入口已经关闭。",
    );
  });

  it("does not misclassify a model-catalog response read failure as a local-file error", () => {
    const raw = "读取模型目录失败：transport failed to read response body";

    expect(humanizeAppError(raw, "en")).toBe(
      "The latest provider data is unavailable. Keep the current settings and try refreshing again later.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "暂时无法获取最新的供应商数据。请保留当前设置，稍后再次刷新。",
    );
  });

  it("keeps explicit local permission failures in local-file guidance", () => {
    const raw = "failed to write local cache: permission denied";

    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "Token Station 无法访问所需的本地文件。请检查文件权限和磁盘空间，然后重试。",
    );
  });

  it("does not treat provider catalog authorization as local-file permission", () => {
    const raw = "Key 无效，或当前账号没有读取模型目录的权限";

    expect(humanizeAppError(raw, "en")).toBe(
      "The API key is invalid, or this account cannot read the provider's model catalog.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "API Key 无效，或当前账号没有读取该供应商模型目录的权限。",
    );
  });

  it("explains a concurrent configuration update in the selected language", () => {
    const raw = "apply_in_progress: 已有配置正在应用";

    expect(humanizeAppError(raw, "en")).toBe(
      "Another configuration update is still running. Wait a moment, then try again.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "另一项配置更新仍在进行。请稍等片刻，然后重试。",
    );
  });

  it("describes a local Agent version probe timeout without blaming the network", () => {
    const error = {
      code: "VERSION_PROBE_TIMEOUT",
      message: "安装入口存在，但版本探测进程未成功运行",
    };

    expect(humanizeAppError(error, "en")).toBe(
      "Agent version detection timed out. Rescan; if it still fails, check that the Agent installation is complete.",
    );
    expect(humanizeAppError(error, "zh-CN")).toBe(
      "Agent 版本检测超时。请重新扫描；如果仍然失败，请检查该 Agent 的安装是否完整。",
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

  it("将额度账户不完整错误转为可操作提示", () => {
    const raw = "额度优先账户 #1 缺少供应商或模型";

    expect(humanizeAppError(raw, "en")).toBe(
      "A quota account is incomplete. Select both a provider and a model, then apply again.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "有一个额度账户尚未配置完整。请同时选择供应商和模型，然后重新应用。",
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

  it("does not mistake a Connector metadata failure for a network failure", () => {
    const raw = "Agent model metadata refresh failed for Connector workbuddy-v1";

    expect(humanizeAppError(raw, "en")).toBe(
      "The proxy is running, but Token Station could not refresh one managed Agent configuration. Open the Agent page, rescan, and repair that Agent before using its route.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "代理已经运行，但一个已接管 Agent 的模型元数据刷新失败。请打开 Agent 页面重新扫描，并修复该 Agent 后再使用它的路由。",
    );
  });

  it("does not misclassify Kimi's missing context limit as invalid TOML", () => {
    const raw = "Kimi Code 接入需要当前有效路由提供正数 context 上限；本次未修改 config.toml。";

    expect(humanizeAppError(raw, "en")).toBe(
      "Kimi Code needs a verified context-window limit for the active route. Complete that model's context window in Providers, restart the proxy, then connect again.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "Kimi Code 需要当前路由模型具备可信的上下文上限。请在供应商页面补全该模型的上下文窗口，重启代理后再次接入。",
    );
  });

  it("does not expose an unknown backend string", () => {
    const raw = "secret internal transaction detail /Users/example/config.json";

    expect(humanizeAppError(raw, "en")).toBe(
      "The operation could not be completed. Try again. If it still fails, update Token Station or contact support.",
    );
    expect(humanizeAppError(raw, "zh-CN")).toBe(
      "操作未能完成。请重试；如果仍然失败，请更新 Token Station 或联系支持。",
    );
    expect(humanizeAppError(raw, "en")).not.toMatch(/recovery mode/i);
    expect(humanizeAppError(raw, "zh-CN")).not.toContain("自救模式");
    expect(humanizeAppError(raw, "en")).not.toContain("/Users/example");
  });

  it("does not direct database failures to a recovery screen", () => {
    expect(humanizeAppError("metrics database schema mismatch", "en")).toBe(
      "The local data could not be opened. Update Token Station and try again; if the problem continues, contact support.",
    );
    expect(humanizeAppError("指标库 schema 不兼容", "zh-CN")).toBe(
      "无法打开本地数据。请更新 Token Station 后重试；如果仍然失败，请联系支持。",
    );
  });

  it("preserves the affected model in actionable OpenCode contract guidance", () => {
    const error = {
      code: "model_contract_missing_max_output_tokens",
      message: "this display copy may change without changing the contract",
      target: "kimi/kimi-k3",
    };

    expect(humanizeAppError(error, "zh-CN")).toBe(
      "模型 `kimi/kimi-k3` 缺少最大输出 Token 上限。请前往供应商页面完善该模型限制，然后重启代理。",
    );
    expect(humanizeAppError(error, "en")).toBe(
      "Model `kimi/kimi-k3` has no maximum output token limit. Complete this model's limits in Providers, then restart the proxy.",
    );
    expect(humanizeAppError(error, "zh-TW")).toContain("`kimi/kimi-k3` 缺少最大輸出 Token 上限");
    expect(humanizeAppError(error, "ja")).toContain("モデル `kimi/kimi-k3` に最大出力トークンの上限がありません");
  });

  it("preserves dynamic route-pool names in the new locales", () => {
    const raw = "pool `tier_high` has no members";
    expect(humanizeAppError(raw, "zh-TW")).toContain("路由集區 `tier_high` 是空的");
    expect(humanizeAppError(raw, "ja")).toContain("ルートプール `tier_high` は空です");
  });

  it.each([
    ["model_contract_exact_routing_unsupported", "OpenCode 固定模型与精确模型路由不兼容"],
    ["model_contract_invalid_route", "OpenCode 路由配置无效"],
    ["model_contract_no_reachable_model", "OpenCode 当前路由没有可达模型"],
    ["model_contract_unknown_provider", "OpenCode 路由引用了未知供应商"],
    ["model_contract_unknown_model", "OpenCode 路由引用了未知模型"],
    ["model_contract_missing_context_window", "缺少上下文上限"],
    ["model_contract_invalid_limits", "Token 上限无效"],
    ["agent_runtime_transition", "代理正在切换运行实例"],
  ])("按稳定错误码 %s 展示契约修复建议", (code, expected) => {
    expect(humanizeAppError({ code, message: "unstable backend copy", target: "p/m" }, "zh-CN"))
      .toContain(expected);
  });
});
