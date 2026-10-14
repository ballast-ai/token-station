import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";
import type { ProviderView } from "../api";
import QuotaPriorityPanel from "./QuotaPriorityPanel";

const fiveHoursMs = 5 * 60 * 60 * 1000;
const oneDayMs = 24 * 60 * 60 * 1000;

const providers: ProviderView[] = [
  {
    name: "deepseek",
    brand_id: "deepseek",
    provider: "openai-compatible",
    base_url: "https://api.deepseek.com/v1",
    models: ["deepseek-v4-flash"],
    has_auth: true,
    quota_plan: {
      windows: [{ len_ms: fiveHoursMs, limit: 5000 }],
      len_ms: fiveHoursMs,
      limit: 5000,
      unit: "tokens",
      rate_limit_per_min: null,
    },
  },
];

function renderQuotaPanel(overrides: Partial<ComponentProps<typeof QuotaPriorityPanel>> = {}) {
  const onSavePlan = vi.fn();
  const onSave = vi.fn();
  render(
    <QuotaPriorityPanel
      providers={providers}
      accounts={[{ upstream: "deepseek", model: "deepseek-v4-flash" }]}
      busy={false}
      applying={false}
      onSave={onSave}
      onViewUsage={vi.fn()}
      onSavePlan={onSavePlan}
      {...overrides}
    />,
  );
  return { onSave, onSavePlan };
}

function renderPanel(overrides: Partial<ComponentProps<typeof QuotaPriorityPanel>> = {}) {
  const result = renderQuotaPanel(overrides);
  const planSection = screen.getByText("额度计划（可选）").closest(".quota-plan-section") as HTMLElement | null;
  if (!planSection) throw new Error("quota plan section is missing");
  return { ...result, planSection };
}

