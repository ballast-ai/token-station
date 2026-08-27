#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");

for (const [file, forbidden] of [
  ["apps/desktop/src/api.ts", "revealAgentPlanSensitiveValues"],
  ["apps/desktop/src/pages/AgentRoutePage.tsx", "Show full values"],
  ["apps/desktop/src-tauri/src/lib.rs", "reveal_agent_plan_sensitive_values"],
  [
    "apps/desktop/src-tauri/src/agent_integration/commands.rs",
    "reveal_agent_plan_sensitive_values",
  ],
]) {
  assert.doesNotMatch(
    read(file),
    new RegExp(forbidden),
    `${file} must not expose complete Agent credential values`,
  );
}

console.log("Agent sensitive-value boundary: PASS");
