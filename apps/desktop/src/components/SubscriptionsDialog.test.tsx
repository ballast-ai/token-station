import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { LanguageProvider } from "./LanguageProvider";
import SubscriptionsDialog from "./SubscriptionsDialog";

vi.mock("./LocalCodexPanel", () => ({ default: () => <div>Existing Codex controller</div> }));
vi.mock("./ChatGPTPlanPanel", () => ({ default: () => <div>Existing ChatGPT API controller</div> }));
vi.mock("./LocalSubscriptionPanel", () => ({ default: ({ kind }: { kind: string }) => <div>Native controller: {kind}</div> }));

describe("Subscription connection window", () => {
  it("shows the six requested channels in the requested order", () => {
    render(<LanguageProvider><SubscriptionsDialog open onClose={vi.fn()} onChanged={vi.fn()} onConnected={vi.fn()} /></LanguageProvider>);
    const list = screen.getByRole("group", { name: "订阅渠道" });
    expect(within(list).getAllByRole("button").map(button => button.getAttribute("aria-label")))
      .toEqual(["Claude", "ChatGPT", "ChatGPT API", "Copilot", "Gemini CLI", "Antigravity"]);
  });

  it.each([
    ["ChatGPT", "Existing Codex controller"],
    ["ChatGPT API", "Existing ChatGPT API controller"],
    ["Claude", "Native controller: claude"],
    ["Copilot", "Native controller: copilot"],
    ["Gemini CLI", "Native controller: gemini-cli"],
    ["Antigravity", "Native controller: antigravity"],
  ])("opens %s and returns to the channel chooser", async (label, expected) => {
    const user = userEvent.setup();
    render(<LanguageProvider><SubscriptionsDialog open onClose={vi.fn()} onChanged={vi.fn()} onConnected={vi.fn()} /></LanguageProvider>);
    await user.click(screen.getByRole("button", { name: label }));
    expect(screen.getByRole("dialog", { name: label })).toHaveTextContent(expected);
    await user.click(screen.getByRole("button", { name: "返回订阅" }));
    expect(screen.getByRole("dialog", { name: "订阅" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: label })).toHaveFocus();
  });
});
