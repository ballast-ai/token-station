import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const appCss = readFileSync(resolve(process.cwd(), "src/App.css"), "utf8");

describe("retained page theme styles", () => {
  it("prevents accidental static-text selection but keeps editable controls selectable", () => {
    const bodyRule = appCss.match(/(?:^|\n)body\s*\{([^}]*)\}/s)?.[1] ?? "";
    const editableRule = appCss.match(
      /body :is\(input, textarea, \[contenteditable="true"\]\)\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(bodyRule).toMatch(/user-select:\s*none/);
    expect(editableRule).toMatch(/user-select:\s*text/);
  });

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

  it("renders top-level routing scopes as cards and nests Agent routes under global routing", () => {
    const scopeCardRule = appCss.match(
      /\.routing-scope-item\[data-slot="button"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const selectedRule = appCss.match(
      /\.routing-scope-item\[data-slot="button"\]\[aria-current="page"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const selectedRailRule = appCss.match(
      /\.routing-scope-item\[data-slot="button"\]\[aria-current="page"\]::before\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const nestedListRule = appCss.match(/\.agent-route-list\s*\{([^}]*)\}/s)?.[1] ?? "";
    const childDisclosureRule = appCss.match(
      /\.agent-route-disclosure\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const childConnectorRule = appCss.match(
      /\.agent-route-disclosure::before\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(scopeCardRule).toMatch(/min-height:\s*62px/);
    expect(scopeCardRule).toMatch(/border:\s*1px solid var\(--line\)/);
    expect(selectedRule).toMatch(/border-color:\s*var\(--signal-selection-border\)/);
    expect(selectedRailRule).toMatch(/width:\s*3px/);
    expect(selectedRailRule).toMatch(/background:\s*var\(--signal\)/);
    expect(childDisclosureRule).toMatch(/margin-left:\s*18px/);
    expect(childDisclosureRule).toMatch(/padding-left:\s*14px/);
    expect(childConnectorRule).toMatch(/border-left:\s*1px solid var\(--line-strong\)/);
    expect(childConnectorRule).toMatch(/border-bottom:\s*1px solid var\(--line-strong\)/);
    expect(nestedListRule).toMatch(/border-left:\s*1px solid var\(--line-strong\)/);
  });

  it("keeps the Agent actions compact in the card's top-right corner", () => {
    const actionRule = appCss.match(/\.overview-agent-actions\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(actionRule).toMatch(/position:\s*absolute/);
    expect(actionRule).toMatch(/top:\s*14px/);
    expect(actionRule).toMatch(/right:\s*14px/);
    expect(actionRule).toMatch(/display:\s*flex/);
    expect(appCss).not.toMatch(/\.overview-agent-summary[^}]*padding-bottom:\s*58px/s);
    expect(appCss).not.toMatch(/\.model-test-(?:target|picker|model-name)/);
  });

  it("constrains provider management to a large internally scrolling dialog", () => {
    const dialogRule = appCss.match(
      /\.provider-management-dialog\[data-slot="dialog-content"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const bodyRule = appCss.match(/\.provider-management-dialog-body\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(dialogRule).toMatch(/max-width:\s*920px/);
    expect(dialogRule).toMatch(/(?:^|;)\s*height:\s*min\(760px,\s*calc\(100vh - 40px\)\)/);
    expect(dialogRule).toMatch(/max-height:/);
    expect(bodyRule).toMatch(/min-height:\s*0/);
    expect(bodyRule).toMatch(/overflow-y:\s*auto/);
  });

  it("keeps long provider-removal impact lists inside the viewport", () => {
    const dialogRule = appCss.match(
      /\.provider-removal-dialog\[data-slot="dialog-content"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const previewRule = appCss.match(
      /\.provider-removal-dialog \.provider-removal-preview\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(dialogRule).toMatch(/max-height:/);
    expect(dialogRule).toMatch(/grid-template-rows:\s*auto minmax\(0, 1fr\) auto/);
    expect(previewRule).toMatch(/overflow-y:\s*auto/);
  });

  it("hides the responsive request latency column by semantic class", () => {
    expect(appCss).toMatch(/\.usage-log-row > \.usage-log-latency\s*\{\s*display:\s*none/);
    expect(appCss).not.toMatch(/\.usage-log-row > :nth-child\(6\)/);
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
    expect(sidebarRule).toMatch(/grid-template-rows:\s*auto minmax\(0, 1fr\)/);
    const subnavRule = appCss.match(/\.settings-subnav\s*\{([^}]*)\}/s)?.[1] ?? "";
    expect(subnavRule).toMatch(/overflow-y:\s*auto/);
    expect(pressedNavigationRule).toMatch(/transform:\s*none/);
    expect(pointerHoverRule).toMatch(/background:\s*var\(--surface\)/);
    expect(appCss).not.toMatch(
      /(?:^|\n)\.settings-subnav \[data-slot="button"\]\.settings-subnav-item:hover\s*\{/,
    );
    expect(appCss).not.toMatch(
      /\.settings-subnav \[data-slot="button"\]\.settings-subnav-item\[aria-current="page"\]::before/,
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

  it("visually hides the duplicate Direct radio mark", () => {
    const radioRule = appCss.match(/\.direct-provider-radio\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(radioRule).toMatch(/position:\s*absolute/);
    expect(radioRule).toMatch(/clip:\s*rect\(0, 0, 0, 0\)/);
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
    expect(appliedTargetRule).toMatch(/white-space:\s*normal/);
    expect(appliedTargetRule).toMatch(/overflow-wrap:\s*anywhere/);
    expect(appliedTargetRule).not.toMatch(/text-overflow:\s*ellipsis/);
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

  it("keeps Agent rows comfortably sized inside the routing card system", () => {
    const permanentGlobalRouteRule = appCss.match(
      /\.agent-master-home\[data-slot="button"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const routeItemRule = appCss.match(
      /\.agent-master-item\[data-slot="button"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const routeIconRule = appCss.match(
      /\.agent-master-icon\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const routeMarkRule = appCss.match(
      /\.agent-master-item \.agent-master-icon > svg\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const agentVectorRule = appCss.match(
      /\.agent-master-item \.agent-brand-glyph > svg\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const agentImageRule = appCss.match(
      /\.agent-master-item \.agent-brand-glyph > img,[^{]*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const routeLabelRule = appCss.match(
      /\.agent-master-item strong\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const selectedAgentRule = appCss.match(
      /\.agent-master-item\[data-slot="button"\]\[aria-current="page"\]\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(permanentGlobalRouteRule).not.toMatch(/background:/);
    expect(permanentGlobalRouteRule).not.toMatch(/border:/);
    expect(routeItemRule).toMatch(/height:\s*52px/);
    expect(routeIconRule).toMatch(/width:\s*38px/);
    expect(routeIconRule).toMatch(/height:\s*38px/);
    expect(routeIconRule).not.toMatch(/border:/);
    expect(routeIconRule).not.toMatch(/background:/);
    expect(routeMarkRule).toMatch(/width:\s*34px/);
    expect(routeMarkRule).toMatch(/height:\s*34px/);
    expect(agentVectorRule).toMatch(/width:\s*34px/);
    expect(agentVectorRule).toMatch(/height:\s*34px/);
    expect(agentImageRule).toMatch(/width:\s*36px/);
    expect(agentImageRule).toMatch(/height:\s*36px/);
    expect(routeLabelRule).toMatch(/font-size:\s*13px/);
    expect(selectedAgentRule).toMatch(/background:\s*var\(--surface-2\)/);
    expect(selectedAgentRule).not.toMatch(/box-shadow:/);
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

    expect(previewRule).toMatch(/border:\s*1px solid var\(--line\)/);
    expect(previewRule).toMatch(/color:\s*var\(--muted\)/);
    expect(previewRule).toMatch(/background:\s*var\(--surface-2\)/);
    expect(previewIconRule).toMatch(/color:\s*var\(--muted\)/);
    expect(`${previewRule}\n${previewIconRule}`).not.toMatch(/var\(--signal\)|var\(--success\)/);
  });

  it("uses a visible neutral border as the composer's focus indicator", () => {
    const composerRule = appCss.match(
      /\.model-test-composer\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const composerFocusRule = appCss.match(
      /\.model-test-composer:focus-visible\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(composerRule).toMatch(/border:\s*1px solid var\(--line\)/);
    expect(composerFocusRule).toMatch(/outline:\s*0/);
    expect(composerFocusRule).toMatch(
      /border-color:\s*color-mix\(in srgb, var\(--ink\) 52%, var\(--line\)\)/,
    );
    expect(appCss).toMatch(/--ink:\s*#[0-9a-f]{6}/i);
    expect(composerFocusRule).not.toMatch(/var\(--signal\)/);
    expect(composerFocusRule).toMatch(/box-shadow:\s*none/);
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

  it("keeps the Usage trend and metric strip in the first desktop viewport", () => {
    const firstViewportRules = appCss.slice(
      appCss.indexOf("/* Usage first-viewport layout */"),
    );
    const workspaceRule = firstViewportRules.match(
      /\.station-content-topnav\s*>\s*\.usage-workspace-page\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const pageRule = firstViewportRules.match(
      /(?:^|\n)\.usage-workspace-page\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const headingRule = firstViewportRules.match(
      /\.usage-workspace-heading\.page-heading-with-action\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const descriptionRule = firstViewportRules.match(
      /\.usage-workspace-heading p\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const chartStageRule = firstViewportRules.match(
      /\.usage-page-embedded \.usage-chart-stage\s*\{([^}]*)\}/s,
    )?.[1] ?? "";
    const chartSvgRule = firstViewportRules.match(
      /\.usage-page-embedded \.usage-trend-svg\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(workspaceRule).toMatch(/padding-top:\s*12px/);
    expect(pageRule).toMatch(/gap:\s*8px/);
    expect(headingRule).toMatch(/flex-direction:\s*row/);
    expect(headingRule).toMatch(/align-items:\s*center/);
    expect(descriptionRule).toMatch(/display:\s*none/);
    expect(chartStageRule).toMatch(/min-height:\s*250px/);
    expect(chartSvgRule).toMatch(/height:\s*250px/);
  });

  it("reserves intrinsic space for Overview routing badges", () => {
    const routeRowRule = appCss.match(
      /\.overview-route-list\s*>\s*div\s*\{([^}]*)\}/s,
    )?.[1] ?? "";

    expect(routeRowRule).toMatch(/grid-template-columns:\s*max-content minmax\(0, 1fr\)/);
    expect(routeRowRule).toMatch(/column-gap:/);
  });
});
