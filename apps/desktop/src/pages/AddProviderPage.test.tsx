import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import AddProviderPage from "./AddProviderPage";

describe("AddProviderPage", () => {
  it("shows the endpoint and credential boundary for a catalog preset", async () => {
    const user = userEvent.setup();
    render(<AddProviderPage onCancel={vi.fn()} onAdded={vi.fn()} />);

    await user.selectOptions(screen.getByLabelText("选择供应商"), "minimax-cn");

    expect(screen.getByDisplayValue("https://api.minimaxi.com/v1")).toBeDisabled();
    expect(screen.getByText("中国开放平台；与国际站 Key 不通用。")).toBeInTheDocument();
    expect(screen.getByText("MiniMax-M3")).toBeInTheDocument();
  });
});
