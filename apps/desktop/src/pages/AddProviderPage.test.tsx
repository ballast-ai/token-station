import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import AddProviderPage from "./AddProviderPage";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (command: string) => {
    if (command === "preview_provider_endpoints") {
      return {
        chat: "https://api.minimaxi.com/v1/chat/completions",
        responses: "https://api.minimaxi.com/v1/responses",
        messages: "https://api.minimaxi.com/v1/messages",
      };
    }
    throw new Error(`unexpected IPC command: ${command}`);
  }),
}));

describe("AddProviderPage", () => {
  it("shows the endpoint and credential boundary for a catalog preset", async () => {
    const user = userEvent.setup();
    render(<AddProviderPage existingNames={[]} onCancel={vi.fn()} onAdded={vi.fn()} />);

    await user.selectOptions(screen.getByLabelText("选择供应商"), "minimax_cn");

    expect(screen.getByDisplayValue("https://api.minimaxi.com/v1")).toBeDisabled();
    expect(screen.getByText("中国开放平台；与国际站 Key 不通用。")).toBeInTheDocument();
    expect(screen.getByText("MiniMax-M3")).toBeInTheDocument();
    expect(await screen.findByText("https://api.minimaxi.com/v1/chat/completions")).toBeInTheDocument();
    expect(screen.getByText("https://api.minimaxi.com/v1/responses")).toBeInTheDocument();
    expect(screen.getByText("https://api.minimaxi.com/v1/messages")).toBeInTheDocument();
  });

  it("frames an existing provider as a safe update", async () => {
    const user = userEvent.setup();
    render(<AddProviderPage existingNames={["deepseek"]} onCancel={vi.fn()} onAdded={vi.fn()} />);

    await user.selectOptions(screen.getByLabelText("选择供应商"), "deepseek");

    expect(screen.getByText(/已经存在/)).toBeInTheDocument();
    expect(screen.getByText(/不会重复创建/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更新供应商" })).toBeInTheDocument();
  });
});
