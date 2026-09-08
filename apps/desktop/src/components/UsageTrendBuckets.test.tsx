import { fireEvent, render } from "@testing-library/react";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { AggView } from "../api";
import UsageTrendChart, { normalizeBuckets, type UsageTrendRange } from "./UsageTrendChart";

const HOUR = 3_600_000;
const DAY = 24 * HOUR;
const aggregate: AggView = {
  requests: 3,
  errors: 1,
  p50_latency_ms: 10,
  p95_latency_ms: 20,
  input_tokens: 11,
  legacy_input_requests: 0,
  output_tokens: 2,
  cache_read_tokens: 4,
  cache_write_tokens: 0,
  reasoning_tokens: 0,
  cost_micros: 7,
  priced_requests: 1,
  unpriced_requests: 2,
  missing_price_requests: 1,
  missing_usage_requests: 1,
  actual_cost_requests: 1,
  estimated_cost_requests: 0,
  cache_read_reported_requests: 1,
  cache_write_reported_requests: 0,
};

function chart(timestamps: number[], range: UsageTrendRange, nowMs: number) {
  return render(<UsageTrendChart
    groups={timestamps.map((timestamp) => [String(timestamp), aggregate])}
    range={range}
    nowMs={nowMs}
  />);
}

function keys(container: HTMLElement) {
  return [...container.querySelectorAll("[data-usage-bucket]")]
    .map((element) => Number(element.getAttribute("data-bucket-key")));
}

