import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { expect, it, vi } from "vitest";
import FirstRunGuide, {
  FIRST_RUN_GUIDE_STORAGE_KEY,
  FIRST_RUN_GUIDE_VERSION,
  FirstRunCompletionDialog,
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

it("升级教程版本，使只看过旧版的用户看到新增概览", () => {
  const storage = new Map([[FIRST_RUN_GUIDE_STORAGE_KEY, "spotlight-setup-v1"]]);
  expect(shouldOpenFirstRunGuide({ getItem: (key) => storage.get(key) ?? null })).toBe(true);
  storage.set(FIRST_RUN_GUIDE_STORAGE_KEY, FIRST_RUN_GUIDE_VERSION);
  expect(shouldOpenFirstRunGuide({ getItem: (key) => storage.get(key) ?? null })).toBe(false);
});

it("从真实概览开始，并在教程内说明之后从哪里重看", async () => {
  const onTargetAction = vi.fn();
  const getBoundingClientRect = vi
    .spyOn(HTMLElement.prototype, "getBoundingClientRect")
    .mockImplementation(function getBoundingClientRectMock(this: HTMLElement) {
      if (this.getAttribute("data-onboarding-target") === "overview") {
        return rect(64, 24, 960, 640);
      }
      return rect(0, 0, 0, 0);
    });

  try {
    render(
      <LanguageProvider>
        <main data-onboarding-target="overview">概览真实内容</main>
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

    const coachmark = await screen.findByRole("dialog", { name: "先看概览" });
    expect(coachmark).toHaveTextContent("代理状态、当前路由、请求与成本");
    expect(coachmark).toHaveTextContent("设置 → 关于 → 重新查看新手引导");
    expect(screen.getByText("概览真实内容")).toHaveAttribute(
      "data-onboarding-active",
      "true",
    );
    expect(document.querySelector(".first-run-spotlight-hole-blocker")).not.toBeNull();
    const user = userEvent.setup();
    await user.tab();
    expect(within(coachmark).getByRole("button", { name: "稍后继续" })).toHaveFocus();
    await user.click(within(coachmark).getByRole("button", { name: "开始配置" }));
    expect(onTargetAction).toHaveBeenCalledOnce();
  } finally {
    getBoundingClientRect.mockRestore();
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

    await screen.findByRole("dialog", { name: "添加你的第一个供应商" });
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
