import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import type { ProviderView } from "../api";
import EnterpriseConnectionPanel from "./EnterpriseConnectionPanel";

const connectedProvider: ProviderView = {
  name: "enterprise-api-example-com",
  provider: "openai-compatible",
  base_url: "https://api.example.com/v1",
  models: ["auto"],
  has_auth: true,
};

describe("EnterpriseConnectionPanel", () => {
  it("shows three connection fields and one managed-route action", () => {
    render(
      <EnterpriseConnectionPanel
        providers={[connectedProvider]}
        busy={false}
        onConnect={vi.fn()}
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
        onConnect={vi.fn()}
      />,
    );

    await user.click(screen.getByRole("button", { name: "接入并使用" }));
    expect(screen.getByRole("status")).toHaveTextContent("请填写 Base URL 和 API Key");
  });

  it("submits a backend-valid derived account name and reports apply as pending", async () => {
    const user = userEvent.setup();
    const onConnect = vi.fn().mockResolvedValue(true);

    render(
      <EnterpriseConnectionPanel
        providers={[]}
        busy={false}
        onConnect={onConnect}
      />,
    );

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.click(screen.getByRole("button", { name: "接入并使用" }));

    await waitFor(() => expect(onConnect).toHaveBeenCalledWith({
      name: "enterprise_api_example_com",
      baseUrl: "https://api.example.com/v1",
      apiKey: "secret-key",
    }));
    expect(screen.queryByRole("checkbox")).toBeNull();
    expect(screen.getByLabelText("API Key")).toHaveValue("");
    expect(screen.getByRole("status")).toHaveTextContent("企业路由已接入，正在应用配置");
  });

  it("rejects an explicit duplicate account name before verification", async () => {
    const user = userEvent.setup();
    render(
      <EnterpriseConnectionPanel
        providers={[connectedProvider]}
        busy={false}
        onConnect={vi.fn()}
      />,
    );

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.type(screen.getByRole("textbox", { name: "账户名称" }), connectedProvider.name);
    await user.click(screen.getByRole("button", { name: "接入并使用" }));

    expect(screen.getByRole("status")).toHaveTextContent("该账户名称已存在");
  });

  it("keeps credentials available for retry when the parent workflow fails", async () => {
    const user = userEvent.setup();
    const onConnect = vi.fn().mockResolvedValue(false);

    render(
      <EnterpriseConnectionPanel
        providers={[]}
        busy={false}
        onConnect={onConnect}
      />,
    );

    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.click(screen.getByRole("button", { name: "接入并使用" }));

    expect(await screen.findByRole("status")).toHaveTextContent("接入未完成");
    expect(screen.getByRole("textbox", { name: "Base URL" })).toHaveValue("https://api.example.com/v1");
    expect(screen.getByLabelText("API Key")).toHaveValue("secret-key");
  });
});
