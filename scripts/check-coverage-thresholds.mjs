import fs from "node:fs";
import path from "node:path";

const [lcovPath, sourcePath, rawThreshold] = process.argv.slice(2);
if (!lcovPath || !sourcePath || !rawThreshold) {
  console.error(
    "usage: node scripts/check-coverage-thresholds.mjs <lcov> <source-path> <minimum-lines-percent>",
  );
  process.exit(2);
}

const threshold = Number(rawThreshold);
if (!Number.isFinite(threshold) || threshold < 0 || threshold > 100) {
  console.error(`invalid line coverage threshold: ${rawThreshold}`);
  process.exit(2);
}

const normalizedNeedle = path.resolve(sourcePath).replaceAll("\\", "/");
const records = fs.readFileSync(lcovPath, "utf8").split("end_of_record");
let found = 0;
let linesFound = 0;
let linesHit = 0;

for (const record of records) {
  const fields = new Map(
    record
      .trim()
      .split(/\r?\n/u)
      .filter(Boolean)
      .map((line) => [line.slice(0, 2), line.slice(3)]),
  );
  const source = fields.get("SF");
  if (!source) continue;
  const normalizedSource = path.resolve(source).replaceAll("\\", "/");
  if (
    normalizedSource !== normalizedNeedle &&
    !normalizedSource.startsWith(`${normalizedNeedle}/`)
  ) {
    continue;
  }
  found += 1;
  linesFound += Number(fields.get("LF") ?? 0);
  linesHit += Number(fields.get("LH") ?? 0);
}

if (found === 0 || linesFound === 0) {
  console.error(`no LCOV source lines matched ${normalizedNeedle}`);
  process.exit(1);
}

const percent = (linesHit / linesFound) * 100;
console.log(
  `${sourcePath}: ${linesHit}/${linesFound} lines (${percent.toFixed(2)}%), required ${threshold.toFixed(2)}%`,
);
if (percent + Number.EPSILON < threshold) process.exit(1);
