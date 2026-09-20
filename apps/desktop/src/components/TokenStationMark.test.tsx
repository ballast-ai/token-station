import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import TokenStationMark from "./TokenStationMark";
import LaunchScreen from "./LaunchScreen";

describe("TokenStationMark", () => {
  it("provides local light and dark artwork without exposing decorative images", () => {
    render(<TokenStationMark size={36} />);

    const mark = screen.getByTestId("token-station-mark");
    expect(mark).toHaveAttribute("aria-hidden", "true");
    expect(mark).toHaveStyle({ width: "36px", height: "36px" });
    const images = Array.from(mark.querySelectorAll("img"));
    expect(images.map((image) => image.getAttribute("src"))).toEqual([
      "/logo.svg",
      "/logo-dark.svg",
    ]);
    for (const image of images) expect(image).toHaveAttribute("alt", "");
    expect(mark).not.toHaveTextContent("TS");
  });

  it("uses optical artwork at small menu and toolbar sizes", () => {
    render(<TokenStationMark size={20} />);
    const sources = Array.from(screen.getByTestId("token-station-mark").querySelectorAll("img"))
      .map((image) => image.getAttribute("src"));
    expect(sources).toEqual(["/logo-small.svg", "/logo-small-dark.svg"]);
  });

  it("keeps the launch status and exit timing while showing the shared product mark", () => {
    render(<LaunchScreen phase="exiting" exitDurationMs={300} />);
    expect(screen.getByRole("status", { name: "Opening Token Station" }))
      .toHaveAttribute("data-phase", "exiting");
    expect(screen.getByTestId("launch-screen"))
      .toHaveStyle({ "--launch-exit-duration": "300ms" });
    expect(screen.getByTestId("token-station-mark"))
      .toHaveStyle({ width: "100%", height: "100%" });
  });
});
