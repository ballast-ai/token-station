import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ErrorToastProvider, useErrorToast } from "./ErrorToast";

function Harness() {
  const { showError, showInfo, showSuccess } = useErrorToast();
  return (
    <>
      <button type="button" onClick={() => showError("代理启动失败", "serve-runtime")}>first</button>
      <button type="button" onClick={() => showError("代理启动失败", "serve-runtime")}>duplicate</button>
      <button type="button" onClick={() => showError("供应商保存失败", "provider-save")}>second</button>
      <button type="button" onClick={() => showInfo("正在启动代理…", "serve-toggle")}>progress</button>
      <button type="button" onClick={() => showSuccess("代理已启动", "serve-toggle")}>success</button>
    </>
  );
}

describe("ErrorToast", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.useRealTimers();
    document.documentElement.lang = "zh-CN";
  });

  it.each([
    ["zh-TW", "通知", "關閉通知"],
    ["ja", "通知", "通知を閉じる"],
  ] as const)("localizes notification accessibility copy for %s", (language, viewportName, closeName) => {
    document.documentElement.lang = language;
    render(<ErrorToastProvider><Harness /></ErrorToastProvider>);
    fireEvent.click(screen.getByRole("button", { name: "first" }));

    expect(screen.getByLabelText(viewportName)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: closeName })).toBeInTheDocument();
  });

  it("reacts when the active document language changes", async () => {
    document.documentElement.lang = "zh-CN";
    render(<ErrorToastProvider><Harness /></ErrorToastProvider>);
    await act(async () => {
      document.documentElement.lang = "ja";
      await Promise.resolve();
    });
    fireEvent.click(screen.getByRole("button", { name: "first" }));

    expect(screen.getByRole("button", { name: "通知を閉じる" })).toBeInTheDocument();
  });

  it("uses English immediately when the document switches from Chinese to English", async () => {
    document.documentElement.lang = "zh-CN";
    window.localStorage.setItem("token-station-language", "zh-CN");
    render(<ErrorToastProvider><Harness /></ErrorToastProvider>);
    await act(async () => {
      document.documentElement.lang = "en";
      await Promise.resolve();
    });
    fireEvent.click(screen.getByRole("button", { name: "first" }));

    expect(screen.getByRole("button", { name: "Close notification" })).toBeInTheDocument();
  });

  it("在左下角去重堆叠操作错误并允许手动关闭", () => {
    render(<ErrorToastProvider><Harness /></ErrorToastProvider>);

    fireEvent.click(screen.getByRole("button", { name: "first" }));
    fireEvent.click(screen.getByRole("button", { name: "duplicate" }));
    fireEvent.click(screen.getByRole("button", { name: "second" }));

    const viewport = screen.getByTestId("error-toast-viewport");
    expect(viewport).toHaveClass("error-toast-viewport");
    expect(screen.getAllByRole("alert")).toHaveLength(2);
    expect(screen.getAllByText("代理启动失败")).toHaveLength(1);

    fireEvent.click(screen.getAllByRole("button", { name: "关闭提示" })[0]);
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    expect(screen.queryByText("代理启动失败")).toBeNull();
  });

  it("用同一稳定 ID 将进行中提示更新为成功并重新计算 8 秒寿命", () => {
    render(<ErrorToastProvider><Harness /></ErrorToastProvider>);

    fireEvent.click(screen.getByRole("button", { name: "progress" }));
    const viewport = screen.getByTestId("error-toast-viewport");
    const progress = within(viewport).getByRole("status");
    expect(progress).toHaveTextContent("正在启动代理…");
    expect(progress).toHaveClass("is-info");

    act(() => vi.advanceTimersByTime(7_500));
    fireEvent.click(screen.getByRole("button", { name: "success" }));
    const success = within(viewport).getByRole("status");
    expect(success).toHaveTextContent("代理已启动");
    expect(success).toHaveClass("is-success");
    expect(success).not.toHaveClass("is-fading");

    act(() => vi.advanceTimersByTime(6_999));
    expect(success).not.toHaveClass("is-fading");
    act(() => vi.advanceTimersByTime(1));
    expect(success).toHaveClass("is-fading");
    act(() => vi.advanceTimersByTime(1_000));
    expect(within(viewport).queryByRole("status")).toBeNull();
  });

  it("8 秒内渐隐并清理，且不移动当前焦点", () => {
    render(<ErrorToastProvider><Harness /></ErrorToastProvider>);
    const trigger = screen.getByRole("button", { name: "first" });
    trigger.focus();
    fireEvent.click(trigger);
    const toast = screen.getByRole("alert");
    expect(trigger).toHaveFocus();

    act(() => vi.advanceTimersByTime(6_999));
    expect(toast).not.toHaveClass("is-fading");
    act(() => vi.advanceTimersByTime(1));
    expect(toast).toHaveClass("is-fading");
    act(() => vi.advanceTimersByTime(1_000));
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
