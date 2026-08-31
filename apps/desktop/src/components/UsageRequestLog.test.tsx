import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getRequestReceipts, type ReceiptView, type RequestPlaintextView } from "../api";
import UsageRequestLog, { diffJson } from "./UsageRequestLog";

const rawInput = JSON.stringify({
  system: [{ type: "text", text: "你是代码审查助手" }],
  messages: [{
    role: "user",
    content: [
      { type: "text", text: "<system-reminder>只读取本地文件</system-reminder>" },
      { type: "text", text: "解释这个错误" },
    ],
  }],
  tools: [{ name: "read_file", description: "读取文件" }],
});
const rawSseOutput = [
  "event: message_start",
  'data:{"type":"message_start","message":{"id":"msg-1"}}',
  "",
  "event: content_block_delta",
  'data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"先检查堆栈"}}',
  "",
  "event: content_block_delta",
  'data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"这是明文输出"}}',
  "",
  "event: content_block_start",
  'data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tool-1","name":"read_file","input":{}}}',
  "",
  "event: content_block_delta",
  'data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\\"path\\":\\"/tmp/a.ts\\"}"}}',
  "",
  "data: not-json",
  "",
  "data: [DONE]",
].join("\n");

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
  it("bounds comparison of deeply nested JSON without overflowing the renderer stack", () => {
    const depth = 5_000;
    const before = `${"[".repeat(depth)}0${"]".repeat(depth)}`;
    const after = `${"[".repeat(depth)}1${"]".repeat(depth)}`;

    expect(diffJson(before, after)).toEqual([
      {
        kind: "modified",
        path: "body",
        before: "<comparison limit reached>",
        after: "<comparison limit reached>",
      },
    ]);
  });
  beforeEach(() => {
    vi.mocked(getRequestReceipts).mockReset().mockImplementation(async ({ page }) => ({
      items: page === 1
        ? [
            receipt({
              decision: receipt({}).routing,
              attempt_records: [{
                ordinal: 1,
                upstream: "deepseek",
                model: "deepseek-v4-pro",
                latency_ms: 820,
                http_status: 200,
                error_code: null,
                stream_outcome: "complete",
                fallback_allowed: false,
              }],
              conversion_reports: [
                {
                  ordinal: 1,
                  stage: "inbound_normalize",
                  source_protocol: "openai-chat-completions",
                  target_protocol: "token-station-chat",
                  succeeded: true,
                  error_code: null,
                },
                {
                  ordinal: 2,
                  stage: "provider_request",
                  source_protocol: "token-station-chat",
                  target_protocol: "openai-compatible",
                  succeeded: true,
                  error_code: null,
                },
                {
                  ordinal: 3,
                  stage: "provider_response",
                  source_protocol: "openai-compatible",
                  target_protocol: "token-station-chat",
                  succeeded: true,
                  error_code: null,
                },
                {
                  ordinal: 4,
                  stage: "outbound_render",
                  source_protocol: "token-station-chat",
                  target_protocol: "openai-chat-completions",
                  succeeded: true,
                  error_code: null,
                },
              ],
            }),
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
      plaintext_by_request_id: page === 1
        ? {
            "req-1": {
              request_id: "req-1",
              captured_at_ms: new Date(2026, 7, 13, 17, 55).getTime(),
              input: rawInput,
              output: rawSseOutput,
              input_truncated: false,
              output_truncated: false,
            },
          }
        : {} as Record<string, RequestPlaintextView>,
      total: 21,
      page,
      page_size: 20,
    }));
  });

  it("请求列表和详情均披露历史上游输入语义", async () => {
    const user = userEvent.setup();
    vi.mocked(getRequestReceipts).mockResolvedValue({
      items: [receipt({ usage_semantics: "provider_reported_v1" })],
      plaintext_by_request_id: {},
      total: 1,
      page: 1,
      page_size: 20,
    });
    render(
      <UsageRequestLog
        since="24h"
        agentId=""
        upstream=""
        model=""
        refreshKey={0}
      />,
    );

    expect(await screen.findByText("1,200 上游上报")).toBeInTheDocument();
    await user.click(screen.getByText("deepseek/deepseek-v4-pro"));
    expect(screen.getByText("上游输入")).toBeInTheDocument();
    expect(screen.getByText("历史上游输入可能不含缓存 Token；总量并非规范总量。")).toBeInTheDocument();
  });

  it("把无 Agent 命名空间的请求标记为主页路由", async () => {
    vi.mocked(getRequestReceipts).mockResolvedValue({
      items: [receipt({ agent_id: null })],
      plaintext_by_request_id: {},
      total: 1,
      page: 1,
      page_size: 20,
    });

    render(
      <UsageRequestLog
        since="24h"
        agentId=""
        upstream=""
        model=""
        refreshKey={0}
      />,
    );

    expect(await screen.findByText("主页")).toBeInTheDocument();
    expect(screen.queryByText("未知 Agent")).toBeNull();
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

    expect((await screen.findAllByText("deepseek/deepseek-v4-pro")).length).toBeGreaterThan(0);
    expect(screen.getAllByText("估算 $0.435000")).toHaveLength(1);
    expect(screen.getByText("未知")).toHaveAttribute("title", "缺少模型价格：unknown-model");
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

  it("shows independently scrollable plaintext input and output below the receipt timeline", async () => {
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

    const openDetails = await screen.findByRole("button", { name: /打开请求详情.*req-1/ });
    await user.click(openDetails);

    expect(document.querySelector("details.usage-log-row")).toBeNull();
    expect(screen.getByRole("dialog", { name: "请求详情" })).toBeInTheDocument();

    const input = screen.getByRole("region", { name: "明文输入" });
    const output = screen.getByRole("region", { name: "明文输出" });
    expect(input).toHaveClass("request-plaintext-scroll");
    expect(output).toHaveClass("request-plaintext-scroll");
    expect(within(input).getAllByText("系统提示词")).toHaveLength(2);
    expect(within(input).getByText("你是代码审查助手")).toBeInTheDocument();
    expect(within(input).getByText("只读取本地文件")).toBeInTheDocument();
    expect(within(input).getByText("用户输入")).toBeInTheDocument();
    expect(within(input).getByText("解释这个错误")).toBeInTheDocument();
    expect(within(input).getByText("工具定义 · 1 个")).toBeInTheDocument();
    expect(within(input).getByText(/read_file/)).toBeInTheDocument();
    expect(within(output).getByText("助手思考")).toBeInTheDocument();
    expect(within(output).getByText("先检查堆栈")).toBeInTheDocument();
    expect(within(output).getByText("助手输出")).toBeInTheDocument();
    expect(within(output).getByText("这是明文输出")).toBeInTheDocument();
    expect(within(output).getByText("工具调用 · read_file")).toBeInTheDocument();
    expect(within(output).getByText(/\/tmp\/a\.ts/)).toBeInTheDocument();
    expect(screen.getByText("默认保留 7 天")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "关闭" })).toBeInTheDocument();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog", { name: "请求详情" })).toBeNull();
    expect(openDetails).toHaveFocus();
  });

  it("formats complete JSON in source view and preserves non-JSON source exactly", async () => {
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

    await user.click((await screen.findAllByText("deepseek/deepseek-v4-pro"))[0]);
    const input = screen.getByRole("region", { name: "明文输入" });
    const output = screen.getByRole("region", { name: "明文输出" });
    const parsedButton = screen.getByRole("button", { name: "解析视图" });
    const sourceButton = screen.getByRole("button", { name: "原文" });

    expect(parsedButton).toHaveAttribute("aria-pressed", "true");
    expect(sourceButton).toHaveAttribute("aria-pressed", "false");
    expect(input.textContent).not.toBe(rawInput);
    expect(within(input).getAllByText("系统提示词")).toHaveLength(2);
    expect(within(output).getByText("助手输出")).toBeInTheDocument();

    await user.click(sourceButton);
    expect(parsedButton).toHaveAttribute("aria-pressed", "false");
    expect(sourceButton).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("region", { name: "明文输入" }).textContent).toBe(
      JSON.stringify(JSON.parse(rawInput), null, 2),
    );
    expect(screen.getByRole("region", { name: "明文输出" }).textContent).toBe(rawSseOutput);

    await user.click(parsedButton);
    const parsedOutput = screen.getByRole("region", { name: "明文输出" });
    expect(within(parsedOutput).getByText("助手思考")).toBeInTheDocument();
    expect(within(parsedOutput).getByText("工具调用 · read_file")).toBeInTheDocument();
  });

  it("shows complete redacted HTTP packets and marks request mutations", async () => {
    const user = userEvent.setup();
    vi.mocked(getRequestReceipts).mockResolvedValue({
      items: [receipt({ stream: false })],
      plaintext_by_request_id: {
        "req-1": {
          request_id: "req-1",
          captured_at_ms: new Date(2026, 7, 13, 17, 55).getTime(),
          input: '{"model":"auto","messages":[{"role":"user","content":"hello"}]}',
          output: '{"choices":[]}',
          input_truncated: false,
          output_truncated: false,
          http_trace: {
            agent_request: {
              method: "POST",
              url: "/agents/claude-code/v1/messages",
              headers: [
                { name: "authorization", value: "<redacted>", redacted: true },
                { name: "user-agent", value: "claude-code/2.1", redacted: false },
              ],
              body: '{"model":"auto","messages":[{"role":"user","content":"hello"}]}',
              body_truncated: false,
            },
            upstream_exchanges: [{
              ordinal: 1,
              upstream: "deepseek",
              model: "deepseek-v4-pro",
              request: {
                method: "POST",
                url: "https://api.deepseek.com/v1/chat/completions",
                headers: [
                  { name: "content-type", value: "application/json", redacted: false },
                  { name: "authorization", value: "<redacted>", redacted: true },
                ],
                body: '{"model":"deepseek-v4-pro","messages":[{"role":"user","content":"hello"}]}',
                body_truncated: false,
              },
              response: {
                status: 200,
                headers: [{ name: "content-type", value: "application/json", redacted: false }],
                body: '{"choices":[]}',
                body_truncated: false,
              },
            }],
            agent_response: {
              status: 200,
              headers: [{ name: "content-type", value: "application/json", redacted: false }],
              body: '{"choices":[]}',
              body_truncated: false,
            },
          },
        },
      },
      total: 1,
      page: 1,
      page_size: 20,
    });

    render(<UsageRequestLog since="24h" agentId="" upstream="" model="" refreshKey={0} />);
    await user.click((await screen.findAllByText("deepseek/deepseek-v4-pro"))[0]);

    expect(screen.getByRole("button", { name: "HTTP 链路" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("Agent → Token Station")).toBeInTheDocument();
    expect(screen.getByText("尝试 1 · Token Station → deepseek/deepseek-v4-pro")).toBeInTheDocument();
    const dialog = screen.getByRole("dialog", { name: "请求详情" });
    const packets = dialog.querySelectorAll("details.http-packet");
    expect(packets).toHaveLength(4);
    expect(packets[0]).not.toHaveAttribute("open");
    expect(packets[1]).not.toHaveAttribute("open");
    expect(packets[2]).not.toHaveAttribute("open");
    expect(packets[3]).not.toHaveAttribute("open");
    expect(dialog.querySelectorAll(".http-packet-disclosure")).toHaveLength(4);
    expect(within(packets[0] as HTMLElement).getByText("请求头")).toBeInTheDocument();
    expect(within(packets[0] as HTMLElement).getByText("请求体")).toBeInTheDocument();
    expect(within(packets[2] as HTMLElement).getByText("响应头")).toBeInTheDocument();
    expect(within(packets[2] as HTMLElement).getByText("响应体")).toBeInTheDocument();

    const changeSet = dialog.querySelector("details.http-change-list");
    expect(changeSet).not.toBeNull();
    expect(changeSet).not.toHaveAttribute("open");
    expect(changeSet?.querySelector(".http-change-disclosure")).not.toBeNull();
    await user.click(within(changeSet as HTMLElement).getByText("Token Station 改动"));
    expect(changeSet).toHaveAttribute("open");
    expect(screen.getByText("body.model")).toBeInTheDocument();
    expect(screen.getAllByText("<redacted>").length).toBeGreaterThanOrEqual(2);
    expect(screen.getAllByText("https://api.deepseek.com/v1/chat/completions", { exact: false }).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText("deepseek → Token Station")).toBeInTheDocument();
    expect(screen.getByText("Token Station → Agent")).toBeInTheDocument();
  });

  it("uses a compact two-row trace and explains conversion stages in user language", async () => {
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

    const route = (await screen.findAllByText("deepseek/deepseek-v4-pro"))[0];
    await user.click(route);
    const row = screen.getByRole("dialog", { name: "请求详情" });

    const summary = within(row).getByLabelText("请求审计摘要");
    expect(summary.tagName).toBe("DL");
    expect(summary).toHaveClass("usage-log-facts");
    const identityFacts = summary.querySelectorAll(".request-detail-fact--identity");
    expect(identityFacts).toHaveLength(2);
    expect(within(identityFacts[0] as HTMLElement).getByText("req-1")).toBeInTheDocument();
    expect(within(identityFacts[1] as HTMLElement).getByText("auto")).toBeInTheDocument();
    expect(summary.querySelectorAll(".request-detail-fact--technical")).toHaveLength(3);

    const trace = within(row).getByTestId("receipt-trace");
    expect(trace).toHaveClass("receipt-timeline-compact");
    expect(within(row).getByRole("list", { name: "协议转换流程" })).toHaveClass("receipt-conversion-flow");
    expect(within(row).getByText("收到调用方请求")).toBeInTheDocument();
    expect(within(row).getByText("转为供应商格式")).toBeInTheDocument();
    expect(within(row).getByText("解析供应商响应")).toBeInTheDocument();
    expect(within(row).getByText("返回调用方格式")).toBeInTheDocument();
    expect(within(row).getByText("inbound_normalize")).toBeInTheDocument();
  });

  it("explains when an older receipt has no retained plaintext", async () => {
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

    await user.click(await screen.findByText("deepseek/unknown-model"));
    expect(screen.getByRole("status")).toHaveTextContent(
      "此请求没有可用明文。它可能早于本功能、写入失败或已被保留策略清理。",
    );
  });
});
