import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ProviderView } from "../api";
import EnterpriseConnectionPanel from "./EnterpriseConnectionPanel";

const connectedProvider: ProviderView = {
  name: "tokenstation",
  provider: "openai-compatible",
  base_url: "https://api.example.com/v1",
  models: ["enterprise-reasoner"],
  has_auth: true,
  managed_route: true,
};

const liveDiscovery = {
  models: ["enterprise-chat", "enterprise-reasoner"],
  source: "live" as const,
  fetched_at_ms: 1,
  warning: null,
};

describe("EnterpriseConnectionPanel", () => {
  it("shows the existing fixed provider without asking for its credential again", () => {
    render(
      <EnterpriseConnectionPanel
        existingProvider={connectedProvider}
        busy={false}
        onVerify={vi.fn()}
        onConnect={vi.fn()}
      />,
    );

    expect(screen.getByText("Token-station")).toBeInTheDocument();
    expect(screen.getByText("https://api.example.com/v1")).toBeInTheDocument();
    expect(screen.queryByLabelText("API Key")).toBeNull();
  });

  it("requires endpoint verification and explicit model selection", async () => {
    const user = userEvent.setup();
    const onVerify = vi.fn().mockResolvedValue(liveDiscovery);
    const onConnect = vi.fn().mockResolvedValue(true);
    render(
      <EnterpriseConnectionPanel busy={false} onVerify={onVerify} onConnect={onConnect} />,
    );

    expect(screen.getByText("Token-station")).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "模型" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "验证并获取模型" }));
    expect(screen.getByRole("status")).toHaveTextContent("请填写 Base URL 和 API Key");

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.click(screen.getByRole("button", { name: "验证并获取模型" }));
    await waitFor(() => expect(onVerify).toHaveBeenCalledWith({
      name: "tokenstation",
      baseUrl: "https://api.example.com/v1",
      apiKey: "secret-key",
    }));
    expect(screen.getByRole("combobox", { name: "模型" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "添加并使用" })).toBeDisabled();

    await user.selectOptions(screen.getByRole("combobox", { name: "模型" }), "enterprise-reasoner");
    await user.click(screen.getByRole("button", { name: "添加并使用" }));
    await waitFor(() => expect(onConnect).toHaveBeenCalledWith({
      name: "tokenstation",
      baseUrl: "https://api.example.com/v1",
      apiKey: "secret-key",
      model: "enterprise-reasoner",
    }));
  });

  it("invalidates verified models when the endpoint or credential changes", async () => {
    const user = userEvent.setup();
    render(
      <EnterpriseConnectionPanel
        busy={false}
        onVerify={vi.fn().mockResolvedValue(liveDiscovery)}
        onConnect={vi.fn()}
      />,
    );

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.click(screen.getByRole("button", { name: "验证并获取模型" }));
    expect(await screen.findByRole("combobox", { name: "模型" })).toBeEnabled();
    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "/next");
    expect(screen.getByRole("combobox", { name: "模型" })).toBeDisabled();
  });
});
