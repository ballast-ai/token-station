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
import Stats, { formatBudgetAmount } from "./Stats";

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

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, reject, resolve };
}

function statsView(by: string | null, total = aggregate) {
  return {
    total,
    groups: by === "upstream"
      ? [["openai", total] as [string, typeof total]]
      : by === "model"
        ? [["gpt-5", total] as [string, typeof total]]
        : by === "hour" || by === "day"
          ? [[String(Date.now()), total] as [string, typeof total]]
          : [["codex", total] as [string, typeof total]],
    by,
    empty: false,
  };
}

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
  it("keeps meaningful decimals for small non-zero budget amounts", () => {
    expect(formatBudgetAmount(1)).toBe("0.000001");
    expect(formatBudgetAmount(1_000)).toBe("0.001");
    expect(formatBudgetAmount(10_000)).toBe("0.01");
    expect(formatBudgetAmount(0)).toBe("0.00");
  });

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

  it("commits a slow refresh result before running one coalesced follow-up", async () => {
    const { unmount } = render(<Stats />);
    await screen.findByText("1,500");

    vi.useFakeTimers();
    try {
      fireEvent.click(screen.getByRole("combobox", { name: "自动刷新" }));
      fireEvent.click(within(screen.getByRole("listbox")).getByRole("option", { name: "30 秒" }));

      const callsBeforeRefresh = vi.mocked(getStats).mock.calls.length;
      const pending: Array<{
        by: string | null;
        request: ReturnType<typeof deferred<ReturnType<typeof statsView>>>;
      }> = [];
      vi.mocked(getStats).mockImplementation((_since, by) => {
        const request = deferred<ReturnType<typeof statsView>>();
        pending.push({ by, request });
        return request.promise;
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(getStats).toHaveBeenCalledTimes(callsBeforeRefresh + 4);

      const slowTotal = { ...aggregate, input_tokens: 2_000, output_tokens: 333 };
      await act(async () => {
        for (const { by, request } of pending.slice(0, 4)) {
          request.resolve(statsView(by, slowTotal));
        }
        await Promise.all(pending.slice(0, 4).map(({ request }) => request.promise));
        await Promise.resolve();
      });

      expect(screen.getAllByTitle("2,333").length).toBeGreaterThan(0);
      expect(getStats).toHaveBeenCalledTimes(callsBeforeRefresh + 8);
    } finally {
      unmount();
      vi.useRealTimers();
    }
  });

  it("keeps a failed request batch in flight until every sibling settles", async () => {
    render(<Stats />);
    await screen.findByText("1,500");

    const siblings = Array.from(
      { length: 3 },
      () => deferred<ReturnType<typeof statsView>>(),
    );
    let call = 0;
    vi.mocked(getStats).mockImplementation((_since, by) => {
      if (call++ === 0) return Promise.reject(new Error("stats failed"));
      return siblings[call - 2].promise.then(() => statsView(by));
    });

    const refresh = screen.getByRole("button", { name: "刷新用量" });
    fireEvent.click(refresh);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(refresh).toBeDisabled();

    await act(async () => {
      for (const sibling of siblings) sibling.resolve(statsView("agent"));
      await Promise.all(siblings.map(({ promise }) => promise));
    });
    await waitFor(() => expect(refresh).not.toBeDisabled());
    expect(screen.getByText("操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。")).toBeInTheDocument();
  });

  it("does not start a queued refresh after the page unmounts", async () => {
    const { unmount } = render(<Stats />);
    await screen.findByText("1,500");

    vi.useFakeTimers();
    try {
      fireEvent.click(screen.getByRole("combobox", { name: "自动刷新" }));
      fireEvent.click(within(screen.getByRole("listbox")).getByRole("option", { name: "30 秒" }));

      const pending: Array<{
        by: string | null;
        request: ReturnType<typeof deferred<ReturnType<typeof statsView>>>;
      }> = [];
      vi.mocked(getStats).mockImplementation((_since, by) => {
        const request = deferred<ReturnType<typeof statsView>>();
        pending.push({ by, request });
        return request.promise;
      });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(pending).toHaveLength(4);

      unmount();
      await act(async () => {
        for (const { by, request } of pending) request.resolve(statsView(by));
        await Promise.all(pending.map(({ request }) => request.promise));
        await Promise.resolve();
      });

      expect(pending).toHaveLength(4);
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
