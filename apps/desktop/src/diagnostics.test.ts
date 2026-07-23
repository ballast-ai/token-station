import { afterEach, describe, expect, it, vi } from "vitest";
import { diagnosticInput, installGlobalDiagnostics } from "./diagnostics";
import { recordFrontendDiagnostic } from "./api";

vi.mock("./api", () => ({ recordFrontendDiagnostic: vi.fn().mockResolvedValue(undefined) }));

afterEach(() => {
  vi.mocked(recordFrontendDiagnostic).mockClear();
});

describe("frontend diagnostics", () => {
  it("keeps an exact field whitelist and a renderer-side size budget", () => {
    const input = diagnosticInput("render_error", new Error(`boom ${"x".repeat(30_000)}`), "component");
    expect(Object.keys(input).sort()).toEqual(["component_stack", "kind", "message", "stack"]);
    expect(input.message.length).toBeLessThanOrEqual(4096);
    expect(input.stack?.length).toBeLessThanOrEqual(12_000);
  });

  it("persists window errors and rejected promises through the local backend", async () => {
    const uninstall = installGlobalDiagnostics();
    window.dispatchEvent(new ErrorEvent("error", { error: new Error("window boom"), message: "window boom" }));
    window.dispatchEvent(new PromiseRejectionEvent("unhandledrejection", { promise: Promise.resolve(), reason: new Error("promise boom") }));
    await Promise.resolve();

    expect(recordFrontendDiagnostic).toHaveBeenCalledWith(expect.objectContaining({ kind: "window_error", message: "window boom" }));
    expect(recordFrontendDiagnostic).toHaveBeenCalledWith(expect.objectContaining({ kind: "unhandled_rejection", message: "promise boom" }));
    uninstall();
  });
});
