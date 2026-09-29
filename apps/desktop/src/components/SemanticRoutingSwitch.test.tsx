import { act, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getSemanticStatus, setSemanticEnabled, type SemanticStatus } from "../api";
import SemanticRoutingSwitch from "./SemanticRoutingSwitch";

vi.mock("../api", () => ({
  getSemanticStatus: vi.fn(),
  setSemanticEnabled: vi.fn(),
}));

function status(overrides: Partial<SemanticStatus> = {}): SemanticStatus {
  return {
    available: true, enabled: true, mode: "route", state: "ready", error: null,
    model_ready: true, timeout_ms: 400, observations: [],
    counts: { classified: 0, disagreements: 0, fallbacks: 0 },
    ...overrides,
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getSemanticStatus).mockResolvedValue(status());
});

afterEach(() => vi.useRealTimers());

describe("SemanticRoutingSwitch", () => {
  it("hides the control and stops polling when the backend reports unavailable", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    vi.mocked(getSemanticStatus).mockResolvedValue(status({ available: false, enabled: false, mode: "off", state: "off" }));
    const view = render(<SemanticRoutingSwitch />);
    await act(async () => {});
    expect(view.container).toBeEmptyDOMElement();
    await act(async () => { await vi.advanceTimersByTimeAsync(6_000); });
    expect(getSemanticStatus).toHaveBeenCalledOnce();
    expect(setSemanticEnabled).not.toHaveBeenCalled();
  });

  it.each([
    ["en", "Local smart tiers", "First enable prepares a local runtime and downloads about 2.5 GB of model files. Install uv first. Classification then runs on this device."],
    ["zh-CN", "本地智能分档", "首次开启会准备本地运行环境并下载约 2.5 GB 模型文件，请先安装 uv。准备完成后，分类在本机运行。"],
    ["zh-TW", "本機智慧分檔", "首次啟用會準備本機執行環境並下載約 2.5 GB 模型檔案，請先安裝 uv。準備完成後，分類在本機執行。"],
    ["ja", "ローカルスマート分層", "初回の有効化時にローカル実行環境を準備し、約 2.5 GB のモデルをダウンロードします。事前に uv をインストールしてください。準備後の分類はこのデバイスで実行します。"],
  ])("explains first-use preparation without enabling classification in %s", async (language, label, description) => {
    window.localStorage.setItem("token-station-language", language);
    vi.mocked(getSemanticStatus).mockResolvedValue(status({ enabled: false, mode: "off", state: "off", model_ready: false }));
    render(<SemanticRoutingSwitch />);
    const control = await screen.findByRole("switch", { name: label });
    expect(control).not.toBeChecked();
    expect(control).toBeEnabled();
    expect(screen.getByText(description)).toBeVisible();
    expect(control).toHaveAccessibleDescription(expect.stringContaining(description));
    expect(setSemanticEnabled).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("reflects saved intent while preparing and allows one pending Off request", async () => {
    const user = userEvent.setup();
    const update = deferred<SemanticStatus>();
    vi.mocked(getSemanticStatus).mockResolvedValue(status({ enabled: true, mode: "off", state: "preparing", model_ready: false }));
    vi.mocked(setSemanticEnabled).mockReturnValue(update.promise);
    render(<SemanticRoutingSwitch />);
    const control = await screen.findByRole("switch", { name: "本地智能分档" });
    expect(control).toBeChecked();
    expect(control).toBeEnabled();
    expect(screen.getByRole("status")).toHaveTextContent("正在准备模型，暂用原有规则。");

    await user.click(control);
    expect(control).toBeDisabled();
    expect(control).toBeChecked();
    await user.click(control);
    expect(setSemanticEnabled).toHaveBeenCalledExactlyOnceWith(false);

    await act(async () => update.resolve(status({ enabled: false, mode: "off", state: "off" })));
    expect(control).not.toBeChecked();
    expect(control).toBeEnabled();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("enables a saved Off choice and permits switching Off during model loading", async () => {
    const user = userEvent.setup();
    vi.mocked(getSemanticStatus).mockResolvedValue(status({ enabled: false, mode: "off", state: "off" }));
    vi.mocked(setSemanticEnabled)
      .mockResolvedValueOnce(status({ state: "loading" }))
      .mockResolvedValueOnce(status({ enabled: false, mode: "off", state: "off" }));
    render(<SemanticRoutingSwitch />);
    const control = await screen.findByRole("switch", { name: "本地智能分档" });
    expect(control).not.toBeChecked();

    await user.click(control);
    expect(setSemanticEnabled).toHaveBeenNthCalledWith(1, true);
    expect(control).toBeChecked();
    expect(control).toBeEnabled();
    expect(screen.getByRole("status")).toHaveTextContent("正在加载模型，暂用原有规则。");
    await user.click(control);
    expect(setSemanticEnabled).toHaveBeenNthCalledWith(2, false);
    expect(control).not.toBeChecked();
  });

  it("keeps the previous choice when saving fails and permits retry", async () => {
    const user = userEvent.setup();
    vi.mocked(setSemanticEnabled).mockRejectedValueOnce(new Error("private settings are not writable"))
      .mockResolvedValueOnce(status({ enabled: false, mode: "off", state: "off" }));
    render(<SemanticRoutingSwitch />);
    const control = await screen.findByRole("switch", { name: "本地智能分档" });
    await user.click(control);
    expect(control).toBeChecked();
    expect(control).toBeEnabled();
    expect(screen.getByRole("alert")).toHaveTextContent("设置未保存，请重试。");
    await user.click(control);
    expect(control).not.toBeChecked();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("ignores a poll started before a successful change and stops polling on unmount", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const user = userEvent.setup();
    const stalePoll = deferred<SemanticStatus>();
    const off = status({ enabled: false, mode: "off", state: "off" });
    vi.mocked(getSemanticStatus).mockResolvedValueOnce(status())
      .mockReturnValueOnce(stalePoll.promise)
      .mockResolvedValue(off);
    vi.mocked(setSemanticEnabled).mockResolvedValue(off);
    const view = render(<SemanticRoutingSwitch />);
    await act(async () => {});
    const control = screen.getByRole("switch", { name: "本地智能分档" });
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(getSemanticStatus).toHaveBeenCalledTimes(2);

    await user.click(control);
    expect(control).not.toBeChecked();
    await act(async () => stalePoll.resolve(status()));
    expect(control).not.toBeChecked();
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(getSemanticStatus).toHaveBeenCalledTimes(3);
    expect(control).not.toBeChecked();

    view.unmount();
    await act(async () => { await vi.advanceTimersByTimeAsync(6_000); });
    expect(getSemanticStatus).toHaveBeenCalledTimes(3);
  });
});
