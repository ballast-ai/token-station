import { render, screen, waitFor, within } from "@testing-library/react";
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
    provider: "openai-compatible",
    base_url: "https://api.deepseek.com/v1",
    models: ["deepseek-v4-flash"],
    has_auth: true,
    quota_plan: {
      len_ms: fiveHoursMs,
      limit: 5000,
      unit: "tokens",
      rate_limit_per_min: null,
    },
  },
];

function renderPanel(overrides: Partial<ComponentProps<typeof QuotaPriorityPanel>> = {}) {
  const onSavePlan = vi.fn();
  render(
    <QuotaPriorityPanel
      providers={providers}
      accounts={[{ upstream: "deepseek", model: "deepseek-v4-flash" }]}
      busy={false}
      applying={false}
      onSave={vi.fn()}
      onViewUsage={vi.fn()}
      onSavePlan={onSavePlan}
      {...overrides}
    />,
  );
  const planSection = screen.getByText("额度计划(可选)").closest(".quota-plan-section") as HTMLElement | null;
  if (!planSection) throw new Error("quota plan section is missing");
  return { onSavePlan, planSection };
}

describe("QuotaPriorityPanel quota plans", () => {
  it("uses the shadcn controls for quota plan rows instead of native selects", () => {
    const { planSection } = renderPanel();

    expect(planSection.querySelector("select")).not.toBeInTheDocument();
    expect(planSection.querySelectorAll('[data-slot="select-trigger"]')).toHaveLength(2);
    expect(planSection.querySelectorAll('[data-slot="input"]')).toHaveLength(1);
    expect(planSection.querySelectorAll('[data-slot="separator"]')).toHaveLength(1);
  });

  it("separates the provider name from a responsive quota control group", () => {
    const { planSection } = renderPanel();
    const row = planSection.querySelector(".quota-plan-row");
    const controls = planSection.querySelector(".quota-plan-controls");

    expect(row).toBeInTheDocument();
    expect(within(row as HTMLElement).getByText("deepseek")).toHaveClass("quota-plan-name");
    expect(controls).toBeInTheDocument();
    expect(controls?.querySelectorAll(".quota-plan-field")).toHaveLength(3);
  });

  it("commits reset-window and unit changes with the existing quota plan parameters", async () => {
    const user = userEvent.setup();
    const { onSavePlan, planSection } = renderPanel();

    await user.click(within(planSection).getByRole("combobox", { name: "deepseek 刷新窗口" }));
    await user.click(await screen.findByRole("option", { name: "1 天" }));
    await waitFor(() => expect(onSavePlan).toHaveBeenLastCalledWith(
      "deepseek",
      oneDayMs,
      5000,
      "tokens",
    ));

    await user.click(within(planSection).getByRole("combobox", { name: "deepseek 单位" }));
    await user.click(await screen.findByRole("option", { name: "requests" }));
    await waitFor(() => expect(onSavePlan).toHaveBeenLastCalledWith(
      "deepseek",
      oneDayMs,
      5000,
      "requests",
    ));
  });

  it("keeps allowance edits local until blur, then commits the parsed limit", async () => {
    const user = userEvent.setup();
    const { onSavePlan, planSection } = renderPanel();
    const limit = within(planSection).getByRole("spinbutton", { name: "deepseek 额度上限" });

    await user.clear(limit);
    await user.type(limit, "24000");
    expect(onSavePlan).not.toHaveBeenCalled();

    await user.tab();
    expect(onSavePlan).toHaveBeenCalledWith("deepseek", fiveHoursMs, 24000, "tokens");
  });
});
