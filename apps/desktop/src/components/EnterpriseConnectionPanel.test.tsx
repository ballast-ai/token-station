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
  it("shows the endpoint connection steps and current available endpoints", () => {
    render(
      <EnterpriseConnectionPanel
        providers={[connectedProvider]}
        busy={false}
        onConnected={vi.fn()}
      />,
    );

    const flow = screen.getByRole("list", { name: "接入流程" });
    for (const step of ["验证接口", "选择模型", "完成接入"]) {
      expect(flow).toHaveTextContent(step);
    }
    expect(screen.getByRole("region", { name: "可用端点" })).toHaveTextContent("enterprise-api-example-com");
    expect(screen.getByRole("region", { name: "可用端点" })).toHaveTextContent("1 个模型");
    expect(screen.queryByText("额度优先")).toBeNull();
    expect(screen.queryByRole("button", { name: "保存并应用" })).toBeNull();
  });

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

  it.each(["Base URL", "API Key", "账户名称"])(
    "invalidates verified models when %s changes",
    async (field) => {
      const user = userEvent.setup();
      discoverProviderModels.mockResolvedValue({
        models: ["enterprise-chat"],
        source: "live",
        fetched_at_ms: 1,
        warning: null,
      });

      render(
        <EnterpriseConnectionPanel
          providers={[]}
          busy={false}
          onConnected={vi.fn()}
        />,
      );

      await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://api.example.com/v1");
      await user.type(screen.getByLabelText("API Key"), "secret-key");
      await user.click(screen.getByRole("button", { name: "验证连接" }));
      expect(await screen.findByRole("checkbox", { name: "enterprise-chat" })).toBeInTheDocument();

      const input = screen.getByLabelText(field);
      if (field === "账户名称") {
        await user.type(input, "changed-account");
      } else {
        await user.clear(input);
        await user.type(input, field === "Base URL" ? "https://other.example.com/v1" : "other-key");
      }

      expect(screen.queryByRole("checkbox", { name: "enterprise-chat" })).not.toBeInTheDocument();
      expect(screen.queryByRole("button", { name: "接入所选模型" })).not.toBeInTheDocument();
    },
  );

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
    await user.click(screen.getByRole("button", { name: "验证连接" }));

    expect(await screen.findByRole("status")).toHaveTextContent("凭据无法使用");
    expect(screen.queryByRole("checkbox", { name: "enterprise-chat" })).not.toBeInTheDocument();
    expect(addProvider).not.toHaveBeenCalled();
  });
});
