#!/usr/bin/env node
// One current architecture, and older diagrams that say they are not it.
//
// The banner already existed in the superseded SVG and was missing from its
// PNG, which is the case that matters: nobody opens an SVG from a file
// listing. A reader who double-clicks the PNG gets a rendered diagram with no
// hint that it stopped being the baseline months ago, and the diagram is
// detailed and confident enough to be believed.
//
// The gate is that the marking is a property of the artifact, not of whichever
// plan currently links to it — plans get superseded too, and the 08-23 one the
// old banner named already has been.

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const docRepo = path.resolve(here, "../../token-station-doc");

if (!fs.existsSync(docRepo)) {
  // The private design repository is not always checked out beside this one.
  console.log("diagram supersession: token-station-doc not present, skipped");
  process.exit(0);
}

const CURRENT = [
  "docs/assets/diagrams/token-station-architecture",
  "docs/assets/diagrams/token-station-dependencies",
];
const SUPERSEDED = ["docs/design/2026-08-20-token-station-target-arch-dataflow"];

const read = (relative) => fs.readFileSync(path.join(docRepo, relative), "utf8");

for (const stem of CURRENT) {
  const svg = read(`${stem}.svg`);
  assert.match(svg, /CURRENT /, `${stem}.svg must say it is the current final-state diagram`);
  assert.doesNotMatch(svg, /SUPERSEDED/, `${stem}.svg must not also claim to be superseded`);
}

for (const stem of SUPERSEDED) {
  const svg = read(`${stem}.svg`);
  assert.match(svg, /SUPERSEDED/, `${stem}.svg must carry a supersession banner`);
  // Point at the artifact that replaced it, not at a plan. A plan reference
  // goes stale the next time a plan is superseded, which is what happened.
  assert.match(
    svg,
    /docs\/assets\/diagrams\//,
    `${stem}.svg must name the diagrams that replaced it, not only a plan`,
  );
}

// A PNG regenerated from its SVG carries whatever the SVG says. One that was
// not regenerated is the actual defect, so compare modification times rather
// than trusting that someone remembered.
for (const stem of [...CURRENT, ...SUPERSEDED]) {
  const svgPath = path.join(docRepo, `${stem}.svg`);
  const pngPath = path.join(docRepo, `${stem}.png`);
  assert.ok(fs.existsSync(pngPath), `${stem}.png is missing`);
  const svgCommit = execFileSync(
    "git",
    ["log", "-1", "--format=%ct", "--", `${stem}.svg`],
    { cwd: docRepo, encoding: "utf8" },
  ).trim();
  const pngCommit = execFileSync(
    "git",
    ["log", "-1", "--format=%ct", "--", `${stem}.png`],
    { cwd: docRepo, encoding: "utf8" },
  ).trim();
  if (svgCommit && pngCommit) {
    assert.ok(
      Number(pngCommit) >= Number(svgCommit),
      `${stem}.png was last committed before its SVG; re-render it`,
    );
  }
}

console.log(
  `diagram supersession: ${CURRENT.length} current and ${SUPERSEDED.length} superseded diagrams are marked`,
);
