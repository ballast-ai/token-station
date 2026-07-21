// @ts-expect-error Vitest runs this regression test in Node; the browser app intentionally omits Node types.
import { readFileSync } from "node:fs";
// @ts-expect-error Vitest runs this regression test in Node; the browser app intentionally omits Node types.
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// @ts-expect-error process is a Node.js global in the Vitest runtime.
const appCss = readFileSync(resolve(process.cwd(), "src/App.css"), "utf8");

describe("retained page theme styles", () => {
  it("uses theme tokens for router JSON code blocks", () => {
    const codeBlockRule = appCss.match(/pre\.block\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(codeBlockRule).toMatch(/color:\s*var\(--ink\)/);
    expect(codeBlockRule).toMatch(/background:\s*var\(--surface-2\)/);
    expect(codeBlockRule).toMatch(/border:\s*1px solid var\(--line\)/);
  });
});
