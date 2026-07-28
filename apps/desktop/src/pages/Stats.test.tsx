import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getAgentBudgets,
  getRequestReceipts,
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
    getRequestReceipts: vi.fn(),
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

const aggregate = {
  requests: 10,
  errors: 1,
  p50_latency_ms: 120,
  p95_latency_ms: 480,
  input_tokens: 1_000,
  output_tokens: 500,
  cache_read_tokens: 400,
  cache_write_tokens: 100,
  reasoning_tokens: 80,
  cost_micros: 1_250_000,
  priced_requests: 9,
  unpriced_requests: 1,
};

beforeEach(() => {
  vi.useRealTimers();
  vi.mocked(getStats).mockReset().mockImplementation(async (_since, by) => ({
    total: aggregate,
    groups: by === "upstream"
      ? [["openai", aggregate]]
      : by === "model"
        ? [["gpt-5", aggregate]]
        : by === "hour" || by === "day"
          ? [[String(Date.now()), aggregate]]
          : [["codex", aggregate]],
    by,
    empty: false,
  }));
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
  vi.mocked(getRequestReceipts).mockReset().mockResolvedValue({
    items: [],
    total: 0,
    page: 1,
    page_size: 20,
  });
  vi.mocked(setAgentBudget).mockReset().mockResolvedValue([approaching]);
  vi.mocked(removeAgentBudget).mockReset().mockResolvedValue([]);
});

describe("usage dashboard and display-only Agent budgets", () => {
  it("applies Agent, upstream, and model filters to the whole dashboard", async () => {
    const user = userEvent.setup();
    render(<Stats />);
    expect(await screen.findByRole("combobox", { name: "Agent 过滤" })).toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "Agent 过滤" }));
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: "Codex" }));
    await user.click(await screen.findByRole("combobox", { name: "供应商过滤" }));
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: "openai" }));
    await user.click(await screen.findByRole("combobox", { name: "模型过滤" }));
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: "gpt-5" }));

    await waitFor(() => expect(getStats).toHaveBeenCalledWith(
      "24h",
      "agent",
      "codex",
      null,
      "openai",
      "gpt-5",
    ));
  });

  it("keeps cache and reasoning as subsets instead of double-counting total tokens", async () => {
    render(<Stats />);

    expect(await screen.findByText("1,500")).toBeInTheDocument();
    expect(screen.getByText(/缓存读 400 · 缓存写 100 · 推理 80/)).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /用量趋势/ })).toBeInTheDocument();
  });

  it("renders an empty token rail without inventing a 50/50 split", async () => {
    vi.mocked(getStats).mockImplementation(async (_since, by) => ({
      total: { ...aggregate, input_tokens: 0, output_tokens: 0 },
      groups: by === "hour" || by === "day"
        ? [[String(Date.now()), { ...aggregate, input_tokens: 0, output_tokens: 0 }]]
        : [["codex", { ...aggregate, input_tokens: 0, output_tokens: 0 }]],
      by,
      empty: false,
    }));

    render(<Stats />);

    expect(await screen.findByLabelText("暂无 Token 数据")).toBeInTheDocument();
  });

  it("does not overlap automatic dashboard refreshes", async () => {
    render(<Stats />);
    await screen.findByRole("combobox", { name: "自动刷新" });

    vi.useFakeTimers();
    try {
      fireEvent.click(screen.getByRole("combobox", { name: "自动刷新" }));
      fireEvent.click(within(screen.getByRole("listbox")).getByRole("option", { name: "30 秒" }));

      const callsBeforeRefresh = vi.mocked(getStats).mock.calls.length;
      vi.mocked(getStats).mockImplementation(() => new Promise(() => undefined));

      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(getStats).toHaveBeenCalledTimes(callsBeforeRefresh + 4);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(getStats).toHaveBeenCalledTimes(callsBeforeRefresh + 4);
    } finally {
      vi.useRealTimers();
    }
  });

  it("switches the detail grouping without exposing a technical group-by select", async () => {
    const user = userEvent.setup();
    render(<Stats />);
    await screen.findByRole("tab", { name: "模型" });
    await user.click(screen.getByRole("tab", { name: "模型" }));

    await waitFor(() => expect(getStats).toHaveBeenCalledWith(
      "24h",
      "model",
      null,
      null,
      null,
      null,
    ));
  });

  it("shows approaching usage as a warning and states that routing is unaffected", async () => {
    const user = userEvent.setup();
    render(<Stats />);

    expect(await screen.findByText(/Codex 已使用 85\.0%/)).toBeInTheDocument();
    expect(screen.getByText(/仅提醒，不影响路由/)).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Future Agent" })).not.toBeInTheDocument();
    expect(screen.getAllByRole("option", { name: "Codex" })).toHaveLength(1);
    await user.click(screen.getByRole("combobox", { name: "Agent 过滤" }));
    expect(within(screen.getByRole("listbox")).getByRole("option", { name: "Codex" })).toBeInTheDocument();
    expect(within(screen.getByRole("listbox")).queryByRole("option", { name: "Future Agent" })).not.toBeInTheDocument();
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
