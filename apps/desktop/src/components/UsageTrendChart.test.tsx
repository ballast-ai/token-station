import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AggView } from "../api";
import UsageTrendChart from "./UsageTrendChart";
import { within } from "@testing-library/react";

const aggregate: AggView = {
  requests: 3,
  errors: 0,
  p50_latency_ms: 120,
  p95_latency_ms: 240,
  input_tokens: 80_000,
  legacy_input_requests: 0,
  output_tokens: 3_000,
  cache_read_tokens: 64_000,
  cache_write_tokens: 1_200,
  reasoning_tokens: 0,
  cost_micros: 420_000,
  priced_requests: 3,
  unpriced_requests: 0,
  cache_read_reported_requests: 3,
  cache_write_reported_requests: 3,
};


const nowMs = new Date(2026, 6, 23, 13, 35).getTime();
const bucketMs = new Date(2026, 6, 23, 11).getTime();

describe("UsageTrendChart", () => {
  it("does not present missing token reports as a zero peak", () => {
    const missing = { ...aggregate, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0,
      input_reported_requests: 0, output_reported_requests: 0, total_reported_requests: 0,
      cache_read_reported_requests: 0, cache_write_reported_requests: 0 };
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), missing]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelector(".usage-chart-meta")).toHaveTextContent("Token 峰值 未上报");
  });
  it("preserves four curves, filled areas, and a dual cost axis without layout switches", () => {
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), aggregate]]} range="24h" nowMs={nowMs} />);
    expect(screen.getByRole("img", { name: /25 个小时槽，活跃 1 个/ })).toHaveAccessibleName(/左轴为 Token，右轴为成本/);
    expect(container.querySelectorAll("[data-usage-bucket]")).toHaveLength(25);
    expect(container.querySelectorAll(".usage-chart-series")).toHaveLength(4);
    expect(container.querySelectorAll(".usage-chart-area")).toHaveLength(4);
    for (const path of container.querySelectorAll(".usage-chart-series, .usage-chart-cost-line")) expect(path.getAttribute("d")).toContain(" C ");
    expect(container.querySelector(".usage-chart-bar, .usage-chart-controls, [data-cost-unknown]")).toBeNull();
    expect(screen.queryByRole("radiogroup")).not.toBeInTheDocument();
  });

  it("keeps exact input, output, cache, and costs together in the original tooltip", async () => {
    const user = userEvent.setup();
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), aggregate]]} range="24h" nowMs={nowMs} />);
    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    for (const value of ["80,000", "3,000", "64,000", "1,200", "$0.42"]) expect(within(screen.getByRole("status")).getByText(value)).toBeInTheDocument();
  });

  it("retains unknown and partial token coverage across range updates", () => {
    const value = { ...aggregate, input_tokens: 10, output_tokens: 0,
      input_reported_requests: 1, output_reported_requests: 0, total_reported_requests: 0 };
    const { container, rerender } = render(<UsageTrendChart groups={[[String(bucketMs), value]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelector(`[data-bucket-key="${bucketMs}"]`)).toHaveAccessibleName(/输入总量 10（部分上报） · 输出 未上报/);
    const day = new Date(2026, 6, 23).getTime();
    rerender(<UsageTrendChart groups={[[String(day), { ...value, output_reported_requests: 3 }]]} range="7d" nowMs={nowMs} />);
    expect(container.querySelector(`[data-bucket-key="${day}"]`)).toHaveAccessibleName(/输出 0/);
  });

  it("distinguishes unreported cache writes from reported zero without yellow crosses", () => {
    const value = { ...aggregate, cache_write_tokens: 0, cache_write_reported_requests: 0 };
    const { container, rerender } = render(<UsageTrendChart groups={[[String(bucketMs), value]]} range="all" nowMs={nowMs} />);
    expect(container.querySelector("[data-usage-bucket]")).toHaveAccessibleName(/缓存写入 未上报/);
    expect(container.querySelector(".usage-chart-series.cache-write")).toHaveAttribute("d", "");
    rerender(<UsageTrendChart groups={[[String(bucketMs), { ...value, cache_write_reported_requests: 3 }]]} range="all" nowMs={nowMs} />);
    expect(container.querySelector("[data-usage-bucket]")).toHaveAccessibleName(/缓存写入 0/);
    expect(container.querySelector(".usage-chart-series.cache-write")?.getAttribute("d")).toMatch(/^M /);
    expect(container.querySelector(".usage-chart-cost-unknown, [data-cost-unknown]")).toBeNull();
  });

  it("breaks unknown cost segments and explains partial known cost in the tooltip", async () => {
    const user = userEvent.setup();
    const partial = { ...aggregate, priced_requests: 1, unpriced_requests: 2, missing_price_requests: 1, missing_usage_requests: 1 };
    const missing = { ...aggregate, cost_micros: null, priced_requests: 0, unpriced_requests: 3 };
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), partial], [String(bucketMs + 3600000), missing]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelector(".usage-chart-cost-line")?.getAttribute("d")?.match(/M /g)).toHaveLength(2);
    expect(container.querySelector(`[data-bucket-key="${bucketMs + 3600000}"]`)).toHaveAccessibleName(/成本 未知/);
    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    expect(screen.getByRole("status")).toHaveTextContent("$0.42（部分）");
    expect(screen.getByRole("status")).toHaveTextContent("1 次缺少价格 · 1 次用量不完整");
  });

  it("keeps a single priced point visible with micro-dollar precision and keyboard inspection", async () => {
    const user = userEvent.setup();
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), { ...aggregate, cost_micros: 1 }]]} range="all" nowMs={nowMs} />);
    expect(container.querySelectorAll("[data-usage-point]")).toHaveLength(4);
    expect(container.querySelector("[data-cost-point]")).toHaveAttribute("r", "2.5");
    await user.tab();
    expect(screen.getByRole("status")).toHaveTextContent("$0.000001");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("does not bridge missing token lines or area segments", async () => {
    const user = userEvent.setup();
    const missing = { ...aggregate, input_reported_requests: 0, input_tokens: 0 };
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), missing]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelector(".usage-chart-series.input")?.getAttribute("d")?.match(/M /g)).toHaveLength(2);
    expect(container.querySelector(".usage-chart-area.input")?.getAttribute("d")?.match(/ Z/g)).toHaveLength(2);
    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    expect(container.querySelector(".usage-chart-crosshair circle.input")).toBeNull();
    await user.hover(container.querySelector("[data-usage-bucket]") as Element);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("retains historical input labels and localizes unknown cost", () => {
    window.localStorage.setItem("token-station-language", "ja");
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), { ...aggregate, legacy_input_requests: 1, cost_micros: null }]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelector("[role='button']")).toHaveAccessibleName(/報告された入力/);
    expect(container.querySelector("[role='button']")).toHaveAccessibleName(/コスト 不明/);
  });
});
