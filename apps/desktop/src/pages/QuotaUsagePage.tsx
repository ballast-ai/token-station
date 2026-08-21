import { useCallback, useEffect, useRef, useState } from "react";
import {
  getQuotaSnapshot,
  type ProviderView,
  type QuotaAccountSnapshot,
  type QuotaSnapshot,
  type QuotaSource,
} from "../api";
import PageBackButton from "../components/PageBackButton";
import { useLocalizedCopy, type LocalizedCopy } from "../components/LanguageProvider";
import { humanizeAppError } from "../errors";
import { useErrorToast } from "../components/ErrorToast";

interface QuotaUsagePageProps {
  providers: ProviderView[];
  onBack: () => void;
}

/** Refresh interval for lightweight polling that keeps runtime quota data current. */
const REFRESH_MS = 5000;

/** Values above this threshold mean no reset, such as OpenRouter prepaid balance with u64::MAX. */
const NO_RESET_MS = 100 * 24 * 60 * 60 * 1000;

function permilleToPercent(permille: number): number {
  return Math.round((permille / 1000) * 1000) / 10;
}

/** Format millisecond durations as compact values such as `2h 5m`, `3m 20s`, or `45s`. */
function formatDuration(ms: number): string {
  if (ms <= 0) return "—";
  const totalSeconds = Math.floor(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

export default function QuotaUsagePage({ providers, onBack }: QuotaUsagePageProps) {
  const { copy } = useLocalizedCopy();
  const { showError } = useErrorToast();
  const [snapshot, setSnapshot] = useState<QuotaSnapshot | null>(null);
  const snapshotRef = useRef<QuotaSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const next = await getQuotaSnapshot();
      snapshotRef.current = next;
      setSnapshot(next);
      setError(null);
    } catch (caught) {
      const message = humanizeAppError(caught);
      if (snapshotRef.current) showError(message, "quota-usage-refresh");
      else setError(message);
    } finally {
      setLoading(false);
    }
  }, [showError]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const sourceLabel = (source: QuotaSource): { text: string; tone: string } => {
    switch (source) {
      case "authoritative":
        return { text: copy("Provider-reported", "供应商权威", "供應商報告", "プロバイダー報告"), tone: "authoritative" };
      case "estimated":
        return { text: copy("Local estimate", "本地估算", "本地估算", "ローカル推定"), tone: "estimated" };
      default:
        return { text: copy("No window data", "无窗口数据", "無視窗資料", "ウィンドウデータなし"), tone: "none" };
    }
  };

  const displayName = (upstream: string): string =>
    providers.find((provider) => provider.name === upstream)?.name ?? upstream;

  const accounts = snapshot?.accounts ?? [];
  const shown = accounts.filter(
    (account) =>
      account.windows.length > 0 ||
      account.cooling_ms_remaining > 0 ||
      account.rate_pressured ||
      account.inflight > 0,
  );

  return (
    <div className="page-stack quota-usage-page">
      <header className="page-title-row">
        <div>
          <PageBackButton onClick={onBack} />
          <h1>{copy("Live quota", "实时额度", "即時額度", "リアルタイムクォータ")}</h1>
          <p>{copy(
            "Per-account allowance across each reset window, live from the running proxy.",
            "各账户在每个刷新窗口的额度余量,来自运行中的代理实时数据。", "每個重置視窗各帳號的額度餘量，來自執行中的代理即時資料。", "各アカウントごとのリセットウィンドウごとのクォータ残量、実行中のプロキシからのリアルタイムデータから取得されます。"
          )}</p>
        </div>
      </header>

      {error ? (
        <section className="panel quota-usage-empty">
          <strong>{copy("Live quota is unavailable", "暂时拿不到实时额度", "即時額度不可用", "リアルタイムクォータは利用不可")}</strong>
          <p>{error}</p>
        </section>
      ) : loading ? (
        <section className="panel quota-usage-empty">
          <p>{copy("Loading…", "加载中…", "載入中…", "読み込み中…")}</p>
        </section>
      ) : shown.length === 0 ? (
        <section className="panel quota-usage-empty">
          <strong>{copy("No quota activity yet", "还没有额度活动", "還沒有額度活動", "まだクォータ活動はありません")}</strong>
          <p>{copy(
            "Accounts appear here once they report limits, are counted locally, or hit a cooldown. Declare a plan or send some traffic in quota-first mode.",
            "账户在上报限额、被本地计数或触发冷却后会出现在这里。可以先声明额度计划,或在额度优先模式下跑一些流量。", "帳號在上報限制、被本地計數或觸發冷卻後會出現在這裡。可以先宣告額度計劃，或在額度優先模式下跑一些流量。", "アカウントは制限を報告したり、ローカルでカウントされたり、クールダウンをトリガーした後、ここに表示されます。まずクォータプランを宣言するか、クォータ優先モードでいくつかのトラフィックを送信してください。"
          )}</p>
        </section>
      ) : (
        <div className="quota-usage-grid">
          {shown.map((account) => (
            <QuotaAccountCard
              key={account.upstream}
              account={account}
              name={displayName(account.upstream)}
              sourceLabel={sourceLabel}
              copy={copy}
            />
          ))}
        </div>
      )}

      <p className="quota-usage-caveat">
        <svg viewBox="0 0 16 16" aria-hidden="true">
          <circle cx="8" cy="8" r="7" />
          <path d="M8 7.2v4M8 4.9h.01" />
        </svg>
        <span>
          {copy(
            "“Provider-reported” figures come from the provider’s own response headers and count all of the key’s usage. “Local estimate” only counts traffic through this gateway, so if you use the same key elsewhere the real remaining may be lower.",
            "「供应商权威」来自供应商自己的响应头,统计该密钥的全部用量;「本地估算」只统计经过本网关的流量,若你在别处也用同一密钥,真实剩余可能更少。", "「供應商報告」來自供應商自己的回應頭部，統計該金鑰的全部用量；「本地估算」只統計經過此閘道器的流量，若你在別處也使用同一金鑰，真實剩餘可能更少。", "「プロバイダー報告」はプロバイダー自身のレスポンスヘッダーから取得され、該当のキーの全使用量を統計します。「ローカル推定」はこのゲートウェイを通るトラフィックのみを統計するため、他の場所でも同じキーを使用している場合、実際の残量はさらに少なくなる可能性があります。"
          )}
        </span>
      </p>
    </div>
  );
}

