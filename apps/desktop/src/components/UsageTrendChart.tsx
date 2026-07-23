import type { AggView } from "../api";

export type UsageTrendMode = "tokens" | "cost";

interface UsageTrendChartProps {
  groups: [string, AggView][];
  mode: UsageTrendMode;
}

const WIDTH = 760;
const HEIGHT = 220;
const PLOT = { left: 18, right: 14, top: 18, bottom: 34 };

function compact(value: number): string {
  return new Intl.NumberFormat("zh-CN", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

function bucketLabel(raw: string): string {
  const date = new Date(Number(raw));
  if (!Number.isFinite(date.getTime())) return raw;
  return date.toLocaleString("zh-CN", {
    month: "numeric",
    day: "numeric",
    hour: date.getHours() === 0 ? undefined : "2-digit",
  });
}

function linePath(values: number[], max: number): string {
  if (values.length === 0 || max <= 0) return "";
  const plotWidth = WIDTH - PLOT.left - PLOT.right;
  const plotHeight = HEIGHT - PLOT.top - PLOT.bottom;
  return values
    .map((value, index) => {
      const x = PLOT.left + (values.length === 1 ? plotWidth / 2 : (index / (values.length - 1)) * plotWidth);
      const y = PLOT.top + plotHeight - (value / max) * plotHeight;
      return `${index === 0 ? "M" : "L"} ${x.toFixed(1)} ${y.toFixed(1)}`;
    })
    .join(" ");
}

export default function UsageTrendChart({ groups, mode }: UsageTrendChartProps) {
  const plotWidth = WIDTH - PLOT.left - PLOT.right;
  const plotHeight = HEIGHT - PLOT.top - PLOT.bottom;
  const values = groups.map(([, aggregate]) =>
    mode === "cost"
      ? (aggregate.cost_micros ?? 0) / 1_000_000
      : aggregate.input_tokens + aggregate.output_tokens,
  );
  const max = Math.max(0, ...values);
  const hasData = groups.length > 0 && max > 0;
  const summary = mode === "cost"
    ? `成本趋势，共 ${groups.length} 个时间桶，峰值 ${max.toFixed(4)}`
    : `Token 趋势，共 ${groups.length} 个时间桶，峰值 ${Math.round(max).toLocaleString()} Token`;

  if (!hasData) {
    return (
      <div className="usage-chart-empty">
        <span aria-hidden="true">⌁</span>
        <strong>当前范围没有可绘制的{mode === "cost" ? "成本" : " Token"}数据</strong>
        <small>{mode === "cost" ? "未定价请求不会被当作零成本。" : "完成一次模型请求后，趋势会出现在这里。"}</small>
      </div>
    );
  }

  const firstLabel = bucketLabel(groups[0][0]);
  const lastLabel = bucketLabel(groups[groups.length - 1][0]);

  return (
    <svg
      className="usage-trend-svg"
      viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
      role="img"
      aria-label={summary}
      preserveAspectRatio="none"
    >
      {[0, 0.5, 1].map((ratio) => {
        const y = PLOT.top + plotHeight * ratio;
        return (
          <line
            key={ratio}
            x1={PLOT.left}
            x2={WIDTH - PLOT.right}
            y1={y}
            y2={y}
            className="usage-chart-grid"
          />
        );
      })}

      {mode === "tokens" ? groups.map(([key, aggregate], index) => {
        const count = groups.length;
        const slot = plotWidth / Math.max(1, count);
        const barWidth = Math.max(2, Math.min(18, slot * 0.62));
        const x = PLOT.left + slot * index + (slot - barWidth) / 2;
        const inputHeight = (aggregate.input_tokens / max) * plotHeight;
        const outputHeight = (aggregate.output_tokens / max) * plotHeight;
        const yOutput = PLOT.top + plotHeight - outputHeight;
        const yInput = yOutput - inputHeight;
        return (
          <g key={key}>
            <title>
              {bucketLabel(key)}：输入 {aggregate.input_tokens.toLocaleString()}，输出 {aggregate.output_tokens.toLocaleString()}
            </title>
            <rect x={x} y={yInput} width={barWidth} height={inputHeight} rx="2" className="usage-chart-input" />
            <rect x={x} y={yOutput} width={barWidth} height={outputHeight} rx="2" className="usage-chart-output" />
          </g>
        );
      }) : (
        <>
          <path d={linePath(values, max)} className="usage-chart-cost-line" />
          {groups.map(([key], index) => {
            const x = PLOT.left + (groups.length === 1 ? plotWidth / 2 : (index / (groups.length - 1)) * plotWidth);
            const y = PLOT.top + plotHeight - (values[index] / max) * plotHeight;
            return (
              <circle key={key} cx={x} cy={y} r="3.5" className="usage-chart-cost-point">
                <title>{bucketLabel(key)}：{values[index].toFixed(4)}</title>
              </circle>
            );
          })}
        </>
      )}

      <text x={PLOT.left} y={HEIGHT - 9} className="usage-chart-axis">{firstLabel}</text>
      <text x={WIDTH - PLOT.right} y={HEIGHT - 9} textAnchor="end" className="usage-chart-axis">{lastLabel}</text>
      <text x={WIDTH - PLOT.right} y={12} textAnchor="end" className="usage-chart-axis">
        {mode === "cost" ? max.toFixed(4) : compact(max)}
      </text>
    </svg>
  );
}
