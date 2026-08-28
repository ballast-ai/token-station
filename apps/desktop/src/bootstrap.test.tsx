import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  AppBootstrap,
  LAUNCH_EXIT_MS,
  LAUNCH_MINIMUM_MS,
  RecoveryBoundary,
} from "./bootstrap";
import { getRecoveryState } from "./api";

vi.mock("./api", () => ({ getRecoveryState: vi.fn() }));
vi.mock("./App", () => ({
  default: ({ onStartupSettled }: { onStartupSettled?: (outcome: "ready" | "actionable-error") => void }) => (
    <div>
      normal application
      <button type="button" onClick={() => onStartupSettled?.("ready")}>settle application</button>
      <button type="button" onClick={() => onStartupSettled?.("actionable-error")}>fail application</button>
    </div>
  ),
}));
vi.mock("./components/RecoveryShell", () => ({
  default: ({ initialState, initialError }: { initialState?: { mode: string }; initialError?: Error }) => (
    <div>recovery shell · {initialState?.mode ?? "error"} · {initialError?.message}</div>
  ),
}));

const normal = {
  mode: "normal" as const, reason_code: null, message: null,
  found_schema: null, supported_schema: 4, metrics_path: "/data/db", backup_dir: "/data", local_only: true,
};

beforeEach(() => vi.mocked(getRecoveryState).mockReset());
afterEach(() => vi.useRealTimers());

describe("recovery bootstrap", () => {
  it("selects the safe shell before the business application mounts", async () => {
    vi.mocked(getRecoveryState).mockResolvedValue({ ...normal, mode: "safe", reason_code: "metrics_schema_newer", message: "future" });
    render(<AppBootstrap />);
    expect(await screen.findByText(/recovery shell · safe/)).toBeInTheDocument();
    expect(screen.queryByText("normal application")).not.toBeInTheDocument();
  });

  it("mounts the application only after a normal compatibility result", async () => {
    vi.mocked(getRecoveryState).mockResolvedValue(normal);
    render(<AppBootstrap />);
    expect(await screen.findByText("normal application")).toBeInTheDocument();
  });

  it("keeps the independent launch screen until presentation and application startup finish", async () => {
    vi.useFakeTimers();
    vi.mocked(getRecoveryState).mockResolvedValue(normal);
    render(<AppBootstrap />);

    await act(async () => Promise.resolve());
    expect(screen.getByRole("status", { name: "Opening Token Station" })).toBeInTheDocument();
    expect(screen.getByText("normal application")).toBeInTheDocument();

    fireEvent.click(screen.getByText("settle application"));
    act(() => vi.advanceTimersByTime(LAUNCH_MINIMUM_MS - 1));
    expect(screen.getByTestId("launch-screen")).toHaveAttribute("data-phase", "presenting");

    act(() => vi.advanceTimersByTime(1));
    expect(screen.getByTestId("launch-screen")).toHaveAttribute("data-phase", "exiting");

    act(() => vi.advanceTimersByTime(LAUNCH_EXIT_MS));
    expect(screen.queryByTestId("launch-screen")).toBeNull();
  });

  it("makes the staged application inert until the launch screen leaves", async () => {
    vi.useFakeTimers();
    vi.mocked(getRecoveryState).mockResolvedValue(normal);
    const { container } = render(<AppBootstrap />);

    await act(async () => Promise.resolve());
    const stage = container.querySelector(".launch-app-stage");
    expect(stage).toHaveAttribute("inert");

    fireEvent.click(screen.getByText("settle application"));
    act(() => vi.advanceTimersByTime(LAUNCH_MINIMUM_MS));
    act(() => vi.advanceTimersByTime(LAUNCH_EXIT_MS));
    expect(stage).not.toHaveAttribute("inert");
  });

  it("exposes an actionable startup error without decorative timing", async () => {
    vi.useFakeTimers();
    vi.mocked(getRecoveryState).mockResolvedValue(normal);
    render(<AppBootstrap />);

    await act(async () => Promise.resolve());
    fireEvent.click(screen.getByText("fail application"));

    expect(screen.queryByTestId("launch-screen")).toBeNull();
  });

  it("bypasses decorative launch timing for the safe recovery shell", async () => {
    vi.useFakeTimers();
    vi.mocked(getRecoveryState).mockResolvedValue({
      ...normal,
      mode: "safe",
      reason_code: "metrics_schema_newer",
      message: "future",
    });
    render(<AppBootstrap />);

    await act(async () => Promise.resolve());
    expect(screen.getByText(/recovery shell · safe/)).toBeInTheDocument();
    expect(screen.queryByTestId("launch-screen")).toBeNull();
  });

  it("skips artificial launch timing when reduced motion is requested", async () => {
    const originalMatchMedia = window.matchMedia;
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn().mockReturnValue({ matches: true }),
    });
    vi.mocked(getRecoveryState).mockResolvedValue(normal);

    try {
      render(<AppBootstrap />);
      await act(async () => Promise.resolve());
      fireEvent.click(screen.getByText("settle application"));
      await act(async () => Promise.resolve());
      expect(screen.queryByTestId("launch-screen")).toBeNull();
    } finally {
      if (originalMatchMedia) {
        Object.defineProperty(window, "matchMedia", {
          configurable: true,
          value: originalMatchMedia,
        });
      } else {
        Reflect.deleteProperty(window, "matchMedia");
      }
    }
  });

  it("turns a render exception into the recovery shell", async () => {
    const Broken = () => { throw new Error("controlled crash"); };
    render(<RecoveryBoundary><Broken /></RecoveryBoundary>);
    expect(await screen.findByText(/recovery shell · error · controlled crash/)).toBeInTheDocument();
  });
});
