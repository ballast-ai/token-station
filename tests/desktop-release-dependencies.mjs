#!/usr/bin/env node

import assert from "node:assert/strict";
import crypto from "node:crypto";
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const fullCi = fs.readFileSync(path.join(root, ".github/workflows/full-ci.yml"), "utf8");
assert.match(fullCi, /npm --prefix apps\/desktop audit --package-lock-only --audit-level=low/, "Full CI must reject frontend advisories from the locked dependency graph");
const lock = JSON.parse(
  fs.readFileSync(path.join(root, "apps/desktop/package-lock.json"), "utf8"),
);
const frontendManifest = JSON.parse(fs.readFileSync(path.join(root, "apps/desktop/package.json"), "utf8"));
assert.equal(frontendManifest.dependencies["@lobehub/icons-static-svg"], "1.95.1", "Pin the dependency-free brand artwork");
for (const name of ["shadcn", "@lobehub/icons", "@lobehub/ui", "katex", "mermaid", "rehype-katex", "remark-math", "braces", "@modelcontextprotocol/sdk", "proxy-addr"]) {
  assert.ok(!Object.keys(lock.packages).some(location => location.endsWith(`node_modules/${name}`)), `Remove the unused ${name} dependency surface`);
}
const provenance = JSON.parse(fs.readFileSync(path.join(root, "apps/desktop/src/assets/brand-icons-provenance.json"), "utf8"));
const artworkPackage = lock.packages["node_modules/@lobehub/icons-static-svg"];
assert.equal(artworkPackage.version, provenance.version);
assert.equal(artworkPackage.integrity, provenance.integrity);
assert.deepEqual(artworkPackage.dependencies ?? {}, {});
assert.deepEqual(artworkPackage.peerDependencies ?? {}, {});
assert.equal(provenance.brands.length, 42);
assert.equal(Object.keys(provenance.files).length, 72);
assert.equal(Object.keys(provenance.avatarSource.localFiles).length, 2);
assert.equal(provenance.avatarSource.version, "5.14.0");
const glyphSource = fs.readFileSync(path.join(root, "apps/desktop/src/localBrandIcons.tsx"), "utf8");
const importedArt = [...glyphSource.matchAll(/from "@lobehub\/icons-static-svg\/([^"?]+)\?raw"/g)].map(match => match[1]);
assert.deepEqual(importedArt.sort(), Object.keys(provenance.files).sort());
const allowedSvgAttributes = new Map(Object.entries({
  svg: ["fill","fill-rule","height","style","viewBox","width","xmlns"],
  title: [],
  path: ["d","fill","fill-rule","clip-rule","fill-opacity"],
  defs: [],
  linearGradient: ["gradientUnits","id","x1","x2","y1","y2"],
  stop: ["stop-color","offset","stop-opacity"],
  circle: ["cx","cy","fill","r"],
  g: ["clip-path","mask","filter"],
  clipPath: ["id"],
  mask: ["height","id","maskUnits","width","x","y"],
  filter: ["color-interpolation-filters","filterUnits","height","id","width","x","y"],
  feFlood: ["flood-opacity","result"],
  feBlend: ["in","in2","result"],
  feGaussianBlur: ["result","stdDeviation"],
}));
function assertSafeArtwork(svg, file) {
  assert.doesNotMatch(svg, /<\s*[!?]|&/, `Artwork declarations and entities are unsupported: ${file}`);
  const remainder = svg.replace(/<\/?([A-Za-z][\w:-]*)([^<>]*?)>/g, (tag, element, attributes) => {
    const allowed = allowedSvgAttributes.get(element);
    assert.ok(allowed, `Unsupported artwork element ${element}: ${file}`);
    if (tag.startsWith("</")) {
      assert.equal(attributes.trim(), "", `Invalid closing artwork tag: ${file}`);
      return "";
    }
    const seen = new Set();
    const unmatched = attributes.replace(/\s+([A-Za-z][\w:-]*)="([^"<>]*)"/g, (_attribute, name, value) => {
      assert.ok(allowed.includes(name), `Unsupported artwork attribute ${element}.${name}: ${file}`);
      assert.ok(!seen.has(name), `Duplicate artwork attribute ${name}: ${file}`);
      seen.add(name);
      if (name === "xmlns") {
        assert.equal(value, "http://www.w3.org/2000/svg", `Unexpected artwork namespace: ${file}`);
      } else if (name === "style") {
        assert.equal(value, "flex:none;line-height:1", `Unexpected artwork style: ${file}`);
      } else {
        assert.doesNotMatch(value, /[a-z][a-z\d+.-]*:/i, `Artwork URL schemes are unsupported: ${file}`);
        if (["fill", "stop-color"].includes(name)) {
          assert.match(value, /^(?:currentColor|gold|#[\da-f]{3,8}|url\(#[\w-]+\))$/i, `Unsupported artwork paint: ${file}`);
        }
        if (["clip-path", "mask", "filter"].includes(name)) {
          assert.match(value, /^url\(#[\w-]+\)$/, `External artwork reference: ${file}`);
        }
      }
      return "";
    }).trim();
    assert.ok(unmatched === "" || unmatched === "/", `Unparsed artwork attributes: ${file}`);
    return "";
  });
  assert.doesNotMatch(remainder, /[<>]|[a-z][a-z\d+.-]*:/i, `Unparsed artwork markup: ${file}`);
}
for (const unsafe of [
  '<svg><script>fixture</script></svg>',
  '<svg><foreignObject>fixture</foreignObject></svg>',
  '<svg onload="fixture()"/>',
  '<svg><path href="//fixture.invalid"/></svg>',
  '<svg><path fill="url(//fixture.invalid)"/></svg>',
  '<svg><animate attributeName="opacity" values="0;1"/></svg>',
  '<svg><set attributeName="onload" to="fixture()"/></svg>',
  '<svg><style>svg {fill:red}</style></svg>',
  '<svg><path fill="&#x75;rl(&#x68;ttps://fixture.invalid)"/></svg>',
  '<!DOCTYPE svg [<!ENTITY label "fixture">]><svg><title>&label;</title></svg>',
  '<?xml-stylesheet href="fixture.css"?><svg/>',
  '<svg><path fill="ftp:fixture.invalid"/></svg>',
]) assert.throws(() => assertSafeArtwork(unsafe, "synthetic.svg"));
for (const [file, expected] of Object.entries({
  ...Object.fromEntries(Object.entries(provenance.files).map(([file, hash]) => [`node_modules/@lobehub/icons-static-svg/${file}`, hash])),
  ...provenance.avatarSource.localFiles,
})) {
  const svg = fs.readFileSync(path.join(root, "apps/desktop", file), "utf8");
  assert.equal(crypto.createHash("sha256").update(svg).digest("hex"), expected, `Artwork source changed: ${file}`);
  const svgRoot = svg.match(/^<svg\b([^>]*)>/)?.[1];
  assert.ok(svgRoot, `Missing SVG root: ${file}`);
  assert.match(svgRoot, /\bviewBox="0 0 24 24"/, `Unsupported artwork viewport: ${file}`);
  for (const [, attribute, value] of svgRoot.matchAll(/([\w:-]+)="([^"]*)"/g)) {
    assert.ok(["fill", "fill-rule", "height", "width", "style", "viewBox", "xmlns"].includes(attribute), `Unsupported root attribute ${attribute}: ${file}`);
    if (attribute === "fill") assert.equal(value, "currentColor");
    if (attribute === "fill-rule") assert.equal(value, "evenodd");
    if (attribute === "style") assert.equal(value, "flex:none;line-height:1");
  }
  assertSafeArtwork(svg, file);
}
const shippedLicense = fs.readFileSync(path.join(root, "apps/desktop", provenance.licenseFile), "utf8");
assert.equal(shippedLicense, fs.readFileSync(path.join(root, "docs/assets/agents/LICENSE.lobehub-icons"), "utf8"));
const appStyles = fs.readFileSync(path.join(root, "apps/desktop/src/App.css"), "utf8");
assert.match(appStyles, /@import "\.\/shadcn-compat\.css"/);
assert.doesNotMatch(appStyles, /@import "shadcn\//);
assert.match(appStyles, /:root\.dark \.local-brand-avatar\[data-avatar-surface="black"\]/);
assert.match(appStyles, /:root\.dark \.local-brand-avatar\[data-avatar-surface="white"\]/);
const compatibility = fs.readFileSync(path.join(root, "apps/desktop/src/shadcn-compat.css"), "utf8");
const { compile } = createRequire(path.join(root, "apps/desktop/package.json"))("tailwindcss");
const animationPackage = path.join(root, "apps/desktop/node_modules/tw-animate-css");
const animationManifest = JSON.parse(fs.readFileSync(path.join(animationPackage, "package.json"), "utf8"));
const animations = fs.readFileSync(path.join(animationPackage, animationManifest.exports["."].style), "utf8");
const utilities = await compile(`${animations}\n${compatibility}\n@tailwind utilities;`);
const generated = utilities.build(["data-open:flex", "data-closed:hidden", "data-checked:block", "data-unchecked:inline", "data-active:flex", "data-disabled:hidden", "data-horizontal:flex-row", "data-vertical:flex-col", "data-open:animate-accordion-down", "data-closed:animate-accordion-up"]);
for (const state of ["open", "closed", "checked", "unchecked", "active"]) {
  assert.ok(generated.includes(`[data-state="${state}"]`), `Preserve the ${state} control state`);
  assert.ok(generated.includes(`[data-${state}]:not([data-${state}="false"])`));
}
assert.ok(generated.includes('[data-disabled]:not([data-disabled="false"])'));
assert.ok(generated.includes('[data-orientation="horizontal"]'));
assert.ok(generated.includes('[data-orientation="vertical"]'));
assert.match(compatibility, /@keyframes accordion-down/);
assert.match(compatibility, /@keyframes accordion-up/);
assert.match(compatibility, /--radix-accordion-content-height/);
assert.match(generated, /@keyframes accordion-down/);
assert.match(generated, /@keyframes accordion-up/);

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
  nanoid: "3.3.17",
  postcss: "8.5.23",
  "source-map-js": "1.2.2",
  undici: "7.29.1",
})) {
  const actual = lock.packages?.[`node_modules/${dependency}`]?.version;
  assert.ok(actual, `锁文件缺少 ${dependency}`);
  assert.ok(
    atLeast(actual, minimum),
    `${dependency} 当前是 ${actual}，发布前至少需要 ${minimum}`,
  );
}

const desktopManifest = fs.readFileSync(path.join(root, "apps/desktop/src-tauri/Cargo.toml"), "utf8");
const updaterDependency = desktopManifest.match(/^tauri-plugin-updater\s*=\s*(.+)$/m)?.[1];
assert.ok(updaterDependency, "Desktop updater dependency is missing");
assert.match(
  updaterDependency,
  /default-features\s*=\s*false/,
  "Manual Windows updates must not enable the updater ZIP extraction feature",
);
assert.match(
  updaterDependency,
  /features\s*=\s*\[[^\]]*"rustls-tls"/,
  "The macOS updater must retain its rustls transport",
);
const desktopRustLock = fs.readFileSync(path.join(root, "apps/desktop/src-tauri/Cargo.lock"), "utf8");
assert.doesNotMatch(
  desktopRustLock,
  /^name = "zip"$/m,
  "The desktop lockfile must exclude the unused vulnerable ZIP extraction dependency",
);

console.log("desktop release dependency floor: PASS");
