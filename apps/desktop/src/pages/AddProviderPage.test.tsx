import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import AddProviderPage from "./AddProviderPage";

describe("AddProviderPage", () => {
  it("shows the endpoint and credential boundary for a catalog preset", async () => {
    const user = userEvent.setup();
    render(<AddProviderPage existingNames={[]} onCancel={vi.fn()} onAdded={vi.fn()} />);

    await user.selectOptions(screen.getByLabelText("选择供应商"), "minimax_cn");

    expect(screen.getByDisplayValue("https://api.minimaxi.com/v1")).toBeDisabled();
    expect(screen.getByText("中国开放平台；与国际站 Key 不通用。")).toBeInTheDocument();
    expect(screen.getByText("MiniMax-M3")).toBeInTheDocument();
  });

  it("frames re-adding an existing provider as an update, not a duplicate", async () => {
    const user = userEvent.setup();
    render(<AddProviderPage existingNames={["deepseek"]} onCancel={vi.fn()} onAdded={vi.fn()} />);

    await user.selectOptions(screen.getByLabelText("选择供应商"), "deepseek");

    expect(screen.getByText(/已经添加过了/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更新供应商" })).toBeInTheDocument();
  });
});
