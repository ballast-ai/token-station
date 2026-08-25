import { useMemo, useState } from "react";
import type { AggView } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";

export type UsageTrendRange = "24h" | "7d" | "30d" | "all";

interface UsageTrendChartProps {
  groups: [string, AggView][];
  range: UsageTrendRange;
  nowMs?: number;
}

interface TrendBucket {
  key: string;
  timestamp: number;
  aggregate: AggView;
}

type TokenSeriesKey =
  | "input_tokens"
  | "output_tokens"
  | "cache_write_tokens"
  | "cache_read_tokens";

const WIDTH = 920;
const HEIGHT = 300;
const PLOT = { left: 58, right: 64, top: 18, bottom: 42 };
const TOKEN_SERIES: {
  key: TokenSeriesKey;
  className: string;
}[] = [
  { key: "input_tokens", className: "input" },
  { key: "output_tokens", className: "output" },
  { key: "cache_write_tokens", className: "cache-write" },
  { key: "cache_read_tokens", className: "cache-read" },
];
const EMPTY_AGGREGATE: AggView = {
  requests: 0,
  errors: 0,
  p50_latency_ms: 0,
  p95_latency_ms: 0,
  input_tokens: 0,
  output_tokens: 0,
  cache_read_tokens: 0,
  cache_write_tokens: 0,
  reasoning_tokens: 0,
  cost_micros: null,
  priced_requests: 0,
  unpriced_requests: 0,
};

