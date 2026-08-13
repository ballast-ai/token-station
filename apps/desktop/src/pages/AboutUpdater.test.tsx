import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import About from "./About";
import {
  checkDesktopUpdate,
  installDesktopUpdateAndRestart,
  listenDesktopUpdateProgress,
} from "../api";
import { ErrorToastProvider } from "../components/ErrorToast";

vi.mock("../api", () => ({
  checkDesktopUpdate: vi.fn(),
  installDesktopUpdateAndRestart: vi.fn(),
  listenDesktopUpdateProgress: vi.fn(),
}));

describe("desktop in-app update", () => {
  beforeEach(() => {
    vi.mocked(checkDesktopUpdate).mockReset();
    vi.mocked(installDesktopUpdateAndRestart).mockReset();
    vi.mocked(listenDesktopUpdateProgress).mockReset().mockResolvedValue(() => undefined);
  });

  it("更新进度监听注册失败时进入全局错误弹窗", async () => {
    vi.mocked(listenDesktopUpdateProgress).mockRejectedValue(
      new Error("update progress subscription failed"),
    );

    render(
      <ErrorToastProvider>
        <About desktopVersion="1.1.2" coreVersion="0.2.0" />
      </ErrorToastProvider>,
    );

    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent("Token Station 无法检查更新");
  });

  it("复制发布链接失败时只在左下角提示且不误报已复制", async () => {
    vi.mocked(checkDesktopUpdate).mockResolvedValue({
      status: "update_available",
      current_version: "1.1.2",
      version: "1.1.3",
      notes: null,
      pub_date: null,
      release_url: "https://example.test/v1.1.3",
      message: null,
    });
    const user = userEvent.setup();
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: vi.fn().mockRejectedValue(new Error("clipboard denied")) },
    });

    render(
      <ErrorToastProvider>
        <About desktopVersion="1.1.2" coreVersion="0.2.0" />
      </ErrorToastProvider>,
    );

    await user.click(screen.getByRole("button", { name: "检查更新" }));
    await user.click(await screen.findByRole("button", { name: "复制链接" }));

    const message = "无法复制发布链接。请检查系统剪贴板权限，然后重试。";
    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent(message);
    expect(screen.getByRole("button", { name: "复制链接" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "已复制" })).not.toBeInTheDocument();
    expect(screen.queryByText(message, { selector: ".banner" })).not.toBeInTheDocument();
  });

  it("shows signed download progress while installation is in flight", async () => {
    let onProgress: ((progress: { downloaded: number; total: number | null }) => void) | undefined;
    vi.mocked(listenDesktopUpdateProgress).mockImplementation(async (handler) => {
      onProgress = handler;
      return () => undefined;
    });
    vi.mocked(checkDesktopUpdate).mockResolvedValue({
      status: "update_available",
      current_version: "1.1.2",
      version: "1.1.3",
      notes: null,
      pub_date: null,
      release_url: "https://example.test/v1.1.3",
      message: null,
    });
    vi.mocked(installDesktopUpdateAndRestart).mockImplementation(() => new Promise(() => undefined));

    const user = userEvent.setup();
    render(<About desktopVersion="1.1.2" coreVersion="0.2.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    await user.click(await screen.findByRole("button", { name: "下载并更新到 1.1.3" }));
    await user.click(screen.getByRole("button", { name: "确认更新并重启" }));
    onProgress?.({ downloaded: 50, total: 100 });

    expect(await screen.findByRole("progressbar")).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByText("已下载 50%")).toBeInTheDocument();
  });

  it("requires confirmation before installing an available signed update", async () => {
    vi.mocked(checkDesktopUpdate).mockResolvedValue({
      status: "update_available",
      current_version: "1.1.2",
      version: "1.1.3",
      notes: "安全更新",
      pub_date: "2026-08-06T07:00:00Z",
      release_url: "https://github.com/ballast-ai/token-station/releases/tag/v1.1.3",
      message: null,
    });
    vi.mocked(installDesktopUpdateAndRestart).mockResolvedValue(true);

    const user = userEvent.setup();
    render(<About desktopVersion="1.1.2" coreVersion="0.2.0" />);

    await user.click(screen.getByRole("button", { name: "检查更新" }));

    expect(await screen.findByText("发现新版本 1.1.3")).toBeInTheDocument();
    expect(screen.getByText("安全更新")).toBeInTheDocument();
    expect(screen.getByText("发布日期：2026-08-06T07:00:00Z")).toBeInTheDocument();
    await user.click(
      screen.getByRole("button", { name: "下载并更新到 1.1.3" }),
    );

    expect(
      await screen.findByRole("alertdialog", { name: "安装应用更新？" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "取消" })).toHaveFocus();
    expect(installDesktopUpdateAndRestart).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "确认更新并重启" }));
    expect(installDesktopUpdateAndRestart).toHaveBeenCalledWith("1.1.3");
  });

  it("returns to update checking when the confirmed latest version changes", async () => {
    vi.mocked(checkDesktopUpdate).mockResolvedValue({
      status: "update_available",
      current_version: "1.1.2",
      version: "1.1.3",
      notes: "安全更新",
      pub_date: "2026-08-06T07:00:00Z",
      release_url: "https://github.com/ballast-ai/token-station/releases/tag/v1.1.3",
      message: null,
    });
    vi.mocked(installDesktopUpdateAndRestart).mockRejectedValue(
      "update_version_changed: 已确认更新到 1.1.3，但当前可用版本已变为 1.1.4；请重新检查并确认",
    );

    const user = userEvent.setup();
    render(<About desktopVersion="1.1.2" coreVersion="0.2.0" />);
    await user.click(screen.getByRole("button", { name: "检查更新" }));
    await user.click(await screen.findByRole("button", { name: "下载并更新到 1.1.3" }));
    await user.click(screen.getByRole("button", { name: "确认更新并重启" }));

    expect(await screen.findByText("可用更新已经发生变化。请重新检查后再确认安装。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "下载并更新到 1.1.3" })).not.toBeInTheDocument();
    expect(screen.getByText(/releases\/tag\/v1.1.3/)).toBeInTheDocument();
  });

  it("localizes a Chinese update result message in English mode", async () => {
    window.localStorage.setItem("token-station-language", "en");
    vi.mocked(checkDesktopUpdate).mockResolvedValue({
      status: "unavailable",
      current_version: "1.1.2",
      version: null,
      notes: null,
      pub_date: null,
      release_url: "https://github.com/ballast-ai/token-station/releases",
      message: "当前构建没有内置官方更新公钥，不能在 App 内安装更新。",
    });

    const user = userEvent.setup();
    render(<About desktopVersion="1.1.2" coreVersion="0.2.0" />);
    await user.click(screen.getByRole("button", { name: "Check for updates" }));

    expect(await screen.findByText(
      "This build cannot install updates because it does not include the official update public key. Download the app from the Releases page instead.",
    )).toBeInTheDocument();
    expect(screen.queryByText(/当前构建没有内置官方更新公钥/)).not.toBeInTheDocument();
  });
});
