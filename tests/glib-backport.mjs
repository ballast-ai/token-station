import test from "node:test";
import assert from "node:assert/strict";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync, symlinkSync, realpathSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import { verifyGlibPatch, projectDesktopLock } from "../scripts/check-glib-backport.mjs";
const root = resolve(import.meta.dirname, "..");
const original = readFileSync(join(root, "vendor/glib-0.18.5/src/variant_iter.rs"), "utf8")
  .replace("let mut p: *mut libc::c_char", "let p: *mut libc::c_char")
  .replace("                &mut p,", "                &p,");
const patched = original.replace("let p: *mut libc::c_char", "let mut p: *mut libc::c_char")
  .replace("                &p,", "                &mut p,");
function fixture(t) {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), "glib-backport-test-")));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  cpSync(join(root, "vendor"), join(dir, "vendor"), { recursive: true });
  cpSync(join(root, "docs/security/upstream"), join(dir, "docs/security/upstream"), { recursive: true });
  writeFileSync(join(dir, "vendor/glib-0.18.5/src/variant_iter.rs"), patched);
  const id = "path+file:///glib#0.18.5";
  const metadata = { packages: [
    { id, name: "glib", version: "0.18.5", source: null, manifest_path: join(dir, "vendor/glib-0.18.5/Cargo.toml") },
    { id: "desktop", name: "token-station-desktop", manifest_path: join(dir, "apps/desktop/src-tauri/Cargo.toml") },
    { id: "gtk", name: "gtk" },
  ], resolve: { root: "desktop", nodes: [
    { id: "desktop", dependencies: ["gtk"] }, { id: "gtk", dependencies: [id] }, { id, dependencies: [] },
  ] } };
  return { dir, metadata, id };
}
test("完整官方包仅含两行回补才接受", t => {
  const f = fixture(t); assert.equal(verifyGlibPatch(f.dir, f.metadata), true);
});
test("丢失两行补丁必须拒绝", t => {
  const f = fixture(t); writeFileSync(join(f.dir, "vendor/glib-0.18.5/src/variant_iter.rs"), original);
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /content|内容/);
});
test("补丁之外的文件修改必须拒绝", t => {
  const f = fixture(t); writeFileSync(join(f.dir, "vendor/glib-0.18.5/README.md"), "changed");
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /content|内容/);
});
test("新增文件必须拒绝", t => {
  const f = fixture(t); writeFileSync(join(f.dir, "vendor/glib-0.18.5/extra"), "extra");
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /file|文件/);
});
test("删除文件必须拒绝", t => {
  const f = fixture(t); rmSync(join(f.dir, "vendor/glib-0.18.5/README.md"));
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /file|文件/);
});
test("vendor中的符号链接必须拒绝", t => {
  const f = fixture(t); const p=join(f.dir, "vendor/glib-0.18.5/README.md"); rmSync(p); symlinkSync("Cargo.toml",p);
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /link|链接/);
});
test("原始归档被替换必须拒绝", t => {
  const f = fixture(t); writeFileSync(join(f.dir, "docs/security/upstream/glib-0.18.5.crate"), "wrong");
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /archive|归档/);
});
test("metadata仍从registry取glib必须拒绝", t => {
  const f = fixture(t); f.metadata.packages[0].source="registry+https://github.com/rust-lang/crates.io-index";
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /metadata/);
});
test("metadata指其他目录必须拒绝", t => {
  const f = fixture(t); f.metadata.packages[0].manifest_path=join(f.dir,"other/Cargo.toml");
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /metadata/);
});
test("metadata重复glib必须拒绝", t => {
  const f = fixture(t); f.metadata.packages.push({...f.metadata.packages[0], id:"other"});
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /metadata/);
});
test("metadata列出但desktop未消费的vendor必须拒绝", t => {
  const f = fixture(t); f.metadata.resolve.nodes[1].dependencies=[];
  assert.throws(() => verifyGlibPatch(f.dir, f.metadata), /metadata/);
});
test("审计投影仅给glib恢复原registry身份", () => {
  const lock='version = 4\n\n[[package]]\nname = "glib"\nversion = "0.18.5"\n\n[[package]]\nname = "host"\nversion = "1.0.0"\n';
  const result=projectDesktopLock(lock);
  assert.match(result,/source = "registry\+https:\/\/github.com\/rust-lang\/crates.io-index"/);
  assert.ok(result.endsWith('[[package]]\nname = "host"\nversion = "1.0.0"\n'));
});
test("审计投影拒绝已有registry来源", () => {
  assert.throws(()=>projectDesktopLock('version = 4\n[[package]]\nname = "glib"\nversion = "0.18.5"\nsource = "registry+anything"\n'), /lock/);
});

import { execFileSync } from "node:child_process";
import { auditDesktop } from "../scripts/audit-desktop.mjs";
function auditFixture(t, otherAdvisory = false) {
  const f = fixture(t);
  const desktop = join(f.dir, "apps/desktop/src-tauri"); mkdirSync(desktop, { recursive: true });
  writeFileSync(join(desktop, "Cargo.lock"), 'version = 4\n\n[[package]]\nname = "glib"\nversion = "0.18.5"\n');
  const db = join(f.dir, "advisory-db"); mkdirSync(join(db, "crates/glib"), { recursive: true });
  const ids = otherAdvisory ? ["RUSTSEC-2024-0429", "RUSTSEC-2026-9999"] : ["RUSTSEC-2024-0429"];
  for (const id of ids) writeFileSync(join(db, `crates/glib/${id}.md`), `\`\`\`toml\n[advisory]\nid = "${id}"\npackage = "glib"\ndate = "2026-01-01"\ninformational = "unsound"\n[versions]\npatched = [">=0.20.0"]\n\`\`\`\n\n# 隔离测试公告\n\n仅用于验证审计工具不会跳过路径依赖。\n`);
  execFileSync("git", ["init", "--quiet", db]);
  execFileSync("git", ["-C", db, "add", "."]);
  execFileSync("git", ["-C", db, "-c", "user.name=Gate Test", "-c", "user.email=gate@example.invalid", "-c", "commit.gpgsign=false", "commit", "--quiet", "-m", "isolated advisory fixture"]);
  return { ...f, options: { metadata: f.metadata, auditArgs: ["--no-fetch", "--stale", "--no-yanked", "--db", db] } };
}
test("实际cargo-audit仅允许已经证实修复的0429", t => {
  const f = auditFixture(t); assert.equal(auditDesktop(f.dir, f.options), true);
});
test("实际cargo-audit必须拒绝其他glib公告，不能因path而漏报", t => {
  const f = auditFixture(t, true);
  assert.throws(() => auditDesktop(f.dir, f.options), /RUSTSEC-2026-9999/);
});
test("审计之前必须核补丁，不能直接ignore0429", t => {
  const f = auditFixture(t); writeFileSync(join(f.dir, "vendor/glib-0.18.5/src/variant_iter.rs"), original);
  assert.throws(() => auditDesktop(f.dir, f.options), /content/);
});
