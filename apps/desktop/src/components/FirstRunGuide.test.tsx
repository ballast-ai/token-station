import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { expect, it, vi } from "vitest";
import FirstRunGuide, {
  FIRST_RUN_GUIDE_STORAGE_KEY,
  FIRST_RUN_GUIDE_VERSION,
  FIRST_RUN_TUTORIAL_CHOICE_STORAGE_KEY,
  FirstRunCompletionDialog,
  FirstRunTutorialPrompt,
  markFirstRunTutorialChoice,
  readFirstRunTutorialChoice,
  shouldShowFirstRunTutorialPrompt,
  shouldOpenFirstRunGuide,
} from "./FirstRunGuide";
import { LanguageProvider } from "./LanguageProvider";
import TierRouteEditor from "./TierRouteEditor";

function rect(top: number, left: number, width: number, height: number): DOMRect {
  return {
    x: left,
    y: top,
    top,
    left,
    right: left + width,
    bottom: top + height,
    width,
    height,
    toJSON: () => ({}),
  };
}

it("升级教程版本，使只看过旧版的用户看到新增步骤", () => {
  const storage = new Map([[FIRST_RUN_GUIDE_STORAGE_KEY, "spotlight-setup-v3"]]);
  expect(shouldOpenFirstRunGuide({ getItem: (key) => storage.get(key) ?? null })).toBe(true);
  storage.set(FIRST_RUN_GUIDE_STORAGE_KEY, FIRST_RUN_GUIDE_VERSION);
  expect(shouldOpenFirstRunGuide({ getItem: (key) => storage.get(key) ?? null })).toBe(false);
});

it("仅在从未处理过教程的新用户首次打开时询问是否需要", () => {
  const storage = new Map<string, string>();
  const reader = { getItem: (key: string) => storage.get(key) ?? null };
  const writer = { setItem: (key: string, value: string) => storage.set(key, value) };

  expect(shouldShowFirstRunTutorialPrompt(reader)).toBe(true);
  markFirstRunTutorialChoice("started", writer);
  expect(readFirstRunTutorialChoice(reader)).toBe("started");
  expect(shouldShowFirstRunTutorialPrompt(reader)).toBe(false);

  storage.clear();
  storage.set(FIRST_RUN_GUIDE_STORAGE_KEY, "spotlight-setup-v3");
  expect(shouldShowFirstRunTutorialPrompt(reader)).toBe(false);
  expect(storage.has(FIRST_RUN_TUTORIAL_CHOICE_STORAGE_KEY)).toBe(false);
});

it("首次教程询问提供开始和暂不需要两个明确选择", async () => {
  const onStart = vi.fn();
  const onDecline = vi.fn();
  const user = userEvent.setup();

  render(
    <LanguageProvider>
      <FirstRunTutorialPrompt open onStart={onStart} onDecline={onDecline} />
    </LanguageProvider>,
  );

  const dialog = screen.getByRole("dialog", { name: "需要新手教程吗？" });
  expect(dialog).toHaveTextContent("只会在第一次打开时询问");
  await user.click(within(dialog).getByRole("button", { name: "开始教程" }));
  expect(onStart).toHaveBeenCalledOnce();
  expect(onDecline).not.toHaveBeenCalled();
});

it("在选择 Agent 前区分本机发现结果与全部支持清单", async () => {
  const onTargetAction = vi.fn();
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "agent-list") {
        return rect(96, 24, 280, 560);
      }
      return rect(0, 0, 0, 0);
    });

  try {
    render(
      <LanguageProvider>
        <section data-onboarding-target="agent-list">Claude Code</section>
        <FirstRunGuide
          open
          microStep="agent-discovery-scope"
          canSkipAgent={false}
          onTargetAction={onTargetAction}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>,
    );

    const coachmark = await screen.findByRole("dialog", {
      name: "这里仅显示扫描到的 Agent",
    });
    expect(coachmark).toHaveTextContent("本机当前扫描到的 Agent");
    expect(coachmark).toHaveTextContent("Token Station 支持的全部 Agent");
    expect(coachmark).toHaveTextContent("设置 → Agent 显示");
    expect(screen.getByText("Claude Code")).toHaveAttribute(
      "data-onboarding-active",
      "true",
    );

    await userEvent.setup().click(
      within(coachmark).getByRole("button", { name: "知道了，选择 Agent" }),
    );
    expect(onTargetAction).toHaveBeenCalledOnce();
  } finally {
    getBoundingClientRect.mockRestore();
  }
});

