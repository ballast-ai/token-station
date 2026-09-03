#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = dirname(fileURLToPath(import.meta.url));
const LIGHT = "#F4F6FA";
const DARK = "#111620";

const COPY = {
  directions: {
    "01": ["Switch Cut", "切换切口"],
    "02": ["Local Aperture", "本地孔径"],
    "03": ["Dual-track Exchange", "双轨交换"],
    "04": ["Token Rack", "Token 机架"],
    "05": ["Signal Control", "信号控制"],
  },
  concepts: {
    "d01-01-relay-slash": ["Relay Slash", "中继斜切"],
    "d01-02-switch-wedge": ["Switch Wedge", "切换楔"],
    "d01-03-ts-cut": ["TS Cut", "TS 切口"],
    "d01-04-crossing-gate": ["Crossing Gate", "交叉闸"],
    "d01-05-signal-stitch": ["Signal Stitch", "信号缝合"],
    "d02-01-loopback-aperture": ["Loopback Aperture", "回环孔径"],
    "d02-02-private-port": ["Private Port", "私有端口"],
    "d02-03-contained-flow": ["Contained Flow", "边界内流"],
    "d02-04-boundary-slot": ["Boundary Slot", "边界槽"],
    "d02-05-kernel-fold": ["Kernel Fold", "内核折带"],
    "d03-01-square-crossover": ["Square Crossover", "方核换轨"],
    "d03-02-offset-exchange": ["Offset Exchange", "偏置交换"],
    "d03-03-relay-pair": ["Relay Pair", "中继对"],
    "d03-04-platform-transfer": ["Platform Transfer", "平台转接"],
    "d03-05-fallback-switch": ["Fallback Switch", "回退切换"],
    "d04-01-perforated-unit": ["Perforated Unit", "穿孔单元"],
    "d04-02-offset-cartridges": ["Offset Cartridges", "偏置卡匣"],
    "d04-03-slot-register": ["Slot Register", "槽位寄存"],
    "d04-04-packet-relay": ["Packet Relay", "数据包中继"],
    "d04-05-meter-cut": ["Meter Cut", "计量切口"],
    "d05-01-signal-break": ["Signal Break", "信号断口"],
    "d05-02-gate-latch": ["Gate Latch", "闸门卡扣"],
    "d05-03-detent-lever": ["Detent Lever", "定位拨杆"],
    "d05-04-relay-flag": ["Relay Flag", "中继旗标"],
    "d05-05-quiet-beacon": ["Quiet Beacon", "静默信标"],
  },
};

const SHORTLIST = [
  {
    code: "D05-05",
    role: "Visual Champion",
    rationale: ["Most distinctive silhouette;", "strongest app-icon presence."],
  },
  {
    code: "D03-05",
    role: "Semantic Champion",
    rationale: ["Clearest routing and fallback", "metaphor in the final set."],
  },
  {
    code: "D05-04",
    role: "Industrial Control",
    rationale: ["Precise control language with", "an asymmetric signal hierarchy."],
  },
  {
    code: "D04-01",
    role: "Infrastructure",
    rationale: ["Strong modular and system-level", "infrastructure character."],
  },
  {
    code: "D01-05",
    role: "Migration Path",
    rationale: ["Closest bridge from the current", "ink-and-red visual identity."],
  },
];

function escapeXml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function titleCase(slug) {
  return slug
    .split("-")
    .map((word) => word.length <= 2 ? word.toUpperCase() : word[0].toUpperCase() + word.slice(1))
    .join(" ");
}

function dataUri(path) {
  return `data:image/png;base64,${readFileSync(path).toString("base64")}`;
}

function inspectPng(path) {
  const buffer = readFileSync(path);
  const signature = "89504e470d0a1a0a";
  if (buffer.subarray(0, 8).toString("hex") !== signature) {
    throw new Error(`Not a PNG: ${path}`);
  }
  const width = buffer.readUInt32BE(16);
  const height = buffer.readUInt32BE(20);
  const colorType = buffer[25];
  const hasTrns = buffer.includes(Buffer.from("tRNS"));
  const hasAlpha = colorType === 4 || colorType === 6 || hasTrns;
  return { width, height, colorType, hasAlpha };
}

