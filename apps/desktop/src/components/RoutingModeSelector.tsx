import { Gauge, Layers3 } from "lucide-react";
import { useLocalizedCopy } from "./LanguageProvider";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";

interface RoutingModeSelectorProps {
  value: "tiered" | "quota_first";
  disabled?: boolean;
  agent?: boolean;
  onValueChange: (value: "tiered" | "quota_first") => void;
}

export default function RoutingModeSelector({
  value,
  disabled = false,
  agent = false,
  onValueChange,
}: RoutingModeSelectorProps) {
  const { copy } = useLocalizedCopy();
  const label = agent
    ? copy("Agent routing strategy", "Agent 路由策略")
    : copy("Routing mode", "路由模式");

  return (
    <section
      className="routing-mode-section"
      aria-label={label}
      data-onboarding-target={agent ? undefined : "route-mode"}
    >
      <div className="routing-mode-copy">
        <span>{copy("ROUTING STRATEGY", "路由策略")}</span>
        <div>
          <h2>{copy("Choose how requests are assigned", "选择请求如何分配")}</h2>
          <p>{copy(
            "Switch between task-complexity tiers and provider quota priority.",
            "在按任务复杂度分档与按供应商额度优先之间切换。",
          )}</p>
        </div>
      </div>
      <Tabs
        className="routing-mode-tabs"
        value={value}
        onValueChange={(next) => onValueChange(next as "tiered" | "quota_first")}
      >
        <TabsList aria-label={label}>
          <TabsTrigger value="tiered" aria-label={copy("Smart tiers", "智能分档")} disabled={disabled}>
            <Layers3 />
            <span><strong>{copy("Smart tiers", "智能分档")}</strong><small>{copy("Match model to task", "按任务复杂度选模型")}</small></span>
          </TabsTrigger>
          <TabsTrigger value="quota_first" aria-label={copy("Quota first", "额度优先")} disabled={disabled}>
            <Gauge />
            <span><strong>{copy("Quota first", "额度优先")}</strong><small>{copy("Prefer available capacity", "优先使用可用额度")}</small></span>
          </TabsTrigger>
        </TabsList>
      </Tabs>
    </section>
  );
}
