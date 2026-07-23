import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getPriceTable, removeModelPrice, setModelPrice } from "../api";
import PricingEditor from "./PricingEditor";

vi.mock("../api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("../api")>();
  return {
    ...original,
    getPriceTable: vi.fn(),
    removeModelPrice: vi.fn(),
    setModelPrice: vi.fn(),
  };
});

const v7 = {
  version: 7,
  models: {
    "model-a": {
      input_per_mtok: 1_000_000,
      output_per_mtok: 2_000_000,
      cache_read_per_mtok: 300_000,
      cache_write_per_mtok: 4_000_000,
      reasoning_per_mtok: 5_000_000,
    },
  },
};

beforeEach(() => {
  vi.mocked(getPriceTable).mockReset().mockResolvedValue(v7);
  vi.mocked(setModelPrice).mockReset().mockResolvedValue({
    version: 8,
    models: {
      "model-a": {
        ...v7.models["model-a"],
        input_per_mtok: 3_500_000,
        reasoning_per_mtok: null,
      },
    },
  });
  vi.mocked(removeModelPrice).mockReset().mockResolvedValue({ version: 9, models: {} });
});

describe("versioned pricing editor", () => {
  it("edits one model against the visible version and renders the appended version", async () => {
    const user = userEvent.setup();
    render(<PricingEditor />);
    expect(await screen.findByText("price v7")).toBeInTheDocument();
    expect(screen.getByText(/历史回执不会重算/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "编辑 model-a" }));
    const input = screen.getByRole("spinbutton", { name: "输入价格" });
    await user.clear(input);
    await user.type(input, "3.5");
    await user.clear(screen.getByRole("spinbutton", { name: "推理价格" }));
    await user.click(screen.getByRole("button", { name: "保存新版本" }));

    await waitFor(() => expect(setModelPrice).toHaveBeenCalledWith("model-a", {
      input_per_mtok: 3_500_000,
      output_per_mtok: 2_000_000,
      cache_read_per_mtok: 300_000,
      cache_write_per_mtok: 4_000_000,
      reasoning_per_mtok: null,
    }, 7));
    expect(await screen.findByText("price v8")).toBeInTheDocument();
  });

  it("accepts an explicit free price and deletes only the current model entry", async () => {
    vi.mocked(getPriceTable).mockResolvedValueOnce({ version: 0, models: {} });
    vi.mocked(setModelPrice).mockResolvedValueOnce({
      version: 1,
      models: {
        free: {
          input_per_mtok: 0,
          output_per_mtok: 0,
          cache_read_per_mtok: 0,
          cache_write_per_mtok: 0,
          reasoning_per_mtok: null,
        },
      },
    });
    const user = userEvent.setup();
    render(<PricingEditor />);
    await screen.findByText("price v0");
    await user.type(screen.getByRole("textbox", { name: "模型 ID" }), "free");
    await user.click(screen.getByRole("button", { name: "保存新版本" }));
    await waitFor(() => expect(setModelPrice).toHaveBeenCalledWith("free", {
      input_per_mtok: 0,
      output_per_mtok: 0,
      cache_read_per_mtok: 0,
      cache_write_per_mtok: 0,
      reasoning_per_mtok: null,
    }, 0));

    await user.click(await screen.findByRole("button", { name: "删除 free" }));
    await waitFor(() => expect(removeModelPrice).toHaveBeenCalledWith("free", 1));
    expect(await screen.findByText("price v9")).toBeInTheDocument();
  });
});
