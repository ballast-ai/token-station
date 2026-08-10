import { useEffect, useLayoutEffect, useMemo, useState, type CSSProperties } from "react";
import { XIcon } from "lucide-react";
import { Dialog as DialogPrimitive } from "radix-ui";
import { useLanguage } from "./LanguageProvider";
import { Button } from "./ui/button";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
} from "./ui/dialog";
import { Progress } from "./ui/progress";

export const FIRST_RUN_GUIDE_STORAGE_KEY = "token-station-first-run-guide";
export const FIRST_RUN_GUIDE_VERSION = "b-v1";

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
  onDismiss: () => void;
}

interface MeasuredRect {
  top: number;
  left: number;
  width: number;
  height: number;
  right: number;
  bottom: number;
}

function clamp(value: number, min: number, max: number) {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

export default function FirstRunGuide({ open, onDismiss }: FirstRunGuideProps) {
  const { copy } = useLanguage();
  const [stepIndex, setStepIndex] = useState(0);
  const steps = useMemo(() => [
    {
      target: "add-provider",
      title: copy("Add a model provider first", "先添加一个模型供应商"),
      description: copy(
        "Choose a provider, enter its credentials, and run the connection test before saving.",
        "选择供应商并填写凭据，保存前先运行连接测试。",
      ),
    },
    {
      target: "routing",
      title: copy("Configure and start routing", "配置并启动路由"),
      description: copy(
        "Choose models for the High, Medium, and Low tiers, then select Save and apply. The route is ready only after the proxy reports Running.",
        "为上、中、下三档选择模型，然后点击“保存并应用”。右上角显示“代理运行中”后，这条路由才真正可用。",
      ),
    },
    {
      target: "agent-connect",
      title: copy("Connect your first Agent", "接入你的第一个 Agent"),
      description: copy(
        "Choose a detected Agent, review the configuration changes, and then explicitly confirm the connection.",
        "选择已检测的 Agent，检查即将修改的配置，再明确确认接入。",
      ),
    },
    {
      target: "settings",
      title: copy("Review this guide anytime", "以后想再看教程"),
      description: copy(
        "Select Settings, open About, and then choose Review getting started guide.",
        "点击顶部“设置”，进入“关于”，再选择“重新查看新手引导”。",
      ),
    },
  ], [copy]);
  const step = steps[stepIndex];
  const [targetRect, setTargetRect] = useState<MeasuredRect | null>(null);

  useEffect(() => {
    if (!open) setStepIndex(0);
  }, [open]);

  useLayoutEffect(() => {
    if (!open) {
      setTargetRect(null);
      return;
    }
    const target = document.querySelector<HTMLElement>(
      `[data-onboarding-target="${step.target}"]`,
    );
    target?.setAttribute("data-onboarding-active", "true");

    const measure = () => {
      if (!target?.isConnected) {
        setTargetRect(null);
        return;
      }
      const rect = target.getBoundingClientRect();
      if (rect.width <= 0 || rect.height <= 0) {
        setTargetRect(null);
        return;
      }
      const padding = 6;
      setTargetRect({
        top: Math.max(8, rect.top - padding),
        left: Math.max(8, rect.left - padding),
        width: rect.width + padding * 2,
        height: rect.height + padding * 2,
        right: rect.right + padding,
        bottom: rect.bottom + padding,
      });
    };

    measure();
    const frame = window.requestAnimationFrame(measure);
    window.addEventListener("resize", measure);
    window.addEventListener("scroll", measure, true);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(measure);
    if (target) observer?.observe(target);

    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener("resize", measure);
      window.removeEventListener("scroll", measure, true);
      observer?.disconnect();
      target?.removeAttribute("data-onboarding-active");
    };
  }, [open, step.target]);

  const placement = stepIndex === 2 ? "left" : "below";
  const dialogStyle: CSSProperties | undefined = targetRect
    ? (() => {
        const cardWidth = Math.min(380, window.innerWidth - 32);
        const cardHeight = 270;
        if (placement === "left") {
          return {
            left: clamp(targetRect.left - cardWidth - 24, 16, window.innerWidth - cardWidth - 16),
            top: clamp(
              targetRect.top + targetRect.height / 2 - cardHeight / 2,
              16,
              window.innerHeight - cardHeight - 16,
            ),
          };
        }
        const preferredLeft = stepIndex === 0
          ? targetRect.right - cardWidth
          : targetRect.left - 24;
        return {
          left: clamp(preferredLeft, 16, window.innerWidth - cardWidth - 16),
          top: clamp(targetRect.bottom + 18, 16, window.innerHeight - cardHeight - 16),
        };
      })()
    : undefined;

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => !nextOpen && onDismiss()}>
      <DialogPortal>
        <DialogOverlay className="first-run-guide-overlay" />
        {targetRect && (
          <div
            className="first-run-guide-spotlight"
            data-onboarding-spotlight={step.target}
            aria-hidden="true"
            style={{
              top: targetRect.top,
              left: targetRect.left,
              width: targetRect.width,
              height: targetRect.height,
            }}
          />
        )}
        <DialogPrimitive.Content
          className="first-run-guide-dialog"
          data-placement={targetRect ? placement : "center"}
          style={dialogStyle}
          onPointerDownOutside={(event) => event.preventDefault()}
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            window.requestAnimationFrame(() => {
              document.querySelector<HTMLElement>("[data-onboarding-return-focus]")?.focus();
            });
          }}
        >
          <DialogHeader className="first-run-guide-header">
            <span className="first-run-guide-step">
              {copy(
                `Page tour · ${stepIndex + 1}/${steps.length}`,
                `页面讲解 · ${stepIndex + 1}/${steps.length}`,
              )}
            </span>
            <Button
              className="first-run-guide-close"
              variant="ghost"
              size="icon-sm"
              type="button"
              aria-label={copy("Close guide", "关闭引导")}
              onClick={onDismiss}
            >
              <XIcon />
            </Button>
            <DialogTitle className="first-run-guide-title">{step.title}</DialogTitle>
            <DialogDescription>{step.description}</DialogDescription>
          </DialogHeader>
          <Progress
            value={((stepIndex + 1) / steps.length) * 100}
            aria-label={copy(
              `Guide progress: step ${stepIndex + 1} of ${steps.length}`,
              `引导进度：第 ${stepIndex + 1} 步，共 ${steps.length} 步`,
            )}
          />
          <DialogFooter className="first-run-guide-actions">
            <Button data-action="skip" variant="ghost" type="button" onClick={onDismiss}>
              {copy("Skip guide", "跳过引导")}
            </Button>
            {stepIndex > 0 && (
              <Button
                variant="outline"
                type="button"
                onClick={() => setStepIndex((current) => current - 1)}
              >
                {copy("Previous", "上一步")}
              </Button>
            )}
            <Button
              type="button"
              onClick={() => {
                if (stepIndex === steps.length - 1) onDismiss();
                else setStepIndex((current) => current + 1);
              }}
            >
              {stepIndex === steps.length - 1
                ? copy("Finish guide", "完成引导")
                : copy("Next", "下一步")}
            </Button>
          </DialogFooter>
        </DialogPrimitive.Content>
      </DialogPortal>
    </Dialog>
  );
}
