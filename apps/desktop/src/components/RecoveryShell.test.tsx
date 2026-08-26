import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import RecoveryShell from "./RecoveryShell";
import {
  checkDesktopUpdate,
  installDesktopUpdateAndRestart,
  recordFrontendDiagnostic,
  type RecoveryState,
} from "../api";

vi.mock("../api", () => ({
  checkDesktopUpdate: vi.fn(),
  installDesktopUpdateAndRestart: vi.fn(),
  recordFrontendDiagnostic: vi.fn(),
}));

const newerSchemaState: RecoveryState = {
  mode: "safe",
  reason_code: "metrics_schema_newer",
  message: "指标库 schema v13 高于当前程序支持的 v10",
  found_schema: 13,
  supported_schema: 10,
  metrics_path: "/data/metrics.sqlite",
  backup_dir: "/data",
  local_only: true,
};

beforeEach(() => {
  vi.mocked(checkDesktopUpdate).mockReset().mockResolvedValue({
    status: "update_available",
    current_version: "1.2.4",
    version: "1.3.4",
    notes: null,
    pub_date: null,
    release_url: "https://example.test",
    message: null,
  });
  vi.mocked(installDesktopUpdateAndRestart).mockReset().mockResolvedValue(true);
  vi.mocked(recordFrontendDiagnostic).mockReset().mockResolvedValue({
    timestamp_ms: 1,
    kind: "render_error",
    message: "boom",
    stack: null,
    component_stack: null,
  });
});

describe("safe startup screen", () => {
  it("turns a newer local data format into one clear update path", async () => {
    render(<RecoveryShell initialState={newerSchemaState} />);

    expect(screen.getByRole("heading", { name: "需要更新 Token Station" })).toBeInTheDocument();
    expect(screen.getByText("本机数据由较新版本创建。更新后即可继续使用，数据不会被删除。")).toBeInTheDocument();
    await waitFor(() => expect(checkDesktopUpdate).toHaveBeenCalledOnce());
    expect(await screen.findByRole("button", { name: "更新到 1.3.4" })).toBeInTheDocument();

    expect(screen.queryByText(/自救|恢复模式|schema|脱敏诊断|备份位置|只读导出/i)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /导出|诊断|备份/ })).not.toBeInTheDocument();
  });

  it("installs the confirmed update and restarts", async () => {
    const user = userEvent.setup();
    render(<RecoveryShell initialState={newerSchemaState} />);

    await user.click(await screen.findByRole("button", { name: "更新到 1.3.4" }));
    expect(screen.getByRole("alertdialog", { name: "安装应用更新？" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "确认更新并重启" }));

    await waitFor(() => expect(installDesktopUpdateAndRestart).toHaveBeenCalledWith("1.3.4"));
  });

  it("shows render failures on a separate reload screen", async () => {
    render(<RecoveryShell initialError={new Error("render boom")} />);

    expect(screen.getByRole("heading", { name: "界面加载失败" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新加载" })).toBeInTheDocument();
    expect(screen.queryByText(/更新 Token Station|自救|导出|schema/i)).not.toBeInTheDocument();
    expect(checkDesktopUpdate).not.toHaveBeenCalled();
    await waitFor(() => expect(recordFrontendDiagnostic).toHaveBeenCalledWith(
      expect.objectContaining({ kind: "render_error", message: "render boom" }),
    ));
  });
});
