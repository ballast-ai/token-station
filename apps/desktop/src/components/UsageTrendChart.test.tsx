import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
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
  it("shows successful usage and legacy cache absence separately from failed traffic on hover", async () => {
    const user = userEvent.setup();
    const value = { ...aggregate, requests: 184, errors: 96, usage_expected_requests: 85,
      failed_without_usage_requests: 99, unpriced_failed_without_usage_requests: 99,
      input_tokens: 6942955, output_tokens: 42628, cache_read_tokens: 5321280, cache_write_tokens: 0,
      input_reported_requests: 85, output_reported_requests: 85, total_reported_requests: 85,
      cache_read_reported_requests: 85, cache_write_reported_requests: 0, cache_write_unrecorded_requests: 85,
      priced_requests: 0, unpriced_requests: 184, missing_price_requests: 85, missing_usage_requests: 99, cost_micros: null };
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), value]]} range="24h" nowMs={nowMs} />);
    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    const tooltip = screen.getByRole("status");
    expect(tooltip).toHaveTextContent("6,942,955");
    expect(tooltip).not.toHaveTextContent("部分上报");
    expect(tooltip).toHaveTextContent("历史未记录");
    expect(tooltip).toHaveTextContent("85 次缺少价格 · 0 次用量不完整 · 99 次失败/取消未返回用量");
    expect(tooltip).toHaveTextContent("184 次请求 · 96 个错误");
    expect(tooltip).toHaveTextContent("未知");
  });
  it("updates the guide and time badge while moving from activity to an empty hour without clicking", () => {
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), aggregate]]} range="24h" nowMs={nowMs} />);
    const svg = screen.getByRole("img");
    vi.spyOn(svg, "getBoundingClientRect").mockReturnValue({ left: 100, top: 200, width: 460, height: 300 } as DOMRect);
    const x = (index: number) => 58 + index * (920 - 58 - 64) / 24;
    fireEvent.mouseMove(svg, { clientX: 100 + x(22) / 2, clientY: 300 });
    expect(screen.getByRole("status")).toHaveTextContent("11:00");
    expect(container.querySelector(".usage-chart-time-badge")).toHaveTextContent("11:00");
    expect(container.querySelector(".usage-chart-crosshair line")).toHaveAttribute("x1", String(x(22)));
    fireEvent.mouseMove(svg, { clientX: 100 + x(23) / 2, clientY: 360 });
    expect(screen.getByRole("status")).toHaveTextContent("12:00");
    expect(screen.getByRole("status")).toHaveTextContent("0 次请求");
    expect(screen.getByRole("status")).toHaveTextContent("$0.00");
    expect(container.querySelector(".usage-chart-time-badge")).toHaveTextContent("12:00");
    fireEvent.mouseLeave(svg);
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("uses only the custom tooltip and avoids generic form classes in its rows", async () => {
    const user = userEvent.setup();
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), aggregate]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelector("[data-usage-bucket] title")).toBeNull();
    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    expect(screen.getByRole("status").querySelector(".input")).toBeNull();
  });

  it("keeps edge tooltips inside the plot and clears inspection outside it or on Escape", () => {
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), aggregate]]} range="24h" nowMs={nowMs} />);
    const svg = screen.getByRole("img");
    vi.spyOn(svg, "getBoundingClientRect").mockReturnValue({ left: 50, top: 100, width: 600, height: 300 } as DOMRect);
    for (const x of [58, 550, 856]) {
      fireEvent.mouseMove(svg, { clientX: 50 + x / 920 * 600, clientY: 350 });
      const tooltip = screen.getByRole("status");
      expect(parseFloat(tooltip.style.left)).toBeGreaterThanOrEqual(12);
      expect(parseFloat(tooltip.style.left) + 260).toBeLessThanOrEqual(588);
      expect(parseFloat(tooltip.style.top)).toBeGreaterThanOrEqual(12);
      expect(container.querySelector(".usage-chart-time-badge")).toBeInTheDocument();
    }
    fireEvent.keyDown(svg, { key: "Escape" });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    fireEvent.mouseMove(svg, { clientX: 350, clientY: 200 });
    expect(screen.getByRole("status")).toBeInTheDocument();
    fireEvent.mouseMove(svg, { clientX: 50, clientY: 200 });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("supports keyboard time inspection through empty neighboring hours", async () => {
    const user = userEvent.setup();
    render(<UsageTrendChart groups={[[String(bucketMs), aggregate]]} range="24h" nowMs={nowMs} />);
    await user.tab();
    expect(screen.getByRole("status")).toHaveTextContent("11:00");
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("status")).toHaveTextContent("12:00");
    expect(screen.getByRole("status")).toHaveTextContent("0 次请求");
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("status")).toHaveTextContent("11:00");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
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
    expect(container.querySelector(".usage-chart-cost-annotation")).toHaveTextContent("成本 $0.000001");
    await user.tab();
    expect(screen.getByRole("status")).toHaveTextContent("$0.000001");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });

  it("explains an isolated partial cost at the current-hour edge without drawing through unknown costs", () => {
    const missing = { ...aggregate, cost_micros: null, priced_requests: 0, unpriced_requests: 3 };
    const partial = { ...aggregate, cost_micros: 8392, priced_requests: 1, unpriced_requests: 2 };
    const current = bucketMs + 7200000;
    const { container } = render(<UsageTrendChart groups={[[String(current - 3600000), missing], [String(current), partial]]} range="24h" nowMs={nowMs} />);
    const label = container.querySelector(".usage-chart-cost-annotation");
    expect(label).toHaveTextContent("成本 $0.008392（部分）");
    expect(label).toHaveAttribute("text-anchor", "end");
    expect(Number(label?.getAttribute("x"))).toBeLessThan(856);
    expect(container.querySelector("[data-cost-point]")).toBeInTheDocument();
    expect(container.querySelector(".usage-chart-cost-line")?.getAttribute("d")).toMatch(/M 856\.0 [\d.]+$/);
    fireEvent.mouseEnter(container.querySelector(`[data-bucket-key="${current}"]`) as Element);
    expect(screen.getByRole("status")).toHaveTextContent("相邻时段成本数据不足，无法形成连续曲线");
    expect(screen.getByRole("status")).toHaveTextContent("13:00");
    expect(container.querySelector("svg title")).toBeNull();
  });

  it("shows only one isolated cost annotation and follows keyboard inspection", () => {
    const missing = { ...aggregate, cost_micros: null, priced_requests: 0, unpriced_requests: 3 };
    const current = bucketMs + 7200000;
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs - 3600000), missing], [String(bucketMs), aggregate], [String(bucketMs + 3600000), missing], [String(current), { ...aggregate, cost_micros: 1 }]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelectorAll(".usage-chart-cost-annotation")).toHaveLength(1);
    expect(container.querySelector(".usage-chart-cost-annotation")).toHaveTextContent("$0.000001");
    fireEvent.focus(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    expect(container.querySelectorAll(".usage-chart-cost-annotation")).toHaveLength(1);
    expect(container.querySelector(".usage-chart-cost-annotation")).toHaveTextContent("$0.42");
    fireEvent.keyDown(screen.getByRole("img"), { key: "ArrowRight" });
    expect(container.querySelector(".usage-chart-cost-annotation")).toBeNull();
    expect(screen.getByRole("status")).not.toHaveTextContent("无法形成连续曲线");
  });

  it("does not annotate continuous cost curves or a known zero as an isolated positive cost", () => {
    const { container, rerender } = render(<UsageTrendChart groups={[[String(bucketMs), aggregate]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelector(".usage-chart-cost-annotation")).toBeNull();
    rerender(<UsageTrendChart groups={[[String(bucketMs), { ...aggregate, cost_micros: 0 }]]} range="all" nowMs={nowMs} />);
    expect(container.querySelector(".usage-chart-cost-annotation")).toBeNull();
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
    expect(screen.getByRole("status")).toHaveTextContent("0 次请求");
  });

  it("retains historical input labels and localizes unknown cost", () => {
    window.localStorage.setItem("token-station-language", "ja");
    const { container } = render(<UsageTrendChart groups={[[String(bucketMs), { ...aggregate, legacy_input_requests: 1, cost_micros: null }]]} range="24h" nowMs={nowMs} />);
    expect(container.querySelector("[role='button']")).toHaveAccessibleName(/報告された入力/);
    expect(container.querySelector("[role='button']")).toHaveAccessibleName(/コスト 不明/);
  });
});