it("高亮主页菜单并说明之后如何切换回主页", async () => {
  const onTargetAction = vi.fn();
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "home-entry") {
        return rect(16, 12, 148, 34);
      }
      return rect(0, 0, 0, 0);
    });

  try {
    render(
      <LanguageProvider>
        <button type="button" data-onboarding-target="home-entry">
          主页
        </button>
        <main>概览真实内容</main>
        <FirstRunGuide
          open
          microStep="overview"
          canSkipAgent={false}
          onTargetAction={onTargetAction}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>,
    );

    const coachmark = await screen.findByRole("dialog", { name: "从这里随时回到主页" });
    expect(coachmark).toHaveStyle({ left: "16px", top: "68px" });
    expect(coachmark).toHaveTextContent(
      "无论当前在哪个页面，点击顶部“主页”都能返回主页",
    );
    expect(coachmark).toHaveTextContent("代理状态、当前路由、请求与成本");
    expect(coachmark).not.toHaveTextContent("设置 → 关于 → 重新查看新手引导");
    expect(screen.getByRole("button", { name: "主页" })).toHaveAttribute(
      "data-onboarding-active",
      "true",
    );
    expect(screen.getByText("概览真实内容")).not.toHaveAttribute("data-onboarding-active");
    expect(document.querySelector(".first-run-spotlight-hole-blocker")).not.toBeNull();
    const user = userEvent.setup();
    await user.tab();
    expect(within(coachmark).getByRole("button", { name: "稍后继续" })).toHaveFocus();
    await user.click(within(coachmark).getByRole("button", { name: "知道了，开始配置" }));
    expect(onTargetAction).toHaveBeenCalledOnce();
  } finally {
    getBoundingClientRect.mockRestore();
  }
});

it("在紧凑窗口中限制概览引导卡宽度并保持在可视区内", async () => {
  const originalInnerWidth = window.innerWidth;
  const originalInnerHeight = window.innerHeight;
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 580 });
  Object.defineProperty(window, "innerHeight", { configurable: true, value: 432 });
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "home-entry") {
        return rect(70, 80, 310, 66);
      }
      return rect(0, 0, 0, 0);
    });

  try {
    render(
      <LanguageProvider>
        <button type="button" data-onboarding-target="home-entry">
          主页
        </button>
        <FirstRunGuide
          open
          microStep="overview"
          canSkipAgent={false}
          onTargetAction={() => {}}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>,
    );

    const coachmark = await screen.findByRole("dialog", { name: "从这里随时回到主页" });
    expect(coachmark).toHaveStyle({ width: "360px", left: "80px", top: "154px" });
    const left = Number.parseFloat(coachmark.style.left);
    const width = Number.parseFloat(coachmark.style.width);
    expect(left).toBeGreaterThanOrEqual(16);
    expect(left + width).toBeLessThanOrEqual(window.innerWidth - 16);
  } finally {
    getBoundingClientRect.mockRestore();
    Object.defineProperty(window, "innerWidth", { configurable: true, value: originalInnerWidth });
    Object.defineProperty(window, "innerHeight", { configurable: true, value: originalInnerHeight });
  }
});

it("在两种完成状态下都说明教程重看路径", () => {
  const view = render(
    <LanguageProvider>
      <FirstRunCompletionDialog open agentSkipped={false} onFinish={() => {}} />
    </LanguageProvider>,
  );

  expect(screen.getByRole("dialog", { name: "首次设置完成" }))
    .toHaveTextContent("设置 → 关于 → 重新查看新手引导");

  view.rerender(
    <LanguageProvider>
      <FirstRunCompletionDialog open agentSkipped onFinish={() => {}} />
    </LanguageProvider>,
  );
  expect(screen.getByRole("dialog", { name: "基础设置完成" }))
    .toHaveTextContent("设置 → 关于 → 重新查看新手引导");
});

