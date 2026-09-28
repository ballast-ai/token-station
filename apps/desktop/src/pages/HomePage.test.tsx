import { render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getSemanticStatus, type SemanticStatus } from "../api";
import HomePage from "./HomePage";

vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return { ...actual, getSemanticStatus: vi.fn() };
});

const status: SemanticStatus = {
  available: true, mode: "route", state: "ready", error: null,
  model_ready: true, timeout_ms: 400, observations: [],
  counts: { classified: 0, disagreements: 0, fallbacks: 0 },
};

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

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getSemanticStatus).mockResolvedValue(status);
});

describe("HomePage global classifier controls", () => {
  it("keeps global SCX controls accessible after Home switches from smart tiers to Direct", async () => {
    const initial = props();
    const view = render(<HomePage {...initial} />);
    expect(await screen.findByRole("radio", { name: "参与路由" })).toBeChecked();
    view.rerender(<HomePage {...initial} routingMode="direct" />);
    expect(screen.getByRole("region", { name: "SCX 本地分档" })).toBeVisible();
    expect(screen.getByRole("radio", { name: "关闭" })).toBeEnabled();
    expect(screen.getByRole("radio", { name: "参与路由" })).toBeChecked();
    expect(screen.getByText("立即应用于使用智能分档的全局及 Agent 路由。")).toBeVisible();
  });

  it("does not show experimental controls on the stable App in Direct mode", async () => {
    vi.mocked(getSemanticStatus).mockResolvedValue({ ...status, available: false });
    render(<HomePage {...props()} routingMode="direct" />);
    await vi.waitFor(() => expect(getSemanticStatus).toHaveBeenCalledOnce());
    expect(screen.queryByRole("region", { name: "SCX 本地分档" })).not.toBeInTheDocument();
  });
});
