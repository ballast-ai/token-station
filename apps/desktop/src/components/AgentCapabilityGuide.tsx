import type { AgentRouteView } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import "./AgentCapabilityGuide.css";

export default function AgentCapabilityGuide({ route }: { route: AgentRouteView }) {
  const { copy } = useLocalizedCopy();
  const source = route.inherits_global === true
    ? copy("Follows global", "跟随全局", "跟隨全域", "グローバルに従う")
    : copy("Independent routing", "独立路由", "獨立路由", "独立ルーティング");
  const target = route.routing_mode === "direct"
    ? route.direct_target?.upstream && route.direct_target?.model
      ? `${route.direct_target.upstream} / ${route.direct_target.model}`
      : copy("Select a provider and model", "尚未选择供应商和模型", "尚未選擇供應商和模型", "プロバイダーとモデルを選択してください")
    : route.routing_mode === "quota_first"
      ? copy("Selected by quota policy", "按额度策略选择", "依額度策略選擇", "クォータポリシーで選択")
      : copy("Selected by three-tier routing", "按三档路由选择", "依三檔路由選擇", "三段階ルーティングで選択");

  return (
    <dl className="agent-capability-route" aria-label={copy("Routing configuration", "路由配置", "路由設定", "ルーティング設定")}>
      <div><dt className="sr-only">{copy("Routing source", "路由来源", "路由來源", "ルーティングソース")}</dt><dd>{source}</dd></div>
      <div className="agent-capability-target"><dt className="sr-only">{copy("Configured target", "配置目标", "設定目標", "設定対象")}</dt><dd><span aria-hidden="true">·</span>{target}</dd></div>
    </dl>
  );
}
