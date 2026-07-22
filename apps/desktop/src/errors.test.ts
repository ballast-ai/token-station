import { describe, expect, it } from "vitest";
import { humanizeErrorCode } from "./errors";

describe("humanizeErrorCode", () => {
  it("returns null when no failure code exists", () => {
    expect(humanizeErrorCode(null)).toBeNull();
    expect(humanizeErrorCode(undefined)).toBeNull();
    expect(humanizeErrorCode("")).toBeNull();
  });

  it.each([
    ["auth", "鉴权", "API Key"],
    ["payment_required", "余额", "额度"],
    ["context_length", "上下文", "缩短"],
    ["provider_protocol_error", "协议", "适配器"],
    ["internal", "本地代理", "请求 ID"],
  ])("maps %s to a layer and an actionable suggestion", (code, layer, suggestion) => {
    const value = humanizeErrorCode(code);
    expect(value?.layer).toContain(layer);
    expect(value?.suggestion).toContain(suggestion);
  });

  it("keeps an unknown structured code diagnosable", () => {
    const value = humanizeErrorCode("teapot");
    expect(value?.layer).toBe("未知");
    expect(value?.message).toContain("teapot");
    expect(value?.suggestion).toContain("请求 ID");
  });
});
