import { createElement, type ComponentType, type SVGProps } from "react";
import { readFileSync } from "node:fs";
import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import * as icons from "./localBrandIcons";
import provenance from "./assets/brand-icons-provenance.json";

type Glyph = ComponentType<SVGProps<SVGSVGElement> & { size: number }>;
type Brand = Glyph & { Color?: Glyph; Avatar: ComponentType<{ size: number }> };

function pathData(element: ParentNode) {
  return [...element.querySelectorAll("path")].map(path => path.getAttribute("d"));
}

function pinnedPaths(file: string, local = false) {
  const location = local ? `./assets/${file}` : `../node_modules/@lobehub/icons-static-svg/${file}`;
  const source = readFileSync(new URL(location, import.meta.url), "utf8");
  return pathData(new DOMParser().parseFromString(source, "image/svg+xml"));
}

describe("trusted brand artwork", () => {
  it("does not accept caller HTML in place of the pinned SVG artwork", () => {
    const { container } = render(createElement(icons.Gemini.Color, {
      size: 28,
      dangerouslySetInnerHTML: { __html: '<script data-untrusted="true">fixture</script>' },
    }));
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("svg path")).not.toBeNull();
  });

  it.each(provenance.brands)("preserves $name artwork, color availability, and avatar sizing", brand => {
    const icon = icons[brand.name as keyof typeof icons] as Brand;
    const { container, unmount } = render(createElement(icon, { size: 29, style: { color: "rgb(102, 51, 153)" }, className: "fixture" }));
    const glyph = container.querySelector("svg");
    expect(glyph).toHaveAttribute("fill", "currentColor");
    expect(glyph).toHaveAttribute("width", "29");
    expect(glyph).toHaveAttribute("height", "29");
    expect(glyph).toHaveStyle({ color: "rgb(102, 51, 153)" });
    expect(glyph).toHaveClass("fixture");
    expect(glyph?.querySelector("path")).not.toBeNull();
    expect(pathData(container)).toEqual(pinnedPaths(brand.mono));
    expect(Boolean(icon.Color)).toBe(Boolean(brand.color));
    unmount();
    const avatar = render(createElement(icon.Avatar, { size: 32 }));
    expect(avatar.container.firstElementChild).toHaveStyle({ width: "32px", height: "32px", background: brand.background, color: brand.foreground });
    expect(avatar.container.querySelector("svg")).toHaveStyle({ transform: `scale(${brand.scale})` });
    expect(pathData(avatar.container)).toEqual(brand.avatar === "Inner"
      ? pinnedPaths(`${brand.slug}-avatar.svg`, true)
      : pinnedPaths(brand.avatar === "Color" ? brand.color! : brand.mono));
    avatar.unmount();
    if (icon.Color) {
      const colored = render(createElement(icon.Color, { size: 26 }));
      expect(colored.container.querySelector("svg path")).not.toBeNull();
      expect(pathData(colored.container)).toEqual(pinnedPaths(brand.color!));
    }
  });

  it("keeps gradient references local to each repeated artwork instance", () => {
    const { container } = render(<><icons.Gemini.Color size={28} /><icons.Gemini.Color size={28} /><icons.Codex.Avatar size={28} /></>);
    const identifiers = [...container.querySelectorAll("svg [id]")].map(element => element.id);
    expect(identifiers.length).toBeGreaterThan(1);
    expect(new Set(identifiers).size).toBe(identifiers.length);
    for (const glyph of container.querySelectorAll("svg")) {
      for (const element of glyph.querySelectorAll("[fill]")) {
        const identifier = element.getAttribute("fill")?.match(/^url\(#(.+)\)$/)?.[1];
        if (identifier) expect([...glyph.querySelectorAll("[id]")].some(local => local.id === identifier)).toBe(true);
      }
    }
  });

  it("locks the SVG viewport while retaining caller accessibility and size overrides", () => {
    const { container } = render(<icons.Github size={18} viewBox="0 0 1 1" aria-label="Repository" width={25}><script>fixture</script></icons.Github>);
    expect(container.querySelector("svg")).toHaveAttribute("viewBox", "0 0 24 24");
    expect(container.querySelector("svg")).toHaveAttribute("aria-label", "Repository");
    expect(container.querySelector("svg")).toHaveAttribute("width", "25");
    expect(container.querySelector("script")).toBeNull();
  });

  it("retains specialized Codex gradient and Stepfun avatar paths", () => {
    const { container } = render(<><icons.Codex.Avatar size={32} /><icons.Stepfun.Avatar size={32} /></>);
    expect(container.querySelector("linearGradient stop")).toHaveAttribute("stop-color", "#B1A7FF");
    expect(container.querySelectorAll("svg")[1].querySelector("path")?.getAttribute("d")).toBe("M1 23h6.335v-6.337H1V23zM8.832 23h6.336v-6.337H8.832V23zM8.832 15.17h6.336V8.835H8.832v6.337zM8.832 7.342h6.336V1.005H8.832v6.337zM16.665 7.337H23V1h-6.335v6.337z");
  });
});
