import { useLayoutEffect, useMemo, useRef, useState } from "react";
import "./UsageTrendChart.css";
import type { AggView } from "../api";
import { useLocalizedCopy } from "./LanguageProvider";
import { cacheMetric, costCoverage, formatUsd, tokenMetric, formatUsageValue, usageCoverageLabel } from "../usagePresentation";

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
  legacy_input_requests: 0,
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
  // Subtract elapsed minutes so the second occurrence of a DST hour stays distinct.
  if (unit === "hour") return ms - date.getMinutes() * 60_000 - date.getSeconds() * 1_000 - date.getMilliseconds();
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

function shiftBucket(ms: number, unit: "hour" | "day", amount: number): number {
  if (unit === "hour") return ms + amount * 3_600_000;
  const date = new Date(ms);
  date.setDate(date.getDate() + amount);
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

export function normalizeBuckets(
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

  const duration = range === "24h" ? 86_400_000 : range === "7d" ? 7 * 86_400_000 : range === "30d" ? 30 * 86_400_000 : null;
  if (duration != null) {
    const end = bucketStart(nowMs, unit);
    // The backend filters by elapsed duration before grouping. Both endpoint
    // buckets can be partial, and calendar-day counts can vary across DST.
    const start = bucketStart(nowMs - duration, unit);
    const buckets: TrendBucket[] = [];
    for (let timestamp = start; timestamp <= end; timestamp = shiftBucket(timestamp, unit, 1)) {
      buckets.push({ key: String(timestamp), timestamp, aggregate: indexed.get(timestamp) ?? EMPTY_AGGREGATE });
    }
    return { unit, buckets };
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
  // Preserve real elapsed spacing without drawing activity across empty years.
  // Zero-valued boundary days return each curve to baseline around sparse gaps.
  const sparse = new Set(timestamps);
  for (let index = 1; index < timestamps.length; index += 1) {
    const after = shiftBucket(timestamps[index - 1], unit, 1);
    const before = shiftBucket(timestamps[index], unit, -1);
    if (after < timestamps[index]) { sparse.add(after); sparse.add(before); }
  }
  const source = continuous[continuous.length - 1] === last ? continuous : [...sparse].sort((a, b) => a - b);
  return {
    unit,
    buckets: source.map((timestamp) => ({
      key: String(timestamp),
      timestamp,
      aggregate: indexed.get(timestamp) ?? EMPTY_AGGREGATE,
    })),
  };
}

function tickIndexes(timestamps: number[]): number[] {
  if (timestamps.length <= 1) return timestamps.map((_, index) => index);
  const last = timestamps.length - 1;
  const minimumGap = (timestamps[last] - timestamps[0]) / 6;
  const indexes = [0];
  for (let index = 1; index < last; index += 1) {
    if (timestamps[index] - timestamps[indexes[indexes.length - 1]] >= minimumGap
      && timestamps[last] - timestamps[index] >= minimumGap) indexes.push(index);
  }
  return [...indexes, last];
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
  values: (number | null)[],
  scaleMax: number,
  xForIndex: (index: number) => number,
  yForValue: (value: number, max: number) => number,
  baseline: number,
): string {
  let result = "";
  let start = 0;
  while (start < values.length) {
    if (values[start] == null) { start += 1; continue; }
    let end = start;
    while (end + 1 < values.length && values[end + 1] != null) end += 1;
    result += smoothPath(values.slice(start, end + 1), scaleMax, (index) => xForIndex(start + index), yForValue);
    result += ` L ${xForIndex(end).toFixed(1)} ${baseline.toFixed(1)} L ${xForIndex(start).toFixed(1)} ${baseline.toFixed(1)} Z `;
    start = end + 1;
  }
  return result;
}

function costLabel(value: number): string { return formatUsd(Math.round(value * 1_000_000)); }

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
  const [pointer, setPointer] = useState<{ x: number; y: number; width: number; height: number } | null>(null);
  const tooltipRef = useRef<HTMLDivElement>(null);
  const [tooltipSize, setTooltipSize] = useState({ width: 260, height: 220 });
  const hasLegacyInput = buckets.some(({ aggregate }) => aggregate.legacy_input_requests > 0);
  const plotWidth = WIDTH - PLOT.left - PLOT.right;
  const plotHeight = HEIGHT - PLOT.top - PLOT.bottom;
  const first = buckets[0]?.timestamp ?? 0;
  const last = buckets[buckets.length - 1]?.timestamp ?? first;
  const xForIndex = (index: number) => PLOT.left + (first === last ? 0.5 : (buckets[index].timestamp - first) / (last - first)) * plotWidth;
  const yForValue = (value: number, max: number) => (
    PLOT.top + plotHeight - (value / max) * plotHeight
  );
  const tokenSeries = TOKEN_SERIES.map((series) => ({
    ...series,
    label: {
      input_tokens: hasLegacyInput
        ? copy("Input reported", "上游输入", "上游輸入", "報告された入力")
        : copy("Input total", "输入总量", "輸入總量", "入力合計"),
      output_tokens: copy("Output", "输出", "輸出", "出力"),
      cache_write_tokens: copy("Cache write", "缓存写入", "快取寫入", "キャッシュ書き込み"),
      cache_read_tokens: copy("Cache hit", "缓存命中", "快取命中", "キャッシュヒット"),
    }[series.key],
    values: buckets.map(({ aggregate }) => (series.key === "cache_read_tokens" || series.key === "cache_write_tokens" ? cacheMetric(aggregate, series.key) : tokenMetric(aggregate, series.key)).value),
  }));
  const rawTokenMaximum = Math.max(
    0,
    ...tokenSeries.flatMap((series) => series.values.map((value) => value ?? 0)),
  );
  const hasReportedTokens = tokenSeries.some((series) => series.values.some((value, index) => value != null && buckets[index].aggregate.requests > 0));
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
  const isolatedCostIndexes = costValues.flatMap((value, index) => value != null && value > 0
    && (index === 0 || costValues[index - 1] == null)
    && (index === buckets.length - 1 || costValues[index + 1] == null) ? [index] : []);
  const annotatedCostIndex = active
    ? (isolatedCostIndexes.includes(activeIndex) ? activeIndex : -1)
    : (isolatedCostIndexes[isolatedCostIndexes.length - 1] ?? -1);
  useLayoutEffect(() => {
    const bounds = tooltipRef.current?.getBoundingClientRect();
    if (bounds?.width && bounds.height) setTooltipSize((previous) => previous.width === bounds.width && previous.height === bounds.height
      ? previous : { width: bounds.width, height: bounds.height });
  }, [active, language, pointer?.width]);
  const clearInspection = () => { setActiveKey(null); setPointer(null); };
  const guideX = active ? xForIndex(activeIndex) : PLOT.left;
  const pointerGuideX = pointer ? guideX / WIDTH * pointer.width : 0;
  const tooltipLeft = pointer
    ? Math.max(12, Math.min(pointer.width - tooltipSize.width - 12,
      Math.max(pointer.x, pointerGuideX) + 16 + tooltipSize.width <= pointer.width - 12
        ? Math.max(pointer.x, pointerGuideX) + 16 : Math.min(pointer.x, pointerGuideX) - tooltipSize.width - 16))
    : `clamp(12px, calc(${guideX / WIDTH * 100}% ${guideX > WIDTH / 2 ? "- 276px" : "+ 16px"}), max(12px, calc(100% - 272px)))`;
  const unitName = unit === "hour" ? copy("hours", "小时", "小時", "時間") : copy("days", "天", "天", "日");
  const summary = copy(
    `Usage trend with ${buckets.length} ${unitName} and ${activeCount} active periods. Tokens use the left axis and cost uses the right axis.`,
    `用量趋势，共 ${buckets.length} 个${unitName}槽，活跃 ${activeCount} 个；左轴为 Token，右轴为成本。`, `用量趨勢，共 ${buckets.length} 個${unitName}槽，活躍 ${activeCount} 個；左軸為 Token，右軸為成本。`, `使用状況のトレンド、${buckets.length} 個の ${unitName} 槽、活発な ${activeCount} 個；左軸は Token、右軸はコスト。`
  );
  const ticks = tickIndexes(buckets.map((bucket) => bucket.timestamp));
  const usageText = (aggregate: AggView, key: TokenSeriesKey) => formatUsageValue(
    key === "cache_read_tokens" || key === "cache_write_tokens" ? cacheMetric(aggregate, key) : tokenMetric(aggregate, key),
    (value) => value.toLocaleString(language), copy,
  );
  const costText = (aggregate: AggView) => aggregate.requests === 0 ? formatUsd(aggregate.cost_micros ?? 0) : aggregate.cost_micros == null
    ? copy("Unknown", "未知", "未知", "不明")
    : formatUsd(aggregate.cost_micros) + (costCoverage(aggregate).complete || aggregate.requests === 0 ? "" : copy(" (partial)", "（部分）", "（部分）", "（一部）"));
  const partialTokens = buckets.some(({ aggregate }) => aggregate.requests > 0 && !tokenMetric(aggregate, "total").complete);
  const partialCost = buckets.some(({ aggregate }) => aggregate.requests > 0 && !costCoverage(aggregate).complete);

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
        <span>{copy("Active", "活跃", "活躍", "活発")} <strong>{activeCount}</strong> / {buckets.length} {copy("periods", "时段", "時段", "期間")}</span>
        <span>{copy("Peak tokens", "Token 峰值", "Token 峰值", "Token ピーク")} <strong>{hasReportedTokens ? compact(rawTokenMaximum, language) : copy("Not reported", "未上报", "未回報", "未報告")}</strong>{hasReportedTokens ? ` / ${unitName}` : ""}{partialTokens && hasReportedTokens && <small>{copy(" (partial)", "（部分上报）", "（部分回報）", "（一部報告）")}</small>}</span>
        <span>
          {copy("Known cost peak", "已知成本峰值", "已知成本峰值", "既知のコスト ピーク")}{" "}
          <strong>{hasPricedCost ? costLabel(rawCostMaximum) : copy("Unknown", "未知", "未知", "不明")}</strong>
          {hasPricedCost ? ` / ${unitName}` : ""}{partialCost && hasPricedCost && <small>{copy(" (partial)", "（部分）", "（部分）", "（一部）")}</small>}
        </span>
      </div>

      <div className="usage-chart-stage">
        <svg
          className="usage-trend-svg"
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          role="img"
          aria-label={summary}
          preserveAspectRatio="none"
          onMouseMove={(event) => {
            const bounds = event.currentTarget.getBoundingClientRect();
            if (!bounds.width || !bounds.height) return;
            const x = (event.clientX - bounds.left) / bounds.width * WIDTH;
            const y = (event.clientY - bounds.top) / bounds.height * HEIGHT;
            if (x < PLOT.left || x > WIDTH - PLOT.right || y < PLOT.top || y > HEIGHT - PLOT.bottom) {
              clearInspection();
              return;
            }
            let nearest = 0;
            for (let index = 1; index < buckets.length; index += 1) {
              if (Math.abs(xForIndex(index) - x) < Math.abs(xForIndex(nearest) - x)) nearest = index;
            }
            setActiveKey(buckets[nearest].key);
            setPointer({ x: event.clientX - bounds.left, y: event.clientY - bounds.top, width: bounds.width, height: bounds.height });
          }}
          onMouseLeave={clearInspection}
          onKeyDown={(event) => {
            if (event.key === "Escape") { clearInspection(); event.preventDefault(); }
            if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
              event.preventDefault();
              const next = Math.max(0, Math.min(buckets.length - 1, activeIndex + (event.key === "ArrowRight" ? 1 : -1)));
              setPointer(null);
              setActiveKey(buckets[next].key);
            }
          }}
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

          {tokenSeries.map((series) => series.values.map((value, index) => value != null && value > 0
            && (index === 0 || series.values[index - 1] == null) && (index === buckets.length - 1 || series.values[index + 1] == null)
            ? <circle key={`${series.key}-${index}`} data-usage-point className={`usage-chart-point ${series.className}`} cx={xForIndex(index)} cy={yForValue(value, tokenMaximum)} r="2.5" /> : null))}
          {isolatedCostIndexes.map((index) => (
            <circle key={index} data-cost-point className="usage-chart-point cost usage-chart-isolated-cost" cx={xForIndex(index)} cy={yForValue(costValues[index]!, costMaximum)} r="2.5" />
          ))}
          {annotatedCostIndex >= 0 && (
            <text
              className="usage-chart-cost-annotation"
              x={xForIndex(annotatedCostIndex) + (xForIndex(annotatedCostIndex) > WIDTH / 2 ? -10 : 10)}
              y={Math.max(PLOT.top + 14, yForValue(costValues[annotatedCostIndex]!, costMaximum) - 10)}
              textAnchor={xForIndex(annotatedCostIndex) > WIDTH / 2 ? "end" : "start"}
            >
              {copy("Cost", "成本", "成本", "コスト")} {costText(buckets[annotatedCostIndex].aggregate)}
            </text>
          )}

          {active && (
            <g className="usage-chart-crosshair" aria-hidden="true">
              <line
                x1={xForIndex(activeIndex)}
                x2={xForIndex(activeIndex)}
                y1={PLOT.top}
                y2={PLOT.top + plotHeight}
              />
              {tokenSeries.filter((series) => series.values[activeIndex] != null).map((series) => (
                <circle
                  key={series.key}
                  cx={xForIndex(activeIndex)}
                  cy={yForValue(series.values[activeIndex] ?? 0, tokenMaximum)}
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
            const label = `${detailedLabel(bucket.timestamp, unit, language)} · ${tokenSeries.map((series) => `${series.label} ${usageText(aggregate, series.key)}`).join(" · ")} · ${copy("Cost", "成本", "成本", "コスト")} ${costText(aggregate)}`;
            return (
              <g
                key={bucket.key}
                data-usage-bucket
                data-bucket-key={bucket.key}
                tabIndex={isActive(aggregate) ? 0 : undefined}
                role={isActive(aggregate) ? "button" : undefined}
                aria-label={isActive(aggregate) ? label : undefined}
                onMouseEnter={() => setActiveKey(bucket.key)}
                onFocus={() => { setPointer(null); setActiveKey(bucket.key); }}
                onBlur={clearInspection}
              >
                <rect
                  x={hitLeft}
                  y={PLOT.top}
                  width={Math.max(1, hitRight - hitLeft)}
                  height={plotHeight}
                  className="usage-chart-hit-area"
                />

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
          {active && (
            <g className="usage-chart-time-badge" aria-hidden="true" transform={`translate(${Math.max(62, Math.min(WIDTH - 62, guideX))}, ${HEIGHT - 13})`}>
              <rect x="-60" y="-12" width="120" height="22" rx="4" />
              <text textAnchor="middle" y="3">{axisLabel(active.timestamp, unit, true, language)}</text>
            </g>
          )}
        </svg>

        {active && (
          <div
            ref={tooltipRef}
            className="usage-chart-tooltip"
            style={{ left: tooltipLeft, top: pointer ? Math.max(12, Math.min(pointer.y + 12, pointer.height - tooltipSize.height - 42)) : 16 }}
            role="status"
          >
            <strong>{detailedLabel(active.timestamp, unit, language)}</strong>
            <div className="usage-chart-tooltip-unit">{unit === "hour" ? copy("Hourly total", "小时合计", "小時合計", "時間ごとの合計") : copy("Daily total", "每日合计", "每日合計", "日別合計")}</div>
            {tokenSeries.map((series) => (
              <span className={`series-${series.className}`} key={series.key}>
                <i />{series.label}<em>{usageText(active.aggregate, series.key)}</em>
              </span>
            ))}
            <span className="series-cost">
              <i />{copy("Cost", "成本", "成本", "コスト")}
              <em>{costText(active.aggregate)}</em>
            </span>
            {isolatedCostIndexes.includes(activeIndex) && (
              <small>{copy(
                "Adjacent periods lack cost data for a continuous curve.",
                "相邻时段成本数据不足，无法形成连续曲线。",
                "相鄰時段成本資料不足，無法形成連續曲線。",
                "隣接する期間のコストデータが不足しているため、連続した曲線を描画できません。",
              )}</small>
            )}
            <small>{copy(
              `${active.aggregate.priced_requests} priced`,
              `${active.aggregate.priced_requests} 次已计价`,
              `${active.aggregate.priced_requests} 次已計價`,
              `${active.aggregate.priced_requests} 件のコストあり`,
            )} · {usageCoverageLabel(active.aggregate, copy)}</small>
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
