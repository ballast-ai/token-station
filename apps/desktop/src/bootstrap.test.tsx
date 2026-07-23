import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AppBootstrap, RecoveryBoundary } from "./bootstrap";
import { getRecoveryState } from "./api";

vi.mock("./api", () => ({ getRecoveryState: vi.fn() }));
vi.mock("./App", () => ({ default: () => <div>normal application</div> }));
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

  it("turns a render exception into the recovery shell", async () => {
    const Broken = () => { throw new Error("controlled crash"); };
    render(<RecoveryBoundary><Broken /></RecoveryBoundary>);
    expect(await screen.findByText(/recovery shell · error · controlled crash/)).toBeInTheDocument();
  });
});