it("aligns the compact add-provider spotlight outline to the button bounds", async () => {
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "add-provider") {
        return rect(40, 120, 176, 28);
      }
      return rect(0, 0, 0, 0);
    });

  try {
    render(
      <LanguageProvider>
        <button data-onboarding-target="add-provider" type="button">添加供应商</button>
        <FirstRunGuide
          open
          microStep="provider-entry"
          canSkipAgent={false}
          onTargetAction={() => {}}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>,
    );

    await screen.findByRole("dialog", { name: "添加你的第一个模型" });
    const outline = document.querySelector<HTMLElement>(".first-run-spotlight-outline");
    expect(outline).not.toBeNull();
    expect(outline).toHaveStyle({
      top: "40px",
      left: "120px",
      width: "176px",
      height: "28px",
    });
  } finally {
    getBoundingClientRect.mockRestore();
  }
});

it("目标尚未挂载时只显示等待状态，挂载后恢复真实操作", async () => {
  const onTargetAction = vi.fn();
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "provider-models") {
        return rect(120, 120, 640, 240);
      }
      return rect(0, 0, 0, 0);
    });

  const guide = (
    <FirstRunGuide
      open
      microStep="provider-models"
      canSkipAgent={false}
      onTargetAction={onTargetAction}
      onBack={() => {}}
      onSkipAgent={() => {}}
      onPause={() => {}}
      onDismiss={() => {}}
    />
  );

  try {
    const view = render(<LanguageProvider>{guide}</LanguageProvider>);

    expect(await screen.findByText("正在定位操作位置…")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "配置好了，去保存" })).toBeNull();

    view.rerender(
      <LanguageProvider>
        <section data-onboarding-target="provider-models">
          <button type="button">模型选择</button>
        </section>
        {guide}
      </LanguageProvider>,
    );

    const continueButton = await screen.findByRole("button", { name: "配置好了，去保存" });
    await userEvent.setup().click(continueButton);
    expect(onTargetAction).toHaveBeenCalledOnce();
  } finally {
    getBoundingClientRect.mockRestore();
  }
});

it("锁定路由工作区时仍允许教学卡片内部滚动", async () => {
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "route-config") {
        return rect(100, 100, 800, 400);
      }
      return rect(0, 0, 0, 0);
    });

  try {
    render(
      <LanguageProvider>
        <main className="station-content">
          <section data-onboarding-target="route-config">
            <button type="button">三档模型配置</button>
          </section>
        </main>
        <FirstRunGuide
          open
          microStep="route-config"
          canSkipAgent={false}
          onTargetAction={() => {}}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>,
    );

    const coachmark = await screen.findByRole("dialog", { name: "配置模型路由" });
    const coachmarkWheel = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
    act(() => coachmark.dispatchEvent(coachmarkWheel));
    expect(coachmarkWheel.defaultPrevented).toBe(false);

    const workspace = document.querySelector<HTMLElement>(".station-content");
    const workspaceWheel = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
    act(() => workspace!.dispatchEvent(workspaceWheel));
    expect(workspaceWheel.defaultPrevented).toBe(true);
  } finally {
    getBoundingClientRect.mockRestore();
  }
});