function makeDarkVariants(directions) {
  const darkRoot = join(ROOT, "dark-variants");
  const diagnostics = [];
  mkdirSync(darkRoot, { recursive: true });

  for (const direction of directions) {
    const targetDir = join(darkRoot, direction.folder);
    mkdirSync(targetDir, { recursive: true });
    for (const asset of direction.assets) {
      const sourceSvg = readFileSync(asset.svgPath, "utf8");
      const matches = sourceSvg.match(/#(?:15181B|182133)/gi) ?? [];
      if (matches.length === 0) {
        throw new Error(`${asset.key}.svg: no supported primary ink color to adapt`);
      }
      const darkSvg = sourceSvg.replace(/#(?:15181B|182133)/gi, "#EEF2F8");
      if (/#(?:15181B|182133)/i.test(darkSvg)) {
        throw new Error(`${asset.key}.svg: primary ink replacement was incomplete`);
      }
      const darkSvgPath = join(targetDir, `${asset.key}.svg`);
      const darkPngPath = join(targetDir, `${asset.key}.png`);
      writeFileSync(darkSvgPath, darkSvg);
      execFileSync("rsvg-convert", [
        "-w", "1024",
        "-h", "1024",
        "-o", darkPngPath,
        darkSvgPath,
      ]);
      const png = inspectPng(darkPngPath);
      if (png.width !== 1024 || png.height !== 1024 || !png.hasAlpha) {
        throw new Error(`${asset.key}.png dark variant failed size/alpha validation`);
      }
      asset.darkSvgPath = darkSvgPath;
      asset.darkPngPath = darkPngPath;
      diagnostics.push(`${direction.folder}/${asset.key}  ink→#EEF2F8  ${png.width}×${png.height} alpha=yes`);
    }
  }
  return diagnostics;
}

function discoverAssets() {
  const directoryNames = readdirSync(ROOT, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && /^direction-\d{2}-/.test(entry.name))
    .map((entry) => entry.name);

  const directions = [];
  const diagnostics = [];
  for (let number = 1; number <= 5; number += 1) {
    const d = String(number).padStart(2, "0");
    const populated = directoryNames
      .filter((name) => name.startsWith(`direction-${d}-`))
      .map((name) => {
        const dir = join(ROOT, name);
        const files = readdirSync(dir);
        const svgs = files.filter((file) => new RegExp(`^d${d}-\\d{2}-.+\\.svg$`).test(file)).sort();
        const pngs = files.filter((file) => new RegExp(`^d${d}-\\d{2}-.+\\.png$`).test(file)).sort();
        return { name, dir, svgs, pngs };
      })
      .filter((item) => item.svgs.length || item.pngs.length);

    if (populated.length !== 1) {
      throw new Error(`D${d}: expected exactly one populated direction folder, found ${populated.length}`);
    }
    const current = populated[0];
    if (current.svgs.length !== 5 || current.pngs.length !== 5) {
      throw new Error(`D${d}: expected 5 SVG + 5 PNG, found ${current.svgs.length} SVG + ${current.pngs.length} PNG`);
    }

    const assets = current.svgs.map((svgFile, index) => {
      const key = basename(svgFile, ".svg");
      const expectedPrefix = `d${d}-${String(index + 1).padStart(2, "0")}-`;
      if (!key.startsWith(expectedPrefix)) {
        throw new Error(`D${d}: unexpected sequence at ${svgFile}`);
      }
      const pngFile = `${key}.png`;
      if (!current.pngs.includes(pngFile)) {
        throw new Error(`Missing PNG pair for ${svgFile}`);
      }
      const svgPath = join(current.dir, svgFile);
      const pngPath = join(current.dir, pngFile);
      const png = inspectPng(pngPath);
      if (png.width !== 1024 || png.height !== 1024 || !png.hasAlpha) {
        throw new Error(`${pngFile}: expected 1024×1024 with alpha, got ${png.width}×${png.height}, alpha=${png.hasAlpha}`);
      }
      const slug = key.replace(/^d\d{2}-\d{2}-/, "");
      const names = COPY.concepts[key] ?? [titleCase(slug), `方向 ${d} 候选 ${index + 1}`];
      diagnostics.push(`${key}.png  ${png.width}×${png.height}  alpha=yes`);
      return {
        code: `D${d}-${String(index + 1).padStart(2, "0")}`,
        key,
        svgPath,
        pngPath,
        english: names[0],
        chinese: names[1],
      };
    });

    const dirSlug = current.name.replace(/^direction-\d{2}-/, "");
    const directionNames = COPY.directions[d] ?? [titleCase(dirSlug), `方向 ${d}`];
    directions.push({
      number: d,
      folder: current.name,
      english: directionNames[0],
      chinese: directionNames[1],
      assets,
    });
  }
  return { directions, diagnostics };
}

function svgOpen(width, height, background) {
  return `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">
  <rect width="${width}" height="${height}" fill="${background}"/>
  <style>
    text { font-family: Inter, "IBM Plex Sans", "PingFang SC", "Noto Sans CJK SC", sans-serif; }
    .mono { font-family: "SFMono-Regular", "IBM Plex Mono", monospace; letter-spacing: 1.4px; }
  </style>`;
}

function renderBoard(svg, outputPath, width, height) {
  execFileSync("rsvg-convert", ["-w", String(width), "-h", String(height), "-o", outputPath], {
    input: svg,
    maxBuffer: 64 * 1024 * 1024,
  });
}

function makeOverview(directions, theme) {
  const isDark = theme === "dark";
  const width = 2048;
  const height = 1760;
  const background = isDark ? DARK : LIGHT;
  const text = isDark ? "#EDF1F7" : "#182133";
  const muted = isDark ? "#929CAC" : "#667185";
  const rule = isDark ? "#293241" : "#DCE2EB";
  const panel = isDark ? "#151B26" : "#FFFFFF";
  const margin = 56;
  const gridX = 316;
  const colGap = 14;
  const cellWidth = 324;
  const rowTop = 128;
  const rowHeight = 296;
  const rowGap = 14;
  let svg = svgOpen(width, height, background);
  svg += `
  <text x="${margin}" y="52" font-size="28" font-weight="650" fill="${text}">Token Station Logo Exploration</text>
  <text x="${margin}" y="84" font-size="15" fill="${muted}">5 directions × 5 candidates · ${isDark ? "Dark adaptive variant · geometry unchanged ·" : "Light source artwork ·"} stage ${background}</text>
  <text x="${width - margin}" y="52" text-anchor="end" font-size="14" class="mono" fill="${muted}">${isDark ? "DARK REVIEW" : "LIGHT REVIEW"}</text>`;

  for (let row = 0; row < directions.length; row += 1) {
    const direction = directions[row];
    const y = rowTop + row * (rowHeight + rowGap);
    svg += `
  <line x1="${margin}" y1="${y}" x2="${width - margin}" y2="${y}" stroke="${rule}"/>
  <text x="${margin}" y="${y + 47}" font-size="13" class="mono" fill="${muted}">D${direction.number}</text>
  <text x="${margin}" y="${y + 79}" font-size="23" font-weight="650" fill="${text}">${escapeXml(direction.english)}</text>
  <text x="${margin}" y="${y + 107}" font-size="16" fill="${muted}">${escapeXml(direction.chinese)}</text>`;

    for (let col = 0; col < direction.assets.length; col += 1) {
      const asset = direction.assets[col];
      const x = gridX + col * (cellWidth + colGap);
      const imageSize = 168;
      const imageX = x + (cellWidth - imageSize) / 2;
      const imageY = y + 22;
      svg += `
  <rect x="${x}" y="${y + 10}" width="${cellWidth}" height="${rowHeight - 20}" rx="10" fill="${panel}" stroke="${rule}"/>
  <image href="${dataUri(isDark ? asset.darkPngPath : asset.pngPath)}" x="${imageX}" y="${imageY}" width="${imageSize}" height="${imageSize}" preserveAspectRatio="xMidYMid meet"/>
  <text x="${x + 20}" y="${y + 216}" font-size="12" class="mono" fill="${muted}">${asset.code}</text>
  <text x="${x + 20}" y="${y + 242}" font-size="17" font-weight="620" fill="${text}">${escapeXml(asset.english)}</text>
  <text x="${x + 20}" y="${y + 266}" font-size="14" fill="${muted}">${escapeXml(asset.chinese)}</text>`;
    }
  }
  svg += `
</svg>`;
  return { svg, width, height };
}

function renderNativePng(svgPath, size) {
  return execFileSync("rsvg-convert", ["-w", String(size), "-h", String(size), svgPath], {
    maxBuffer: 4 * 1024 * 1024,
  });
}

function makeSmallSizeBoard(directions) {
  const width = 2400;
  const height = 2048;
  const margin = 56;
  const headerHeight = 148;
  const cardWidth = 448;
  const cardHeight = 350;
  const gap = 12;
  const panelWidth = 188;
  const panelHeight = 184;
  const panelGap = 14;
  const panelOffset = (cardWidth - panelWidth * 2 - panelGap) / 2;
  const allAssets = directions.flatMap((direction) => direction.assets);
  const native = new Map();

  let defs = "";
  for (const asset of allAssets) {
    for (const variant of ["light", "dark"]) {
      const svgPath = variant === "dark" ? asset.darkSvgPath : asset.svgPath;
      for (const size of [64, 32, 16]) {
        const buffer = renderNativePng(svgPath, size);
        const id = `${variant}-${asset.key}-${size}`;
        native.set(`${variant}-${asset.key}-${size}`, id);
        defs += `<image id="${id}" width="${size}" height="${size}" href="data:image/png;base64,${buffer.toString("base64")}"/>`;
      }
    }
  }

  let svg = svgOpen(width, height, "#E9EDF4");
  svg += `
  <defs>${defs}</defs>
  <text x="${margin}" y="52" font-size="28" font-weight="650" fill="#182133">Token Station Small-size QA</text>
  <text x="${margin}" y="84" font-size="15" fill="#667185">Light source + dark adaptive SVGs are rasterized natively at 64 / 32 / 16 px, then embedded at 1:1.</text>
  <text x="${width - margin}" y="52" text-anchor="end" font-size="14" class="mono" fill="#667185">25 MARKS · TRUE-SIZE</text>`;

  for (let index = 0; index < allAssets.length; index += 1) {
    const asset = allAssets[index];
    const col = index % 5;
    const row = Math.floor(index / 5);
    const x = margin + col * (cardWidth + gap);
    const y = headerHeight + row * (cardHeight + gap);
    svg += `
  <rect x="${x}" y="${y}" width="${cardWidth}" height="${cardHeight}" rx="10" fill="#FFFFFF" stroke="#D5DCE7"/>
  <text x="${x + 22}" y="${y + 31}" font-size="12" class="mono" fill="#667185">${asset.code}</text>
  <text x="${x + 22}" y="${y + 58}" font-size="17" font-weight="650" fill="#182133">${escapeXml(asset.english)}</text>
  <text x="${x + cardWidth - 22}" y="${y + 57}" text-anchor="end" font-size="14" fill="#667185">${escapeXml(asset.chinese)}</text>`;

    const panelY = y + 82;
    for (let panelIndex = 0; panelIndex < 2; panelIndex += 1) {
      const isDark = panelIndex === 1;
      const panelX = x + panelOffset + panelIndex * (panelWidth + panelGap);
      const panelBg = isDark ? DARK : LIGHT;
      const panelText = isDark ? "#AAB3C1" : "#667185";
      svg += `
  <rect x="${panelX}" y="${panelY}" width="${panelWidth}" height="${panelHeight}" rx="7" fill="${panelBg}"/>
  <text x="${panelX + 12}" y="${panelY + 20}" font-size="10" class="mono" fill="${panelText}">${isDark ? "DARK" : "LIGHT"}</text>`;
      const positions = [
        { size: 64, x: panelX + 14, y: panelY + 42 },
        { size: 32, x: panelX + 101, y: panelY + 74 },
        { size: 16, x: panelX + 158, y: panelY + 90 },
      ];
      for (const position of positions) {
        const variant = isDark ? "dark" : "light";
        const id = native.get(`${variant}-${asset.key}-${position.size}`);
        svg += `
  <use href="#${id}" x="${position.x}" y="${position.y}" width="${position.size}" height="${position.size}"/>
  <text x="${position.x + position.size / 2}" y="${panelY + 132}" text-anchor="middle" font-size="10" class="mono" fill="${panelText}">${position.size}</text>`;
      }
    }
    svg += `
  <text x="${x + 22}" y="${y + 298}" font-size="11" class="mono" fill="#8A94A5">NATIVE RASTER · NO UPSCALE</text>
  <line x1="${x + 22}" y1="${y + 318}" x2="${x + cardWidth - 22}" y2="${y + 318}" stroke="#E1E6EE"/>
  <text x="${x + 22}" y="${y + 338}" font-size="11" fill="#667185">64 px</text>
  <text x="${x + 92}" y="${y + 338}" font-size="11" fill="#667185">32 px</text>
  <text x="${x + 162}" y="${y + 338}" font-size="11" fill="#667185">16 px</text>`;
  }
  svg += `
</svg>`;
  return { svg, width, height };
}

function makeShortlistBoard(directions) {
  const width = 2200;
  const height = 1000;
  const margin = 56;
  const cardTop = 144;
  const cardWidth = 406;
  const cardHeight = 790;
  const cardGap = 14;
  const panelWidth = 176;
  const panelHeight = 330;
  const panelGap = 14;
  const panelInset = 20;
  const allAssets = directions.flatMap((direction) => direction.assets);
  const byCode = new Map(allAssets.map((asset) => [asset.code, asset]));
  const ranked = SHORTLIST.map((entry) => {
    const asset = byCode.get(entry.code);
    if (!asset) throw new Error(`Shortlist asset not found: ${entry.code}`);
    return { ...entry, asset };
  });

  let defs = "";
  const native = new Map();
  for (const { asset } of ranked) {
    for (const variant of ["light", "dark"]) {
      const path = variant === "dark" ? asset.darkSvgPath : asset.svgPath;
      for (const size of [128, 32]) {
        const buffer = renderNativePng(path, size);
        const id = `shortlist-${variant}-${asset.key}-${size}`;
        native.set(`${variant}-${asset.key}-${size}`, id);
        defs += `<image id="${id}" width="${size}" height="${size}" href="data:image/png;base64,${buffer.toString("base64")}"/>`;
      }
    }
  }

  let svg = svgOpen(width, height, "#E9EDF4");
  svg += `
  <defs>${defs}</defs>
  <text x="${margin}" y="52" font-size="30" font-weight="680" fill="#182133">Token Station — Concept Shortlist</text>
  <text x="${margin}" y="86" font-size="16" fill="#667185">concept shortlist — refinement required</text>
  <text x="${width - margin}" y="52" text-anchor="end" font-size="13" class="mono" fill="#667185">5 CANDIDATES · ORDERED</text>
  <line x1="${margin}" y1="116" x2="${width - margin}" y2="116" stroke="#D5DCE7"/>`;

  for (let index = 0; index < ranked.length; index += 1) {
    const { asset, role, rationale } = ranked[index];
    const x = margin + index * (cardWidth + cardGap);
    const y = cardTop;
    const isChampion = index === 0;
    const border = isChampion ? "#D89A2B" : "#D5DCE7";
    const rankColor = isChampion ? "#B9790B" : "#667185";
    const roleColor = isChampion ? "#9B690C" : "#526078";
    svg += `
  <rect x="${x}" y="${y}" width="${cardWidth}" height="${cardHeight}" rx="12" fill="${isChampion ? "#FFFCF5" : "#FFFFFF"}" stroke="${border}" stroke-width="${isChampion ? 2 : 1}"/>
  ${isChampion ? `<rect x="${x + 2}" y="${y + 2}" width="${cardWidth - 4}" height="4" rx="2" fill="#D89A2B"/>` : ""}
  <text x="${x + 22}" y="${y + 40}" font-size="25" font-weight="700" fill="${rankColor}">${String(index + 1).padStart(2, "0")}</text>
  <text x="${x + cardWidth - 22}" y="${y + 38}" text-anchor="end" font-size="12" class="mono" fill="#7A8598">${asset.code}</text>
  <text x="${x + 22}" y="${y + 82}" font-size="22" font-weight="680" fill="#182133">${escapeXml(asset.english)}</text>
  <text x="${x + 22}" y="${y + 108}" font-size="15" fill="#667185">${escapeXml(asset.chinese)}</text>
  <text x="${x + 22}" y="${y + 145}" font-size="12" font-weight="700" class="mono" fill="${roleColor}">${escapeXml(role.toUpperCase())}</text>`;

    const panelY = y + 174;
    for (let panelIndex = 0; panelIndex < 2; panelIndex += 1) {
      const variant = panelIndex === 0 ? "light" : "dark";
      const isDark = variant === "dark";
      const panelX = x + panelInset + panelIndex * (panelWidth + panelGap);
      const panelBg = isDark ? DARK : LIGHT;
      const panelText = isDark ? "#AAB3C1" : "#667185";
      const mainId = native.get(`${variant}-${asset.key}-128`);
      const smallId = native.get(`${variant}-${asset.key}-32`);
      svg += `
  <rect x="${panelX}" y="${panelY}" width="${panelWidth}" height="${panelHeight}" rx="8" fill="${panelBg}"/>
  <text x="${panelX + 12}" y="${panelY + 22}" font-size="10" class="mono" fill="${panelText}">${isDark ? "DARK ADAPTIVE" : "LIGHT SOURCE"}</text>
  <use href="#${mainId}" x="${panelX + 24}" y="${panelY + 48}" width="128" height="128"/>
  <text x="${panelX + 88}" y="${panelY + 201}" text-anchor="middle" font-size="10" class="mono" fill="${panelText}">128 PX</text>
  <line x1="${panelX + 16}" y1="${panelY + 220}" x2="${panelX + panelWidth - 16}" y2="${panelY + 220}" stroke="${isDark ? "#2A3444" : "#DCE2EB"}"/>
  <use href="#${smallId}" x="${panelX + 72}" y="${panelY + 242}" width="32" height="32"/>
  <text x="${panelX + 88}" y="${panelY + 301}" text-anchor="middle" font-size="10" class="mono" fill="${panelText}">TRUE 32 PX</text>`;
    }

    svg += `
  <text x="${x + 22}" y="${y + 555}" font-size="11" class="mono" fill="#8A94A5">WHY SHORTLISTED</text>
  <text x="${x + 22}" y="${y + 589}" font-size="15" fill="#334056">${escapeXml(rationale[0])}</text>
  <text x="${x + 22}" y="${y + 614}" font-size="15" fill="#334056">${escapeXml(rationale[1])}</text>
  <line x1="${x + 22}" y1="${y + 681}" x2="${x + cardWidth - 22}" y2="${y + 681}" stroke="#E1E6EE"/>
  <text x="${x + 22}" y="${y + 718}" font-size="11" class="mono" fill="#7A8598">128 PX MAIN</text>
  <text x="${x + cardWidth - 22}" y="${y + 718}" text-anchor="end" font-size="11" class="mono" fill="#7A8598">32 PX QA</text>
  <text x="${x + 22}" y="${y + 757}" font-size="12" fill="#667185">Geometry unchanged across color modes</text>`;
  }

  svg += `
</svg>`;
  return { svg, width, height };
}

const { directions, diagnostics } = discoverAssets();
const darkDiagnostics = makeDarkVariants(directions);

const light = makeOverview(directions, "light");
renderBoard(light.svg, join(ROOT, "board-light.png"), light.width, light.height);

const dark = makeOverview(directions, "dark");
renderBoard(dark.svg, join(ROOT, "board-dark.png"), dark.width, dark.height);

const small = makeSmallSizeBoard(directions);
renderBoard(small.svg, join(ROOT, "small-size-qa.png"), small.width, small.height);

const shortlist = makeShortlistBoard(directions);
renderBoard(shortlist.svg, join(ROOT, "shortlist-board.png"), shortlist.width, shortlist.height);

console.log(`Generated ${join(ROOT, "board-light.png")}`);
console.log(`Generated ${join(ROOT, "board-dark.png")}`);
console.log(`Generated ${join(ROOT, "small-size-qa.png")}`);
console.log(`Generated ${join(ROOT, "shortlist-board.png")}`);
console.log(`Validated ${diagnostics.length} source PNG files:`);
console.log(diagnostics.join("\n"));
console.log(`Generated and validated ${darkDiagnostics.length} dark variant pairs:`);
console.log(darkDiagnostics.join("\n"));
