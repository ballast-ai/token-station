import { describe, expect, it } from "vitest";
import tauri from "../src-tauri/tauri.conf.json";
import { browserAdminEndpoint } from "./api";

describe("desktop webview security boundary", () => {
  it("ships a restrictive CSP and never sources a virtual key from localStorage", () => {
    const csp = tauri.app?.security?.csp;
    expect(csp).toContain("default-src 'self'");
    expect(csp).toContain("script-src 'self'");
    expect(csp).not.toContain("unsafe-eval");

    const reads: string[] = [];
    const endpoint = browserAdminEndpoint({
      getItem(key) {
        reads.push(key);
        return key === "ts_listen" ? "127.0.0.1:9999" : "persisted-secret";
      },
    });
    expect(endpoint).toEqual({ base: "http://127.0.0.1:9999", key: null });
    expect(reads).toEqual(["ts_listen"]);
  });
});
