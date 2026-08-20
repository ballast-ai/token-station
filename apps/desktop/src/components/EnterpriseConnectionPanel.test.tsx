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
  models: ["auto"],
  has_auth: true,
};

beforeEach(() => {
  discoverProviderModels.mockReset();
  addProvider.mockReset();
});

describe("EnterpriseConnectionPanel", () => {
  it("shows three connection fields and one managed-route action", () => {
    render(
      <EnterpriseConnectionPanel
        providers={[connectedProvider]}
        busy={false}
        onConnected={vi.fn()}
      />,
    );

    expect(screen.getByRole("textbox", { name: "Base URL" })).toBeInTheDocument();
    expect(screen.getByLabelText("API Key")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "账户名称" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "接入并使用" })).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "接入流程" })).toBeNull();
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.queryByText("enterprise-api-example-com")).toBeNull();
  });

  it("requires both an endpoint and API key before connection", async () => {
    const user = userEvent.setup();
    render(
      <EnterpriseConnectionPanel
        providers={[]}
        busy={false}
        onConnected={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "接入并使用" }));
    expect(screen.getByRole("status")).toHaveTextContent("请填写 Base URL 和 API Key");
    expect(discoverProviderModels).not.toHaveBeenCalled();
  });

  it.each([
    { discoveredModels: [] },
    { discoveredModels: ["enterprise-chat", "enterprise-reasoner"] },
  ])("uses auto without persisting the discovered model list $discoveredModels", async ({ discoveredModels }) => {
    const user = userEvent.setup();
    const onConnected = vi.fn().mockResolvedValue(true);
    discoverProviderModels.mockResolvedValue({
      models: discoveredModels,
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
    await user.click(screen.getByRole("button", { name: "接入并使用" }));

    await waitFor(() => expect(addProvider).toHaveBeenCalledWith(
      "enterprise-api-example-com",
      "https://api.example.com/v1",
      ["auto"],
      "secret-key",
      false,
      "store",
      null,
      "openai-compatible",
    ));
    expect(onConnected).toHaveBeenCalledWith(
      expect.objectContaining({ providers: [connectedProvider] }),
      "enterprise-api-example-com",
    );
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.getByLabelText("API Key")).toHaveValue("");
    expect(screen.getByRole("status")).toHaveTextContent("企业路由已接入并应用");
  });

  it("rejects an explicit duplicate account name before verification", async () => {
    const user = userEvent.setup();
    render(
      <EnterpriseConnectionPanel
        providers={[connectedProvider]}
        busy={false}
        onConnected={vi.fn()}
      />,
    );

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.type(screen.getByRole("textbox", { name: "账户名称" }), connectedProvider.name);
    await user.click(screen.getByRole("button", { name: "接入并使用" }));

    expect(screen.getByRole("status")).toHaveTextContent("该账户名称已存在");
    expect(discoverProviderModels).not.toHaveBeenCalled();
  });

  it("does not treat a cached discovery fallback as live credential verification", async () => {
    const user = userEvent.setup();
    discoverProviderModels.mockResolvedValue({
      models: ["enterprise-chat"],
      source: "cache",
      fetched_at_ms: 1,
      warning: "Provider rejected the API key",
    });

    render(
      <EnterpriseConnectionPanel
        providers={[]}
        busy={false}
        onConnected={vi.fn()}
      />,
    );

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "invalid-key");
    await user.click(screen.getByRole("button", { name: "接入并使用" }));

    expect(await screen.findByRole("status")).toHaveTextContent("凭据无法使用");
    expect(addProvider).not.toHaveBeenCalled();
  });
});
