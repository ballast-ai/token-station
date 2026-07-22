import { render, screen, within } from "@testing-library/react";
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
      decided_by: { tier: "heuristic", score: 30, threshold: 22 },
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

beforeEach(() => {
  vi.mocked(getRecentReceipts).mockReset();
});

describe("RecentReceipts", () => {
  it("mount 时只读取一次并把异常超量结果截断为 5 条", async () => {
    vi.mocked(getRecentReceipts).mockResolvedValue(Array.from({ length: 7 }, (_, index) => receipt(index + 1)));
    const { rerender } = render(<RecentReceipts />);

    expect(await screen.findAllByTestId("receipt-row")).toHaveLength(5);
    expect(getRecentReceipts).toHaveBeenCalledTimes(1);
    expect(getRecentReceipts).toHaveBeenCalledWith(5);

    rerender(<RecentReceipts />);
    expect(getRecentReceipts).toHaveBeenCalledTimes(1);
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
    expect(await screen.findByRole("alert")).toHaveTextContent("database unavailable");
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