function compact(value: number, locale: string): string {
  return new Intl.NumberFormat(locale, {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

function bucketStart(ms: number, unit: "hour" | "day"): number {
  const date = new Date(ms);
  if (unit === "hour") date.setMinutes(0, 0, 0);
  else date.setHours(0, 0, 0, 0);
  return date.getTime();
}

function shiftBucket(ms: number, unit: "hour" | "day", amount: number): number {
  const date = new Date(ms);
  if (unit === "hour") date.setHours(date.getHours() + amount);
  else date.setDate(date.getDate() + amount);
  return date.getTime();
}

function detailedLabel(timestamp: number, unit: "hour" | "day", locale: string): string {
  const date = new Date(timestamp);
  return date.toLocaleString(locale, unit === "hour"
    ? { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false }
    : { year: "numeric", month: "2-digit", day: "2-digit" });
}

function axisLabel(timestamp: number, unit: "hour" | "day", edge: boolean, locale: string): string {
  const date = new Date(timestamp);
  if (unit === "day") {
    return date.toLocaleDateString(locale, { month: "numeric", day: "numeric" });
  }
  const time = date.toLocaleTimeString(locale, {
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  });
  if (edge || date.getHours() === 0) return `${date.getMonth() + 1}/${date.getDate()} ${time}`;
  return time;
}

function isActive(aggregate: AggView): boolean {
  return aggregate.requests > 0
    || aggregate.input_tokens > 0
    || aggregate.output_tokens > 0
    || aggregate.cache_read_tokens > 0
    || aggregate.cache_write_tokens > 0;
}

function normalizeBuckets(
  groups: [string, AggView][],
  range: UsageTrendRange,
  nowMs: number,
): { buckets: TrendBucket[]; unit: "hour" | "day" } {
  const unit = range === "24h" ? "hour" : "day";
  const indexed = new Map<number, AggView>();
  for (const [raw, aggregate] of groups) {
    const timestamp = Number(raw);
    if (Number.isFinite(timestamp)) indexed.set(bucketStart(timestamp, unit), aggregate);
  }

  const fixedCount = range === "24h" ? 24 : range === "7d" ? 7 : range === "30d" ? 30 : null;
  if (fixedCount != null) {
    const end = bucketStart(nowMs, unit);
    const start = shiftBucket(end, unit, -(fixedCount - 1));
    return {
      unit,
      buckets: Array.from({ length: fixedCount }, (_, index) => {
        const timestamp = shiftBucket(start, unit, index);
        return {
          key: String(timestamp),
          timestamp,
          aggregate: indexed.get(timestamp) ?? EMPTY_AGGREGATE,
        };
      }),
    };
  }

  const timestamps = [...indexed.keys()].sort((left, right) => left - right);
  if (timestamps.length === 0) return { buckets: [], unit };
  const first = timestamps[0];
  const last = timestamps[timestamps.length - 1];
  const continuous: number[] = [];
  for (
    let cursor = first;
    cursor <= last && continuous.length < 120;
    cursor = shiftBucket(cursor, unit, 1)
  ) {
    continuous.push(cursor);
  }
  const source = continuous.length < 120 ? continuous : timestamps;
  return {
    unit,
    buckets: source.map((timestamp) => ({
      key: String(timestamp),
      timestamp,
      aggregate: indexed.get(timestamp) ?? EMPTY_AGGREGATE,
    })),
  };
}

function tickIndexes(length: number): number[] {
  if (length <= 7) return Array.from({ length }, (_, index) => index);
  return [...new Set([0, 0.2, 0.4, 0.6, 0.8, 1].map(
    (ratio) => Math.round((length - 1) * ratio),
  ))];
}

function niceMaximum(value: number): number {
  if (value <= 0) return 1;
  const power = 10 ** Math.floor(Math.log10(value));
  const normalized = value / power;
  const ceiling = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return ceiling * power;
}

function smoothPath(
  values: (number | null)[],
  scaleMax: number,
  xForIndex: (index: number) => number,
  yForValue: (value: number, max: number) => number,
): string {
  let path = "";
  let previous: { x: number; y: number } | null = null;
  values.forEach((value, index) => {
    if (value == null) {
      previous = null;
      return;
    }
    const point = { x: xForIndex(index), y: yForValue(value, scaleMax) };
    if (!previous) {
      path += `M ${point.x.toFixed(1)} ${point.y.toFixed(1)}`;
    } else {
      const midpoint = (previous.x + point.x) / 2;
      path += ` C ${midpoint.toFixed(1)} ${previous.y.toFixed(1)}, ${midpoint.toFixed(1)} ${point.y.toFixed(1)}, ${point.x.toFixed(1)} ${point.y.toFixed(1)}`;
    }
    previous = point;
  });
  return path;
}

function areaPath(
  values: number[],
  scaleMax: number,
  xForIndex: (index: number) => number,
  yForValue: (value: number, max: number) => number,
  baseline: number,
): string {
  if (values.length === 0) return "";
  const line = smoothPath(values, scaleMax, xForIndex, yForValue);
  return `${line} L ${xForIndex(values.length - 1).toFixed(1)} ${baseline.toFixed(1)} L ${xForIndex(0).toFixed(1)} ${baseline.toFixed(1)} Z`;
}

function costLabel(value: number): string {
  if (value === 0) return "$0";
  if (value >= 1) return `$${value.toFixed(value >= 10 ? 0 : 1)}`;
  return `$${value.toFixed(3)}`;
}

export default function UsageTrendChart({
  groups,
  range,
  nowMs = Date.now(),
}: UsageTrendChartProps) {
  const { language, copy } = useLocalizedCopy();
  const { buckets, unit } = useMemo(
    () => normalizeBuckets(groups, range, nowMs),
    [groups, range, nowMs],
  );
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const plotWidth = WIDTH - PLOT.left - PLOT.right;
  const plotHeight = HEIGHT - PLOT.top - PLOT.bottom;
  const xForIndex = (index: number) => (
    PLOT.left + (index / Math.max(1, buckets.length - 1)) * plotWidth
  );
  const yForValue = (value: number, max: number) => (
    PLOT.top + plotHeight - (value / max) * plotHeight
  );
  const tokenSeries = TOKEN_SERIES.map((series) => ({
    ...series,
    label: {
      input_tokens: copy("Input total", "输入总量", "輸入總量", "入力合計"),
      output_tokens: copy("Output", "输出", "輸出", "出力"),
      cache_write_tokens: copy("Cache write", "缓存写入", "快取寫入", "キャッシュ書き込み"),
      cache_read_tokens: copy("Cache hit", "缓存命中", "快取命中", "キャッシュヒット"),
    }[series.key],
    values: buckets.map(({ aggregate }) => aggregate[series.key]),
  }));
  const rawTokenMaximum = Math.max(
    0,
    ...tokenSeries.flatMap((series) => series.values),
  );
  const tokenMaximum = niceMaximum(rawTokenMaximum);
  const costValues = buckets.map(({ aggregate }) => {
    if (aggregate.cost_micros != null) return aggregate.cost_micros / 1_000_000;
    return aggregate.requests > 0 ? null : 0;
  });
  const rawCostMaximum = Math.max(
    0,
    ...costValues.filter((value): value is number => value != null),
  );
  const costMaximum = niceMaximum(rawCostMaximum);
  const hasPricedCost = buckets.some(
    ({ aggregate }) => aggregate.priced_requests > 0 && aggregate.cost_micros != null,
  );
  const activeCount = buckets.filter(({ aggregate }) => isActive(aggregate)).length;
  const activeIndex = buckets.findIndex((bucket) => bucket.key === activeKey);
  const active = activeIndex >= 0 ? buckets[activeIndex] : null;
  const unitName = unit === "hour" ? copy("hours", "小时", "小時", "時間") : copy("days", "天", "天", "日");
  const summary = copy(
    `Usage trend with ${buckets.length} ${unitName} and ${activeCount} active periods. Tokens use the left axis and cost uses the right axis.`,
    `用量趋势，共 ${buckets.length} 个${unitName}槽，活跃 ${activeCount} 个；左轴为 Token，右轴为成本。`, `用量趨勢，共 ${buckets.length} 個${unitName}槽，活躍 ${activeCount} 個；左軸為 Token，右軸為成本。`, `使用状況のトレンド、${buckets.length} 個の ${unitName} 槽、活発な ${activeCount} 個；左軸は Token、右軸はコスト。`
  );
  const ticks = tickIndexes(buckets.length);

  if (activeCount === 0) {
    return (
      <div className="usage-chart-empty">
        <span aria-hidden="true">⌁</span>
        <strong>{copy(
          "No usage data to chart in this range",
          "当前范围没有可绘制的用量数据", "此範圍內沒有可繪製的用量資料", "この範囲には描画可能な使用状況データがありません"
        )}</strong>
        <small>{copy(
          "Input, output, cache, and cost trends appear after the first model request.",
          "完成一次模型请求后，输入、输出、缓存与成本趋势会出现在这里。", "完成一次模型請求後，輸入、輸出、快取與成本趨勢會出現在這裡。", "モデルの最初のリクエストを完了した後、入力、出力、キャッシュ、コストのトレンドがここに表示されます。"
        )}</small>
      </div>
    );
  }

  return (
    <div className="usage-trend-chart">
      <div className="usage-chart-meta">
        <span>{copy("Active", "活跃", "活躍", "活発")} <strong>{activeCount}</strong> / {buckets.length} {unitName}</span>
        <span>{copy("Peak tokens", "Token 峰值", "Token 峰值", "Token ピーク")} <strong>{compact(rawTokenMaximum, language)}</strong> / {unitName}</span>
        <span>
          {copy("Peak cost", "成本峰值", "成本峰值", "コスト ピーク")}{" "}
          <strong>{hasPricedCost ? costLabel(rawCostMaximum) : copy("Unpriced", "未定价", "未定價", "未設定")}</strong>
          {hasPricedCost ? ` / ${unitName}` : ""}
        </span>
      </div>

      <div className="usage-chart-stage">
        <svg
          className="usage-trend-svg"
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          role="img"
          aria-label={summary}
          preserveAspectRatio="none"
          onMouseLeave={() => setActiveKey(null)}
        >
          <defs>
            {tokenSeries.map((series) => (
              <linearGradient
                key={series.key}
                id={`usage-gradient-${series.className}`}
                x1="0"
                y1="0"
                x2="0"
                y2="1"
              >
                <stop offset="0%" className={`usage-chart-gradient ${series.className}`} />
                <stop offset="100%" className="usage-chart-gradient-end" />
              </linearGradient>
            ))}
          </defs>

          {[0, 0.25, 0.5, 0.75, 1].map((ratio) => {
            const y = PLOT.top + plotHeight * ratio;
            const tokenValue = tokenMaximum * (1 - ratio);
            const costValue = costMaximum * (1 - ratio);
            return (
              <g key={ratio}>
                <line
                  x1={PLOT.left}
                  x2={WIDTH - PLOT.right}
                  y1={y}
                  y2={y}
                  className="usage-chart-grid"
                />
                <text x={PLOT.left - 10} y={y + 3} textAnchor="end" className="usage-chart-axis">
                  {compact(tokenValue, language)}
                </text>
                <text x={WIDTH - PLOT.right + 10} y={y + 3} textAnchor="start" className="usage-chart-axis">
                  {hasPricedCost ? costLabel(costValue) : ratio === 1 ? "$0" : "—"}
                </text>
              </g>
            );
          })}

          {tokenSeries.map((series) => (
            <g key={series.key}>
              <path
                d={areaPath(
                  series.values,
                  tokenMaximum,
                  xForIndex,
                  yForValue,
                  PLOT.top + plotHeight,
                )}
                className={`usage-chart-area ${series.className}`}
                fill={`url(#usage-gradient-${series.className})`}
              />
              <path
                d={smoothPath(series.values, tokenMaximum, xForIndex, yForValue)}
                className={`usage-chart-series ${series.className}`}
              />
            </g>
          ))}

          <path
            d={smoothPath(costValues, costMaximum, xForIndex, yForValue)}
            className="usage-chart-cost-line"
          />

          {active && (
            <g className="usage-chart-crosshair" aria-hidden="true">
              <line
                x1={xForIndex(activeIndex)}
                x2={xForIndex(activeIndex)}
                y1={PLOT.top}
                y2={PLOT.top + plotHeight}
              />
              {tokenSeries.map((series) => (
                <circle
                  key={series.key}
                  cx={xForIndex(activeIndex)}
                  cy={yForValue(active.aggregate[series.key], tokenMaximum)}
                  r="4"
                  className={series.className}
                />
              ))}
              {costValues[activeIndex] != null && (
                <circle
                  cx={xForIndex(activeIndex)}
                  cy={yForValue(costValues[activeIndex] ?? 0, costMaximum)}
                  r="4"
                  className="cost"
                />
              )}
            </g>
          )}

          {buckets.map((bucket, index) => {
            const aggregate = bucket.aggregate;
            const x = xForIndex(index);
            const nextX = index < buckets.length - 1 ? xForIndex(index + 1) : WIDTH - PLOT.right;
            const previousX = index > 0 ? xForIndex(index - 1) : PLOT.left;
            const hitLeft = index === 0 ? PLOT.left : (previousX + x) / 2;
            const hitRight = index === buckets.length - 1 ? WIDTH - PLOT.right : (x + nextX) / 2;
            const currentCost = costValues[index];
            const cacheWriteValue = aggregate.cache_write_tokens > 0
              ? aggregate.cache_write_tokens.toLocaleString(language)
              : "N/A";
            const label = copy(
              `${detailedLabel(bucket.timestamp, unit, language)}: input total ${aggregate.input_tokens.toLocaleString(language)}, output ${aggregate.output_tokens.toLocaleString(language)}, cache write ${cacheWriteValue}, cache hit ${aggregate.cache_read_tokens.toLocaleString(language)}, cost ${currentCost == null ? "unknown" : costLabel(currentCost)}`,
              `${detailedLabel(bucket.timestamp, unit, language)}：输入总量 ${aggregate.input_tokens.toLocaleString(language)}，输出 ${aggregate.output_tokens.toLocaleString(language)}，缓存写入 ${cacheWriteValue}，缓存命中 ${aggregate.cache_read_tokens.toLocaleString(language)}，成本 ${currentCost == null ? "未知" : costLabel(currentCost)}`,
              `${detailedLabel(bucket.timestamp, unit, language)}：輸入總量 ${aggregate.input_tokens.toLocaleString(language)}，輸出 ${aggregate.output_tokens.toLocaleString(language)}，快取寫入 ${cacheWriteValue}，快取命中 ${aggregate.cache_read_tokens.toLocaleString(language)}，成本 ${currentCost == null ? "未知" : costLabel(currentCost)}`,
              `${detailedLabel(bucket.timestamp, unit, language)}：入力合計 ${aggregate.input_tokens.toLocaleString(language)}、出力 ${aggregate.output_tokens.toLocaleString(language)}、キャッシュ書き込み ${cacheWriteValue}、キャッシュヒット ${aggregate.cache_read_tokens.toLocaleString(language)}、コスト ${currentCost == null ? "不明" : costLabel(currentCost)}`
            );
            return (
              <g
                key={bucket.key}
                data-usage-bucket
                data-bucket-key={bucket.key}
                tabIndex={isActive(aggregate) ? 0 : undefined}
                aria-label={isActive(aggregate) ? label : undefined}
                onMouseEnter={() => setActiveKey(bucket.key)}
                onFocus={() => isActive(aggregate) && setActiveKey(bucket.key)}
                onBlur={() => setActiveKey(null)}
              >
                <title>{label}</title>
                <rect
                  x={hitLeft}
                  y={PLOT.top}
                  width={Math.max(1, hitRight - hitLeft)}
                  height={plotHeight}
                  className="usage-chart-hit-area"
                />
                {currentCost == null && aggregate.requests > 0 && (
                  <g data-cost-unknown className="usage-chart-cost-unknown">
                    <circle cx={x} cy={PLOT.top + plotHeight - 5} r="4" />
                    <path d={`M ${x - 2} ${PLOT.top + plotHeight - 7} L ${x + 2} ${PLOT.top + plotHeight - 3} M ${x + 2} ${PLOT.top + plotHeight - 7} L ${x - 2} ${PLOT.top + plotHeight - 3}`} />
                  </g>
                )}
              </g>
            );
          })}

          {ticks.map((index) => (
            <text
              key={buckets[index].key}
              x={xForIndex(index)}
              y={HEIGHT - 13}
              textAnchor={index === 0 ? "start" : index === buckets.length - 1 ? "end" : "middle"}
              className="usage-chart-axis usage-chart-x-axis"
            >
              {axisLabel(
                buckets[index].timestamp,
                unit,
                index === 0 || index === buckets.length - 1,
                language,
              )}
            </text>
          ))}
        </svg>

        {active && (
          <div
            className={`usage-chart-tooltip ${activeIndex / Math.max(1, buckets.length - 1) > 0.62 ? "align-right" : ""}`}
            style={{ left: `${(xForIndex(activeIndex) / WIDTH) * 100}%` }}
            role="status"
          >
            <strong>{detailedLabel(active.timestamp, unit, language)}</strong>
            {tokenSeries.map((series) => (
              <span className={series.className} key={series.key}>
                <i />{series.label}<em>{series.key === "cache_write_tokens"
                  && active.aggregate.cache_write_tokens === 0
                  ? "N/A"
                  : active.aggregate[series.key].toLocaleString(language)}</em>
              </span>
            ))}
            <span className="cost">
              <i />{copy("Cost", "成本", "成本", "コスト")}
              <em>{costValues[activeIndex] == null
                ? copy("Unknown", "未知", "未知", "不明")
                : costLabel(costValues[activeIndex] ?? 0)}</em>
            </span>
            <small>{copy(
              `${active.aggregate.requests.toLocaleString(language)} requests · ${active.aggregate.errors.toLocaleString(language)} errors · Cache metrics are a subset of input`,
              `${active.aggregate.requests.toLocaleString(language)} 次请求 · ${active.aggregate.errors.toLocaleString(language)} 个错误 · 缓存指标属于输入子集`, `${active.aggregate.requests.toLocaleString(language)} 次請求 · ${active.aggregate.errors.toLocaleString(language)} 個錯誤 · 快取指標屬於輸入子集`, `${active.aggregate.requests.toLocaleString(language)} 回のリクエスト · ${active.aggregate.errors.toLocaleString(language)} 件のエラー · キャッシュメトリクスは入力のサブセット`
            )}</small>
          </div>
        )}
      </div>
    </div>
  );
}
