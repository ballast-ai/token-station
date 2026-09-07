import { describe, expect, it } from "vitest";
import { cacheMetric, costCoverage, formatUsd, tokenMetric, receiptTokenMetric, receiptHasUsableUsage } from "./usagePresentation";
import type { AggView, ReceiptView } from "./api";

describe("accounting presentation", () => {
  it("distinguishes absent, explicit zero, and partially reported token totals", () => {
    const value = { requests: 2, input_tokens: 10, output_tokens: 0,
      input_reported_requests: 1, output_reported_requests: 0, total_reported_requests: 0 } as AggView;
    expect(tokenMetric(value, "input_tokens")).toEqual({ value: 10, reported: 1, complete: false });
    expect(tokenMetric(value, "output_tokens")).toEqual({ value: null, reported: 0, complete: false });
    expect(tokenMetric(value, "total")).toEqual({ value: 10, reported: 0, complete: false });
    expect(tokenMetric({ ...value, output_reported_requests: 2 }, "output_tokens"))
      .toEqual({ value: 0, reported: 2, complete: true });
    expect(tokenMetric({ ...value, input_tokens: 0, input_reported_requests: 0 }, "total").value).toBeNull();
  });
  it("preserves legacy numeric tokens without claiming historical zero cache reports", () => {
    const value = { requests: 1, input_tokens: 0, output_tokens: 0, cache_read_tokens: 0 } as AggView;
    expect(tokenMetric(value, "total").value).toBe(0);
    expect(cacheMetric(value, "cache_read_tokens").value).toBeNull();
  });
  it("uses receipt field presence for partial totals and reported zeros", () => {
    const receipt = { usage: { input_tokens: 10, output_tokens: 99 },
      usage_observation: { input_tokens: 10, output_tokens: null } } as ReceiptView;
    expect(receiptTokenMetric(receipt, "output_tokens").value).toBeNull();
    expect(receiptTokenMetric(receipt, "total")).toEqual({ value: 10, reported: 0, complete: false });
    expect(receiptTokenMetric({ ...receipt, usage_observation: { input_tokens: 10, output_tokens: 0 } }, "total"))
      .toEqual({ value: 10, reported: 1, complete: true });
    expect(receiptTokenMetric({ ...receipt, usage_observation: null }, "total").value).toBe(109);
  });
  it("never labels incomplete snapshots complete even when every field was reported", () => {
    const value = { requests: 1, input_tokens: 10, output_tokens: 2,
      input_reported_requests: 1, output_reported_requests: 1, total_reported_requests: 1,
      cache_write_tokens: 0, cache_write_reported_requests: 1, incomplete_usage_requests: 1 } as AggView;
    expect(tokenMetric(value, "total")).toEqual({ value: 12, reported: 1, complete: false });
    expect(tokenMetric(value, "input_tokens").complete).toBe(false);
    expect(cacheMetric(value, "cache_write_tokens").complete).toBe(false);
    const receipt = { usage_observation: { input_tokens: 10, output_tokens: 2, incomplete: true } } as ReceiptView;
    expect(receiptTokenMetric(receipt, "total")).toEqual({ value: 12, reported: 1, complete: false });
  });
  it("classifies incomplete and inconsistent receipt usage separately from missing prices", () => {
    const receipt = { usage: { input_tokens: 10, output_tokens: 2, cache_read_tokens: 0, cache_write_tokens: 0 },
      usage_observation: { input_tokens: 10, output_tokens: 2 } } as ReceiptView;
    expect(receiptHasUsableUsage(receipt)).toBe(true);
    expect(receiptHasUsableUsage({ ...receipt, usage_observation: { ...receipt.usage_observation, incomplete: true } })).toBe(false);
    expect(receiptHasUsableUsage({ ...receipt, usage_observation: { ...receipt.usage_observation, cache_read_tokens: 11 } })).toBe(false);
    expect(receiptHasUsableUsage({ ...receipt, usage_observation: { ...receipt.usage_observation, cache_write_1h_tokens: 1 } })).toBe(false);
  });
  it("does not label partial known cost as complete", () => {
    const value = { requests: 210, priced_requests: 8, unpriced_requests: 202,
      missing_price_requests: 133, missing_usage_requests: 69, cost_micros: 151120 } as AggView;
    expect(costCoverage(value)).toEqual({ priced: 8, missingPrice: 133, missingUsage: 69, complete: false });
  });
  it("distinguishes historical zero, observed zero, and partial positive cache totals", () => {
    const value = { requests: 3, cache_write_tokens: 0 } as AggView;
    expect(cacheMetric(value, "cache_write_tokens").value).toBeNull();
    expect(cacheMetric({ ...value, cache_write_reported_requests: 3 }, "cache_write_tokens")).toEqual({ value: 0, reported: 3, complete: true });
    expect(cacheMetric({ ...value, cache_write_tokens: 8, cache_write_reported_requests: 1 }, "cache_write_tokens")).toEqual({ value: 8, reported: 1, complete: false });
  });
  it("keeps microdollar costs visible", () => {
    expect(formatUsd(1)).toBe("$0.000001");
    expect(formatUsd(null)).toBe("—");
    expect(formatUsd(0)).toBe("$0.00");
  });
});
