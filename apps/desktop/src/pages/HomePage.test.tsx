import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getSemanticStatus, type SemanticStatus } from "../api";
import HomePage from "./HomePage";

vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return { ...actual, getSemanticStatus: vi.fn() };
});

beforeEach(() => {
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
  ] as const)("keeps the compact memory switch and routing selection in %s mode", async (routingMode, selectedLabel) => {
    const user = userEvent.setup();
    const initial = props();
    render(<HomePage {...initial} routingMode={routingMode} />);

    expect(screen.queryByRole("region", { name: "SCX 本地分档" })).not.toBeInTheDocument();
    expect(await screen.findByRole("switch", { name: "本地智能分档" })).toBeEnabled();
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
});
