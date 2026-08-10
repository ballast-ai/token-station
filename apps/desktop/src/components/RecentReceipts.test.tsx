import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getRecentReceipts, type ReceiptFeaturesView, type ReceiptView } from "../api";
import RecentReceipts from "./RecentReceipts";

vi.mock("../api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("../api")>();
  return { ...original, getRecentReceipts: vi.fn() };
});

const features: ReceiptFeaturesView = {
  estimated_input_tokens: 24,
  message_count: 2,
  tool_count: 1,
  has_images: false,
  requires_json_schema: true,
  code_block_count: 0,
  requested_max_output_tokens: 512,
  hint_count: 1,
  reasoning_marker_count: 0,
  technical_term_count: 1,
  simple_indicator_count: 0,
  code_keyword_count: 0,
  math_term_count: 0,
  creative_term_count: 0,
  multi_step_signal: 0,
  question_count: 1,
  system_format_hint: false,
};

function receipt(index: number, overrides: Partial<ReceiptView> = {}): ReceiptView {
  return {
    request_id: `request-${index}`,
    started_at_ms: 1_752_000_000_000 + index,
    latency_ms: 120 + index,
    protocol: "openai-chat-completions",
    requested_model: "auto",
    stream: false,
    status: 200,
    error_code: null,
    attempts: 1,
    routing: {
      upstream: "provider-final",
      model: "model-final",
      pool: "tier_mid",
      decided_by: { tier: "heuristic", score: 30, matched_band_at_least: 22 },
      fallbacks: 0,
      features,
    },
    usage: {
      input_tokens: 10,
      output_tokens: 5,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      reasoning_tokens: 0,
    },
    cost_micros: 125_000,
    price_version: 3,
    agent_id: "codex",
    running_revision: 9,
    cost_kind: "estimated",
    decision: null,
    attempt_records: [],
    conversion_reports: [],
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.mocked(getRecentReceipts).mockReset();
});

