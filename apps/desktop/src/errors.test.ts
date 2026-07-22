import { describe, expect, it } from "vitest";
import { humanizeErrorCode } from "./errors";

describe("humanizeErrorCode", () => {
  it("returns null for missing codes", () => {
    expect(humanizeErrorCode(null)).toBeNull();
    expect(humanizeErrorCode(undefined)).toBeNull();
    expect(humanizeErrorCode("")).toBeNull();
  });

  it("maps a known code to layer + message + suggestion", () => {
    const human = humanizeErrorCode("auth");
    expect(human).not.toBeNull();
    expect(human?.layer).toContain("鉴权");
    expect(human?.suggestion).toContain("Key");
  });

  it("falls back to a diagnosable shape for unknown codes", () => {
    const human = humanizeErrorCode("teapot");
    expect(human?.layer).toBe("未知");
    expect(human?.message).toContain("teapot");
    expect(human?.suggestion).toContain("请求 ID");
  });
});
