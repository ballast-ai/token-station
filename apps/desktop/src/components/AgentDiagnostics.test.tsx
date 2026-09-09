import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { cancelModelTestChat, getAgentRequestEvidence, testAgentRoute, type ReceiptView } from "../api";
import AgentDiagnostics from "./AgentDiagnostics";

vi.mock("../api", () => ({ getAgentRequestEvidence: vi.fn(), testAgentRoute: vi.fn(), cancelModelTestChat: vi.fn() }));

beforeEach(() => {
  vi.mocked(getAgentRequestEvidence).mockReset();
  vi.mocked(testAgentRoute).mockReset();
  vi.mocked(cancelModelTestChat).mockReset();
});

it("shows declared limits without claiming the maximum capacity was tested", () => {
  render(<AgentDiagnostics agentId="openclaw" configured runningRevision={5}
    modelLimits={{ context: 1000000, output: 900000, max_input: 100000, source: "configured" }} />);
  expect(screen.getByText("1000000 / 900000 / 100000")).toBeInTheDocument();
  expect(screen.getByText("来自模型配置；最大容量尚未实测。短消息探测不能验证此上限。")).toBeInTheDocument();
});

it("separates configured state from native capabilities and shows the observed route", async () => {
  vi.mocked(getAgentRequestEvidence).mockResolvedValue([{
    agent_id: "deepseek-harness", request_id: "observed-request", started_at_ms: 1000,
    status: 200, error_code: null, running_revision: 4, path_kind: "chat_completions",
    routing: { upstream: "channel-a", model: "actual-model" }, attempt_records: [],
  } as unknown as ReceiptView]);
  render(<AgentDiagnostics agentId="deepseek-harness" configured runningRevision={5} />);
  expect(screen.getByText("配置已写入，原生能力尚未验证")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "检查最近请求" }));
  expect(await screen.findByText("channel-a / actual-model")).toBeInTheDocument();
  expect(screen.getByText("此请求来自其他运行版本，不能验证当前路由。")).toBeInTheDocument();
  expect(screen.getByText("工具、搜索、网页读取和压缩需要在 Agent 内验证。请求成功不代表这些能力可用。")).toBeInTheDocument();
});

it("keeps cancellation failures visible while the original bounded request is running", async () => {
  vi.mocked(testAgentRoute).mockReturnValue(new Promise(() => {}));
  vi.mocked(cancelModelTestChat).mockRejectedValue(new Error("IPC unavailable"));
  render(<AgentDiagnostics agentId="kimi-code" configured runningRevision={5} />);
  fireEvent.click(screen.getByRole("button", { name: "测试当前 Agent 路由" }));
  fireEvent.click(screen.getByRole("button", { name: "取消测试" }));
  expect(await screen.findByText("取消请求未成功。测试仍在运行，可重试取消或等待超时。")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "测试当前 Agent 路由" })).toBeDisabled();
});

it("discards request evidence after switching Agents", async () => {
  let resolve!: (items: ReceiptView[]) => void;
  vi.mocked(getAgentRequestEvidence).mockReturnValue(new Promise((done) => { resolve = done; }));
  const { rerender } = render(<AgentDiagnostics agentId="kimi-code" configured runningRevision={5} />);
  fireEvent.click(screen.getByRole("button", { name: "检查最近请求" }));
  rerender(<AgentDiagnostics agentId="workbuddy" configured runningRevision={5} />);
  await act(async () => resolve([{
    agent_id: "kimi-code", started_at_ms: 1, path_kind: "chat_completions",
    routing: { upstream: "wrong-agent", model: "wrong-model" }, attempt_records: [],
  } as unknown as ReceiptView]));
  expect(screen.queryByText("wrong-agent / wrong-model")).not.toBeInTheDocument();
});

it("explains native search dependencies without treating a route probe as search verification", () => {
  render(<AgentDiagnostics agentId="workbuddy" configured runningRevision={5} />);
  expect(screen.getByText("在 WorkBuddy 内确认官方账号已登录，再运行原生搜索。模型 API Key 不等于搜索登录。")).toBeInTheDocument();
});

it("tests the running Agent route without claiming native capability verification", async () => {
  vi.mocked(testAgentRoute).mockResolvedValue({ content: "hello", latency_ms: 120, first_token_ms: 80 });
  render(<AgentDiagnostics agentId="kimi-code" configured runningRevision={5} />);
  fireEvent.click(screen.getByRole("button", { name: "测试当前 Agent 路由" }));
  expect(await screen.findByText("路由探测成功；Agent 原生能力仍需单独验证。")).toBeInTheDocument();
  expect(testAgentRoute).toHaveBeenCalledWith("kimi-code", expect.any(String));
});

it("does not report a partial HTTP 200 stream as a successful request", async () => {
  vi.mocked(getAgentRequestEvidence).mockResolvedValue([{
    agent_id: "grok-build", request_id: "partial", started_at_ms: 2000,
    status: 200, error_code: null, running_revision: 5, path_kind: "responses", stream: true,
    routing: null, attempt_records: [{ stream_outcome: "failed_after_partial" }],
  } as unknown as ReceiptView]);
  render(<AgentDiagnostics agentId="grok-build" configured runningRevision={5} />);
  fireEvent.click(screen.getByRole("button", { name: "检查最近请求" }));
  expect(await screen.findByText("最近请求失败或中断")).toBeInTheDocument();
  expect(screen.queryByText("最近请求成功")).not.toBeInTheDocument();
});
