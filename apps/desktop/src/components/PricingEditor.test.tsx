import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getPriceTable,
  removeModelPrice,
  setModelPrice,
  suggestModelPrice,
} from "../api";
import PricingEditor from "./PricingEditor";
import { ErrorToastProvider } from "./ErrorToast";

vi.mock("../api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("../api")>();
  return {
    ...original,
    getPriceTable: vi.fn(),
    removeModelPrice: vi.fn(),
    setModelPrice: vi.fn(),
    suggestModelPrice: vi.fn(),
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
  vi.mocked(suggestModelPrice).mockReset().mockResolvedValue(null);
});

describe("versioned pricing editor", () => {
  it("edits one model against the visible version and renders the appended version", async () => {
    const user = userEvent.setup();
    render(<ErrorToastProvider><PricingEditor /></ErrorToastProvider>);
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
    const viewport = screen.getByTestId("error-toast-viewport");
    expect(await within(viewport).findByText("已生成 price v8；正在运行的代理需重新应用配置。"))
      .toBeInTheDocument();
    expect(screen.queryByText("已生成 price v8；正在运行的代理需重新应用配置。", { selector: ".pricing-section .banner" }))
      .toBeNull();
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
    render(<ErrorToastProvider><PricingEditor /></ErrorToastProvider>);
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
    const viewport = screen.getByTestId("error-toast-viewport");
    expect(await within(viewport).findByText("已生成 price v9；历史回执保持原成本。"))
      .toBeInTheDocument();
    expect(screen.queryByText("已生成 price v9；历史回执保持原成本。", { selector: ".pricing-section .banner" }))
      .toBeNull();
  });

  it("prefills a public catalog suggestion but never saves before confirmation", async () => {
    vi.mocked(getPriceTable).mockResolvedValueOnce({ version: 0, models: {} });
    vi.mocked(suggestModelPrice).mockResolvedValue({
      model_id: "gpt-5",
      display_name: "GPT-5",
      provider_id: "openai",
      provider_name: "OpenAI",
      source: "models.dev",
      catalog_source: "live",
      fetched_at_ms: 1_753_334_400_000,
      input_per_mtok: 1_250_000,
      output_per_mtok: 10_000_000,
      cache_read_per_mtok: 125_000,
      cache_write_per_mtok: 0,
      reasoning_per_mtok: null,
    });
    const user = userEvent.setup();
    render(<PricingEditor />);
    await screen.findByText("price v0");

    await user.type(screen.getByRole("textbox", { name: "模型 ID" }), "gpt-5");
    expect(suggestModelPrice).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "查询公开价格" }));

    expect(await screen.findByText(/models\.dev.*OpenAI.*GPT-5/)).toBeInTheDocument();
    expect(screen.getByRole("spinbutton", { name: "输入价格" })).toHaveValue(1.25);
    expect(screen.getByRole("spinbutton", { name: "输出价格" })).toHaveValue(10);
    expect(setModelPrice).not.toHaveBeenCalled();

    await user.clear(screen.getByRole("textbox", { name: "模型 ID" }));
    expect(screen.getByRole("spinbutton", { name: "输入价格" })).toHaveValue(0);
    expect(screen.getByRole("spinbutton", { name: "输出价格" })).toHaveValue(0);
    await user.type(screen.getByRole("textbox", { name: "模型 ID" }), "gpt-5");
    await user.click(screen.getByRole("button", { name: "查询公开价格" }));
    await screen.findByText(/models\.dev.*OpenAI.*GPT-5/);

    await user.click(screen.getByRole("button", { name: "保存新版本" }));
    await waitFor(() => expect(setModelPrice).toHaveBeenCalledWith("gpt-5", {
      input_per_mtok: 1_250_000,
      output_per_mtok: 10_000_000,
      cache_read_per_mtok: 125_000,
      cache_write_per_mtok: 0,
      reasoning_per_mtok: null,
    }, 0));
  });

  it("公开价格查询失败时用左下角 Toast 提示并保留手工录入", async () => {
    vi.mocked(getPriceTable).mockResolvedValueOnce({ version: 0, models: {} });
    vi.mocked(suggestModelPrice).mockRejectedValueOnce(new Error("catalog unavailable"));
    const user = userEvent.setup();
    render(<ErrorToastProvider><PricingEditor /></ErrorToastProvider>);
    await screen.findByText("price v0");

    await user.type(screen.getByRole("textbox", { name: "模型 ID" }), "gpt-5");
    await user.click(screen.getByRole("button", { name: "查询公开价格" }));

    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent("暂时无法获取最新的供应商数据");
    expect(screen.getByRole("spinbutton", { name: "输入价格" })).toBeEnabled();
    expect(screen.queryByText("暂时无法获取最新的供应商数据", { selector: ".pricing-section .banner" }))
      .toBeNull();
  });

  it("公开价格查询失败后可以重试同一模型", async () => {
    vi.mocked(getPriceTable).mockResolvedValueOnce({ version: 0, models: {} });
    vi.mocked(suggestModelPrice)
      .mockRejectedValueOnce(new Error("catalog unavailable"))
      .mockResolvedValueOnce({
        model_id: "gpt-5",
        display_name: "GPT-5",
        provider_id: "openai",
        provider_name: "OpenAI",
        source: "models.dev",
        catalog_source: "live",
        fetched_at_ms: 1_753_334_400_000,
        input_per_mtok: 1_250_000,
        output_per_mtok: 10_000_000,
        cache_read_per_mtok: 125_000,
        cache_write_per_mtok: 0,
        reasoning_per_mtok: null,
      });
    const user = userEvent.setup();
    render(<ErrorToastProvider><PricingEditor /></ErrorToastProvider>);
    await screen.findByText("price v0");

    await user.type(screen.getByRole("textbox", { name: "模型 ID" }), "gpt-5");
    const lookup = screen.getByRole("button", { name: "查询公开价格" });
    await user.click(lookup);
    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent("暂时无法获取最新的供应商数据");

    await user.click(lookup);

    expect(await screen.findByText(/models\.dev.*OpenAI.*GPT-5/)).toBeInTheDocument();
    await waitFor(() => expect(within(screen.getByTestId("error-toast-viewport")).queryByRole("alert"))
      .toBeNull());
    expect(suggestModelPrice).toHaveBeenCalledTimes(2);
  });

  it("未找到公开价格时明确提示并允许重试", async () => {
    vi.mocked(getPriceTable).mockResolvedValueOnce({ version: 0, models: {} });
    vi.mocked(suggestModelPrice)
      .mockResolvedValueOnce(null)
      .mockResolvedValueOnce(null);
    const user = userEvent.setup();
    render(<ErrorToastProvider><PricingEditor /></ErrorToastProvider>);
    await screen.findByText("price v0");

    await user.type(screen.getByRole("textbox", { name: "模型 ID" }), "unknown-model");
    const lookup = screen.getByRole("button", { name: "查询公开价格" });
    await user.click(lookup);

    expect(await within(screen.getByTestId("error-toast-viewport"))
      .findByText("未找到公开价格。"))
      .toBeInTheDocument();
    expect(suggestModelPrice).toHaveBeenCalledTimes(1);

    await user.click(lookup);

    await waitFor(() => expect(suggestModelPrice).toHaveBeenCalledTimes(2));
  });

  it("does not overwrite a price the user has started entering", async () => {
    vi.mocked(getPriceTable).mockResolvedValueOnce({ version: 0, models: {} });
    const user = userEvent.setup();
    render(<PricingEditor />);
    await screen.findByText("price v0");

    await user.type(screen.getByRole("textbox", { name: "模型 ID" }), "gpt-5");
    const input = screen.getByRole("spinbutton", { name: "输入价格" });
    await user.clear(input);
    await user.type(input, "9");

    await user.click(screen.getByRole("button", { name: "查询公开价格" }));
    await new Promise((resolve) => setTimeout(resolve, 450));
    expect(suggestModelPrice).not.toHaveBeenCalled();
    expect(input).toHaveValue(9);
  });

  it("clears a prior model's manual price before saving a different model", async () => {
    vi.mocked(getPriceTable).mockResolvedValueOnce({ version: 0, models: {} });
    const user = userEvent.setup();
    render(<PricingEditor />);
    await screen.findByText("price v0");
    const model = screen.getByRole("textbox", { name: "模型 ID" });
    await user.type(model, "old-model");
    const input = screen.getByRole("spinbutton", { name: "输入价格" });
    await user.clear(input);
    await user.type(input, "9");
    await user.clear(model);
    await user.type(model, "new-model");
    expect(input).toHaveValue(0);
    await user.click(screen.getByRole("button", { name: "保存新版本" }));
    await waitFor(() => expect(setModelPrice).toHaveBeenCalledWith("new-model", expect.objectContaining({
      input_per_mtok: 0,
    }), 0));
  });
});
