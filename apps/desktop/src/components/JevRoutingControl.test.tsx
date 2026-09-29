import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearJevKey, getJevStatus, saveJevKey, setJevEnabled, testJevConnection,
  type JevStatus,
} from "../api";
import JevRoutingControl from "./JevRoutingControl";
import { LanguageProvider } from "./LanguageProvider";

vi.mock("../api", () => ({
  clearJevKey: vi.fn(), getJevStatus: vi.fn(), saveJevKey: vi.fn(),
  setJevEnabled: vi.fn(), testJevConnection: vi.fn(),
}));

function status(overrides: Partial<JevStatus> = {}): JevStatus {
  return {
    enabled: false, has_key: false, model: "jev-latest", timeout_ms: 1500,
    confidence_threshold: 0.7, last_outcome: null, last_tier: null, last_latency_ms: null,
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
  vi.mocked(getJevStatus).mockResolvedValue(status());
});
afterEach(() => vi.useRealTimers());

describe("JevRoutingControl", () => {
  it("starts compact and keeps the cloud notice and key state visible", async () => {
    vi.mocked(getJevStatus).mockResolvedValue(status({ has_key: true }));
    render(<JevRoutingControl />);
    expect(await screen.findByText("已配置 Key")).toBeVisible();
    expect(screen.getByRole("button", { name: "Jev 设置" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.getByRole("button", { name: "Jev 设置" })).toHaveAccessibleDescription("已配置 Key");
    expect(screen.getByText("文本发送至 TypeSafe · 独立 API 计费")).toBeVisible();
    expect(screen.getByLabelText("Jev API Key")).not.toBeVisible();
    expect(screen.queryByRole("button", { name: "测试连接" })).not.toBeInTheDocument();
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("expands by keyboard and retains a draft without mutating routing", async () => {
    const user = userEvent.setup();
    render(<JevRoutingControl />);
    await screen.findByText("未配置 Key");
    await user.tab();
    const disclosure = screen.getByRole("button", { name: "Jev 设置" });
    expect(disclosure).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(disclosure).toHaveAttribute("aria-expanded", "true");
    const panel = document.getElementById(disclosure.getAttribute("aria-controls")!);
    expect(panel).toBeVisible();
    const input = screen.getByLabelText("Jev API Key");
    await user.type(input, "unsaved-draft");
    await user.click(disclosure);
    expect(input).not.toBeVisible();
    await user.tab();
    expect(input).not.toHaveFocus();
    await user.click(disclosure);
    expect(input).toHaveValue("unsaved-draft");
    await user.keyboard(" ");
    expect(disclosure).toHaveAttribute("aria-expanded", "false");
    for (const operation of [saveJevKey, clearJevKey, setJevEnabled, testJevConnection]) {
      expect(operation).not.toHaveBeenCalled();
    }
    expect(Object.values(window.localStorage)).not.toContain("unsaved-draft");
  });

  it("switches routing while collapsed without expanding the settings", async () => {
    const user = userEvent.setup();
    vi.mocked(getJevStatus).mockResolvedValue(status({ has_key: true }));
    vi.mocked(setJevEnabled).mockResolvedValue(status({ has_key: true, enabled: true }));
    render(<JevRoutingControl />);
    await screen.findByText("已配置 Key");
    await user.click(screen.getByRole("switch"));
    expect(setJevEnabled).toHaveBeenCalledExactlyOnceWith(true);
    expect(screen.getByRole("switch")).toBeChecked();
    expect(screen.getByRole("button", { name: "Jev 设置" })).toHaveAttribute("aria-expanded", "false");
  });

  it("shows the cloud, cost, fallback, and plaintext storage boundaries before activation", async () => {
    const user = userEvent.setup();
    render(<JevRoutingControl />);
    expect(await screen.findByText("未配置 Key")).toBeVisible();
    const control = screen.getByRole("switch", { name: "Jev 云端智能分档" });
    expect(control).not.toBeChecked();
    expect(control).toBeDisabled();
    expect(control).toHaveAccessibleDescription(/TypeSafe/);
    await user.click(screen.getByRole("button", { name: "Jev 设置" }));
    expect(screen.getByText(/有长度上限的用户和助手文本/)).toBeVisible();
    expect(screen.getByText(/独立 API 费用/)).toBeVisible();
    expect(screen.getByText(/低置信度/)).toHaveTextContent("不再调用本地分档");
    expect(screen.getByText(/明文/)).toHaveTextContent("不是系统钥匙串");
    expect(screen.getByLabelText("Jev API Key")).toHaveAttribute("type", "password");
    expect(screen.getByRole("button", { name: "测试连接" })).toBeDisabled();
  });

  it("saves a key by keyboard without enabling routing and clears the input", async () => {
    const user = userEvent.setup();
    vi.mocked(saveJevKey).mockResolvedValue(status({ has_key: true }));
    render(<JevRoutingControl />);
    await screen.findByText("未配置 Key");
    await user.click(screen.getByRole("button", { name: "Jev 设置" }));
    const input = screen.getByLabelText("Jev API Key");
    await user.type(input, "  synthetic-jev-key  {Enter}");
    expect(saveJevKey).toHaveBeenCalledExactlyOnceWith("synthetic-jev-key");
    expect(input).toHaveValue("");
    expect(screen.getByText("已配置 Key")).toBeVisible();
    expect(setJevEnabled).not.toHaveBeenCalled();
    expect(screen.getByRole("switch", { name: "Jev 云端智能分档" })).toBeEnabled();
    expect(Object.values(window.localStorage)).not.toContain("synthetic-jev-key");
  });

  it("serializes save, test, enable, and removal and clears a replacement draft on removal", async () => {
    const user = userEvent.setup();
    const saving = deferred<JevStatus>();
    const onEnabledChange = vi.fn();
    vi.mocked(getJevStatus).mockResolvedValue(status({ has_key: true }));
    vi.mocked(saveJevKey).mockReturnValue(saving.promise);
    vi.mocked(setJevEnabled).mockResolvedValue(status({ has_key: true, enabled: true, last_outcome: "ready" }));
    vi.mocked(clearJevKey).mockResolvedValue(status());
    render(<JevRoutingControl onEnabledChange={onEnabledChange} />);
    await screen.findByText("已配置 Key");
    await user.click(screen.getByRole("button", { name: "Jev 设置" }));
    await user.type(screen.getByLabelText("Jev API Key"), "replacement-key");
    const save = screen.getByRole("button", { name: "保存 Key" });
    await user.click(save);
    fireEvent.submit(save.closest("form")!);
    expect(saveJevKey).toHaveBeenCalledOnce();
    for (const name of ["保存 Key", "测试连接", "移除 Key"]) {
      expect(screen.getByRole("button", { name })).toBeDisabled();
    }
    expect(screen.getByRole("switch")).toBeDisabled();
    await act(async () => saving.resolve(status({ has_key: true })));
    await user.click(screen.getByRole("switch"));
    expect(setJevEnabled).toHaveBeenCalledExactlyOnceWith(true);
    expect(onEnabledChange).toHaveBeenLastCalledWith(true);
    expect(screen.getByRole("status")).toHaveTextContent("Jev 已启用，优先于本地分档");
    await user.type(screen.getByLabelText("Jev API Key"), "unused-draft");
    await user.click(screen.getByRole("button", { name: "移除 Key" }));
    expect(clearJevKey).toHaveBeenCalledOnce();
    expect(screen.getByLabelText("Jev API Key")).toHaveValue("");
    expect(screen.getByRole("switch")).not.toBeChecked();
    expect(screen.getByRole("switch")).toBeDisabled();
    expect(onEnabledChange).toHaveBeenLastCalledWith(false);
  });

  it("tests only the saved key, displays the result, and leaves routing disabled", async () => {
    const user = userEvent.setup();
    vi.mocked(getJevStatus).mockResolvedValue(status({ has_key: true }));
    vi.mocked(testJevConnection).mockResolvedValue(status({ has_key: true, last_outcome: "ready", last_latency_ms: 120 }));
    render(<JevRoutingControl />);
    await screen.findByText("已配置 Key");
    await user.click(screen.getByRole("button", { name: "Jev 设置" }));
    expect(screen.getByText(/测试仅发送合成文本/)).toBeVisible();
    await user.click(screen.getByRole("button", { name: "测试连接" }));
    expect(testJevConnection).toHaveBeenCalledExactlyOnceWith();
    expect(screen.getByRole("status")).toHaveTextContent("连接测试通过");
    expect(screen.getByRole("switch")).not.toBeChecked();
    expect(setJevEnabled).not.toHaveBeenCalled();
  });

  it("keeps the previous setting on failure and never renders raw errors or a saved key", async () => {
    const user = userEvent.setup();
    vi.mocked(getJevStatus).mockResolvedValue(status({ has_key: true }));
    vi.mocked(setJevEnabled).mockRejectedValueOnce(new Error("secret remote payload sk-private"))
      .mockResolvedValueOnce(status({ has_key: true, enabled: true }));
    render(<JevRoutingControl />);
    await screen.findByText("已配置 Key");
    await user.click(screen.getByRole("switch"));
    expect(screen.getByRole("alert")).toHaveTextContent("设置未保存，请重试");
    expect(screen.getByRole("alert")).toBeVisible();
    expect(screen.getByRole("button", { name: "Jev 设置" })).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByText(/sk-private/)).not.toBeInTheDocument();
    expect(screen.getByRole("switch")).not.toBeChecked();
    await user.click(screen.getByRole("switch"));
    expect(screen.getByRole("switch")).toBeChecked();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Jev API Key")).toHaveValue("");
  });

  it("offers retry after the first status read fails", async () => {
    const user = userEvent.setup();
    vi.mocked(getJevStatus).mockRejectedValueOnce(new Error("unavailable"))
      .mockResolvedValueOnce(status());
    render(<JevRoutingControl />);
    expect(await screen.findByRole("alert")).toHaveTextContent("无法读取 Jev 状态");
    await user.click(screen.getByRole("button", { name: "重试" }));
    expect(await screen.findByText("未配置 Key")).toBeVisible();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("allows disabling an enabled configuration when its stored key is missing", async () => {
    const user = userEvent.setup();
    vi.mocked(getJevStatus).mockResolvedValue(status({ enabled: true, last_outcome: "missing_key" }));
    vi.mocked(setJevEnabled).mockResolvedValue(status());
    render(<JevRoutingControl />);
    await screen.findByText("未配置 Key");
    const control = screen.getByRole("switch");
    expect(control).toBeEnabled();
    await user.click(control);
    expect(setJevEnabled).toHaveBeenCalledExactlyOnceWith(false);
    expect(control).not.toBeChecked();
  });

  it("refreshes a failed connection test to show the safe authentication outcome while routing is off", async () => {
    const user = userEvent.setup();
    vi.mocked(getJevStatus).mockResolvedValueOnce(status({ has_key: true }))
      .mockResolvedValueOnce(status({ has_key: true, last_outcome: "unauthorized" }));
    vi.mocked(testJevConnection).mockRejectedValue(new Error("private response body"));
    render(<JevRoutingControl />);
    await screen.findByText("已配置 Key");
    await user.click(screen.getByRole("button", { name: "Jev 设置" }));
    await user.click(screen.getByRole("button", { name: "测试连接" }));
    expect(screen.getByRole("alert")).toHaveTextContent("Key 无效");
    expect(screen.getByRole("alert")).toHaveTextContent("连接测试失败");
    expect(screen.getByRole("switch")).not.toBeChecked();
    expect(screen.queryByText(/private response body/)).not.toBeInTheDocument();
  });

  it("refreshes a partial removal failure and ignores an older poll while retaining the failure", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const user = userEvent.setup();
    const stale = deferred<JevStatus>();
    vi.mocked(getJevStatus).mockResolvedValueOnce(status({ has_key: true, enabled: true }))
      .mockReturnValueOnce(stale.promise)
      .mockResolvedValueOnce(status({ has_key: true, enabled: false }));
    vi.mocked(clearJevKey).mockRejectedValue(new Error("synthetic storage failure"));
    render(<JevRoutingControl />);
    await act(async () => {});
    await user.click(screen.getByRole("button", { name: "Jev 设置" }));
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    await user.click(screen.getByRole("button", { name: "移除 Key" }));
    expect(screen.getByRole("switch")).not.toBeChecked();
    expect(screen.getByRole("alert")).toHaveTextContent("Key 未移除");
    expect(screen.getByText("已配置 Key")).toBeVisible();
    await act(async () => stale.resolve(status({ has_key: true, enabled: true })));
    expect(screen.getByRole("switch")).not.toBeChecked();
    expect(screen.getByRole("alert")).toHaveTextContent("Key 未移除");
  });

  it.each([
    ["timeout", "超时"], ["low_confidence", "置信度不足"],
    ["unauthorized", "Key 无效"], ["local_only", "仅本地请求"],
    ["rate_limited", "限流"], ["unsupported", "不支持"],
  ])("explains the %s outcome without raw server details", async (outcome, label) => {
    const user = userEvent.setup();
    vi.mocked(getJevStatus).mockResolvedValue(status({ has_key: true, enabled: true, last_outcome: outcome }));
    render(<JevRoutingControl />);
    await screen.findByText("已配置 Key");
    await user.click(screen.getByRole("button", { name: "Jev 设置" }));
    expect(screen.getByRole(outcome === "unauthorized" ? "alert" : "status")).toHaveTextContent(label);
  });

  it("loads the controls after StrictMode replays effects and rejects the abandoned read", async () => {
    const abandoned = deferred<JevStatus>();
    vi.mocked(getJevStatus).mockReturnValueOnce(abandoned.promise).mockResolvedValue(status({ has_key: true }));
    render(<StrictMode><JevRoutingControl /></StrictMode>);
    expect(await screen.findByText("已配置 Key")).toBeVisible();
    await act(async () => abandoned.resolve(status()));
    expect(screen.getByText("已配置 Key")).toBeVisible();
  });

  it("rejects stale polls after removal and polls only while enabled", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    const user = userEvent.setup();
    const stale = deferred<JevStatus>();
    vi.mocked(getJevStatus).mockResolvedValueOnce(status({ has_key: true, enabled: true }))
      .mockReturnValueOnce(stale.promise).mockResolvedValue(status());
    vi.mocked(clearJevKey).mockResolvedValue(status());
    const view = render(<JevRoutingControl />);
    await act(async () => {});
    await user.click(screen.getByRole("button", { name: "Jev 设置" }));
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(getJevStatus).toHaveBeenCalledTimes(2);
    await user.click(screen.getByRole("button", { name: "移除 Key" }));
    await act(async () => stale.resolve(status({ has_key: true, enabled: true })));
    expect(screen.getByRole("switch")).not.toBeChecked();
    expect(screen.getByText("未配置 Key")).toBeVisible();
    await act(async () => { await vi.advanceTimersByTimeAsync(6_000); });
    expect(getJevStatus).toHaveBeenCalledTimes(2);
    view.unmount();
    await act(async () => { await vi.advanceTimersByTimeAsync(4_000); });
    expect(getJevStatus).toHaveBeenCalledTimes(2);
  });

  it.each([
    ["en", "Jev cloud smart tiers", "Save key", "Jev settings"],
    ["zh-TW", "Jev 雲端智慧分檔", "儲存 Key", "Jev 設定"],
    ["ja", "Jev クラウドスマート分層", "キーを保存", "Jev 設定"],
  ])("provides accessible controls in %s", async (language, switchName, saveName, settingsName) => {
    const user = userEvent.setup();
    window.localStorage.setItem("token-station-language", language);
    render(<LanguageProvider><JevRoutingControl /></LanguageProvider>);
    expect(await screen.findByRole("switch", { name: switchName })).toBeVisible();
    await user.click(screen.getByRole("button", { name: settingsName }));
    expect(screen.getByRole("button", { name: saveName })).toBeVisible();
    expect(screen.getByLabelText("Jev API Key")).toHaveAttribute("autocomplete", "off");
  });
});
