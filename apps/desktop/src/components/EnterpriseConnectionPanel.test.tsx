import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import EnterpriseConnectionPanel from "./EnterpriseConnectionPanel";

const liveDiscovery = {
  models: ["enterprise-chat", "enterprise-reasoner"],
  source: "live" as const,
  fetched_at_ms: 1,
  warning: null,
};

describe("EnterpriseConnectionPanel", () => {
  it("keeps every discovered model selectable for the submitted endpoint", async () => {
    const user = userEvent.setup();
    const onVerify = vi.fn().mockResolvedValue(liveDiscovery);
    const onConnect = vi.fn().mockResolvedValue(true);
    render(
      <EnterpriseConnectionPanel
        busy={false}
        onVerify={onVerify}
        onConnect={onConnect}
      />,
    );

    expect(screen.getByText("Token-station")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Base URL" })).toHaveValue("");
    expect(screen.getByRole("textbox", { name: "Base URL" })).toBeEnabled();
    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "replacement-key");
    await user.click(screen.getByRole("button", { name: "验证并获取模型" }));

    expect(await screen.findByRole("radio", { name: "enterprise-reasoner" })).toBeEnabled();
    expect(screen.getByRole("radio", { name: "enterprise-chat" })).toBeEnabled();
    await user.click(screen.getByRole("radio", { name: "enterprise-chat" }));
    await user.click(screen.getByRole("button", { name: "添加并使用" }));

    await waitFor(() => expect(onConnect).toHaveBeenCalledWith({
      baseUrl: "https://api.example.com/v1",
      apiKey: "replacement-key",
      model: "enterprise-chat",
    }));
  });

  it("requires endpoint verification and explicit model selection", async () => {
    const user = userEvent.setup();
    const onVerify = vi.fn().mockResolvedValue(liveDiscovery);
    const onConnect = vi.fn().mockResolvedValue(true);
    render(
      <EnterpriseConnectionPanel busy={false} onVerify={onVerify} onConnect={onConnect} />,
    );

    expect(screen.getByText("Token-station")).toBeInTheDocument();
    expect(screen.queryByRole("radiogroup", { name: "模型" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "验证并获取模型" }));
    expect(screen.getByRole("status")).toHaveTextContent("请填写 Base URL 和 API Key");

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.click(screen.getByRole("button", { name: "验证并获取模型" }));
    await waitFor(() => expect(onVerify).toHaveBeenCalledWith({
      baseUrl: "https://api.example.com/v1",
      apiKey: "secret-key",
    }));
    expect(screen.getByRole("radiogroup", { name: "模型" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "enterprise-chat" })).not.toBeChecked();
    expect(screen.getByRole("radio", { name: "enterprise-reasoner" })).not.toBeChecked();
    expect(screen.getByRole("button", { name: "添加并使用" })).toBeDisabled();

    await user.click(screen.getByRole("radio", { name: "enterprise-chat" }));
    await user.click(screen.getByRole("radio", { name: "enterprise-reasoner" }));
    expect(screen.getByRole("radio", { name: "enterprise-chat" })).not.toBeChecked();
    expect(screen.getByRole("radio", { name: "enterprise-reasoner" })).toBeChecked();
    expect(screen.getByRole("button", { name: "添加并使用" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "添加并使用" }));
    await waitFor(() => expect(onConnect).toHaveBeenCalledWith({
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
    expect(await screen.findByRole("radiogroup", { name: "模型" })).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "/next");
    expect(screen.queryByRole("radiogroup", { name: "模型" })).toBeNull();
  });
});
