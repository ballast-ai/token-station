import type { AggView, ReceiptView } from "./api";
import type { LocalizedCopy } from "./components/LanguageProvider";

export type TokenMetricKey = "input_tokens" | "output_tokens" | "total";

export function tokenMetric(value: AggView, key: TokenMetricKey) {
  const expected = value.usage_expected_requests ?? value.requests;
  // Older transports supplied numeric totals without field coverage. Preserve
  // their presentation; new transports explicitly send zero for absent reports.
  const input = value.input_reported_requests ?? value.requests;
  const output = value.output_reported_requests ?? value.requests;
  const reported = key === "input_tokens" ? input : key === "output_tokens" ? output
    : value.total_reported_requests ?? Math.min(input, output);
  const available = key === "total" ? input > 0 || output > 0 : reported > 0;
  return {
    value: value.requests === 0 || available
      ? key === "total" ? value.input_tokens + value.output_tokens : value[key]
      : null,
    reported,
    complete: expected === 0 || (reported === expected && !(value.incomplete_usage_requests ?? 0)),
  };
}

export function receiptTokenMetric(receipt: ReceiptView, key: TokenMetricKey) {
  const input = receipt.usage_observation != null
    ? receipt.usage_observation.input_tokens : receipt.usage?.input_tokens;
  const output = receipt.usage_observation != null
    ? receipt.usage_observation.output_tokens : receipt.usage?.output_tokens;
  const complete = key === "input_tokens" ? input != null : key === "output_tokens" ? output != null
    : input != null && output != null;
  return {
    value: key === "input_tokens" ? input ?? null : key === "output_tokens" ? output ?? null
      : input != null || output != null ? (input ?? 0) + (output ?? 0) : null,
    reported: Number(complete),
    complete: complete && !receipt.usage_observation?.incomplete,
  };
}

/** Mirrors accounting::can_estimate for receipt explanations, not settlement. */
export function receiptHasUsableUsage(receipt: ReceiptView): boolean {
  if (!receipt.usage || !receiptTokenMetric(receipt, "total").complete) return false;
  const observation = receipt.usage_observation;
  const input = observation?.input_tokens ?? receipt.usage.input_tokens;
  const read = observation?.cache_read_tokens ?? receipt.usage.cache_read_tokens;
  const write = observation?.cache_write_tokens ?? receipt.usage.cache_write_tokens;
  const short = observation?.cache_write_5m_tokens ?? 0;
  const long = observation?.cache_write_1h_tokens ?? 0;
  return read + write <= input && short + long <= write;
}

export function formatUsageValue(
  metric: { value: number | null; complete: boolean; historicallyUnrecorded?: boolean },
  format: (value: number) => string,
  copy: LocalizedCopy,
): string {
  if (metric.value == null) return metric.historicallyUnrecorded
    ? copy("Historically unrecorded", "历史未记录", "歷史未記錄", "過去の記録なし")
    : copy("Not reported", "未上报", "未回報", "未報告");
  return format(metric.value) + (metric.complete ? "" : copy(" (partial)", "（部分上报）", "（部分回報）", "（一部報告）"));
}

export function formatUsd(micros: number | null): string {
  if (micros == null) return "—";
  return `$${(micros / 1_000_000).toLocaleString("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 6 })}`;
}

export function costCoverage(value: AggView) {
  const missingUsage = value.missing_usage_requests ?? 0;
  return {
    priced: value.priced_requests,
    missingPrice: value.missing_price_requests ?? Math.max(0, value.unpriced_requests - missingUsage),
    missingUsage,
    complete: value.requests > 0 && value.priced_requests === value.requests && value.cost_micros != null,
  };
}

export type CacheMetricKey = "cache_read_tokens" | "cache_write_tokens";

export function cacheMetric(value: AggView, key: CacheMetricKey) {
  const expected = value.usage_expected_requests ?? value.requests;
  const reported = value[key === "cache_read_tokens" ? "cache_read_reported_requests" : "cache_write_reported_requests"] ?? 0;
  const unrecorded = value[key === "cache_read_tokens" ? "cache_read_unrecorded_requests" : "cache_write_unrecorded_requests"] ?? 0;
  return {
    value: value.requests === 0 || reported > 0 || value[key] > 0 ? value[key] : null,
    reported,
    complete: expected === 0 || (reported === expected && !(value.incomplete_usage_requests ?? 0)),
    ...(expected > 0 && unrecorded === expected ? { historicallyUnrecorded: true } : {}),
  };
}

/** Explain cost gaps without treating failed requests with absent usage as free. */
export function usageCoverageLabel(value: AggView, copy: LocalizedCopy): string {
  const coverage = costCoverage(value);
  const missingUsage = Math.max(0, coverage.missingUsage - (value.unpriced_failed_without_usage_requests ?? 0));
  const failed = value.failed_without_usage_requests ?? 0;
  const gaps = copy(
    `${coverage.missingPrice} missing price · ${missingUsage} missing usage`,
    `${coverage.missingPrice} 次缺少价格 · ${missingUsage} 次用量不完整`,
    `${coverage.missingPrice} 次缺少價格 · ${missingUsage} 次用量不完整`,
    `${coverage.missingPrice} 件の価格なし · ${missingUsage} 件の使用量不完全`,
  );
  return gaps + (failed > 0 ? copy(
    ` · ${failed} failed/cancelled without usage`,
    ` · ${failed} 次失败/取消未返回用量`,
    ` · ${failed} 次失敗/取消未回報用量`,
    ` · ${failed} 件の失敗・キャンセルで使用量なし`,
  ) : "");
}
