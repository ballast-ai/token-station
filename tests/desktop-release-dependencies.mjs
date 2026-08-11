#!/usr/bin/env node

import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const lock = JSON.parse(
  fs.readFileSync(path.join(root, "apps/desktop/package-lock.json"), "utf8"),
);

function numericVersion(version) {
  return version.split(".").map((part) => Number.parseInt(part, 10));
}

function atLeast(actual, expected) {
  const left = numericVersion(actual);
  const right = numericVersion(expected);
  for (let index = 0; index < Math.max(left.length, right.length); index += 1) {
    const difference = (left[index] ?? 0) - (right[index] ?? 0);
    if (difference !== 0) return difference > 0;
  }
  return true;
}

for (const [dependency, minimum] of Object.entries({
  dompurify: "3.4.13",
  mermaid: "11.16.1",
  nanoid: "3.3.17",
  postcss: "8.5.23",
  undici: "7.29.0",
})) {
  const actual = lock.packages?.[`node_modules/${dependency}`]?.version;
  assert.ok(actual, `锁文件缺少 ${dependency}`);
  assert.ok(
    atLeast(actual, minimum),
    `${dependency} 当前是 ${actual}，发布前至少需要 ${minimum}`,
  );
}

console.log("desktop release dependency floor: PASS");
