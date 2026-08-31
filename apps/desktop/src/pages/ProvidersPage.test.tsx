import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import ProvidersPage from "./ProvidersPage";

const baseProps = {
  providers: [],
  deletedProviders: [],
  recoveryError: null,
  serveRunning: false,
  busy: false,
  onRemove: vi.fn(),
  onRestore: vi.fn(),
  onPurgeDeleted: vi.fn(),
  onStateChange: vi.fn(),
  onAddProvider: vi.fn(),
  onVerifyEnterprise: vi.fn().mockResolvedValue({
    models: ["enterprise-chat", "enterprise-reasoner"],
    source: "live" as const,
    fetched_at_ms: 1,
    warning: null,
  }),
  onConnectEnterprise: vi.fn().mockResolvedValue(true),
};

describe("ProvidersPage enterprise entry", () => {
  it("renders provider models as a compact responsive index", () => {
    render(
      <ProvidersPage
        {...baseProps}
        providers={[{
          name: "deepseek",
          provider: "openai-compatible",
          base_url: "https://api.deepseek.com/v1",
          models: ["deepseek-v4-flash", "deepseek-v4-flash-vision-exp", "deepseek-v4-pro"],
          has_auth: true,
          managed_route: false,
        }]}
      />,
    );

    const models = screen.getByRole("list", { name: "deepseek 模型" });
    expect(models).toHaveAttribute("data-layout", "compact-model-index");
    expect(within(models).getAllByRole("listitem")).toHaveLength(3);
  });

  it("shows stable display names for multiple managed channels", () => {
    render(
      <ProvidersPage
        {...baseProps}
        providers={[
          {
            name: "tokenstation",
            provider: "openai-compatible",
            base_url: "https://first.example/v1",
            models: ["first-model"],
            has_auth: true,
            managed_route: true,
          },
          {
            name: "tokenstation_2",
            provider: "openai-compatible",
            base_url: "https://second.example/v1",
            models: ["second-model"],
            has_auth: true,
            managed_route: true,
          },
        ]}
      />,
    );

    expect(screen.getByRole("group", { name: "Token-station 供应商" })).toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Token-station 2 供应商" })).toBeInTheDocument();
  });

  it("places Enterprise routing before Add model and requires explicit model selection", async () => {
    const user = userEvent.setup();
    const onConnectEnterprise = vi.fn().mockResolvedValue(true);
    render(<ProvidersPage {...baseProps} onConnectEnterprise={onConnectEnterprise} />);

    const enterprise = screen.getByRole("button", { name: "添加企业模型" });
    const addModel = screen.getByRole("button", { name: "添加模型" });
    expect(enterprise.compareDocumentPosition(addModel) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();

    await user.click(enterprise);
    const dialog = screen.getByRole("dialog", { name: "添加企业模型" });
    expect(within(dialog).getByText("Token-station")).toBeInTheDocument();
    await user.type(within(dialog).getByRole("textbox", { name: "Base URL" }), "https://enterprise.example.com/v1");
    await user.type(within(dialog).getByLabelText("API Key"), "secret-key");
    await user.click(within(dialog).getByRole("button", { name: "验证并获取模型" }));
    expect(await within(dialog).findByRole("radiogroup", { name: "模型" })).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "添加并使用" })).toBeDisabled();

    await user.click(within(dialog).getByRole("radio", { name: "enterprise-reasoner" }));
    await user.click(within(dialog).getByRole("button", { name: "添加并使用" }));
    expect(onConnectEnterprise).toHaveBeenCalledWith({
      baseUrl: "https://enterprise.example.com/v1",
      apiKey: "secret-key",
      model: "enterprise-reasoner",
    });
  });

  it("starts a new editable endpoint form when managed channels already exist", async () => {
    const user = userEvent.setup();
    render(
      <ProvidersPage
        {...baseProps}
        providers={[{
          name: "tokenstation",
          provider: "openai-compatible",
          base_url: "https://first.example.com/v1",
          models: ["first-model"],
          has_auth: true,
          managed_route: true,
        }]}
      />,
    );

    await user.click(screen.getByRole("button", { name: "添加企业模型" }));
    const baseUrl = within(screen.getByRole("dialog", { name: "添加企业模型" }))
      .getByRole("textbox", { name: "Base URL" });
    expect(baseUrl).toBeEnabled();
    expect(baseUrl).toHaveValue("");
  });

  it("keeps the enterprise form available when an ordinary provider owns the first managed id", async () => {
    const user = userEvent.setup();
    const onVerifyEnterprise = vi.fn();
    render(
      <ProvidersPage
        {...baseProps}
        providers={[{
          name: "tokenstation",
          provider: "openai-compatible",
          base_url: "https://ordinary.example/v1",
          models: ["ordinary-model"],
          has_auth: true,
          managed_route: false,
        }]}
        onVerifyEnterprise={onVerifyEnterprise}
      />,
    );

    await user.click(screen.getByRole("button", { name: "添加企业模型" }));
    const dialog = screen.getByRole("dialog", { name: "添加企业模型" });
    expect(within(dialog).getByRole("textbox", { name: "Base URL" })).toBeEnabled();
    expect(within(dialog).getByLabelText("API Key")).toBeInTheDocument();
    await user.type(within(dialog).getByRole("textbox", { name: "Base URL" }), "https://managed.example/v1");
    await user.type(within(dialog).getByLabelText("API Key"), "managed-key");
    await user.click(within(dialog).getByRole("button", { name: "验证并获取模型" }));
    expect(onVerifyEnterprise).toHaveBeenCalledWith({
      baseUrl: "https://managed.example/v1",
      apiKey: "managed-key",
    });
  });
});