describe("RecentReceipts", () => {
  it("挂载立即读取并把异常超量结果截断为 5 条", async () => {
    vi.mocked(getRecentReceipts).mockResolvedValue(
      Array.from({ length: 7 }, (_, index) => receipt(index + 1)),
    );
    render(<RecentReceipts />);

    expect(await screen.findAllByTestId("receipt-row")).toHaveLength(5);
    expect(getRecentReceipts).toHaveBeenCalledTimes(1);
    expect(getRecentReceipts).toHaveBeenCalledWith(5);
  });

  it("用户可手动刷新，等待期间保留旧回执并在成功后显示新回执", async () => {
    const user = userEvent.setup();
    let resolveRefresh!: (value: ReceiptView[]) => void;
    vi.mocked(getRecentReceipts)
      .mockResolvedValueOnce([receipt(1, {
        routing: { ...receipt(1).routing!, model: "old-model" },
      })])
      .mockImplementationOnce(() => new Promise((resolve) => {
        resolveRefresh = resolve;
      }));
    render(<RecentReceipts />);

    expect(await screen.findByText("provider-final/old-model")).toBeInTheDocument();
    const refreshButton = screen.getByRole("button", { name: "刷新最近请求" });

    await user.click(refreshButton);

    expect(getRecentReceipts).toHaveBeenCalledTimes(2);
    expect(refreshButton).toBeDisabled();
    expect(refreshButton).toHaveAttribute("aria-busy", "true");
    expect(screen.getByText("provider-final/old-model")).toBeInTheDocument();

    resolveRefresh([receipt(2, {
      routing: { ...receipt(2).routing!, model: "new-model" },
    })]);

    expect(await screen.findByText("provider-final/new-model")).toBeInTheDocument();
    expect(screen.queryByText("provider-final/old-model")).not.toBeInTheDocument();
    await waitFor(() => expect(refreshButton).not.toBeDisabled());
  });

  it("页面可见时每 10 秒刷新，隐藏时暂停并在窗口重新聚焦时立即刷新", async () => {
    vi.useFakeTimers();
    const initialVisibility = document.visibilityState;
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    });
    vi.mocked(getRecentReceipts).mockResolvedValue([]);
    const { unmount } = render(<RecentReceipts />);

    try {
      await act(async () => {
        await Promise.resolve();
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(1);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(9_999);
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(1);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(1);
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(2);

      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        value: "hidden",
      });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(30_000);
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(2);

      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        value: "visible",
      });
      await act(async () => {
        window.dispatchEvent(new Event("focus"));
        await Promise.resolve();
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(3);
    } finally {
      unmount();
      Object.defineProperty(document, "visibilityState", {
        configurable: true,
        value: initialVisibility,
      });
      vi.useRealTimers();
    }
  });

  it("慢刷新期间合并重复触发，并在一次后续读取完成前保持 busy", async () => {
    vi.useFakeTimers();
    const firstRefresh = deferred<ReceiptView[]>();
    const followUpRefresh = deferred<ReceiptView[]>();
    vi.mocked(getRecentReceipts)
      .mockResolvedValueOnce([receipt(1)])
      .mockImplementationOnce(() => firstRefresh.promise)
      .mockImplementationOnce(() => followUpRefresh.promise);
    const { unmount } = render(<RecentReceipts />);

    try {
      await act(async () => {
        await Promise.resolve();
      });
      const refreshButton = screen.getByRole("button", { name: "刷新最近请求" });

      await act(async () => {
        await vi.advanceTimersByTimeAsync(10_000);
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(2);
      expect(refreshButton).toBeDisabled();

      await act(async () => {
        await vi.advanceTimersByTimeAsync(20_000);
        window.dispatchEvent(new Event("focus"));
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(2);

      await act(async () => {
        firstRefresh.resolve([receipt(2)]);
        await firstRefresh.promise;
        await Promise.resolve();
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(3);
      expect(refreshButton).toBeDisabled();

      await act(async () => {
        followUpRefresh.resolve([receipt(3)]);
        await followUpRefresh.promise;
        await Promise.resolve();
      });
      expect(refreshButton).not.toBeDisabled();
    } finally {
      unmount();
      vi.useRealTimers();
    }
  });

  it("后台刷新失败时保留旧数据和成功时间，并在重试成功后清除警告", async () => {
    const user = userEvent.setup();
    vi.mocked(getRecentReceipts)
      .mockResolvedValueOnce([receipt(1)])
      .mockRejectedValueOnce(new Error("database busy"))
      .mockResolvedValueOnce([receipt(2)]);
    render(<RecentReceipts />);

    expect(await screen.findByText("provider-final/model-final")).toBeInTheDocument();
    const updatedAt = screen.getByTestId("receipt-updated-at");
    const successfulDateTime = updatedAt.getAttribute("dateTime");
    const refreshButton = screen.getByRole("button", { name: "刷新最近请求" });

    await user.click(refreshButton);

    expect(await screen.findByText(/更新失败，当前显示上次数据/)).toBeInTheDocument();
    expect(screen.getByText("provider-final/model-final")).toBeInTheDocument();
    expect(screen.getByTestId("receipt-updated-at")).toHaveAttribute(
      "dateTime",
      successfulDateTime,
    );

    await user.click(refreshButton);

    await waitFor(() => expect(getRecentReceipts).toHaveBeenCalledTimes(3));
    expect(await screen.findByText("request-2")).toBeInTheDocument();
    expect(screen.queryByText(/更新失败，当前显示上次数据/)).not.toBeInTheDocument();
  });

  it("相同回执刷新后保持已展开详情", async () => {
    const user = userEvent.setup();
    vi.mocked(getRecentReceipts)
      .mockResolvedValueOnce([receipt(1)])
      .mockResolvedValueOnce([receipt(1)]);
    render(<RecentReceipts />);

    const row = (await screen.findAllByTestId("receipt-row"))[0] as HTMLDetailsElement;
    await user.click(row.querySelector("summary")!);
    expect(row.open).toBe(true);

    await user.click(screen.getByRole("button", { name: "刷新最近请求" }));

    await waitFor(() => expect(getRecentReceipts).toHaveBeenCalledTimes(2));
    expect(row.open).toBe(true);
  });

  it("卸载后忽略迟到结果且不执行已排队刷新", async () => {
    vi.useFakeTimers();
    const slowRefresh = deferred<ReceiptView[]>();
    vi.mocked(getRecentReceipts)
      .mockResolvedValueOnce([receipt(1)])
      .mockImplementationOnce(() => slowRefresh.promise);
    const { unmount } = render(<RecentReceipts />);

    try {
      await act(async () => {
        await Promise.resolve();
        await vi.advanceTimersByTimeAsync(20_000);
      });
      expect(getRecentReceipts).toHaveBeenCalledTimes(2);

      unmount();
      await act(async () => {
        slowRefresh.resolve([receipt(2)]);
        await slowRefresh.promise;
        await Promise.resolve();
      });

      expect(getRecentReceipts).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("按 decision → attempts → conversions 展开固定详情", async () => {
    vi.mocked(getRecentReceipts).mockResolvedValue([receipt(1, {
      attempts: 2,
      decision: {
        upstream: "provider-first",
        model: "model-first",
        pool: "tier_mid",
        decided_by: { tier: "rule", rule: "tools" },
        fallbacks: 1,
        features,
      },
      attempt_records: [{
        ordinal: 1,
        upstream: "provider-first",
        model: "model-first",
        latency_ms: 80,
        http_status: 429,
        error_code: "rate_limit",
        stream_outcome: null,
        fallback_allowed: true,
      }, {
        ordinal: 2,
        upstream: "provider-final",
        model: "model-final",
        latency_ms: 40,
        http_status: 200,
        error_code: null,
        stream_outcome: "complete",
        fallback_allowed: false,
      }],
      conversion_reports: [{
        ordinal: 1,
        stage: "inbound_normalize",
        source_protocol: "openai-chat-completions",
        target_protocol: "canonical-chat",
        succeeded: true,
        error_code: null,
      }],
    })]);
    const user = userEvent.setup();
    render(<RecentReceipts />);

    const row = (await screen.findAllByTestId("receipt-row"))[0] as HTMLDetailsElement;
    expect(row.open).toBe(false);
    await user.click(row.querySelector("summary")!);
    expect(row.open).toBe(true);

    expect(within(row).getByLabelText("决策记录")).toHaveTextContent("provider-first/model-first");
    expect(within(row).getByLabelText("上游尝试记录")).toHaveTextContent("HTTP 429");
    expect(within(row).getByLabelText("上游尝试记录")).toHaveTextContent("provider-final/model-final");
    expect(within(row).getByLabelText("协议转换记录")).toHaveTextContent("inbound_normalize");
  });

  it("把结构化错误翻译为可执行诊断并可复制请求 ID", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    vi.mocked(getRecentReceipts).mockResolvedValue([receipt(1, {
      status: 401,
      error_code: "auth",
    })]);
    render(<RecentReceipts />);

    const row = (await screen.findAllByTestId("receipt-row"))[0] as HTMLDetailsElement;
    await user.click(row.querySelector("summary")!);
    expect(within(row).getByLabelText("错误诊断")).toHaveTextContent("鉴权 · Key");
    expect(within(row).getByLabelText("错误诊断")).toHaveTextContent("下一步");

    await user.click(within(row).getByRole("button", { name: "复制请求 ID" }));
    expect(writeText).toHaveBeenCalledWith("request-1");
    expect(within(row).getByRole("button", { name: "已复制" })).toBeInTheDocument();
  });

  it("覆盖 loading、error 与空态", async () => {
    let resolve!: (value: ReceiptView[]) => void;
    vi.mocked(getRecentReceipts).mockReturnValue(new Promise((done) => { resolve = done; }));
    const first = render(<RecentReceipts />);
    expect(screen.getByRole("status")).toHaveTextContent("正在读取请求回执");
    resolve([]);
    expect(await screen.findByText("还没有请求回执")).toBeInTheDocument();
    first.unmount();

    vi.mocked(getRecentReceipts).mockRejectedValue(new Error("database unavailable"));
    render(<RecentReceipts />);
    expect(await screen.findByRole("alert")).toHaveTextContent("本地数据无法安全打开。请使用自救模式检查或导出本地数据。");
  });

  it("unknown 成本不把后端异常零值展示成零成本", async () => {
    vi.mocked(getRecentReceipts).mockResolvedValue([receipt(1, {
      cost_kind: "unknown",
      cost_micros: 0,
      price_version: null,
      usage: null,
    })]);
    render(<RecentReceipts />);

    expect(await screen.findByText("成本未知")).toBeInTheDocument();
    expect(screen.getByText("token 未知")).toBeInTheDocument();
    expect(screen.queryByText(/0\.0000/)).not.toBeInTheDocument();
    expect(screen.queryByText(/零成本/)).not.toBeInTheDocument();
  });

  it("最小正成本也不会因展示精度变成 0", async () => {
    vi.mocked(getRecentReceipts).mockResolvedValue([receipt(1, {
      cost_kind: "estimated",
      cost_micros: 1,
    })]);
    render(<RecentReceipts />);

    expect(await screen.findByText("估算成本 0.000001")).toBeInTheDocument();
  });
});
