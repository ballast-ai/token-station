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
      name: "tokenstation",
      baseUrl: "https://enterprise.example.com/v1",
      apiKey: "secret-key",
      model: "enterprise-reasoner",
    });
  });
});
