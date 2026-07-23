import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getAgentBudgets,
  getStats,
  listAgentRegistry,
  removeAgentBudget,
  setAgentBudget,
} from "../api";
import Stats from "./Stats";

vi.mock("../components/PricingEditor", () => ({ default: () => null }));

vi.mock("../api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("../api")>();
  return {
    ...original,
    getAgentBudgets: vi.fn(),
    getStats: vi.fn(),
    listAgentRegistry: vi.fn(),
    removeAgentBudget: vi.fn(),
    setAgentBudget: vi.fn(),
  };
});

const approaching = {
  agent_id: "codex",
  limit_micros: 10_000_000,
  used_micros: 8_500_000,
  remaining_micros: 1_500_000,
  warning_percent: 80,
  usage_percent: 85,
  unpriced_requests: 0,
  period_start_ms: null,
  period_end_ms: 1_800_000_000_000,
  expiry_warning_days: 7,
  usage_level: "approaching" as const,
  expiry_level: "active" as const,
  enforcement: "observe_only" as const,
  routing_affected: false as const,
};

beforeEach(() => {
  vi.mocked(getStats).mockReset().mockResolvedValue({
    total: {
      requests: 0,
      errors: 0,
      p50_latency_ms: 0,
      p95_latency_ms: 0,
      input_tokens: 0,
      output_tokens: 0,
      cost_micros: null,
      priced_requests: 0,
      unpriced_requests: 0,
    },
    groups: [],
    by: null,
    empty: true,
  });
  vi.mocked(listAgentRegistry).mockReset().mockResolvedValue([
    {
      agent_id: "codex",
      legacy_kind: null,
      display_name: "Codex",
      icon_key: "codex",
      admission: "supported",
    },
    {
      agent_id: "future-agent",
      legacy_kind: null,
      display_name: "Future Agent",
      icon_key: "future",
      admission: "discovery_only",
    },
  ]);
  vi.mocked(getAgentBudgets).mockReset().mockResolvedValue([approaching]);
  vi.mocked(setAgentBudget).mockReset().mockResolvedValue([approaching]);
  vi.mocked(removeAgentBudget).mockReset().mockResolvedValue([]);
});

describe("display-only Agent budgets", () => {
  it("filters usage by exact Agent and inbound protocol source", async () => {
    const user = userEvent.setup();
    render(<Stats />);
    expect(await screen.findAllByRole("option", { name: "Codex" })).toHaveLength(2);

    await user.selectOptions(screen.getByRole("combobox", { name: "Agent 过滤" }), "codex");
    await user.selectOptions(screen.getByRole("combobox", { name: "来源过滤" }), "openai-responses");

    await waitFor(() => expect(getStats).toHaveBeenLastCalledWith(
      "all",
      null,
      "codex",
      "openai-responses",
    ));
  });

  it("shows approaching usage as a warning and states that routing is unaffected", async () => {
    render(<Stats />);

    expect(await screen.findByText(/Codex 已使用 85\.0%/)).toBeInTheDocument();
    expect(screen.getByText(/仅提醒，不影响路由/)).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Future Agent" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("option", { name: "Codex" })).toHaveLength(2);
  });

  it("renders exceeded and expired states without turning them into enforcement", async () => {
    vi.mocked(getAgentBudgets).mockResolvedValueOnce([{
      ...approaching,
      used_micros: 11_000_000,
      remaining_micros: 0,
      usage_percent: 110,
      usage_level: "exceeded",
      expiry_level: "expired",
    }]);

    render(<Stats />);

    expect(await screen.findByText(/Codex 已使用 110\.0%，已超过预算上限/)).toBeInTheDocument();
    expect(screen.getByText(/预算周期已到期/)).toBeInTheDocument();
    expect(screen.getByText(/仅提醒，不影响路由/)).toBeInTheDocument();
  });

  it("persists and removes a per-Agent budget through exact named fields", async () => {
    const user = userEvent.setup();
    render(<Stats />);
    await screen.findByText(/Codex 已使用 85\.0%/);

    const limit = screen.getByRole("spinbutton", { name: "预算上限" });
    await user.clear(limit);
    await user.type(limit, "12.5");
    await user.click(screen.getByRole("button", { name: "保存预算" }));

    await waitFor(() => expect(setAgentBudget).toHaveBeenCalledWith(
      "codex",
      12_500_000,
      80,
      null,
      1_800_000_000_000,
      7,
    ));

    await user.click(screen.getByRole("button", { name: "删除预算" }));
    await waitFor(() => expect(removeAgentBudget).toHaveBeenCalledWith("codex"));
  });
});
