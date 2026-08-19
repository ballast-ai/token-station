import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderView, StateView } from "../api";
import EnterpriseConnectionPanel from "./EnterpriseConnectionPanel";

const { discoverProviderModels, addProvider } = vi.hoisted(() => ({
  discoverProviderModels: vi.fn(),
  addProvider: vi.fn(),
}));

vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return { ...actual, discoverProviderModels, addProvider };
});

const connectedProvider: ProviderView = {
  name: "enterprise-api-example-com",
  provider: "openai-compatible",
  base_url: "https://api.example.com/v1",
  models: ["enterprise-chat"],
  has_auth: true,
};

beforeEach(() => {
  discoverProviderModels.mockReset();
  addProvider.mockReset();
});

describe("EnterpriseConnectionPanel", () => {
  it("requires both an endpoint and API key before verification", async () => {
    const user = userEvent.setup();
    render(
      <EnterpriseConnectionPanel
        providers={[]}
        busy={false}
        onConnected={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "验证连接" }));
    expect(screen.getByRole("status")).toHaveTextContent("请填写 Base URL 和 API Key");
    expect(discoverProviderModels).not.toHaveBeenCalled();
  });

  it("verifies credentials, lets the user choose models, and stores only the selection", async () => {
    const user = userEvent.setup();
    const onConnected = vi.fn();
    discoverProviderModels.mockResolvedValue({
      models: ["enterprise-chat", "enterprise-reasoner"],
      source: "live",
      fetched_at_ms: 1,
      warning: null,
    });
    addProvider.mockResolvedValue({ providers: [connectedProvider] } as StateView);

    render(
      <EnterpriseConnectionPanel
        providers={[]}
        busy={false}
        onConnected={onConnected}
      />,
    );

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.click(screen.getByRole("button", { name: "验证连接" }));

    expect(await screen.findByRole("checkbox", { name: "enterprise-chat" })).not.toBeChecked();
    expect(screen.getByRole("checkbox", { name: "enterprise-reasoner" })).not.toBeChecked();
    await user.click(screen.getByRole("checkbox", { name: "enterprise-chat" }));
    await user.click(screen.getByRole("button", { name: "接入所选模型" }));

    await waitFor(() => expect(addProvider).toHaveBeenCalledWith(
      "enterprise-api-example-com",
      "https://api.example.com/v1",
      ["enterprise-chat"],
      "secret-key",
      false,
      "store",
      null,
      "openai-compatible",
    ));
    expect(onConnected).toHaveBeenCalledWith(
      expect.objectContaining({ providers: [connectedProvider] }),
      "enterprise-api-example-com",
      ["enterprise-chat"],
    );
    expect(screen.getByLabelText("API Key")).toHaveValue("");
  });
});
