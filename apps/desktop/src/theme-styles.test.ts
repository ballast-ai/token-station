import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const appCss = readFileSync(resolve(process.cwd(), "src/App.css"), "utf8");

describe("retained page theme styles", () => {
  it("contains fixed overview runtime cards inside their grid track", () => {
    const overviewGridRule = appCss.match(
      /\.station-content-overview > \.overview-page\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const runtimeCardRule = appCss.match(
      /\.station-content-overview \.overview-runtime-metrics > \[data-slot="card"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(overviewGridRule).toMatch(
      /grid-template-rows:\s*auto clamp\(128px, 18vh, 140px\) clamp\(300px, 45vh, 330px\)/,
    );
    expect(overviewGridRule).toMatch(/align-content:\s*start/);
    expect(runtimeCardRule).toMatch(/min-height:\s*0/);
    expect(runtimeCardRule).toMatch(/height:\s*100%/);
    expect(runtimeCardRule).toMatch(/overflow:\s*hidden/);

    const statusHeaderRule = appCss.match(
      /\.station-content-overview \.overview-status-card \[data-slot="card-header"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    expect(statusHeaderRule).toMatch(/grid-template-columns:\s*minmax\(0, 1fr\) auto/);

    const summaryHeaderRule = appCss.match(
      /\.station-content-overview \.overview-summary-card > \[data-slot="card-header"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const summaryLinkRule = appCss.match(
      /\.station-content-overview \.overview-summary-link\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const summaryListRule = appCss.match(
      /\.station-content-overview \.overview-summary-list\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    expect(summaryHeaderRule).toMatch(/padding:\s*16px 52px 8px 16px/);
    expect(summaryLinkRule).toMatch(/top:\s*14px/);
    expect(summaryLinkRule).toMatch(/right:\s*14px/);
    expect(summaryLinkRule).toMatch(/bottom:\s*auto/);
    expect(summaryListRule).toMatch(/flex:\s*1/);
    expect(summaryListRule).toMatch(/grid-template-rows:\s*repeat\(5, minmax\(0, 1fr\)\)/);
  });

  it("keeps a one-row global route snapshot at the top of its summary card", () => {
    const routeSnapshotRule = appCss.match(
      /\.overview-route-summary \.overview-route-list\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(routeSnapshotRule).toMatch(/align-content:\s*start/);
    expect(routeSnapshotRule).not.toMatch(/align-content:\s*center/);
  });

  it("hides the duplicate provider model summary while management is expanded", () => {
    const expandedProviderModelsRule = appCss.match(
      /\.provider-card\.expanded > \.provider-primary-models\[data-layout="compact-three-column"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(expandedProviderModelsRule).toMatch(/display:\s*none/);
  });

  it("keeps Settings category changes in a stable inner scroller", () => {
    const workspaceRule = appCss.match(
      /\.station-content\.station-content-settings\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const pageRule = appCss.match(/(?:^|\n)\.settings-page\s*\{([^}]*)\}/s)?.[1] ?? "";
    const contentRule = appCss.match(
      /\.settings-page \.settings-content\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const sidebarRule = appCss.match(/\.settings-sidebar\s*\{([^}]*)\}/s)?.[1] ?? "";
    const pressedNavigationRule = appCss.match(
      /\.settings-subnav \[data-slot="button"\]\.settings-subnav-item:active\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const pointerHoverRule = appCss.match(
      /\.settings-subnav\[data-input-mode="pointer"\] \[data-slot="button"\]\.settings-subnav-item:hover\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(workspaceRule).toMatch(/overflow:\s*hidden/);
    expect(pageRule).toMatch(/height:\s*100%/);
    expect(pageRule).toMatch(/min-height:\s*0/);
    expect(contentRule).toMatch(/overflow-y:\s*auto/);
    expect(contentRule).toMatch(/scrollbar-gutter:\s*stable/);
    expect(sidebarRule).toMatch(/align-content:\s*start/);
    expect(pressedNavigationRule).toMatch(/transform:\s*none/);
    expect(pointerHoverRule).toMatch(/background:\s*var\(--surface\)/);
    expect(appCss).not.toMatch(
      /(?:^|\n)\.settings-subnav \[data-slot="button"\]\.settings-subnav-item:hover\s*\{/,
    );
  });

  it("returns Settings scrolling to the outer workspace on narrow windows", () => {
    const narrowStart = appCss.lastIndexOf("@media (max-width: 820px)");
    const narrowEnd = appCss.indexOf("/* Final cascade guard", narrowStart);
    const narrowRules = appCss.slice(narrowStart, narrowEnd);
    const workspaceRule = narrowRules.match(
      /\.station-content\.station-content-settings\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const pageRule = narrowRules.match(/\.settings-page\s*\{([^}]*)\}/s)?.[1] ?? "";
    const contentRule = narrowRules.match(
      /\.settings-page \.settings-content\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(workspaceRule).toMatch(/overflow:\s*auto/);
    expect(pageRule).toMatch(/height:\s*auto/);
    expect(pageRule).toMatch(/min-height:\s*100%/);
    expect(contentRule).toMatch(/overflow-y:\s*visible/);
    expect(contentRule).toMatch(/overscroll-behavior:\s*auto/);
    expect(contentRule).toMatch(/scrollbar-gutter:\s*auto/);
  });

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

  it("uses one focus indicator for enterprise connection inputs", () => {
    const inputFocusRule = appCss.match(
      /\.enterprise-connection-panel \[data-slot="input"\]:focus-visible\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(inputFocusRule).toMatch(/outline:\s*none/);
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

  it("keeps usage filter popovers in viewport coordinates above the overview", () => {
    const toolbarRules = Array.from(
      appCss.matchAll(/\.usage-toolbar\s*\{([^}]*)\}/g),
      (match) => match[1],
    ).join("\n");
    const revealFrames = appCss.match(
      /@keyframes usage-filter-enter\s*\{([\s\S]*?)\n\}/,
    )?.[1] ?? "";

    expect(toolbarRules).toMatch(/position:\s*relative/);
    expect(toolbarRules).toMatch(/z-index:\s*\d+/);
    expect(toolbarRules).toMatch(/display:\s*grid/);
    expect(revealFrames).toMatch(/opacity:/);
    expect(revealFrames).not.toMatch(/transform:/);
    expect(appCss).not.toMatch(/@media \(max-width: 1120px\)\s*\{\s*\.usage-toolbar/);
  });

  it("groups the usage overview without internal dashboard dividers", () => {
    const primaryRule = appCss.match(/\.usage-primary-metric\s*\{([^}]*)\}/s)?.[1] ?? "";
    const healthRule = appCss.match(/\.usage-kpi-grid\s*\{([^}]*)\}/s)?.[1] ?? "";
    const healthItemRule = appCss.match(/\.usage-kpi-grid\s*>\s*div\s*\{([^}]*)\}/s)?.[1] ?? "";
    const compositionRule = appCss.match(/\.usage-token-rail\s*\{([^}]*)\}/s)?.[1] ?? "";
    const detailRule = appCss.match(/\.usage-token-details\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(primaryRule).not.toMatch(/border-(?:right|bottom|left|top):/);
    expect(healthRule).toMatch(/gap:/);
    expect(healthItemRule).toMatch(/border-radius:/);
    expect(healthItemRule).not.toMatch(/border-(?:right|bottom|left|top):/);
    expect(compositionRule).toMatch(/border-radius:/);
    expect(compositionRule).not.toMatch(/border-(?:right|bottom|left|top):/);
    expect(detailRule).not.toMatch(/border:/);
  });

  it("reserves intrinsic space for Overview routing badges", () => {
    const routeRowRule = appCss.match(
      /\.overview-route-list\s*>\s*div\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(routeRowRule).toMatch(/grid-template-columns:\s*max-content minmax\(0, 1fr\)/);
    expect(routeRowRule).toMatch(/column-gap:/);
  });
});
