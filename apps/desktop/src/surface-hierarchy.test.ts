import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const appCss = readFileSync(resolve(process.cwd(), "src/App.css"), "utf8");

function lastRule(pattern: RegExp) {
  const matches = Array.from(appCss.matchAll(pattern));
  return matches[matches.length - 1]?.[1] ?? "";
}

describe("quiet surface hierarchy", () => {
  it("renders Overview metrics as an open strip while keeping destination cards", () => {
    const runtimeCardRule = lastRule(
      /\.station-content-overview \.overview-runtime-metrics > \[data-slot="card"\]\s*\{([^}]*)\}/gs,
    );
    const summaryCardRule = lastRule(
      /\.station-content-overview \.overview-summary-card\[data-slot="card"\]\s*\{([^}]*)\}/gs,
    );

    expect(runtimeCardRule).toMatch(/border:\s*0/);
    expect(runtimeCardRule).toMatch(/background:\s*transparent/);
    expect(runtimeCardRule).toMatch(/box-shadow:\s*none/);
    expect(summaryCardRule).toMatch(/border:\s*1px solid var\(--line\)/);
  });

  it("removes redundant Routing shells without weakening interactive rows", () => {
    const masterListRule = lastRule(/^\.agent-master-list-card\s*\{([^}]*)\}/gms);
    const routePanelRule = lastRule(
      /\.agent-master-content \.home-page > \.panel\s*\{([^}]*)\}/gs,
    );
    const scopeItemRule = lastRule(
      /\.routing-scope-item\[data-slot="button"\]\s*\{([^}]*)\}/gs,
    );
    const directRowRule = appCss.match(/\.direct-provider-row\s*\{([^}]*)\}/s)?.[1] ?? "";

    expect(masterListRule).toMatch(/border:\s*0/);
    expect(masterListRule).toMatch(/border-right:\s*1px solid var\(--line\)/);
    expect(routePanelRule).toMatch(/border:\s*0/);
    expect(scopeItemRule).toMatch(/border:\s*1px solid var\(--line\)/);
    expect(directRowRule).toMatch(/border:\s*1px solid var\(--line\)/);
  });

  it("opens the Agent detail canvas while retaining meaningful fact groups", () => {
    const heroRule = lastRule(/\.agent-route-embedded \.agent-route-hero\s*\{([^}]*)\}/gs);
    const detailRule = lastRule(
      /\.agent-route-embedded \.agent-connection-detail\s*\{([^}]*)\}/gs,
    );
    const changeRule = lastRule(/\.agent-connection-change\s*\{([^}]*)\}/gs);

    expect(heroRule).toMatch(/border:\s*0/);
    expect(heroRule).toMatch(/border-bottom:\s*1px solid var\(--line\)/);
    expect(detailRule).toMatch(/border:\s*0/);
    expect(changeRule).toMatch(/border:\s*1px solid var\(--line\)/);
  });

  it("uses one containment layer for Models and Settings", () => {
    const providerPanelRule = lastRule(/\.providers-page > \.provider-panel\s*\{([^}]*)\}/gs);
    const providerCardRule = lastRule(/\.providers-page \.provider-card\s*\{([^}]*)\}/gs);
    const modelCellRule = lastRule(
      /\.provider-card \.provider-primary-models\[data-layout="compact-three-column"\] > div\s*\{([^}]*)\}/gs,
    );
    const settingsCardRule = lastRule(/\.settings-card\s*\{([^}]*)\}/gs);
    const aboutCardRule = lastRule(
      /\.settings-page \.about-product-card,[^{]*\{([^}]*)\}/gs,
    );

    expect(providerPanelRule).toMatch(/border:\s*0/);
    expect(providerCardRule).toMatch(/border:\s*1px solid var\(--line\)/);
    expect(modelCellRule).toMatch(/border:\s*0/);
    expect(settingsCardRule).toMatch(/border:\s*0/);
    expect(aboutCardRule).toMatch(/border:\s*0/);

    expect(appCss).toMatch(/\.theme-option\s*\{[^}]*border:\s*1px solid var\(--line\)/s);
    expect(appCss).toMatch(/\.language-option\s*\{[^}]*border:\s*1px solid var\(--line\)/s);
  });

  it("flattens nested static summaries but keeps semantic separators", () => {
    const enterpriseSummaryRule = lastRule(/\.enterprise-connected-summary\s*\{([^}]*)\}/gs);
    const usageSummaryRule = lastRule(/\.usage-page \.usage-hero\s*\{([^}]*)\}/gs);

    expect(enterpriseSummaryRule).toMatch(/border:\s*0/);
    expect(enterpriseSummaryRule).toMatch(/border-top:\s*1px solid var\(--line\)/);
    expect(usageSummaryRule).toMatch(/border:\s*0/);
    expect(usageSummaryRule).toMatch(/box-shadow:\s*none/);
  });
});