describe("QuotaPriorityPanel quota plans", () => {
  it("places the primary apply action in the card header", () => {
    renderPanel();
    expect(screen.getByRole("button", { name: "保存并应用" }).closest(".panel-head"))
      .toBeInTheDocument();
  });

  it("用两句易读文案说明本地估算边界", () => {
    const { planSection } = renderPanel();
    expect(planSection).toHaveTextContent(
      "填写供应商的额度上限和刷新周期后，Token Station 会在本机估算剩余额度。",
    );
    expect(planSection).toHaveTextContent("若供应商会自动上报额度，则无需填写。");
  });

  it("shows provider brands in quota provider controls", async () => {
    const user = userEvent.setup();
    renderPanel();
    const trigger = screen.getByRole("combobox", { name: "账户 1 供应商" });
    expect(trigger.querySelector('[data-provider-brand="deepseek"]')).toBeInTheDocument();
    await user.click(trigger);
    expect(screen.getByRole("option", { name: "deepseek" })
      .querySelector('[data-provider-brand="deepseek"]')).toBeInTheDocument();
  });

  it("额度下拉框不用自定义 deepseek 名称冒充官方品牌", async () => {
    const user = userEvent.setup();
    renderPanel({
      providers: [{
        name: "deepseek",
        brand_id: null,
        provider: "openai-compatible",
        base_url: "https://custom.example/v1",
        models: ["deepseek-v4-flash"],
        has_auth: true,
      }],
    });

    const trigger = screen.getByRole("combobox", { name: "账户 1 供应商" });
    expect(trigger.querySelector('[data-provider-brand="deepseek"]')).toBeNull();
    expect(trigger.querySelector(".brand-fallback")).toHaveTextContent("D");
    await user.click(trigger);
    const option = screen.getByRole("option", { name: "deepseek" });
    expect(option.querySelector('[data-provider-brand="deepseek"]')).toBeNull();
    expect(option.querySelector(".brand-fallback")).toHaveTextContent("D");
  });

  it("exposes quota configuration and its real apply action to onboarding", () => {
    renderPanel();

    expect(screen.getByRole("region", { name: "额度路由配置" }))
      .toHaveAttribute("data-onboarding-target", "route-config");
    expect(screen.getByRole("button", { name: "保存并应用" }))
      .toHaveAttribute("data-onboarding-target", "route-apply");
  });

  it("模型未选时禁止应用并在完成选择后恢复", async () => {
    const user = userEvent.setup();
    const { onSave } = renderQuotaPanel({
      accounts: [{ upstream: "deepseek", model: "" }],
    });

    const apply = screen.getByRole("button", { name: "保存并应用" });
    const model = screen.getByRole("combobox", { name: "账户 1 模型" });
    const row = model.closest(".quota-entry-row");
    const warning = screen.getByText("请选择模型。");
    expect(apply).toBeDisabled();
    expect(row).toHaveClass("quota-entry-row-top-aligned");
    expect(model).toHaveAttribute("aria-invalid", "true");
    expect(model).toHaveAttribute("aria-describedby", warning.id);
    await user.click(apply);
    expect(onSave).not.toHaveBeenCalled();

    await user.click(model);
    await user.click(screen.getByRole("option", { name: "deepseek-v4-flash" }));
    expect(screen.queryByText("请选择模型。")).not.toBeInTheDocument();
    expect(apply).toBeEnabled();
    await user.click(apply);
    expect(onSave).toHaveBeenCalledWith([
      { upstream: "deepseek", model: "deepseek-v4-flash" },
    ]);
  });

  it("uses the shadcn controls for quota plan rows instead of native selects", () => {
    const { planSection } = renderPanel();

    expect(planSection.querySelector("select")).not.toBeInTheDocument();
    expect(planSection.querySelectorAll('[data-slot="select-trigger"]')).toHaveLength(2);
    expect(planSection.querySelectorAll('[data-slot="input"]')).toHaveLength(2);
    expect(planSection.querySelectorAll('[data-slot="separator"]')).toHaveLength(1);
  });

  it("separates the provider name from a responsive quota control group", () => {
    const { planSection } = renderPanel();
    const row = planSection.querySelector(".quota-plan-row");
    const controls = planSection.querySelector(".quota-plan-controls");

    expect(row).toBeInTheDocument();
    expect(within(row as HTMLElement).getByText("deepseek")).toHaveClass("quota-plan-name");
    expect(controls).toBeInTheDocument();
    expect(row?.querySelectorAll(".quota-plan-field")).toHaveLength(4);
  });

  it("saves all windows and rate limits only after an explicit save", async () => {
    const user = userEvent.setup();
    const { onSavePlan, planSection } = renderPanel({ providers: [{ ...providers[0], quota_plan: {
      len_ms: fiveHoursMs, limit: 5000, unit: "tokens", rate_limit_per_min: 60,
      windows: [{ len_ms: fiveHoursMs, limit: 5000 }, { len_ms: oneDayMs, limit: 24000 }],
    } }] });
    const limit = within(planSection).getByRole("spinbutton", { name: "deepseek 窗口 1 额度上限" });
    await user.clear(limit);
    await user.type(limit, "8000");
    await user.tab();
    expect(onSavePlan).not.toHaveBeenCalled();
    await user.click(within(planSection).getByRole("button", { name: "保存 deepseek 额度计划" }));
    expect(onSavePlan).toHaveBeenCalledWith("deepseek", {
      windows: [{ len_ms: fiveHoursMs, limit: 8000 }, { len_ms: oneDayMs, limit: 24000 }],
      unit: "tokens", rate_limit_per_min: 60,
    });
  });

  it("blocks invalid limits and duplicate windows and supports clearing", async () => {
    const user = userEvent.setup();
    const { onSavePlan, planSection } = renderPanel();
    const save = within(planSection).getByRole("button", { name: "保存 deepseek 额度计划" });
    const limit = within(planSection).getByRole("spinbutton", { name: "deepseek 窗口 1 额度上限" });
    await user.clear(limit);
    await user.type(limit, "1.5");
    expect(save).toBeDisabled();
    expect(onSavePlan).not.toHaveBeenCalled();
    await user.click(within(planSection).getByRole("button", { name: "移除 deepseek 窗口 1" }));
    expect(save).toBeEnabled();
    await user.click(save);
    expect(onSavePlan).toHaveBeenCalledWith("deepseek", { windows: [], unit: "tokens", rate_limit_per_min: null });
  });

  it("adds and saves a custom window without changing the first window", async () => {
    const user = userEvent.setup();
    const { onSavePlan, planSection } = renderPanel();
    await user.click(within(planSection).getByRole("button", { name: "添加 deepseek 窗口" }));
    await user.click(within(planSection).getByRole("combobox", { name: "deepseek 窗口 2 刷新周期" }));
    await user.click(screen.getByRole("option", { name: "自定义" }));
    const duration = within(planSection).getByRole("spinbutton", { name: "deepseek 窗口 2 自定义周期（毫秒）" });
    await user.clear(duration);
    await user.type(duration, "7200000");
    await user.type(within(planSection).getByRole("spinbutton", { name: "deepseek 窗口 2 额度上限" }), "9000");
    await user.click(within(planSection).getByRole("button", { name: "保存 deepseek 额度计划" }));
    expect(onSavePlan).toHaveBeenCalledWith("deepseek", {
      windows: [{ len_ms: fiveHoursMs, limit: 5000 }, { len_ms: 7200000, limit: 9000 }],
      unit: "tokens", rate_limit_per_min: null,
    });
  });


  it("rejects duplicate periods and invalid RPM without saving", async () => {
    const user = userEvent.setup();
    const { onSavePlan, planSection } = renderPanel();
    const save = within(planSection).getByRole("button", { name: "保存 deepseek 额度计划" });
    await user.click(within(planSection).getByRole("button", { name: "添加 deepseek 窗口" }));
    await user.type(within(planSection).getByRole("spinbutton", { name: "deepseek 窗口 2 额度上限" }), "9000");
    await user.click(within(planSection).getByRole("combobox", { name: "deepseek 窗口 2 刷新周期" }));
    await user.click(screen.getByRole("option", { name: "5 小时" }));
    expect(save).toBeDisabled();
    await user.click(within(planSection).getByRole("button", { name: "移除 deepseek 窗口 2" }));
    await user.type(within(planSection).getByRole("spinbutton", { name: "deepseek RPM" }), "0");
    expect(save).toBeDisabled();
    expect(onSavePlan).not.toHaveBeenCalled();
  });

  it("retains a saved custom period and edits RPM on explicit save", async () => {
    const user = userEvent.setup();
    const { onSavePlan, planSection } = renderPanel({ providers: [{ ...providers[0], quota_plan: {
      len_ms: 1234567, limit: 5000, unit: "requests", rate_limit_per_min: 90,
      windows: [{ len_ms: 1234567, limit: 5000 }],
    } }] });
    expect(within(planSection).getByRole("spinbutton", { name: "deepseek 窗口 1 自定义周期（毫秒）" })).toHaveValue(1234567);
    const rate = within(planSection).getByRole("spinbutton", { name: "deepseek RPM" });
    await user.clear(rate);
    await user.type(rate, "120");
    await user.click(within(planSection).getByRole("button", { name: "保存 deepseek 额度计划" }));
    expect(onSavePlan).toHaveBeenCalledWith("deepseek", {
      windows: [{ len_ms: 1234567, limit: 5000 }], unit: "requests", rate_limit_per_min: 120,
    });
  });


  it("keeps edits and shows failure when a plan save fails", async () => {
    const user = userEvent.setup();
    const onSavePlan = vi.fn().mockResolvedValue(false);
    const { planSection } = renderPanel({ onSavePlan });
    const limit = within(planSection).getByRole("spinbutton", { name: "deepseek 窗口 1 额度上限" });
    await user.clear(limit);
    await user.type(limit, "8000");
    await user.click(within(planSection).getByRole("button", { name: "保存 deepseek 额度计划" }));
    expect(await within(planSection).findByText("保存失败，已保留编辑内容。")).toBeInTheDocument();
    expect(limit).toHaveValue(8000);
  });

  it("excludes subscription providers from quota rotation", async () => {
    const user = userEvent.setup();
    renderQuotaPanel({ providers: [...providers, { ...providers[0], name: "codex", provider: "codex-plan", credential_source: "codex_account" }] });
    await user.click(screen.getByRole("combobox", { name: "账户 1 供应商" }));
    expect(screen.queryByRole("option", { name: "codex" })).not.toBeInTheDocument();
  });
});
