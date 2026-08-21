import { useState } from "react";
import { RefreshCw } from "lucide-react";
import UsageRequestLog from "../components/UsageRequestLog";
import { useLocalizedCopy } from "../components/LanguageProvider";
import { Button } from "../components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../components/ui/select";

export default function RequestLogsPage({ embedded = false }: { embedded?: boolean }) {
  const { copy } = useLocalizedCopy();
  const [since, setSince] = useState("24h");
  const [refreshKey, setRefreshKey] = useState(0);
  return (
    <div className={`page-stack request-logs-page ${embedded ? "request-logs-embedded" : ""}`}>
      {!embedded && <header className="overview-heading">
        <div>
          <h1>{copy("Request logs", "请求日志", "請求日誌", "リクエストログ")}</h1>
          <p>{copy("Inspect routing outcomes, failures, and locally retained plaintext bodies.", "查看路由结果、失败原因和本地保留的明文正文。", "檢視路由結果、失敗原因和本機保留的明文主體", "ルーティング結果、失敗原因およびローカルに保持された平文本文を確認")}</p>
        </div>
        <div className="request-log-page-tools">
          <Select value={since} onValueChange={setSince}>
            <SelectTrigger aria-label={copy("Log time range", "日志时间范围", "日誌時間範圍", "ログ時間範囲")}><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="24h">{copy("Last 24 hours", "近 24 小时", "近 24 小時", "過去 24 時間")}</SelectItem>
              <SelectItem value="7d">{copy("Last 7 days", "近 7 天", "近 7 天", "過去 7 日間")}</SelectItem>
              <SelectItem value="30d">{copy("Last 30 days", "近 30 天", "近 30 天", "過去 30 日間")}</SelectItem>
              <SelectItem value="all">{copy("All time", "全部历史", "全部歷史", "すべての履歴")}</SelectItem>
            </SelectContent>
          </Select>
          <Button variant="outline" size="sm" onClick={() => setRefreshKey((value) => value + 1)}><RefreshCw />{copy("Refresh", "刷新", "重新整理", "更新")}</Button>
        </div>
      </header>}
      <UsageRequestLog since={since} agentId="" upstream="" model="" refreshKey={refreshKey} />
    </div>
  );
}
