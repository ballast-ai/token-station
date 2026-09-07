import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { getAgentBudgets, listAgentRegistry, setAgentBudget, type BudgetStatus } from "../api";
import { DraftNavigationBoundary } from "../components/DraftNavigation";
import BudgetPricingPage from "./BudgetPricingPage";
import { ErrorToastProvider } from "../components/ErrorToast";
vi.mock("../components/PricingEditor", () => ({ default: () => null }));
vi.mock("../api", async (loadOriginal) => ({
  ...await loadOriginal<typeof import("../api")>(),
  getAgentBudgets: vi.fn(), listAgentRegistry: vi.fn(), setAgentBudget: vi.fn(),
}));
it("retains Agent B draft when an earlier Agent A write completes", async () => {
  const initial: BudgetStatus[] = ["codex", "claude-code"].map((agent_id, index) => ({
    agent_id, limit_micros: index ? 50_000_000 : 10_000_000,
    used_micros: 0, remaining_micros: index ? 50_000_000 : 10_000_000,
    warning_percent: 80, usage_percent: 0, unpriced_requests: 0,
    period_start_ms: null, period_end_ms: null, expiry_warning_days: 7,
    usage_level: "healthy", expiry_level: "active", enforcement: "observe_only", routing_affected: false,
  }));
  vi.mocked(listAgentRegistry).mockResolvedValue(initial.map((item, index) => ({
    agent_id: item.agent_id, legacy_kind: null, display_name: index ? "Claude Code" : "Codex",
    icon_key: item.agent_id, admission: "supported",
  })));
  vi.mocked(getAgentBudgets).mockResolvedValue(initial);
  let resolveSave!: (value: BudgetStatus[]) => void;
  const pendingSave = new Promise<BudgetStatus[]>((resolve) => { resolveSave = resolve; });
  const afterSave = initial.map((item) => item.agent_id === "codex" ? { ...item, limit_micros: 25_000_000 } : item);
  vi.mocked(setAgentBudget).mockReturnValueOnce(pendingSave).mockResolvedValue(afterSave);
  const user = userEvent.setup();
  render(<ErrorToastProvider><BudgetPricingPage onBack={vi.fn()} /></ErrorToastProvider>);
  const limit = screen.getByRole("spinbutton", { name: "预算上限" });
  await waitFor(() => expect(screen.getByRole("combobox", { name: "Agent" })).toHaveTextContent("Codex"));
  await user.clear(limit);
  await user.type(limit, "25");
  await user.click(screen.getByRole("button", { name: "保存预算" }));
  expect(setAgentBudget).toHaveBeenNthCalledWith(1, "codex", 25_000_000, 80, null, null, 7);
  await user.click(screen.getByRole("combobox", { name: "Agent" }));
  await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: "Claude Code" }));
  expect(limit).toHaveValue(50);
  await act(async () => { resolveSave(afterSave); await pendingSave; });
  expect(screen.getByRole("combobox", { name: "Agent" })).toHaveTextContent("Claude Code");
  expect(limit).toHaveValue(50);
  await user.click(screen.getByRole("button", { name: "保存预算" }));
  expect(setAgentBudget).toHaveBeenNthCalledWith(2, "claude-code", 50_000_000, 80, null, null, 7);
});

it("confirms leaving edits made after a pending budget submission", async () => {
  const initial: BudgetStatus[] = ["codex", "claude-code"].map((agent_id, index) => ({
    agent_id, limit_micros: index ? 50_000_000 : 10_000_000,
    used_micros: 0, remaining_micros: index ? 50_000_000 : 10_000_000,
    warning_percent: 80, usage_percent: 0, unpriced_requests: 0,
    period_start_ms: null, period_end_ms: null, expiry_warning_days: 7,
    usage_level: "healthy", expiry_level: "active", enforcement: "observe_only", routing_affected: false,
  }));
  vi.mocked(listAgentRegistry).mockResolvedValue(initial.map((item, index) => ({
    agent_id: item.agent_id, legacy_kind: null, display_name: index ? "Claude Code" : "Codex",
    icon_key: item.agent_id, admission: "supported",
  })));
  vi.mocked(getAgentBudgets).mockResolvedValue(initial);
  let resolveSave!: (value: BudgetStatus[]) => void;
  const pendingSave = new Promise<BudgetStatus[]>((resolve) => { resolveSave = resolve; });
  const afterSave = initial.map((item) => item.agent_id === "codex" ? { ...item, limit_micros: 25_000_000 } : item);
  vi.mocked(setAgentBudget).mockReset().mockReturnValueOnce(pendingSave).mockResolvedValue(afterSave);
  const user = userEvent.setup();
  render(<ErrorToastProvider><DraftNavigationBoundary><BudgetPricingPage onBack={vi.fn()} /></DraftNavigationBoundary></ErrorToastProvider>);
  const limit = screen.getByRole("spinbutton", { name: "预算上限" });
  await waitFor(() => expect(screen.getByRole("combobox", { name: "Agent" })).toHaveTextContent("Codex"));
  await user.clear(limit); await user.type(limit, "25");
  await user.click(screen.getByRole("button", { name: "保存预算" }));
  await user.clear(limit); await user.type(limit, "35");
  const chooseOther = async () => {
    await user.click(screen.getByRole("combobox", { name: "Agent" }));
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: "Claude Code" }));
  };
  await chooseOther();
  await user.click(screen.getByRole("button", { name: "继续编辑" }));
  expect(limit).toHaveValue(35);
  expect(screen.getByRole("combobox", { name: "Agent" })).toHaveTextContent("Codex");
  await chooseOther();
  await user.click(screen.getByRole("button", { name: "放弃更改并离开" }));
  expect(limit).toHaveValue(50);
  await act(async () => { resolveSave(afterSave); await pendingSave; });
  expect(limit).toHaveValue(50);
});
