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
import BudgetPricingPage from "./BudgetPricingPage";
import { ErrorToastProvider } from "../components/ErrorToast";
import UsageRequestLog from "../components/UsageRequestLog";
import type { ReceiptView } from "../api";

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
  legacy_input_requests: 0,
  output_tokens: 500,
  cache_read_tokens: 400,
  cache_write_tokens: 100,
  cache_read_reported_requests: 10,
  cache_write_reported_requests: 10,
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
  it("keeps complete successful totals while explaining failed requests without usage separately", async () => {
    const value = { ...aggregate, requests: 184, errors: 96, usage_expected_requests: 85,
      failed_without_usage_requests: 99, unpriced_failed_without_usage_requests: 99,
      input_tokens: 6942955, output_tokens: 42628, cache_read_tokens: 5321280, cache_write_tokens: 0,
      input_reported_requests: 85, output_reported_requests: 85, total_reported_requests: 85,
      cache_read_reported_requests: 85, cache_write_reported_requests: 0, cache_write_unrecorded_requests: 85,
      priced_requests: 0, unpriced_requests: 184, missing_price_requests: 85, missing_usage_requests: 99, cost_micros: null };
    vi.mocked(getStats).mockImplementation(async (_since, by) => ({ ...statsView(by), total: value, groups: [["p", value]] }));
    const { container } = render(<Stats />);
    const overview = await screen.findByLabelText("用量总览");
    expect(overview.querySelector(".usage-primary-metric strong")).toHaveTextContent(/^6,985,583$/);
    expect(overview.querySelector(".usage-primary-metric small")).not.toHaveTextContent("部分上报");
    expect(container.querySelector(".usage-unpriced-note")).toHaveTextContent("85 次缺少价格 · 0 次用量不完整 · 99 次失败/取消未返回用量");
    expect(screen.getByRole("table").querySelector('td[title="6,985,583"]')).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Token 构成" })).toHaveTextContent("历史未记录");
  });
  it("shows partial totals and absent output in the overview, composition, and contribution rows", async () => {
    const value = { ...aggregate, output_tokens: 0, input_reported_requests: 10,
      output_reported_requests: 0, total_reported_requests: 0, incomplete_usage_requests: 1 };
    vi.mocked(getStats).mockImplementation(async (_since, by) => statsView(by, value));
    render(<Stats />);
    const overview = await screen.findByLabelText("用量总览");
    expect(overview.querySelector(".usage-primary-metric strong")).toHaveTextContent(/^1,000$/);
    expect(overview.querySelector(".usage-primary-metric small")).toHaveTextContent("部分上报");
    const composition = screen.getByRole("group", { name: "Token 构成" });
    expect(within(composition).getByText("输出").parentElement).toHaveTextContent("未上报");
    expect(within(composition).getByLabelText("用量不完整，无法计算占比")).toBeInTheDocument();
    expect(screen.getByRole("table").querySelector('td[title="1,000（部分上报）"]')).toBeInTheDocument();
  });

  it("labels only known contribution cost as partial and explains both missing categories", async () => {
    const unknown = { ...aggregate, cost_micros: null, priced_requests: 0, unpriced_requests: 10,
      missing_price_requests: 4, missing_usage_requests: 6 };
    const partial = { ...unknown, cost_micros: 1, priced_requests: 1, unpriced_requests: 9, missing_price_requests: 3 };
    vi.mocked(getStats).mockImplementation(async (_since, by) => ({
      ...statsView(by), total: unknown, groups: [["unknown", unknown], ["partial", partial]],
    }));
    render(<Stats />);
    const table = await screen.findByRole("table");
    const missing = within(table).getByTitle("4 次缺少价格 · 6 次用量不完整");
    expect(missing).toHaveTextContent(/^未知$/);
    expect(within(table).getByTitle("3 次缺少价格 · 6 次用量不完整")).toHaveTextContent("$0.000001（部分）");
  });

  it("updates receipt log and detail coverage from missing output to reported zero", async () => {
    const user = userEvent.setup();
    const receipt: ReceiptView = {
      request_id: "coverage-receipt", started_at_ms: Date.now(), latency_ms: 10,
      protocol: "anthropic", requested_model: "m", stream: true, status: 502,
      error_code: null, attempts: 1, routing: null, cost_kind: "unknown", cost_micros: null,
      price_version: null, agent_id: null, running_revision: null, decision: null,
      attempt_records: [], conversion_reports: [],
      usage: { input_tokens: 10, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, reasoning_tokens: 0 },
      usage_observation: { input_tokens: 10, output_tokens: null, incomplete: true },
    };
    const page = { items: [receipt], total: 1, page: 1, page_size: 20 };
    vi.mocked(getRequestReceipts).mockResolvedValue(page);
    const props = { since: "24h", agentId: "", upstream: "", model: "", refreshKey: 0 };
    const { rerender } = render(<UsageRequestLog {...props} />);
    const row = await screen.findByRole("button", { name: /打开请求详情 coverage-receipt/ });
    expect(row).toHaveTextContent("10（部分上报）");
    await user.click(row);
    const dialog = screen.getByRole("dialog");
    expect(within(dialog).getByText("输出").parentElement).toHaveTextContent("未上报");
    expect(within(dialog).getByText("输入").parentElement).toHaveTextContent("10（部分上报）");
    vi.mocked(getRequestReceipts).mockResolvedValue({ ...page, items: [{ ...receipt,
      usage_observation: { input_tokens: 10, output_tokens: 0, cache_write_tokens: 0 } }] });
    rerender(<UsageRequestLog {...props} since="7d" />);
    await waitFor(() => expect(within(screen.getByRole("dialog")).getByText("输出").parentElement).toHaveTextContent("输出 0"));
    expect(within(screen.getByRole("dialog")).getByText("缓存写").parentElement).toHaveTextContent("缓存写 0");
    expect(within(screen.getByRole("dialog")).getByText("缓存读").parentElement).toHaveTextContent("未上报");
  });
  it("keeps meaningful decimals for small non-zero budget amounts", () => {
    expect(formatBudgetAmount(1)).toBe("0.000001");
    expect(formatBudgetAmount(1_000)).toBe("0.001");
    expect(formatBudgetAmount(10_000)).toBe("0.01");
    expect(formatBudgetAmount(0)).toBe("0.00");
  });

  it("labels unscoped Agent traffic as model tests", async () => {
    vi.mocked(getStats).mockImplementation(async (_since, by) => ({
      total: aggregate,
      groups: by === "upstream"
        ? [["openai", aggregate]]
        : by === "model"
          ? [["gpt-5", aggregate]]
          : by === "hour" || by === "day"
            ? [[String(Date.now()), aggregate]]
            : [["(unrouted)", aggregate]],
      by,
      empty: false,
    }));

    render(<Stats />);

    expect(await screen.findByText("模型测试")).toBeInTheDocument();
    expect(screen.queryByText("(unrouted)")).toBeNull();
  });

  it("applies Agent, upstream, and model filters to the whole dashboard", async () => {
    const user = userEvent.setup();
    render(<Stats />);
    await screen.findByText(/Codex 已使用 85\.0%/);
    expect(screen.queryByRole("combobox", { name: "Agent 过滤" })).toBeNull();
    const filters = screen.getByRole("button", { name: "筛选" });
    expect(filters).toHaveAttribute("aria-expanded", "false");
    await user.click(filters);
    expect(await screen.findByRole("combobox", { name: "Agent 过滤" })).toBeInTheDocument();
    expect(filters).toHaveAttribute("aria-expanded", "true");

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

    await user.click(filters);
    expect(screen.queryByRole("combobox", { name: "模型过滤" })).toBeNull();
    await user.click(filters);
    expect(screen.getByRole("combobox", { name: "模型过滤" })).toHaveAttribute("title", "gpt-5");
  });

  it("keeps cache and reasoning as subsets instead of double-counting total tokens", async () => {
    render(<Stats />);

    expect(await screen.findByText("1,500")).toBeInTheDocument();
    const composition = screen.getByRole("group", { name: "Token 构成" });
    expect(within(composition).getByText("输入总量")).toBeInTheDocument();
    expect(within(composition).getByText("输出")).toBeInTheDocument();
    expect(within(composition).getByText("缓存读")).toBeInTheDocument();
    expect(within(composition).getByText("缓存写")).toBeInTheDocument();
    expect(within(composition).getByText("推理")).toBeInTheDocument();
    expect(within(composition).getByText("缓存复用率")).toBeInTheDocument();
    expect(within(composition).getByText("40.0%")).toBeInTheDocument();
    expect(within(composition).getByText("总 Token = 输入 + 输出；缓存和推理为子项，不重复计数。"))
      .toBeInTheDocument();
    expect(within(composition).getByLabelText("输入占 66.7%，输出占 33.3%"))
      .toBeInTheDocument();
    expect(screen.getByRole("img", { name: /用量趋势/ })).toBeInTheDocument();
  });

  it("labels mixed history as provider-reported instead of claiming canonical totals", async () => {
    const legacyAggregate = { ...aggregate, legacy_input_requests: 2 };
    vi.mocked(getStats).mockImplementation(async (_since, by) => statsView(by, legacyAggregate));

    render(<Stats />);

    expect(within(await screen.findByLabelText("用量总览")).getByText("上报 Token")).toBeInTheDocument();
    const composition = screen.getByRole("group", { name: "Token 构成" });
    expect(within(composition).getByText("上游输入")).toBeInTheDocument();
    expect(within(composition).getByText("2 个历史请求使用上游原始输入")).toBeInTheDocument();
    expect(within(composition).getByText(/历史输入可能不包含缓存子项/)).toBeInTheDocument();
  });

  it("preserves the original trend, overview, composition order and compact coverage", async () => {
    const value = { ...aggregate, requests: 413, priced_requests: 8, unpriced_requests: 405,
      missing_price_requests: 227, missing_usage_requests: 178 };
    vi.mocked(getStats).mockImplementation(async (_since, by) => statsView(by, value));
    render(<Stats />);

    const trend = (await screen.findByRole("img", { name: /用量趋势/ })).closest("section");
    const overview = screen.getByLabelText("用量总览");
    const composition = screen.getByRole("group", { name: "Token 构成" }).closest("section");
    const details = screen.getByRole("heading", { name: "贡献明细" }).closest("section");

    expect(trend).not.toBeNull();
    expect(composition).not.toBeNull();
    expect(details).not.toBeNull();
    expect(trend!.compareDocumentPosition(overview) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(overview.compareDocumentPosition(composition!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(composition!.compareDocumentPosition(details!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(document.querySelector(".usage-cost-coverage")).toBeNull();
    expect(composition!.querySelector(".usage-unpriced-note")).toHaveTextContent("8/413 次请求已计价 · 227 次缺少价格 · 178 次用量不完整");
    expect(trend!.querySelector(".usage-dual-axis-note")).toHaveTextContent("左轴 Token · 右轴成本");
    expect(trend!.querySelectorAll(".usage-chart-legend > span")).toHaveLength(5);
    expect(within(trend!).queryByRole("radiogroup")).toBeNull();
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

  it("shows an explicitly reported zero cache-write aggregate", async () => {
    vi.mocked(getStats).mockImplementation(async (_since, by) => {
      const withoutCacheWrites = { ...aggregate, cache_write_tokens: 0 };
      return statsView(by, withoutCacheWrites);
    });

    render(<Stats />);

    const composition = await screen.findByRole("group", { name: "Token 构成" });
    const cacheWrite = within(composition).getByText("缓存写").closest("div");
    expect(cacheWrite).toHaveTextContent("0");
    expect(cacheWrite).not.toHaveAttribute("title");
  });

  it("exposes refresh in the collapsed landing row and keeps current data while refreshing", async () => {
    const user = userEvent.setup();
    render(<Stats />);

    await screen.findByText("1,500");
    expect(screen.getByRole("button", { name: "筛选" })).toHaveAttribute("aria-expanded", "false");
    const filter = screen.getByRole("button", { name: "筛选" });
    const refresh = screen.getByRole("button", { name: "刷新用量" });
    expect(filter.parentElement).toBe(refresh.parentElement);
    expect(filter.compareDocumentPosition(refresh) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(refresh).toHaveTextContent("刷新");

    const pending: Array<{
      by: string | null;
      request: ReturnType<typeof deferred<ReturnType<typeof statsView>>>;
    }> = [];
    vi.mocked(getStats).mockImplementation((_since, by) => {
      const request = deferred<ReturnType<typeof statsView>>();
      pending.push({ by, request });
      return request.promise;
    });

    await user.click(refresh);
    expect(refresh).toBeDisabled();
    expect(refresh).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("1,500")).toBeInTheDocument();

    const refreshedTotal = { ...aggregate, input_tokens: 2_000, output_tokens: 333 };
    await act(async () => {
      for (const { by, request } of pending) request.resolve(statsView(by, refreshedTotal));
      await Promise.all(pending.map(({ request }) => request.promise));
    });

    await waitFor(() => expect(refresh).not.toBeDisabled());
    expect(refresh).toHaveAttribute("aria-busy", "false");
    expect(screen.getAllByTitle("2,333").length).toBeGreaterThan(0);
  });

  it("does not overlap automatic dashboard refreshes", async () => {
    render(<Stats />);
    fireEvent.click(await screen.findByRole("button", { name: "筛选" }));
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
    fireEvent.click(screen.getByRole("button", { name: "筛选" }));

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
    render(<ErrorToastProvider><Stats /></ErrorToastProvider>);
    await screen.findByText("1,500");
    fireEvent.click(screen.getByRole("button", { name: "筛选" }));

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
    expect(within(screen.getByTestId("error-toast-viewport")).getByText(
      "操作失败：stats failed",
    )).toBeInTheDocument();
  });

  it("does not start a queued refresh after the page unmounts", async () => {
    const { unmount } = render(<Stats />);
    await screen.findByText("1,500");
    fireEvent.click(screen.getByRole("button", { name: "筛选" }));

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

  it("switches from engine to fallback with tab arrow-key navigation", async () => {
    const user = userEvent.setup();
    render(<Stats />);
    const engine = await screen.findByRole("tab", { name: "引擎" });
    const fallback = screen.getByRole("tab", { name: "回退原因" });

    await user.click(engine);
    await waitFor(() => expect(getStats).toHaveBeenCalledWith(
      "24h",
      "engine",
      null,
      null,
      null,
      null,
    ));

    engine.focus();
    await user.keyboard("{ArrowRight}");

    expect(fallback).toHaveFocus();
    expect(fallback).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(getStats).toHaveBeenCalledWith(
      "24h",
      "fallback",
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
    await user.click(screen.getByRole("button", { name: "筛选" }));
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
    render(<ErrorToastProvider><BudgetPricingPage onBack={vi.fn()} /></ErrorToastProvider>);
    await screen.findByRole("heading", { name: "Agent 预算预警" });

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
    const viewport = screen.getByTestId("error-toast-viewport");
    expect(await within(viewport).findByText("预算已保存 · 仅用于展示与预警"))
      .toBeInTheDocument();
    expect(screen.queryByText("预算已保存 · 仅用于展示与预警", { selector: ".banner" }))
      .toBeNull();

    await user.click(screen.getByRole("button", { name: "删除预算" }));
    await waitFor(() => expect(removeAgentBudget).toHaveBeenCalledWith("codex"));
    expect(await within(viewport).findByText("预算已删除")).toBeInTheDocument();
    expect(screen.queryByText("预算已删除", { selector: ".banner" })).toBeNull();
  });

  it("uses the shared Select component and loads the selected Agent budget", async () => {
    vi.mocked(listAgentRegistry).mockResolvedValueOnce([
      {
        agent_id: "codex",
        legacy_kind: null,
        display_name: "Codex",
        icon_key: "codex",
        admission: "supported",
      },
      {
        agent_id: "claude-code",
        legacy_kind: null,
        display_name: "Claude Code",
        icon_key: "claude-code",
        admission: "supported",
      },
    ]);
    vi.mocked(getAgentBudgets).mockResolvedValueOnce([
      approaching,
      {
        ...approaching,
        agent_id: "claude-code",
        limit_micros: 25_000_000,
        warning_percent: 70,
      },
    ]);
    const user = userEvent.setup();
    render(<ErrorToastProvider><BudgetPricingPage onBack={vi.fn()} /></ErrorToastProvider>);

    const trigger = await screen.findByRole("combobox", { name: "Agent" });
    expect(trigger).toHaveAttribute("data-slot", "select-trigger");
    await user.click(trigger);
    await user.click(within(screen.getByRole("listbox")).getByRole("option", { name: "Claude Code" }));

    expect(screen.getByRole("spinbutton", { name: "预算上限" })).toHaveValue(25);
    expect(screen.getByRole("spinbutton", { name: "预警阈值" })).toHaveValue(70);
  });

  it("把预算保存请求失败放到全局错误弹窗", async () => {
    vi.mocked(setAgentBudget).mockRejectedValue(new Error("budget write failed"));
    const user = userEvent.setup();
    render(<ErrorToastProvider><BudgetPricingPage onBack={vi.fn()} /></ErrorToastProvider>);
    await screen.findByRole("heading", { name: "Agent 预算预警" });

    await user.click(screen.getByRole("button", { name: "保存预算" }));

    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent("操作失败：budget write failed");
  });
});
