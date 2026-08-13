import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const appCss = readFileSync(resolve(process.cwd(), "src/App.css"), "utf8");

describe("retained page theme styles", () => {
  it("uses theme tokens for router JSON code blocks", () => {
    const codeBlockRule = appCss.match(/pre\.block\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(codeBlockRule).toMatch(/color:\s*var\(--ink\)/);
    expect(codeBlockRule).toMatch(/background:\s*var\(--surface-2\)/);
    expect(codeBlockRule).toMatch(/border:\s*1px solid var\(--line\)/);
  });

  it("shows a distinct Direct row dragging state and disables its motion when requested", () => {
    const draggingRule = appCss.match(/\.direct-provider-row\.dragging\s*\{([^}]*)\}/s)?.[1] ?? "";
    const draggingWrapperRule = appCss.match(/\.direct-provider-sortable\.dragging\s*\{([^}]*)\}/s)?.[1] ?? "";
    const reducedMotion = Array.from(
      appCss.matchAll(/@media \(prefers-reduced-motion: reduce\)\s*\{([\s\S]*?)\n\}/g),
      (match) => match[1],
    ).join("\n");

    expect(draggingRule).toMatch(/box-shadow:/);
    expect(draggingRule).toMatch(/opacity:/);
    expect(draggingWrapperRule).toMatch(/z-index:/);
    expect(reducedMotion).toMatch(/\.direct-provider-sortable[^}]*transition:\s*none/);
    expect(reducedMotion).toMatch(/\.direct-provider-row[^}]*transition:\s*none/);
  });
});
