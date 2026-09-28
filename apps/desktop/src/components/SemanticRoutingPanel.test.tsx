import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getSemanticStatus, prepareSemanticModel, setSemanticMode, type SemanticStatus } from "../api";
import SemanticRoutingPanel from "./SemanticRoutingPanel";

vi.mock("../api", () => ({
  getSemanticStatus: vi.fn(),
  prepareSemanticModel: vi.fn(),
  setSemanticMode: vi.fn(),
}));

const status = (overrides: Partial<SemanticStatus> = {}): SemanticStatus => ({
  available: true,
  mode: "off",
  state: "off",
  error: null,
  model_ready: true,
  timeout_ms: 1500,
  observations: [],
  counts: { classified: 0, disagreements: 0, fallbacks: 0 },
  ...overrides,
});

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getSemanticStatus).mockResolvedValue(status());
});

afterEach(() => vi.useRealTimers());

describe("SemanticRoutingPanel", () => {
  it("does not expose experiment controls in the stable App", async () => {
    vi.mocked(getSemanticStatus).mockResolvedValue(status({ available: false }));
    render(<SemanticRoutingPanel />);
    await waitFor(() => expect(getSemanticStatus).toHaveBeenCalledOnce());
    expect(screen.queryByRole("region", { name: "SCX 本地分档" })).not.toBeInTheDocument();
  });

  it("prepares the model explicitly before enabling observation or routing", async () => {
    vi.mocked(getSemanticStatus).mockResolvedValue(status({ model_ready: false, state: "unprepared" }));
    vi.mocked(prepareSemanticModel).mockResolvedValue(status({ model_ready: false, state: "preparing" }));
    const user = userEvent.setup();
    render(<SemanticRoutingPanel />);
    const prepare = await screen.findByRole("button", { name: "准备本地模型" });
    expect(screen.getByRole("radio", { name: "仅观察" })).toBeDisabled();
    await user.click(prepare);
    expect(prepareSemanticModel).toHaveBeenCalledOnce();
    expect(screen.getByText("正在准备运行环境和模型…")).toBeInTheDocument();
    expect(setSemanticMode).not.toHaveBeenCalled();
  });

  it("selects observation using the keyboard and keeps Off available during loading", async () => {
    vi.mocked(setSemanticMode).mockResolvedValue(status({ mode: "observe", state: "loading" }));
    const user = userEvent.setup();
    render(<SemanticRoutingPanel />);
    const off = await screen.findByRole("radio", { name: "关闭" });
    off.focus();
    await user.keyboard("{ArrowRight}");
    expect(setSemanticMode).toHaveBeenCalledWith("observe");
    expect(await screen.findByText("正在加载模型，请求继续使用原有规则。")).toBeInTheDocument();
    expect(screen.getByText("仅记录建议档位和耗时，请求仍按原有规则路由。")).toBeInTheDocument();
    expect(off).toBeEnabled();
  });

  it("cancels preparation explicitly when the Off radio is already selected", async () => {
    vi.mocked(getSemanticStatus).mockResolvedValue(status({ mode: "off", model_ready: false, state: "preparing" }));
    vi.mocked(setSemanticMode).mockResolvedValue(status({ mode: "off", model_ready: false, state: "unprepared" }));
    const user = userEvent.setup();
    render(<SemanticRoutingPanel />);
    const cancel = await screen.findByRole("button", { name: "取消准备" });
    expect(screen.getByRole("radio", { name: "关闭" })).toBeChecked();
    expect(cancel).toBeEnabled();
    await user.click(cancel);
    expect(setSemanticMode).toHaveBeenCalledWith("off");
    expect(await screen.findByRole("button", { name: "准备本地模型" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "取消准备" })).not.toBeInTheDocument();
  });

  it("shows a failed change without claiming the mode was applied", async () => {
    vi.mocked(setSemanticMode).mockRejectedValue(new Error("Runtime unavailable"));
    const user = userEvent.setup();
    render(<SemanticRoutingPanel />);
    await user.click(await screen.findByRole("radio", { name: "参与路由" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Runtime unavailable");
    expect(screen.getByRole("radio", { name: "关闭" })).toBeChecked();
  });

  it("shows ten recent comparisons with applied and fallback outcomes", async () => {
    vi.mocked(getSemanticStatus).mockResolvedValue(status({
      mode: "route", state: "ready",
      observations: Array.from({ length: 12 }, (_, index) => ({
        id: index + 1, mode: "route", baseline_tier: "low", suggested_tier: index === 11 ? null : "high",
        applied: index !== 11, latency_ms: index === 11 ? null : 47.4, outcome: index === 11 ? "timeout" : "applied",
      })),
      counts: { classified: 11, disagreements: 11, fallbacks: 1 },
    }));
    render(<SemanticRoutingPanel />);
    const table = await screen.findByRole("table", { name: "最近分档对照" });
    expect(within(table).getAllByRole("row")).toHaveLength(11);
    expect(within(table).getByText("超时，使用原有规则")).toBeInTheDocument();
    expect(within(table).getAllByText("已采用 SCX")).toHaveLength(9);
    expect(within(table).getByText("#12")).toBeInTheDocument();
    expect(within(table).queryByText("#1")).not.toBeInTheDocument();
  });

  it("polls for completion and stops polling after unmount", async () => {
    vi.useFakeTimers();
    const view = render(<SemanticRoutingPanel />);
    await act(async () => { await Promise.resolve(); });
    expect(getSemanticStatus).toHaveBeenCalledOnce();
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(getSemanticStatus).toHaveBeenCalledTimes(2);
    view.unmount();
    await act(async () => { await vi.advanceTimersByTimeAsync(4000); });
    expect(getSemanticStatus).toHaveBeenCalledTimes(2);
  });

  it("does not let a stale poll undo a successful mode change", async () => {
    vi.useFakeTimers();
    let finishPoll!: (value: SemanticStatus) => void;
    vi.mocked(getSemanticStatus)
      .mockResolvedValueOnce(status())
      .mockImplementationOnce(() => new Promise((resolve) => { finishPoll = resolve; }));
    vi.mocked(setSemanticMode).mockResolvedValue(status({ mode: "route", state: "ready" }));
    render(<SemanticRoutingPanel />);
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    await act(async () => { fireEvent.click(screen.getByRole("radio", { name: "参与路由" })); });
    await act(async () => { finishPoll(status()); });
    expect(screen.getByRole("radio", { name: "参与路由" })).toBeChecked();
  });

  it("clears a recovered status error without hiding the panel", async () => {
    vi.useFakeTimers();
    vi.mocked(getSemanticStatus).mockResolvedValueOnce(status()).mockRejectedValueOnce(new Error("Status unavailable")).mockResolvedValue(status());
    render(<SemanticRoutingPanel />);
    await act(async () => { await Promise.resolve(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(screen.getByRole("alert")).toHaveTextContent("Status unavailable");
    await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "关闭" })).toBeChecked();
  });
});
