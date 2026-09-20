// 校验官方完整发布包与唯一允许的两行回补，再核对实际 Cargo 解析图。
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { lstatSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const checksum = "233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5";
const crate = "glib-0.18.5";
export function desktopMetadata(root) {
  return JSON.parse(execFileSync("cargo", ["metadata", "--locked", "--format-version", "1",
    "--all-features", "--filter-platform", "x86_64-unknown-linux-gnu", "--manifest-path",
    join(root, "apps/desktop/src-tauri/Cargo.toml")], { cwd: root, encoding: "utf8", maxBuffer: 32 * 1024 * 1024 }));
}
function files(dir, prefix = "") {
  const result = [];
  for (const name of readdirSync(dir).sort()) {
    const relative = prefix ? `${prefix}/${name}` : name;
    const path = join(dir, name);
    const stat = lstatSync(path);
    if (stat.isSymbolicLink()) throw new Error(`vendor link forbidden: ${relative}`);
    if (stat.isDirectory()) result.push(...files(path, relative));
    else if (stat.isFile()) result.push(relative);
    else throw new Error(`vendor file type forbidden: ${relative}`);
  }
  return result.sort();
}
export function verifyGlibPatch(root, metadata = undefined) {
  root = realpathSync(root);
  const archive = join(root, "docs/security/upstream", `${crate}.crate`);
  if (!lstatSync(archive).isFile() || lstatSync(archive).isSymbolicLink()
    || createHash("sha256").update(readFileSync(archive)).digest("hex") !== checksum) {
    throw new Error("glib archive checksum mismatch");
  }
  const vendor = join(root, "vendor", crate);
  if (realpathSync(vendor) !== vendor || !lstatSync(vendor).isDirectory()) throw new Error("vendor link/directory mismatch");
  const temporary = mkdtempSync(join(tmpdir(), "glib-official-"));
  try {
    // 仅解压上述固定摘要的官方归档，不接受任意输入 tar。
    execFileSync("tar", ["-xzf", archive, "-C", temporary]);
    const expected = join(temporary, crate);
    const path = join(expected, "src/variant_iter.rs");
    const original = readFileSync(path, "utf8");
    const patched = original.replace("let p: *mut libc::c_char", "let mut p: *mut libc::c_char")
      .replace("                &p,", "                &mut p,");
    writeFileSync(path, patched);
    const expectedFiles = files(expected);
    if (JSON.stringify(files(vendor)) !== JSON.stringify(expectedFiles)) throw new Error("glib file set mismatch");
    for (const file of expectedFiles) {
      if (!readFileSync(join(expected, file)).equals(readFileSync(join(vendor, file)))) {
        throw new Error(`glib content mismatch: ${file}`);
      }
    }
  } finally { rmSync(temporary, { recursive: true, force: true }); }
  metadata ??= desktopMetadata(root);
  const glib = metadata.packages.filter(p => p.name === "glib");
  const desktop = metadata.packages.find(p => p.name === "token-station-desktop"
    && resolve(p.manifest_path) === join(root, "apps/desktop/src-tauri/Cargo.toml"));
  if (glib.length !== 1 || glib[0].version !== "0.18.5" || glib[0].source !== null
    || resolve(glib[0].manifest_path) !== join(vendor, "Cargo.toml") || !desktop
    || metadata.resolve?.root !== desktop.id) throw new Error("glib metadata identity mismatch");
  const nodes = new Map(metadata.resolve.nodes.map(n => [n.id, n.dependencies]));
  const seen = new Set(); const pending = [desktop.id];
  while (pending.length) {
    const id = pending.pop();
    if (seen.has(id)) continue;
    seen.add(id);
    if (!nodes.has(id)) throw new Error("glib metadata resolve graph incomplete");
    pending.push(...nodes.get(id));
  }
  if (!seen.has(glib[0].id)) throw new Error("glib metadata package is not consumed by desktop");
  return true;
}
export function projectDesktopLock(lock) {
  const blocks = lock.split("[[package]]");
  const matching = blocks.map((b, i) => /^name = "glib"$/m.test(b) ? i : -1).filter(i => i >= 0);
  if (matching.length !== 1) throw new Error("glib lock package count mismatch");
  const index = matching[0]; const block = blocks[index];
  if (!/^version = "0.18.5"$/m.test(block) || /^(source|checksum)\s*=/m.test(block)) {
    throw new Error("glib lock must contain only the verified path identity");
  }
  blocks[index] = block.replace('version = "0.18.5"', `version = "0.18.5"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "${checksum}"`);
  return blocks.join("[[package]]");
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  verifyGlibPatch(resolve(import.meta.dirname, ".."));
  console.log("glib official backport: PASS (full package, exact patch, actual desktop dependency)");
}
