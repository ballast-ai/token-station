import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecoveryShell from "./RecoveryShell";
import {
  checkDesktopUpdate,
  exportRecoveryBundle,
  getRecoveryDiagnostics,
  getRecoveryState,
  openRecoveryFolder,
  recordFrontendDiagnostic,
  installDesktopUpdateAndRestart,
  type RecoveryState,
} from "../api";

vi.mock("../api", () => ({
  checkDesktopUpdate: vi.fn(),
  installDesktopUpdateAndRestart: vi.fn(),
  exportRecoveryBundle: vi.fn(),
  getRecoveryDiagnostics: vi.fn(),
  getRecoveryState: vi.fn(),
  openRecoveryFolder: vi.fn(),
  recordFrontendDiagnostic: vi.fn(),
}));

const safeState: RecoveryState = {
  mode: "safe",
  reason_code: "metrics_schema_newer",
  message: "指标库 schema v12 高于当前程序支持的 v4",
  found_schema: 12,
  supported_schema: 4,
  metrics_path: "/data/metrics.sqlite",
  backup_dir: "/data",
  local_only: true,
};

beforeEach(() => {
  vi.mocked(getRecoveryState).mockReset().mockResolvedValue(safeState);
  vi.mocked(getRecoveryDiagnostics).mockReset().mockResolvedValue({
    recovery: safeState,
    frontend_events: [{ timestamp_ms: 1, kind: "window_error", message: "boom", stack: null, component_stack: null }],
    export_includes: ["原始本地指标库", "脱敏诊断清单"],
    local_only: true,
    redacted: true,
    auto_upload: false,
  });
  vi.mocked(exportRecoveryBundle).mockReset().mockResolvedValue("/data/recovery-export-1");
  vi.mocked(openRecoveryFolder).mockReset().mockResolvedValue("/data");
  vi.mocked(checkDesktopUpdate).mockReset().mockResolvedValue({
    status: "update_available",
    current_version: "1",
    version: "2",
    notes: "安全更新",
    pub_date: null,
    release_url: "https://example.test",
    message: null,
  });
  vi.mocked(installDesktopUpdateAndRestart).mockReset().mockResolvedValue(true);
  vi.mocked(recordFrontendDiagnostic).mockReset().mockResolvedValue({ timestamp_ms: 1, kind: "render_error", message: "boom", stack: null, component_stack: null });
});

describe("RecoveryShell", () => {
  it("stays usable without the business DB and exposes only recovery actions", async () => {
    render(<RecoveryShell initialState={safeState} />);
    expect(await screen.findByRole("heading", { name: "Token Station 自救模式" })).toBeInTheDocument();
    expect(screen.getByText("本地数据无法安全打开。请使用自救模式检查或导出本地数据。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "打开备份位置" })).toBeInTheDocument();
    expect(screen.queryByText("启动代理")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByText((_, element) =>
      element?.tagName === "PRE" && Boolean(element.textContent?.includes('"message": "boom"')),
    )).toBeInTheDocument());
  });

  it("checks and installs a signed update only after explicit recovery-mode confirmation", async () => {
    const user = userEvent.setup();
    render(<RecoveryShell initialState={safeState} />);
    await user.click(await screen.findByRole("button", { name: "检查更新" }));
    expect(await screen.findByText("发现新版本 2")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "下载并更新到 2" }));
    expect(screen.getByRole("alertdialog", { name: "安装应用更新？" })).toBeInTheDocument();
    expect(installDesktopUpdateAndRestart).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "确认更新并重启" }));
    await waitFor(() => expect(installDesktopUpdateAndRestart).toHaveBeenCalledWith("2"));
  });

  it("returns to update checking when the recovery-mode candidate changes", async () => {
    vi.mocked(installDesktopUpdateAndRestart).mockRejectedValue(
      "update_version_changed: 已确认更新到 2，但当前可用版本已变为 3；请重新检查并确认",
    );
    const user = userEvent.setup();
    render(<RecoveryShell initialState={safeState} />);
    await user.click(await screen.findByRole("button", { name: "检查更新" }));
    await user.click(await screen.findByRole("button", { name: "下载并更新到 2" }));
    await user.click(screen.getByRole("button", { name: "确认更新并重启" }));

    expect(await screen.findByText("可用更新已经发生变化。请重新检查后再确认安装。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "下载并更新到 2" })).not.toBeInTheDocument();
    expect(screen.getByText(/https:\/\/example\.test/)).toBeInTheDocument();
  });

  it("requires an explicit raw-data confirmation before export", async () => {
    const user = userEvent.setup();
    render(<RecoveryShell initialState={safeState} />);
    const exportButton = await screen.findByRole("button", { name: "导出自救包" });
    expect(exportButton).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: /确认导出/ }));
    await user.click(exportButton);
    await waitFor(() => expect(exportRecoveryBundle).toHaveBeenCalledWith(true));
    expect(await screen.findByText(/recovery-export-1/)).toBeInTheDocument();
  });

  it("builds copied diagnostics only from the backend-redacted preview", async () => {
    const user = userEvent.setup();
    const privateState = {
      ...safeState,
      metrics_path: "/Users/private-person/Library/Application Support/token-station/metrics.sqlite",
      backup_dir: "/Users/private-person/Library/Application Support/token-station",
    };
    vi.mocked(getRecoveryDiagnostics).mockResolvedValueOnce({
      recovery: {
        ...privateState,
        metrics_path: "$DATA_DIR/metrics.sqlite",
        backup_dir: "$DATA_DIR",
      },
      frontend_events: [],
      export_includes: ["脱敏诊断清单"],
      local_only: true,
      redacted: true,
      auto_upload: false,
    });
    const writeText = vi.fn().mockResolvedValue(undefined);
    vi.spyOn(navigator, "clipboard", "get").mockReturnValue({ writeText } as unknown as Clipboard);

    render(<RecoveryShell initialState={privateState} />);
    await waitFor(() => expect(screen.getByText((_, element) =>
      element?.tagName === "PRE" && Boolean(element.textContent?.includes("$DATA_DIR/metrics.sqlite")),
    )).toBeInTheDocument());
    expect(screen.queryByText((_, element) =>
      element?.tagName === "PRE" && Boolean(element.textContent?.includes("private-person")),
    )).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "复制脱敏诊断" }));
    expect(writeText).toHaveBeenCalledOnce();
    expect(writeText.mock.calls[0][0]).toContain("$DATA_DIR/metrics.sqlite");
    expect(writeText.mock.calls[0][0]).not.toContain("private-person");
  });

  it("turns a render crash into the same recovery surface and records it", async () => {
    render(<RecoveryShell initialState={{ ...safeState, mode: "normal", reason_code: null, message: null }} initialError={new Error("render boom")} />);
    expect(await screen.findByText("操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。")).toBeInTheDocument();
    await waitFor(() => expect(recordFrontendDiagnostic).toHaveBeenCalledWith(expect.objectContaining({ kind: "render_error", message: "render boom" })));
  });
});