it("keeps the route spotlight aligned when a rejected scroll is restored", async () => {
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "route-config") {
        const workspace = this.closest<HTMLElement>(".station-content");
        return rect(100 - (workspace?.scrollTop ?? 0), 100, 800, 400);
      }
      return rect(0, 0, 0, 0);
    });

  try {
    render(
      <LanguageProvider>
        <main className="station-content">
          <section data-onboarding-target="route-config">
            <button type="button">上档</button>
            <button type="button">中档</button>
            <button type="button">下档</button>
          </section>
        </main>
        <FirstRunGuide
          open
          microStep="route-config"
          canSkipAgent={false}
          onTargetAction={() => {}}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>,
    );

    await screen.findByRole("dialog", { name: "配置模型路由" });
    const workspace = document.querySelector<HTMLElement>(".station-content");
    const outline = document.querySelector<HTMLElement>(".first-run-spotlight-outline");
    expect(workspace).not.toBeNull();
    expect(outline).not.toBeNull();
    await waitFor(() => {
      expect(outline).toHaveStyle({ top: "93px", height: "414px" });
    });

    act(() => {
      workspace!.scrollTop = 220;
      workspace!.dispatchEvent(new Event("scroll"));
    });

    expect(workspace!.scrollTop).toBe(0);
    await waitFor(() => {
      expect(outline).toHaveStyle({ top: "93px", height: "414px" });
    });
  } finally {
    getBoundingClientRect.mockRestore();
  }
});

it("keeps all three tiers highlighted after replacing an existing model", async () => {
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "route-config") {
        const workspace = this.closest<HTMLElement>(".station-content");
        return rect(100 - (workspace?.scrollTop ?? 0), 100, 800, 400);
      }
      if (this.getAttribute("aria-label") === "下档模型") {
        return rect(620, 500, 300, 48);
      }
      return rect(0, 0, 0, 0);
    });
  const scrollIntoView = vi
    .spyOn(HTMLElement.prototype, "scrollIntoView")
    .mockImplementation(function scrollIntoViewMock(this: HTMLElement) {
      if (this.getAttribute("aria-label") !== "下档模型") return;
      const workspace = this.closest<HTMLElement>(".station-content");
      if (!workspace) return;
      workspace.scrollTop = 220;
      workspace.dispatchEvent(new Event("scroll"));
    });

  function Harness() {
    const [tiers, setTiers] = useState({
      high: { upstream: "deepseek", model: "deepseek-v4-flash" },
      mid: { upstream: "deepseek", model: "deepseek-v4-flash" },
      low: { upstream: "deepseek", model: "deepseek-v4-flash" },
    });
    return (
      <LanguageProvider>
        <main className="station-content">
          <section data-onboarding-target="route-config">
            <TierRouteEditor
              tiers={tiers}
              providers={[{
                name: "deepseek",
                provider: "openai-compatible",
                base_url: "https://api.deepseek.com/v1",
                models: ["deepseek-v4-flash", "deepseek-reasoner"],
                has_auth: true,
              }]}
              onTierChange={(slot, upstream, model) => {
                setTiers((current) => ({ ...current, [slot]: { upstream, model } }));
              }}
            />
          </section>
        </main>
        <FirstRunGuide
          open
          microStep="route-config"
          canSkipAgent={false}
          onTargetAction={() => {}}
          onBack={() => {}}
          onSkipAgent={() => {}}
          onPause={() => {}}
          onDismiss={() => {}}
        />
      </LanguageProvider>
    );
  }

  try {
    const user = userEvent.setup();
    render(<Harness />);
    await screen.findByRole("dialog", { name: "配置模型路由" });
    const workspace = document.querySelector<HTMLElement>(".station-content");
    const outline = document.querySelector<HTMLElement>(".first-run-spotlight-outline");
    expect(workspace).not.toBeNull();
    expect(outline).not.toBeNull();
    await waitFor(() => {
      expect(outline).toHaveStyle({ top: "93px", height: "414px" });
    });

    await user.click(screen.getByRole("combobox", { name: "下档模型" }));
    await user.click(screen.getByRole("option", { name: "deepseek-reasoner" }));

    expect(screen.getByRole("combobox", { name: "下档模型" }))
      .toHaveTextContent("deepseek-reasoner");
    expect(workspace!.scrollTop).toBe(0);
    expect(outline).toHaveStyle({ top: "93px", height: "414px" });
  } finally {
    scrollIntoView.mockRestore();
    getBoundingClientRect.mockRestore();
  }
});
