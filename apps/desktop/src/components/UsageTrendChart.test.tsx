import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AggView } from "../api";
import UsageTrendChart from "./UsageTrendChart";

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
};

describe("UsageTrendChart", () => {
  it("renders cc-Switch-style token and cost series on a continuous 24-hour axis", async () => {
    const user = userEvent.setup();
    const nowMs = new Date(2026, 6, 23, 13, 35).getTime();
    const bucketMs = new Date(2026, 6, 23, 11).getTime();
    const { container } = render(
      <UsageTrendChart
        groups={[[String(bucketMs), aggregate]]}
        range="24h"
        nowMs={nowMs}
      />,
    );

    expect(screen.getByRole("img", { name: /24 个小时槽，活跃 1 个/ })).toBeInTheDocument();
    expect(screen.getByRole("img", { name: /左轴为 Token，右轴为成本/ })).toBeInTheDocument();
    expect(screen.getByText((_, element) => element?.textContent === "活跃 1 / 24 小时")).toBeInTheDocument();
    expect(container.querySelectorAll("[data-usage-bucket]")).toHaveLength(24);
    expect(container.querySelectorAll(".usage-chart-series")).toHaveLength(4);
    expect(container.querySelector(".usage-chart-cost-line")).toBeInTheDocument();

    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    expect(screen.getByText("输入总量")).toBeInTheDocument();
    expect(screen.getByText("80,000")).toBeInTheDocument();
    expect(screen.getByText("3,000")).toBeInTheDocument();
    expect(screen.getByText("1,200")).toBeInTheDocument();
    expect(screen.getByText("64,000")).toBeInTheDocument();
    expect(screen.getAllByText("$0.420")).toHaveLength(2);
    expect(screen.getByText("3 次请求 · 0 个错误 · 缓存指标属于输入子集")).toBeInTheDocument();
  });

  it("shows zero cache writes without inventing missing-data semantics", async () => {
    const user = userEvent.setup();
    const nowMs = new Date(2026, 6, 23, 13, 35).getTime();
    const bucketMs = new Date(2026, 6, 23, 12).getTime();
    const { container } = render(
      <UsageTrendChart
        groups={[[String(bucketMs), { ...aggregate, cache_write_tokens: 0 }]]}
        range="24h"
        nowMs={nowMs}
      />,
    );

    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    expect(screen.getByText("缓存写入").nextElementSibling).toHaveTextContent("0");
    expect(container.querySelector('[aria-label*="缓存写入 0"]')).toBeInTheDocument();
  });

  it("labels legacy buckets as provider-reported input", async () => {
    const user = userEvent.setup();
    const nowMs = new Date(2026, 6, 23, 13, 35).getTime();
    const bucketMs = new Date(2026, 6, 23, 12).getTime();
    const { container } = render(
      <UsageTrendChart
        groups={[[String(bucketMs), { ...aggregate, legacy_input_requests: 1 }]]}
        range="24h"
        nowMs={nowMs}
      />,
    );

    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    expect(screen.getByText("上游输入")).toBeInTheDocument();
    expect(container.querySelector('[aria-label*="上游输入 80,000"]')).toBeInTheDocument();
  });

  it("marks an unpriced request bucket as unknown instead of zero cost", async () => {
    const user = userEvent.setup();
    const nowMs = new Date(2026, 6, 23, 13, 35).getTime();
    const bucketMs = new Date(2026, 6, 23, 12).getTime();
    const unpriced = {
      ...aggregate,
      cost_micros: null,
      priced_requests: 0,
      unpriced_requests: 3,
    };
    const { container } = render(
      <UsageTrendChart
        groups={[[String(bucketMs), unpriced]]}
        range="24h"
        nowMs={nowMs}
      />,
    );

    expect(screen.getByText("未定价")).toBeInTheDocument();
    await user.hover(container.querySelector(`[data-bucket-key="${bucketMs}"]`) as Element);
    expect(screen.getByText("未知")).toBeInTheDocument();
    expect(container.querySelector("[data-cost-unknown]")).toBeInTheDocument();
  });

  it("localizes unknown cost in the Japanese accessible bucket label", () => {
    window.localStorage.setItem("token-station-language", "ja");
    const nowMs = new Date(2026, 6, 23, 13, 35).getTime();
    const bucketMs = new Date(2026, 6, 23, 12).getTime();
    const { container } = render(
      <UsageTrendChart
        groups={[[String(bucketMs), { ...aggregate, cost_micros: null }]]}
        range="24h"
        nowMs={nowMs}
      />,
    );
    expect(container.querySelector('[aria-label*="コスト 不明"]')).toBeInTheDocument();
  });
});
