// cargo-audit 跳过 path 包；先验证精确回补，再审计原 registry 身份。
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { verifyGlibPatch, projectDesktopLock } from "./check-glib-backport.mjs";

export function auditDesktop(root, { metadata, auditArgs = [] } = {}) {
  // 仅供隔离公告库测试；生产 CLI 不开放附加参数或额外 ignore。
  for (let i = 0; i < auditArgs.length; i++) {
    if (["--no-fetch", "--stale", "--no-yanked"].includes(auditArgs[i])) continue;
    if (auditArgs[i] === "--db" && typeof auditArgs[i + 1] === "string" && !auditArgs[i + 1].startsWith("--")) { i++; continue; }
    throw new Error("unsupported audit fixture option");
  }
  verifyGlibPatch(root, metadata);
  const projected = projectDesktopLock(readFileSync(join(root, "apps/desktop/src-tauri/Cargo.lock"), "utf8"));
  const temporary = mkdtempSync(join(tmpdir(), "desktop-audit-"));
  try {
    const lock = join(temporary, "Cargo.lock"); writeFileSync(lock, projected);
    const result = spawnSync("cargo", ["audit", "--file", lock, "-D", "unsound",
      "--ignore", "RUSTSEC-2024-0429", ...auditArgs], {
      cwd: root, encoding: "utf8", maxBuffer: 16 * 1024 * 1024,
    });
    if (result.error || result.status !== 0) {
      throw new Error(`desktop audit failed: ${result.error?.message ?? ""}\n${result.stdout ?? ""}${result.stderr ?? ""}`);
    }
    process.stdout.write(result.stdout); process.stderr.write(result.stderr);
    return true;
  } finally { rmSync(temporary, { recursive: true, force: true }); }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 2) throw new Error("desktop audit accepts no CLI overrides");
  auditDesktop(resolve(import.meta.dirname, ".."));
  console.log("Desktop audit: PASS (verified glib backport, original registry projection)");
}
