import { XIcon } from "lucide-react";
import { useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { createPortal } from "react-dom";
import { useLanguage } from "./LanguageProvider";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "./ui/dialog";

export const FIRST_RUN_GUIDE_STORAGE_KEY = "token-station-first-run-guide";
export const FIRST_RUN_GUIDE_VERSION = "spotlight-setup-v4";

export type FirstRunSetupStep = "provider" | "route" | "agent" | "complete";
export type FirstRunMicroStep =
  | "overview"
  | "provider-entry"
  | "provider-choice"
  | "provider-credential"
  | "provider-models"
  | "provider-save"
  | "route-entry"
  | "route-mode"
  | "route-config"
  | "route-apply"
  | "agent-entry"
  | "agent-discovery-scope"
  | "agent-select"
  | "agent-installation"
  | "agent-connect"
  | "agent-connect-multiple"
  | "agent-scan-empty"
  | "complete";

export function shouldOpenFirstRunGuide(storage: Pick<Storage, "getItem"> = window.localStorage) {
  try {
    return storage.getItem(FIRST_RUN_GUIDE_STORAGE_KEY) !== FIRST_RUN_GUIDE_VERSION;
  } catch {
    return false;
  }
}

export function markFirstRunGuideDismissed(
  storage: Pick<Storage, "setItem"> = window.localStorage,
) {
  try {
    storage.setItem(FIRST_RUN_GUIDE_STORAGE_KEY, FIRST_RUN_GUIDE_VERSION);
  } catch {
    // Onboarding is optional. A denied preference write must never block the App.
  }
}

interface FirstRunGuideProps {
  open: boolean;
  microStep: FirstRunMicroStep;
  canSkipAgent: boolean;
  onTargetAction: () => void;
  onBack: () => void;
  onSkipAgent: () => void;
  onPause: () => void;
  onDismiss: () => void;
}

interface SpotlightRect {
  top: number;
  left: number;
  right: number;
  bottom: number;
  width: number;
  height: number;
  borderRadius: string;
}

interface CoachmarkContent {
  target: string | null;
  index: string;
  title: string;
  description: string;
  allowTargetInteraction?: boolean;
  advanceOnTargetClick: boolean;
  continueLabel: string | null;
  backLabel?: string;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

export default function FirstRunGuide({
  open,
  microStep,
  canSkipAgent,
  onTargetAction,
  onBack,
  onSkipAgent,
  onPause,
  onDismiss,
}: FirstRunGuideProps) {
  const { copy } = useLanguage();
  const [targetRect, setTargetRect] = useState<SpotlightRect | null>(null);
  const targetActionRef = useRef(onTargetAction);
  const coachmarkRef = useRef<HTMLElement>(null);
  targetActionRef.current = onTargetAction;
  const content: CoachmarkContent = microStep === "overview"
    ? {
        target: "overview-entry",
        index: copy("GET ORIENTED · OVERVIEW", "认识 Token Station · 概览"),
        title: copy("Return to Overview anytime", "从这里随时回到概览"),
        description: copy(
          "Wherever you are, select the Token Station mark at the top left to switch to Overview. It brings proxy status, current routing, requests, cost, Agents, and providers together.",
          "无论当前在哪个页面，点击左上角 Token Station 标志都能切换到概览页。这里汇总代理状态、当前路由、请求与成本，以及 Agent 和供应商规模。",
        ),
        allowTargetInteraction: false,
        advanceOnTargetClick: false,
        continueLabel: copy("Got it, start setup", "知道了，开始配置"),
      }
    : microStep === "provider-entry"
    ? {
        target: "add-provider",
        index: copy("ADD PROVIDER · 1/5", "添加供应商 · 1/5"),
        title: copy("Add your first provider", "添加你的第一个供应商"),
        description: copy(
          "Select this button to open provider setup. The guide continues after the real action.",
          "点击这里进入供应商配置，完成操作后引导会自动继续。",
        ),
        advanceOnTargetClick: true,
        continueLabel: null,
      }
    : microStep === "provider-choice"
      ? {
          target: "provider-choice",
          index: copy("ADD PROVIDER · 2/5", "添加供应商 · 2/5"),
          title: copy("Choose a model provider", "选择一个模型供应商"),
          description: copy(
            "Choose the provider you want to configure. You remain in control of the provider and model choice.",
            "点击你要配置的供应商；供应商和模型始终由你自己选择。",
          ),
          advanceOnTargetClick: false,
          continueLabel: null,
        }
      : microStep === "provider-credential"
        ? {
            target: "provider-credential",
            index: copy("ADD PROVIDER · 3/5", "添加供应商 · 3/5"),
            title: copy("Enter provider credentials", "填写供应商凭据"),
            description: copy(
              "Enter the credential required by this provider. The guide never reads or displays its value.",
              "填写该供应商需要的凭据；引导不会读取或展示凭据内容。",
            ),
            advanceOnTargetClick: false,
            continueLabel: copy("Next: choose models", "下一项：选择模型"),
          }
        : microStep === "provider-models"
          ? {
              target: "provider-models",
              index: copy("ADD PROVIDER · 4/5", "添加供应商 · 4/5"),
              title: copy("Choose at least one model", "选择至少一个模型"),
              description: copy(
                "Keep or choose the models that this provider should expose, then continue to the real save action.",
                "保留或选择这个供应商要提供的模型，然后前往真实保存操作。",
              ),
              advanceOnTargetClick: false,
              continueLabel: copy("Configuration ready, go to save", "配置好了，去保存"),
              backLabel: copy("Back to credentials", "返回填写凭据"),
            }
          : microStep === "provider-save"
            ? {
                target: "provider-save",
                index: copy("ADD PROVIDER · 5/5", "添加供应商 · 5/5"),
                title: copy("Save the provider", "保存供应商"),
                description: copy(
                  "Select the real save button. Routing begins only after the provider is saved successfully.",
                  "点击真实保存按钮；只有供应商保存成功后，才会进入路由配置。",
                ),
                advanceOnTargetClick: false,
                continueLabel: null,
                backLabel: copy("Back to models", "返回选择模型"),
              }
            : microStep === "route-entry"
              ? {
                  target: "routing",
                  index: copy("CONFIGURE ROUTING · 1/4", "配置路由 · 1/4"),
                  title: copy("Open routing setup", "打开路由配置"),
                  description: copy(
                    "Select Global routing on Home to continue.",
                    "点击主页左侧的“全局路由”，进入下一阶段配置。",
                  ),
                  advanceOnTargetClick: true,
                  continueLabel: null,
                }
              : microStep === "route-mode"
                ? {
            target: "route-mode",
            index: copy("CONFIGURE ROUTING · 2/4", "配置路由 · 2/4"),
            title: copy("Choose a routing mode", "选择路由模式"),
            description: copy("Review or choose how requests should be routed.", "查看并选择请求的路由方式。"),
            advanceOnTargetClick: false,
            continueLabel: copy("Use this mode", "沿用当前模式"),
          }
                : microStep === "route-config"
                  ? {
                      target: "route-config",
                      index: copy("CONFIGURE ROUTING · 3/4", "配置路由 · 3/4"),
                      title: copy("Configure model routing", "配置模型路由"),
                      description: copy(
                        "Review the providers and models required by the current routing mode, then continue to apply.",
                        "检查当前路由模式所需的供应商和模型，然后前往应用。",
                      ),
                      advanceOnTargetClick: false,
                      continueLabel: copy("Configuration ready, go to apply", "配置好了，去应用"),
                      backLabel: copy("Back to routing mode", "返回路由模式"),
                    }
                  : microStep === "route-apply"
                    ? {
                        target: "route-apply",
                        index: copy("CONFIGURE ROUTING · 4/4", "配置路由 · 4/4"),
                        title: copy("Save and apply routing", "保存并应用路由"),
                        description: copy(
                          "Select the real save action. The guide waits for this exact revision to become reachable.",
                          "点击真实保存操作；引导会等待这一版 revision 真正运行且监听可达。",
                        ),
                        advanceOnTargetClick: false,
                        continueLabel: null,
                        backLabel: copy("Back to route configuration", "返回检查配置"),
                      }
                    : microStep === "agent-entry"
                      ? {
                          target: "agent-entry",
                          index: copy("CONNECT AGENT · 1/4", "接入 Agent · 1/4"),
                          title: copy("Open Agent management", "打开 Agent 管理"),
                          description: copy(
                            "Select a detected Agent on Home to continue.",
                            "点击主页左侧扫描到的 Agent，查看本机接入配置。",
                          ),
                          advanceOnTargetClick: true,
                          continueLabel: null,
                        }
                      : microStep === "agent-discovery-scope"
                        ? {
                            target: "agent-list",
                            index: copy("CONNECT AGENT · 2/4", "接入 Agent · 2/4"),
                            title: copy(
                              "Only scanned Agents appear here",
                              "这里仅显示扫描到的 Agent",
                            ),
                            description: copy(
                              "This list only contains Agents currently discovered on this device. To see every Agent supported by Token Station, go to Settings → Agent Display. You can also control which Agents appear on Home there.",
                              "这里仅展示本机当前扫描到的 Agent。想查看 Token Station 支持的全部 Agent，请前往“设置 → Agent 显示”；你也可以在那里控制它们是否显示在主页。",
                            ),
                            advanceOnTargetClick: false,
                            continueLabel: copy(
                              "Got it, choose an Agent",
                              "知道了，选择 Agent",
                            ),
                          }
                      : microStep === "agent-select"
                        ? {
              target: "agent-list",
              index: copy("CONNECT AGENT · 3/4", "接入 Agent · 3/4"),
              title: copy("Choose an Agent", "选择一个 Agent"),
              description: copy("Choose a detected Agent to continue.", "选择一个检测到的 Agent 继续。"),
              advanceOnTargetClick: false,
              continueLabel: null,
            }
                        : microStep === "agent-installation"
                          ? {
                              target: "agent-installation",
                              index: copy("CONNECT AGENT · 4/5", "接入 Agent · 4/5"),
                              title: copy("Choose the installation to manage", "选择要接管的安装"),
                              description: copy(
                                "Multiple installations were detected. Open the real picker and choose the exact path to manage.",
                                "检测到多份安装；打开真实选择器，选择要接管的精确路径。",
                              ),
                              advanceOnTargetClick: false,
                              continueLabel: null,
                              backLabel: copy("Back to Agent list", "返回 Agent 列表"),
                            }
                        : microStep === "agent-connect"
                          ? {
                              target: "agent-connect",
                              index: copy("CONNECT AGENT · 4/4", "接入 Agent · 4/4"),
                              title: copy("Connect the Agent", "一键接入 Agent"),
                              description: copy(
                                "Select the real connection action. Completion waits for the cached Agent state to report CONNECTED.",
                                "点击真实接入操作；完成状态会由 Agent 缓存状态确认。",
                              ),
                              advanceOnTargetClick: false,
                              continueLabel: null,
                              backLabel: copy("Back to Agent list", "返回 Agent 列表"),
                            }
                          : microStep === "agent-connect-multiple"
                            ? {
                                target: "agent-connect",
                                index: copy("CONNECT AGENT · 5/5", "接入 Agent · 5/5"),
                                title: copy("Connect the Agent", "一键接入 Agent"),
                                description: copy(
                                  "Select the real connection action. Completion waits for the cached Agent state to report CONNECTED.",
                                  "点击真实接入操作；完成状态会由 Agent 缓存状态确认。",
                                ),
                                advanceOnTargetClick: false,
                                continueLabel: null,
                                backLabel: copy("Back to installation", "返回选择安装"),
                              }
                          : microStep === "agent-scan-empty"
                          ? {
                              target: null,
                              index: copy("CONNECT AGENT · 2/2", "接入 Agent · 2/2"),
                              title: copy("No connectable Agent detected", "未检测到可接入 Agent"),
                              description: copy(
                                "Agent discovery runs once before Home appears. Install an Agent and restart the app, or finish the basic setup without one.",
                                "Agent 会在主页出现前扫描一次。安装 Agent 后请重启应用，也可以暂不接入并完成基础设置。",
                              ),
                              advanceOnTargetClick: false,
                              continueLabel: null,
                            }
                          : {
                    target: null,
                    index: copy("FIRST SETUP · COMPLETE", "首次设置 · 已完成"),
                    title: copy("First setup is complete", "首次设置已完成"),
                    description: copy(
                      "No repeated setup is required. To review this guide later, go to Settings → About → Review getting started guide.",
                      "无需重复配置。需要重看教程时，前往“设置 → 关于 → 重新查看新手引导”。",
                    ),
                    advanceOnTargetClick: false,
                    continueLabel: copy("Back to Home", "返回主页"),
                  };
  const lockWorkspaceScroll = content.target === "route-mode"
    || content.target === "route-config"
    || content.target === "route-apply";
  const targetPending = content.target !== null && targetRect === null;

  useLayoutEffect(() => {
    if (!open) return undefined;
    const previous = document.body.getAttribute("data-first-run-guide-active");
    document.body.setAttribute("data-first-run-guide-active", "true");
    return () => {
      if (previous === null) document.body.removeAttribute("data-first-run-guide-active");
      else document.body.setAttribute("data-first-run-guide-active", previous);
    };
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !content.target) {
      setTargetRect(null);
      return undefined;
    }
    let activeTarget: HTMLElement | null = null;
    let detachTarget = () => {};

    const attachTarget = (target: HTMLElement) => {
      activeTarget = target;
      const originalDescription = target.getAttribute("aria-describedby");
      const originalTabIndex = target.getAttribute("tabindex");
      const descriptionIds = new Set((originalDescription ?? "").split(/\s+/).filter(Boolean));
      descriptionIds.add("first-run-coachmark-description");
      target.setAttribute("aria-describedby", [...descriptionIds].join(" "));
      target.setAttribute("data-onboarding-active", "true");
      if (target.tabIndex < 0) target.setAttribute("tabindex", "-1");

      const measure = () => {
        const rect = target.getBoundingClientRect();
        if (rect.width <= 0 || rect.height <= 0) {
          setTargetRect(null);
          return;
        }
        setTargetRect({
          top: rect.top,
          left: rect.left,
          right: rect.right,
          bottom: rect.bottom,
          width: rect.width,
          height: rect.height,
          borderRadius: window.getComputedStyle(target).borderRadius,
        });
      };
      const activate = () => targetActionRef.current();
      const workspace = target.closest<HTMLElement>(".station-content");
      if (content.target === "provider-choice" && workspace) {
        // The provider catalog is taller than compact windows. Keep the page header
        // visible so the coachmark can sit above the catalog instead of covering cards.
        workspace.scrollTop = 0;
      } else {
        target.scrollIntoView({
          block: lockWorkspaceScroll ? "center" : "nearest",
          inline: "nearest",
          behavior: "auto",
        });
      }
      target.focus({ preventScroll: true });
      let unlockWorkspaceScroll = () => {};
      if (lockWorkspaceScroll && workspace) {
        const lockedScrollTop = workspace.scrollTop;
        const lockedScrollLeft = workspace.scrollLeft;
        const preventUserScroll = (event: Event) => {
          const eventTarget = event.target;
          if (
            eventTarget instanceof Element
            && eventTarget.closest('[data-onboarding-floating="true"], .first-run-coachmark')
          ) return;
          event.preventDefault();
        };
        const restoreWorkspaceScroll = () => {
          let restored = false;
          if (workspace.scrollTop !== lockedScrollTop) {
            workspace.scrollTop = lockedScrollTop;
            restored = true;
          }
          if (workspace.scrollLeft !== lockedScrollLeft) {
            workspace.scrollLeft = lockedScrollLeft;
            restored = true;
          }
          if (restored) measure();
        };
        workspace.setAttribute("data-onboarding-scroll-locked", "true");
        window.addEventListener("wheel", preventUserScroll, { passive: false, capture: true });
        window.addEventListener("touchmove", preventUserScroll, { passive: false, capture: true });
        workspace.addEventListener("scroll", restoreWorkspaceScroll);
        unlockWorkspaceScroll = () => {
          window.removeEventListener("wheel", preventUserScroll, true);
          window.removeEventListener("touchmove", preventUserScroll, true);
          workspace.removeEventListener("scroll", restoreWorkspaceScroll);
          workspace.removeAttribute("data-onboarding-scroll-locked");
        };
      }
      measure();
      if (content.advanceOnTargetClick) target.addEventListener("click", activate);
      window.addEventListener("resize", measure);
      window.addEventListener("scroll", measure, true);
      const resizeObserver = typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(measure);
      resizeObserver?.observe(target);

      detachTarget = () => {
        unlockWorkspaceScroll();
        target.removeEventListener("click", activate);
        window.removeEventListener("resize", measure);
        window.removeEventListener("scroll", measure, true);
        resizeObserver?.disconnect();
        target.removeAttribute("data-onboarding-active");
        if (originalDescription === null) target.removeAttribute("aria-describedby");
        else target.setAttribute("aria-describedby", originalDescription);
        if (originalTabIndex === null) target.removeAttribute("tabindex");
        else target.setAttribute("tabindex", originalTabIndex);
      };
    };

    const findTarget = () => {
      const nextTarget = document.querySelector<HTMLElement>(
        `[data-onboarding-target="${content.target}"]`,
      );
      if (nextTarget === activeTarget) return;
      detachTarget();
      activeTarget = null;
      setTargetRect(null);
      if (nextTarget) attachTarget(nextTarget);
    };

    findTarget();
    const mutationObserver = typeof MutationObserver === "undefined"
      ? null
      : new MutationObserver(findTarget);
    mutationObserver?.observe(document.body, { childList: true, subtree: true });

    return () => {
      mutationObserver?.disconnect();
      detachTarget();
    };
  }, [content.advanceOnTargetClick, content.target, lockWorkspaceScroll, open]);

  useLayoutEffect(() => {
    if (!open) return undefined;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onPause();
        return;
      }
      if (event.key !== "Tab") return;
      const target = content.target
        ? document.querySelector<HTMLElement>(
            `[data-onboarding-target="${content.target}"][data-onboarding-active="true"]`,
          )
        : null;
      const focusableSelector = [
        "button:not([disabled])",
        "input:not([disabled])",
        "select:not([disabled])",
        "textarea:not([disabled])",
        "a[href]",
        "[tabindex]",
      ].join(",");
      const floatingCandidates = [
        ...document.querySelectorAll<HTMLElement>('[data-onboarding-floating="true"]'),
      ].flatMap((layer) => [
        ...(layer.matches(focusableSelector) ? [layer] : []),
        ...layer.querySelectorAll<HTMLElement>(focusableSelector),
      ]);
      const targetCandidates = target
        ? content.allowTargetInteraction === false
          ? [target]
          : [target, ...target.querySelectorAll<HTMLElement>(focusableSelector)]
        : [];
      const candidates = [
        ...targetCandidates,
        ...floatingCandidates,
        ...(coachmarkRef.current?.querySelectorAll<HTMLElement>(focusableSelector) ?? []),
      ].filter((element, index, items) => (
        !element.hasAttribute("disabled") && items.indexOf(element) === index
      ));
      if (candidates.length === 0) return;
      event.preventDefault();
      const currentIndex = candidates.indexOf(document.activeElement as HTMLElement);
      const nextIndex = event.shiftKey
        ? (currentIndex <= 0 ? candidates.length - 1 : currentIndex - 1)
        : (currentIndex < 0 || currentIndex === candidates.length - 1 ? 0 : currentIndex + 1);
      candidates[nextIndex].focus();
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [content.allowTargetInteraction, content.target, onPause, open]);

  if (!open) return null;
  const padding = 7;
  const viewportInset = 8;
  const viewportWidth = window.innerWidth;
  const viewportHeight = window.innerHeight;
  const hole = targetRect ? {
    top: clamp(targetRect.top - padding, viewportInset, viewportHeight - viewportInset),
    left: clamp(targetRect.left - padding, viewportInset, viewportWidth - viewportInset),
    right: clamp(targetRect.right + padding, viewportInset, viewportWidth - viewportInset),
    bottom: clamp(targetRect.bottom + padding, viewportInset, viewportHeight - viewportInset),
  } : null;
  const outline = targetRect && hole
    ? content.target === "add-provider"
      ? {
          top: targetRect.top,
          left: targetRect.left,
          right: targetRect.right,
          bottom: targetRect.bottom,
          borderRadius: targetRect.borderRadius,
        }
      : { ...hole, borderRadius: undefined }
    : null;
  const isOverviewEntry = content.target === "overview-entry";
  // The overview copy does not benefit from a banner-sized coachmark. Keeping
  // it close to the regular step width also leaves room at high OS/browser
  // zoom levels, where the effective CSS viewport can be much narrower than
  // the native desktop window.
  const cardWidth = Math.min(isOverviewEntry ? 440 : 360, viewportWidth - 32);
  const estimatedCardHeight = isOverviewEntry ? 210 : 230;
  const cardGap = isOverviewEntry ? 14 : 18;
  const overviewRightWidth = targetRect
    ? viewportWidth - targetRect.right - cardGap - 16
    : 0;
  const cardStyle: CSSProperties = targetRect && targetRect.width > 0
    ? isOverviewEntry && overviewRightWidth >= 420
      ? {
          width: Math.min(cardWidth, overviewRightWidth),
          left: targetRect.right + cardGap,
          top: clamp(targetRect.top, 16, viewportHeight - estimatedCardHeight - 16),
        }
      : targetRect.bottom + cardGap + estimatedCardHeight <= viewportHeight - 16
      ? {
          width: cardWidth,
          left: clamp(targetRect.left, 16, viewportWidth - cardWidth - 16),
          top: targetRect.bottom + cardGap,
        }
      : targetRect.right + cardGap + cardWidth <= viewportWidth - 16
        ? {
            width: cardWidth,
            left: targetRect.right + cardGap,
            top: clamp(targetRect.top, 16, viewportHeight - estimatedCardHeight - 16),
          }
        : targetRect.left - cardGap - cardWidth >= 16
          ? {
              width: cardWidth,
              left: targetRect.left - cardGap - cardWidth,
              top: clamp(targetRect.top, 16, viewportHeight - estimatedCardHeight - 16),
            }
          : {
              width: cardWidth,
              left: clamp(targetRect.left, 16, viewportWidth - cardWidth - 16),
              top: clamp(
                targetRect.top - cardGap - estimatedCardHeight,
                16,
                viewportHeight - estimatedCardHeight - 16,
              ),
            }
    : {
        width: cardWidth,
        left: Math.max(16, (viewportWidth - cardWidth) / 2),
        top: Math.max(16, (viewportHeight - 250) / 2),
      };

  return createPortal(
    <div className="first-run-spotlight-root">
      {hole ? (
        <>
          <div className="first-run-spotlight-blocker" style={{ top: 0, left: 0, right: 0, height: hole.top }} />
          <div className="first-run-spotlight-blocker" style={{ top: hole.top, left: 0, width: hole.left, height: hole.bottom - hole.top }} />
          <div className="first-run-spotlight-blocker" style={{ top: hole.top, left: hole.right, right: 0, height: hole.bottom - hole.top }} />
          <div className="first-run-spotlight-blocker" style={{ top: hole.bottom, left: 0, right: 0, bottom: 0 }} />
          {content.allowTargetInteraction === false && (
            <div
              className="first-run-spotlight-hole-blocker"
              aria-hidden="true"
              style={{
                top: hole.top,
                left: hole.left,
                width: hole.right - hole.left,
                height: hole.bottom - hole.top,
              }}
            />
          )}
          <div
            className="first-run-spotlight-outline"
            aria-hidden="true"
            style={{
              top: outline!.top,
              left: outline!.left,
              width: outline!.right - outline!.left,
              height: outline!.bottom - outline!.top,
              borderRadius: outline!.borderRadius,
            }}
          />
        </>
      ) : (
        <div className="first-run-spotlight-blocker" style={{ inset: 0 }} />
      )}
      <section
        ref={coachmarkRef}
        className={`first-run-coachmark${isOverviewEntry ? " first-run-coachmark-overview-entry" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="first-run-coachmark-title"
        style={cardStyle}
      >
        <span className="first-run-guide-step">{content.index}</span>
        <Button
          className="first-run-guide-close"
          variant="ghost"
          size="icon-sm"
          type="button"
          aria-label={copy("Continue later", "稍后继续")}
          onClick={onPause}
        >
          <XIcon />
        </Button>
        <h2 id="first-run-coachmark-title">{content.title}</h2>
        <p id="first-run-coachmark-description" aria-live={targetPending ? "polite" : undefined}>
          {targetPending
            ? copy("Locating the action…", "正在定位操作位置…")
            : content.description}
        </p>
        <div className="first-run-coachmark-actions">
          <Button variant="ghost" size="sm" type="button" onClick={onDismiss}>
            {copy("Don't show again", "不再提示")}
          </Button>
          {!targetPending && content.backLabel && (
            <Button variant="outline" size="sm" type="button" onClick={onBack}>
              {content.backLabel}
            </Button>
          )}
          {!targetPending && content.continueLabel && (
            <Button size="sm" type="button" onClick={onTargetAction}>
              {content.continueLabel}
            </Button>
          )}
          {!targetPending && microStep === "agent-scan-empty" && canSkipAgent && (
            <Button size="sm" type="button" onClick={onSkipAgent}>
              {copy("Finish without an Agent", "暂不接入，完成设置")}
            </Button>
          )}
        </div>
      </section>
    </div>,
    document.body,
  );
}

interface FirstRunCompletionDialogProps {
  open: boolean;
  agentSkipped: boolean;
  onFinish: () => void;
}

export function FirstRunCompletionDialog({
  open,
  agentSkipped,
  onFinish,
}: FirstRunCompletionDialogProps) {
  const { copy } = useLanguage();
  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onFinish()}>
      <DialogContent className="first-run-complete-dialog" showCloseButton={false}>
        <DialogHeader>
          <span className="first-run-guide-step">
            {copy("FIRST SETUP · COMPLETE", "首次设置 · 已完成")}
          </span>
          <DialogTitle>
            {agentSkipped
              ? copy("Basic setup complete", "基础设置完成")
              : copy("First setup complete", "首次设置完成")}
          </DialogTitle>
          <DialogDescription>
            {agentSkipped
              ? copy(
                  "The provider and route are ready. No Agent has been connected yet.",
                  "供应商和路由已就绪，Agent 尚未接入。",
                )
              : copy(
                  "The provider, route, and Agent are ready. Token Station can now manage model requests.",
                  "供应商、路由和 Agent 均已就绪，Token Station 现在可以接管模型请求。",
                )}
          </DialogDescription>
          <p className="first-run-complete-revisit">
            {copy(
              "To review this guide later, go to Settings → About → Review getting started guide.",
              "需要重看教程时，前往“设置 → 关于 → 重新查看新手引导”。",
            )}
          </p>
        </DialogHeader>
        <DialogFooter>
          <Button type="button" onClick={onFinish}>
            {copy("Back to Home", "返回主页")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
