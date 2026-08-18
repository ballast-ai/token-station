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

  it("uses stable WebView-safe colors for selected routing states", () => {
    const routingModeRule = appCss.match(
      /\.routing-mode-tabs \[data-slot="tabs-trigger"\]\[data-state="active"\],[^{]*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const selectedProviderRule = appCss.match(/\.direct-provider-row\.selected\s*\{([^}]*)\}/s)?.[1] ?? "";
    const hoveredProviderRule = appCss.match(
      /\.direct-provider-row:hover:not\(\.unavailable\)\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const appliedTargetRule = appCss.match(/\.direct-applied-target\s*\{([^}]*)\}/s)?.[1] ?? "";
    const toastRule = appCss.match(/\.error-toast\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(routingModeRule).toMatch(/border-color:\s*var\(--signal-selection-border\)/);
    expect(routingModeRule).toMatch(/background:\s*var\(--signal-soft\)/);
    expect(routingModeRule).toMatch(/box-shadow:\s*var\(--signal-selection-shadow\)/);
    expect(selectedProviderRule).toMatch(/border-color:\s*var\(--signal-selection-border\)/);
    expect(selectedProviderRule).toMatch(/background:\s*var\(--signal-soft\)/);
    expect(hoveredProviderRule).toMatch(/border-color:\s*var\(--signal-hover-border\)/);
    expect(hoveredProviderRule).toMatch(/background:\s*var\(--signal-hover-soft\)/);
    expect(appliedTargetRule).toMatch(/border:\s*1px solid var\(--success-border\)/);
    expect(appliedTargetRule).toMatch(/background:\s*var\(--success-soft\)/);
    expect(toastRule).toMatch(/border:\s*1px solid var\(--toast-border\)/);
    expect(toastRule).toMatch(/background:\s*var\(--toast-soft\)/);

    for (const rule of [routingModeRule, selectedProviderRule, hoveredProviderRule, appliedTargetRule, toastRule]) {
      expect(rule).not.toContain("color-mix(");
    }

    expect(appCss).toMatch(/--signal-selection-border:\s*#[0-9a-f]{6}/i);
    expect(appCss).toMatch(/--signal-selection-shadow:[^;]*rgb\(/i);
    expect(appCss).toMatch(/--success-border:\s*#[0-9a-f]{6}/i);
    expect(appCss).toMatch(/--success-soft:\s*#[0-9a-f]{6}/i);
    expect(appCss).toMatch(/--danger-border:\s*#[0-9a-f]{6}/i);
    expect(appCss).toMatch(/--danger-soft:\s*#[0-9a-f]{6}/i);
    expect(appCss).toMatch(/\.error-toast\.is-info\s*\{[^}]*--toast-soft:\s*var\(--signal-soft\)/s);
    expect(appCss).toMatch(/\.error-toast\.is-success\s*\{[^}]*--toast-soft:\s*var\(--success-soft\)/s);
    expect(appCss).toMatch(/\.error-toast\.is-error\s*\{[^}]*--toast-soft:\s*var\(--danger-soft\)/s);
  });

  it("keeps the global route row neutral until it is selected", () => {
    const permanentGlobalRouteRule = appCss.match(
      /\.agent-master-home\[data-slot="button"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const selectedAgentRule = appCss.match(
      /\.agent-master-item\[data-slot="button"\]\[aria-current="page"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(permanentGlobalRouteRule).not.toMatch(/background:/);
    expect(permanentGlobalRouteRule).not.toMatch(/border:/);
    expect(selectedAgentRule).toMatch(/background:\s*var\(--signal-soft\)/);
  });

  it("keeps the proxy control width stable across runtime states", () => {
    const baseRuntimeRules = Array.from(
      appCss.matchAll(/\.station-runtime-pill\[data-slot="button"\]\s*\{([^}]*)\}/g),
      (match) => match[1],
    ).join("\n");
    const stateRuntimeRules = Array.from(
      appCss.matchAll(/\.station-runtime-pill\[data-slot="button"\](?:\.healthy|:hover)[^{]*\{([^}]*)\}/g),
      (match) => match[1],
    ).join("\n");

    expect(baseRuntimeRules).toMatch(/width:\s*112px/);
    expect(stateRuntimeRules).not.toMatch(/(?:^|;)\s*width:/);
  });

  it("styles the pre-connection route card as a neutral preview", () => {
    const previewRule = appCss.match(/\.agent-default-route-state\s*\{([^}]*)\}/s)?.[1] ?? "";
    const previewIconRule = appCss.match(/\.agent-default-route-state\s*>\s*span\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(previewRule).toMatch(/var\(--signal\)/);
    expect(previewIconRule).toMatch(/var\(--signal-soft\)/);
    expect(`${previewRule}\n${previewIconRule}`).not.toMatch(/var\(--success\)/);
  });

  it("removes Agent discovery motion when reduced motion is requested", () => {
    const revealRule = appCss.match(
      /\.agent-master-item-revealing\[data-slot="button"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const reducedMotion = Array.from(
      appCss.matchAll(/@media \(prefers-reduced-motion: reduce\)\s*\{([\s\S]*?)\n\}/g),
      (match) => match[1],
    ).join("\n");

    expect(revealRule).toMatch(/animation:\s*agent-discovery-reveal/);
    expect(reducedMotion).toMatch(
      /\.agent-master-item-revealing\[data-slot="button"\][^}]*animation:\s*none/,
    );
  });

  it("uses active theme surfaces for the usage plaintext inspector", () => {
    const plaintextRule = appCss.match(/\.request-plaintext\s*\{([^}]*)\}/s)?.[1] ?? "";
    const scrollRule = appCss.match(/\.request-plaintext-scroll\s*\{([^}]*)\}/s)?.[1] ?? "";
    const semanticBlockRule = appCss.match(/\.request-semantic-block\s*\{([^}]*)\}/s)?.[1] ?? "";
    const semanticBodyRule = appCss.match(/\.request-semantic-block > pre\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(plaintextRule).toMatch(/background:\s*var\(--surface-2\)/);
    expect(scrollRule).toMatch(/border:\s*1px solid var\(--line-strong\)/);
    expect(scrollRule).toMatch(/color:\s*var\(--ink\)/);
    expect(scrollRule).toMatch(/background:\s*var\(--canvas\)/);
    expect(semanticBlockRule).toMatch(/border:\s*1px solid var\(--line-strong\)/);
    expect(semanticBlockRule).toMatch(/background:\s*var\(--surface\)/);
    expect(semanticBodyRule).toMatch(/color:\s*var\(--ink\)/);

    for (const rule of [plaintextRule, scrollRule, semanticBlockRule, semanticBodyRule]) {
      expect(rule).not.toMatch(/#08101d|#0b1220|#fff\b/i);
    }
  });
});
