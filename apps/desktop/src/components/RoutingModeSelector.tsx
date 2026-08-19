import { Gauge, Layers3, Target } from "lucide-react";
import type { RoutingMode } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { Tabs, TabsList, TabsTrigger } from "./ui/tabs";

interface RoutingModeSelectorProps {
  value: RoutingMode;
  disabled?: boolean;
  agent?: boolean;
  onValueChange: (value: RoutingMode) => void;
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
      <Tabs
        className="routing-mode-tabs"
        value={value}
        onValueChange={(next) => onValueChange(next as RoutingMode)}
      >
        <TabsList className="routing-mode-tabs-list" variant="line" aria-label={label}>
          <TabsTrigger className="routing-mode-tab" value="direct" aria-label={copy("Direct routing", "简单路由")} title={copy("One exact provider and model", "固定一个供应商和模型")} disabled={disabled}>
            <Target />
            <span>{copy("Direct routing", "简单路由")}</span>
          </TabsTrigger>
          <TabsTrigger className="routing-mode-tab" value="tiered" aria-label={copy("Smart tiers", "智能分档")} title={copy("Match model to task", "按任务复杂度选模型")} disabled={disabled}>
            <Layers3 />
            <span>{copy("Smart tiers", "智能分档")}</span>
          </TabsTrigger>
          <TabsTrigger className="routing-mode-tab" value="quota_first" aria-label={copy("Quota first", "额度优先")} title={copy("Prefer available capacity", "优先使用可用额度")} disabled={disabled}>
            <Gauge />
            <span>{copy("Quota first", "额度优先")}</span>
          </TabsTrigger>
        </TabsList>
      </Tabs>
    </section>
  );
}
