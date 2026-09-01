#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const read = (relativePath) => fs.readFileSync(path.join(root, relativePath), "utf8");

for (const [file, forbidden] of [
  ["apps/desktop/src/api.ts", /reveal|SensitivePlanValueView/i],
  [
    "apps/desktop/src/pages/AgentRoutePage.tsx",
    /reveal|show.{0,24}(?:full|complete|exact).{0,24}(?:value|credential|secret)/i,
  ],
  ["apps/desktop/src-tauri/src/lib.rs", /reveal_agent_plan/i],
  [
    "apps/desktop/src-tauri/src/agent_integration/commands.rs",
    /reveal_plan|SensitivePlanValueView/i,
  ],
]) {
  assert.doesNotMatch(
    read(file),
    forbidden,
    `${file} must not add a separate Agent credential reveal surface`,
  );
}

console.log("Agent credential reveal surface: PASS");