describe("usage trend time boundaries", () => {
  // Use a real DST zone so repeated hours and elapsed-day cutoffs are exercised.
  beforeAll(() => vi.stubEnv("TZ", "America/New_York"));
  afterAll(() => vi.unstubAllEnvs());

  it.each([
    ["24h", 24 * HOUR, 25],
    ["7d", 7 * DAY, 8],
    ["30d", 30 * DAY, 31],
  ] as const)("keeps the cutoff and current partial slots for %s", (range, duration, count) => {
    const now = new Date(2026, 6, 23, 13, 35).getTime();
    const cutoff = new Date(now - duration);
    const current = new Date(now);
    if (range === "24h") {
      cutoff.setMinutes(0, 0, 0);
      current.setMinutes(0, 0, 0);
    } else {
      cutoff.setHours(0, 0, 0, 0);
      current.setHours(0, 0, 0, 0);
    }
    const { buckets } = normalizeBuckets([
      [String(cutoff.getTime()), aggregate], [String(current.getTime()), aggregate],
    ], range, now);
    expect(buckets).toHaveLength(count);
    expect(buckets[0].timestamp).toBe(cutoff.getTime());
    expect(buckets[count - 1].timestamp).toBe(current.getTime());
    expect(buckets.filter((bucket) => bucket.aggregate === aggregate)).toHaveLength(2);
  });

  it("includes an exact cutoff hour and excludes slots outside the rolling window", () => {
    const now = new Date(2026, 6, 23, 13).getTime();
    const cutoff = now - DAY;
    const { buckets } = normalizeBuckets(
      [cutoff - HOUR, cutoff, now, now + HOUR].map((timestamp) => [String(timestamp), aggregate]), "24h", now,
    );
    expect(buckets.map((bucket) => bucket.timestamp)).toEqual(Array.from({ length: 25 }, (_, index) => cutoff + index * HOUR));
    expect(buckets.filter((bucket) => bucket.aggregate === aggregate)).toHaveLength(2);
  });

  it.each([
    ["spring forward", "2026-03-08T13:35:00-04:00"],
    ["fall back", "2026-11-01T13:35:00-05:00"],
    ["the repeated current hour", "2026-11-01T01:35:00-05:00"],
  ])("steps through 24 elapsed hours during %s", (_name, iso) => {
    const now = Date.parse(iso);
    const end = now - 35 * 60_000;
    const expected = Array.from({ length: 25 }, (_, index) => end - DAY + index * HOUR);
    const { buckets } = normalizeBuckets(expected.map((timestamp) => [String(timestamp), aggregate]), "24h", now);
    expect(buckets.map((bucket) => bucket.timestamp)).toEqual(expected);
    expect(buckets.filter((bucket) => bucket.aggregate === aggregate)).toHaveLength(25);
  });

  it.each([
    ["7d", "2026-03-09T00:30:00-04:00", "2026-03-01T00:00:00-05:00", 9],
    ["7d", "2026-11-02T23:30:00-05:00", "2026-10-27T00:00:00-04:00", 7],
    ["30d", "2026-03-09T00:30:00-04:00", "2026-02-06T00:00:00-05:00", 32],
  ] as const)("uses the exact elapsed cutoff across DST for %s at %s", (range, iso, firstIso, count) => {
    const now = Date.parse(iso);
    const first = Date.parse(firstIso);
    const { buckets } = normalizeBuckets([[String(first), aggregate]], range, now);
    expect(buckets[0].timestamp).toBe(first);
    expect(buckets[0].aggregate).toBe(aggregate);
    expect(buckets).toHaveLength(count);
    for (const bucket of buckets) expect(new Date(bucket.timestamp).getHours()).toBe(0);
  });

  it("keeps every aggregate and coverage field beyond the 120-day display threshold", () => {
    const first = new Date(2026, 0, 1).getTime();
    const timestamps = Array.from({ length: 121 }, (_, index) => new Date(2026, 0, index + 1).getTime());
    const { buckets } = normalizeBuckets(timestamps.map((timestamp) => [String(timestamp), aggregate]), "all", first);
    expect(buckets.map((bucket) => bucket.timestamp)).toEqual(timestamps);
    for (const bucket of buckets) expect(bucket.aggregate).toBe(aggregate);
    const shorter = normalizeBuckets([[String(first), aggregate], [String(timestamps[119]), aggregate]], "all", first);
    expect(shorter.buckets).toHaveLength(120);
  });

  it("positions sparse long history by elapsed time and preserves every receipt subtotal", () => {
    const timestamps = [
      new Date(2025, 0, 1).getTime(),
      new Date(2025, 0, 2).getTime(),
      new Date(2026, 0, 1).getTime(),
    ];
    const { container } = chart(timestamps, "all", timestamps[2]);
    const normalized = normalizeBuckets(timestamps.map((timestamp) => [String(timestamp), aggregate]), "all", timestamps[2]);
    expect(normalized.buckets.filter((bucket) => bucket.aggregate.requests > 0).map((bucket) => bucket.timestamp)).toEqual(timestamps);
    expect(normalized.buckets.reduce((sum, bucket) => sum + bucket.aggregate.input_tokens, 0)).toBe(33);
    expect(keys(container)).toHaveLength(5);
    expect(container.querySelectorAll(".usage-chart-x-axis")).toHaveLength(2);
    const centers = timestamps.map((timestamp) => {
      fireEvent.focus(container.querySelector(`[data-bucket-key="${timestamp}"]`)!);
      expect(container.querySelector(".usage-chart-tooltip")).toHaveTextContent("1 次已计价 · 1 次缺少价格 · 1 次用量不完整");
      expect(container.querySelector(".usage-chart-tooltip")).toHaveTextContent("3 次请求 · 1 个错误");
      return Number(container.querySelector(".usage-chart-crosshair line")!.getAttribute("x1"));
    });
    expect((centers[2] - centers[1]) / (centers[1] - centers[0]))
      .toBeCloseTo((timestamps[2] - timestamps[1]) / (timestamps[1] - timestamps[0]));
    // Empty boundary days return the curve to zero instead of a year-long plateau.
    expect(normalized.buckets[2].aggregate.requests).toBe(0);
    expect(normalized.buckets[3].aggregate.requests).toBe(0);
  });

  it("fills short-history gaps and keeps a single bucket centered", () => {
    const first = new Date(2026, 6, 1).getTime();
    const last = new Date(2026, 6, 3).getTime();
    const { container, rerender } = chart([first, last], "all", last);
    expect(keys(container)).toEqual([first, first + DAY, last]);
    rerender(<UsageTrendChart groups={[[String(first), aggregate]]} range="all" nowMs={first} />);
    expect(keys(container)).toEqual([first]);
    const hit = container.querySelector(".usage-chart-hit-area")!;
    const grid = container.querySelector(".usage-chart-grid")!;
    expect(Number(hit.getAttribute("x")) + Number(hit.getAttribute("width")) / 2)
      .toBe((Number(grid.getAttribute("x1")) + Number(grid.getAttribute("x2"))) / 2);
  });
});