function QuotaAccountCard({
  account,
  name,
  sourceLabel,
  copy,
}: {
  account: QuotaAccountSnapshot;
  name: string;
  sourceLabel: (source: QuotaSource) => { text: string; tone: string };
  copy: LocalizedCopy;
}) {
  const source = sourceLabel(account.source);
  const cooling = account.cooling_ms_remaining > 0;
  return (
    <section className="panel quota-account-card">
      <div className="quota-account-head">
        <div>
          <h2>{name}</h2>
          <span className={`quota-source-badge ${source.tone}`}>{source.text}</span>
        </div>
        <div className="quota-account-flags">
          {cooling && (
            <span className="quota-flag cooling">
              {copy("Cooling", "冷却中", "冷卻中", "クールダウン中")} · {formatDuration(account.cooling_ms_remaining)}
            </span>
          )}
          {!cooling && account.exhausted && (
            <span className="quota-flag exhausted">{copy("Exhausted", "已耗尽", "已耗盡", "使用上限に達しました")}</span>
          )}
          {account.rate_pressured && (
            <span className="quota-flag pressured">{copy("Rate-pressured", "速率吃紧", "速率吃緊", "レート制限に達しました")}</span>
          )}
          {account.inflight > 0 && (
            <span className="quota-flag inflight">
              {copy(`${account.inflight} in flight`, `${account.inflight} 个在途`, `${account.inflight} 個在途`, `${account.inflight} 個進行中`)}
            </span>
          )}
        </div>
      </div>

      {account.windows.length > 0 ? (
        <div className="quota-window-list">
          {account.windows.map((window, index) => {
            const percent = permilleToPercent(window.remaining_permille);
            return (
              <div className="quota-window-row" key={index}>
                <div className="quota-window-bar" role="img" aria-label={`${percent}%`}>
                  <span
                    className={`quota-window-fill ${percent <= 10 ? "low" : percent <= 33 ? "mid" : ""}`}
                    style={{ width: `${percent}%` }}
                  />
                </div>
                <div className="quota-window-meta">
                  <strong>{percent}%</strong>
                  <span>
                    {window.ms_until_reset >= NO_RESET_MS
                      ? copy("no reset (balance)", "余额 · 不刷新", "不重新整理 · 餘額", "リセットなし · おつり")
                      : `${copy("resets in", "距刷新", "距重新整理", "リセットまで")} ${formatDuration(window.ms_until_reset)}`}
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      ) : (
        <p className="quota-window-none">
          {copy(
            "No window data — rate and cooldown only. Declare a plan for this provider to estimate its allowance.",
            "无窗口数据——仅有速率与冷却。为该供应商声明额度计划即可估算余量。", "無視窗資料——僅有速率與冷卻。為該供應商宣告額度計劃即可估算餘量。", "ウィンドウデータなし——のみレートとクールダウンがあります。このプロバイダーにクォータプランを宣言することで余量を推定できます。"
          )}
        </p>
      )}

      <div className="quota-rate-row">
        <span>{copy("Rate headroom", "速率余量", "速率餘量", "レートヘッドルーム")}</span>
        <strong>{permilleToPercent(account.rate_headroom_permille)}%</strong>
      </div>
    </section>
  );
}
