import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getJevStatus, getSemanticStatus, type SemanticStatus } from "../api";
import HomePage from "./HomePage";

vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return { ...actual, getSemanticStatus: vi.fn(), getJevStatus: vi.fn() };
});

beforeEach(() => {
  vi.mocked(getJevStatus).mockClear();
  vi.mocked(getJevStatus).mockResolvedValue({
    enabled: false, has_key: false, model: "jev-latest", timeout_ms: 1500,
    confidence_threshold: 0.7, last_outcome: null, last_tier: null, last_latency_ms: null,
  });
  vi.mocked(getSemanticStatus).mockClear();
  vi.mocked(getSemanticStatus).mockResolvedValue({
    available: true, enabled: true, mode: "route", state: "ready", error: null,
    model_ready: true, timeout_ms: 400, observations: [],
    counts: { classified: 0, disagreements: 0, fallbacks: 0 },
  } satisfies SemanticStatus);
});

function props(): React.ComponentProps<typeof HomePage> {
  return {
    providers: [],
    tiers: {
      high: { upstream: null, model: null },
      mid: { upstream: null, model: null },
      low: { upstream: null, model: null },
    },
    keywords: { high: [], mid: [], low: [] },
    profiles: [], routingMode: "tiered", quotaAccounts: [],
    busy: false, applying: false, configError: null, saveStatus: "",
    onSetRoutingMode: vi.fn(), onApplyDirect: vi.fn(), onSaveQuota: vi.fn(),
    onSaveQuotaPlan: vi.fn(), onViewQuotaUsage: vi.fn(), onTierChange: vi.fn(),
    onSaveProfile: vi.fn(), onDeleteProfile: vi.fn(), onAddKeyword: vi.fn(),
    onRemoveKeyword: vi.fn(), onSave: vi.fn(), onApplyAll: vi.fn(),
  };
}

describe("HomePage routing controls", () => {
  it.each([
    ["tiered", "智能分档"],
    ["direct", "简单路由"],
    ["quota_first", "额度优先"],
  ] as const)("shows the memory switch only for smart tiers while preserving %s mode controls", async (routingMode, selectedLabel) => {
    const user = userEvent.setup();
    const initial = props();
    render(<HomePage {...initial} routingMode={routingMode} />);

    expect(screen.queryByRole("region", { name: "SCX 本地分档" })).not.toBeInTheDocument();
    if (routingMode === "tiered") {
      expect(await screen.findByRole("switch", { name: "本地智能分档" })).toBeEnabled();
      expect(await screen.findByRole("switch", { name: "Jev 云端智能分档" })).toBeVisible();
    } else {
      expect(screen.queryByRole("switch", { name: "本地智能分档" })).not.toBeInTheDocument();
      expect(getSemanticStatus).not.toHaveBeenCalled();
      expect(screen.queryByRole("switch", { name: "Jev 云端智能分档" })).not.toBeInTheDocument();
      expect(getJevStatus).not.toHaveBeenCalled();
    }
    expect(screen.queryByRole("radio", { name: "仅观察" })).not.toBeInTheDocument();
    expect(screen.getByRole("tablist", { name: "路由模式" })).toBeVisible();
    expect(screen.getByRole("tab", { name: selectedLabel })).toHaveAttribute("aria-selected", "true");
    for (const label of ["简单路由", "智能分档", "额度优先"]) {
      expect(screen.getByRole("tab", { name: label })).toBeEnabled();
    }

    const nextMode = routingMode === "direct" ? "tiered" : "direct";
    const nextLabel = nextMode === "tiered" ? "智能分档" : "简单路由";
    await user.click(screen.getByRole("tab", { name: nextLabel }));
    expect(initial.onSetRoutingMode).toHaveBeenCalledWith(nextMode);
  });

  it("shows Jev in the ordinary App when local classification is unavailable", async () => {
    vi.mocked(getSemanticStatus).mockResolvedValue({ available: false } as SemanticStatus);
    render(<HomePage {...props()} />);
    expect(await screen.findByRole("switch", { name: "Jev 云端智能分档" })).toBeVisible();
    expect(screen.queryByRole("switch", { name: "本地智能分档" })).not.toBeInTheDocument();
  });

  it("explains that an enabled Jev takes precedence over the saved local classifier preference", async () => {
    vi.mocked(getJevStatus).mockResolvedValue({
      enabled: true, has_key: true, model: "jev-latest", timeout_ms: 1500,
      confidence_threshold: 0.7, last_outcome: "ready", last_tier: null, last_latency_ms: null,
    });
    render(<HomePage {...props()} />);
    const local = await screen.findByRole("switch", { name: "本地智能分档" });
    expect(local).toBeChecked();
    expect(local).toHaveAccessibleDescription(/本地开关仅保留设置/);
    expect(screen.getByRole("switch", { name: "Jev 云端智能分档" })).toBeChecked();
  });
});
