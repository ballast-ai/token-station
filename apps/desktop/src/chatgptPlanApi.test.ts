import { beforeEach, describe, expect, it, vi } from "vitest";
import { openUrl } from "@tauri-apps/plugin-opener";
import { openChatGPTAuthorization, openChatGPTUsage } from "./chatgptPlanApi";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

describe("ChatGPT browser destinations", () => {
  beforeEach(() => vi.mocked(openUrl).mockReset());
  it.each([
    "http://auth.openai.com/api/accounts/authorize",
    "https://auth.openai.com.evil.test/api/accounts/authorize",
    "https://auth.openai.com:8443/api/accounts/authorize",
    "https://attacker@auth.openai.com/api/accounts/authorize",
    "https://auth.openai.com/api/accounts/other",
    "https://auth.openai.com/api/accounts/authorize#secret",
    "javascript:alert(1)",
  ])("rejects a noncanonical issuer URL: %s", async url => {
    await expect(openChatGPTAuthorization(url)).rejects.toThrow();
    expect(openUrl).not.toHaveBeenCalled();
  });
  it("opens the fixed usage page", async () => {
    await openChatGPTUsage();
    expect(openUrl).toHaveBeenCalledWith("https://chatgpt.com/settings/usage");
  });
});
