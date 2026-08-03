import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getRequestReceipts, type ReceiptView } from "../api";
import UsageRequestLog from "./UsageRequestLog";

vi.mock("../api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("../api")>();
  return {
    ...original,
    getRequestReceipts: vi.fn(),
  };
});

function receipt(overrides: Partial<ReceiptView>): ReceiptView {
  return {
    request_id: "req-1",
    started_at_ms: new Date(2026, 6, 23, 10).getTime(),
    latency_ms: 820,
    protocol: "anthropic-messages",
    requested_model: "auto",
    stream: true,
    status: 200,
    error_code: null,
    attempts: 1,
    routing: {
      upstream: "deepseek",
      model: "deepseek-v4-pro",
      pool: "upper",
      decided_by: { tier: "default" },
      fallbacks: 0,
      features: {
        estimated_input_tokens: 100,
        message_count: 2,
        tool_count: 0,
        has_images: false,
        requires_json_schema: false,
        code_block_count: 0,
        requested_max_output_tokens: null,
        hint_count: 0,
        reasoning_marker_count: 0,
        technical_term_count: 0,
        simple_indicator_count: 0,
        code_keyword_count: 0,
        math_term_count: 0,
        creative_term_count: 0,
        multi_step_signal: 0,
        question_count: 0,
        system_format_hint: false,
      },
    },
    usage: {
      input_tokens: 1_000,
      output_tokens: 200,
      cache_read_tokens: 600,
      cache_write_tokens: 0,
      reasoning_tokens: 50,
    },
    cost_micros: 435_000,
    price_version: 1,
    agent_id: "claude-code",
    running_revision: 3,
    cost_kind: "estimated",
    decision: null,
    attempt_records: [],
    conversion_reports: [],
    ...overrides,
  };
}

describe("UsageRequestLog", () => {
  beforeEach(() => {
    vi.mocked(getRequestReceipts).mockReset().mockImplementation(async ({ page }) => ({
      items: page === 1
        ? [
            receipt({}),
            receipt({
              request_id: "req-unknown",
              routing: {
                ...receipt({}).routing!,
                model: "unknown-model",
              },
              cost_kind: "unknown",
              cost_micros: null,
              price_version: null,
            }),
          ]
        : [receipt({ request_id: "req-page-2" })],
      total: 21,
      page,
      page_size: 20,
    }));
  });

  it("shows paginated receipt metadata and explains unknown model pricing", async () => {
    const user = userEvent.setup();
    render(
      <UsageRequestLog
        since="24h"
        agentId=""
        upstream=""
        model=""
        refreshKey={0}
      />,
    );

    expect(await screen.findByText("deepseek/deepseek-v4-pro")).toBeInTheDocument();
    expect(screen.getAllByText("估算 $0.435000")).toHaveLength(2);
    expect(screen.getByText("缺少模型价格：unknown-model")).toBeInTheDocument();
    expect(screen.getByText("1–20 / 21")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "下一页" }));
    await waitFor(() => expect(getRequestReceipts).toHaveBeenLastCalledWith(
      expect.objectContaining({ page: 2, status: null }),
    ));
  });

  it("applies the local success/error filter at the backend seam", async () => {
    const user = userEvent.setup();
    render(
      <UsageRequestLog
        since="7d"
        agentId="codex"
        upstream="openrouter"
        model="gpt-5.5"
        refreshKey={0}
      />,
    );
    await screen.findByText("请求日志");

    const statusFilter = screen.getByRole("combobox", { name: "请求状态" });
    expect(statusFilter.tagName).toBe("BUTTON");
    await user.click(statusFilter);
    await user.click(await screen.findByRole("option", { name: "失败" }));
    await waitFor(() => expect(getRequestReceipts).toHaveBeenLastCalledWith({
      since: "7d",
      agentId: "codex",
      upstream: "openrouter",
      model: "gpt-5.5",
      status: "error",
      page: 1,
      pageSize: 20,
    }));
  });
});
