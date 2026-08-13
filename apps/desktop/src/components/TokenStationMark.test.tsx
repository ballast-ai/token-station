import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import TokenStationMark from "./TokenStationMark";

describe("TokenStationMark", () => {
  it("renders the shared wordless product mark as decorative artwork", () => {
    render(<TokenStationMark size={36} />);

    const mark = screen.getByTestId("token-station-mark");
    expect(screen.getByTestId("station-brand-icon")).toBeInTheDocument();
    expect(mark).toHaveAttribute("aria-hidden", "true");
    const image = mark.querySelector("img");
    expect(image).toHaveAttribute("src", "/icon.png");
    expect(image).toHaveAttribute("alt", "");
    expect(mark).not.toHaveTextContent("TS");
  });
});
