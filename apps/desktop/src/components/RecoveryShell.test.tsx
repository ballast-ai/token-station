import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecoveryShell from "./RecoveryShell";
import {
  checkUpgrade,
  exportRecoveryBundle,
  getRecoveryDiagnostics,
  getRecoveryState,
  openRecoveryFolder,
  recordFrontendDiagnostic,
  type RecoveryState,
} from "../api";

vi.mock("../api", () => ({
  checkUpgrade: vi.fn(),
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
  vi.mocked(checkUpgrade).mockReset().mockResolvedValue({ current: "1", latest_tag: "2", html_url: "https://example.test", newer: true });
  vi.mocked(recordFrontendDiagnostic).mockReset().mockResolvedValue({ timestamp_ms: 1, kind: "render_error", message: "boom", stack: null, component_stack: null });
});

describe("RecoveryShell", () => {
  it("stays usable without the business DB and exposes only recovery actions", async () => {
    render(<RecoveryShell initialState={safeState} />);
    expect(await screen.findByRole("heading", { name: "Token Station 自救模式" })).toBeInTheDocument();
    expect(screen.getByText(safeState.message as string)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查更新" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "打开备份位置" })).toBeInTheDocument();
    expect(screen.queryByText("启动代理")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByText((_, element) =>
      element?.tagName === "PRE" && Boolean(element.textContent?.includes('"message": "boom"')),
    )).toBeInTheDocument());
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
    expect(await screen.findByText(/render boom/)).toBeInTheDocument();
    await waitFor(() => expect(recordFrontendDiagnostic).toHaveBeenCalledWith(expect.objectContaining({ kind: "render_error", message: "render boom" })));
  });
});
