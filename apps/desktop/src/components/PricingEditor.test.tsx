import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  getPriceTable,
  getPricingInventory,
  syncModelPrices,
  setPriceSyncEnabled,
  type PricingInventoryView,
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
    getPricingInventory: vi.fn(),
    syncModelPrices: vi.fn(),
    setPriceSyncEnabled: vi.fn(),
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
  vi.mocked(getPricingInventory).mockReset().mockImplementation(async () => inventory(await getPriceTable()));
  vi.mocked(syncModelPrices).mockReset().mockResolvedValue(inventory());
  vi.mocked(setPriceSyncEnabled).mockReset().mockResolvedValue(inventory());
  vi.mocked(setModelPrice).mockReset().mockImplementation(async () => {
    const next = {
      version: 8,
      models: {
        "model-a": {
          ...v7.models["model-a"],
          input_per_mtok: 3_500_000,
          reasoning_per_mtok: null,
        },
      },
    };
    vi.mocked(getPriceTable).mockResolvedValue(next);
    return next;
  });
  vi.mocked(removeModelPrice).mockReset().mockImplementation(async () => {
    const next = { version: 9, models: {} };
    vi.mocked(getPriceTable).mockResolvedValue(next);
    return next;
  });
  vi.mocked(suggestModelPrice).mockReset().mockResolvedValue(null);
});

function inventory(table: PricingInventoryView["table"] = v7): PricingInventoryView {
  return {
    table, offerings: [], requires_apply: false,
    sync: { enabled: false, running: false, last_attempt_ms: null, last_sync_ms: null, errors: [] },
  };
}

function configuredInventory(): PricingInventoryView {
  return {
    ...inventory(),
    offerings: [
      { upstream: "direct", model: "model-a", key: "direct/model-a", price: v7.models["model-a"], source: "models.dev", fetched_at_ms: 1788760800000 },
      { upstream: "custom", model: "model-a", key: "custom/model-a", price: v7.models["model-a"], source: "fallback", fetched_at_ms: null },
      { upstream: "custom", model: "missing-model", key: "custom/missing-model", price: null, source: "missing", fetched_at_ms: null },
    ],
  };
}

describe("configured offering pricing inventory", () => {
  it("shows every offering, pending prices and reference coverage separately from legacy rows", async () => {
    vi.mocked(getPricingInventory).mockResolvedValue(configuredInventory());
    render(<PricingEditor />);
    const grid = await screen.findByRole("table", { name: "已接入模型价格" });
    expect(within(grid).getAllByRole("row")).toHaveLength(4);
    expect(screen.getByText("1 / 3 已配置渠道价格 · 1 通用参考价 · 1 待补价")).toBeInTheDocument();
    const missing = within(grid).getByRole("row", { name: /missing-model/ });
    expect(within(missing).getAllByText("—")).toHaveLength(5);
    expect(within(missing).queryByText("0")).not.toBeInTheDocument();
    expect(screen.getByText("models.dev · 估算基准")).toBeInTheDocument();
    expect(screen.getByText(/用于估算，非账单扣费/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "编辑 model-a" })).toBeEnabled();
    expect(screen.getByRole("switch", { name: "自动同步价格" })).not.toBeChecked();
  });

  it("opens a pending scoped offering without filling unknown amounts with zero", async () => {
    vi.mocked(getPricingInventory).mockResolvedValue(configuredInventory());
    const user = userEvent.setup();
    render(<PricingEditor />);
    await user.click(await screen.findByRole("button", { name: "编辑 custom/missing-model" }));
    expect(screen.getByRole("textbox", { name: "模型 ID" })).toHaveValue("custom/missing-model");
    expect(screen.getByRole("textbox", { name: "模型 ID" })).toHaveFocus();
    expect(screen.getByRole("spinbutton", { name: "输入价格" })).toHaveValue(null);
    await user.click(screen.getByRole("button", { name: "保存新版本" }));
    expect(setModelPrice).not.toHaveBeenCalled();
    await user.type(screen.getByRole("spinbutton", { name: "输入价格" }), "0.84");
    await user.type(screen.getByRole("spinbutton", { name: "输出价格" }), "2.64");
    await user.type(screen.getByRole("spinbutton", { name: "缓存读取价格" }), "0.15");
    await user.type(screen.getByRole("spinbutton", { name: "缓存写入价格" }), "0");
    await user.click(screen.getByRole("button", { name: "保存新版本" }));
    expect(setModelPrice).toHaveBeenCalledWith("custom/missing-model", {
      input_per_mtok: 840000, output_per_mtok: 2640000,
      cache_read_per_mtok: 150000, cache_write_per_mtok: 0, reasoning_per_mtok: null,
    }, 7);
  });

  it("disables synchronization and removal while a local price draft is dirty", async () => {
    vi.mocked(getPricingInventory).mockResolvedValue(configuredInventory());
    const user = userEvent.setup();
    render(<PricingEditor />);
    await user.click(await screen.findByRole("button", { name: "编辑 model-a" }));
    await user.type(screen.getByRole("spinbutton", { name: "输入价格" }), "2");
    expect(screen.getByRole("button", { name: "立即同步" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: "自动同步价格" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "删除 model-a" })).toBeDisabled();
    expect(screen.getByText("请先保存或取消当前价格草稿，再同步价格。" )).toBeInTheDocument();
  });

  it("serializes sync actions and displays saved synchronization state and failures", async () => {
    vi.mocked(getPricingInventory).mockResolvedValue(configuredInventory());
    let resolveSync!: (value: PricingInventoryView) => void;
    vi.mocked(syncModelPrices).mockImplementation(() => new Promise((resolve) => { resolveSync = resolve; }));
    const user = userEvent.setup();
    render(<PricingEditor />);
    await user.click(await screen.findByRole("button", { name: "立即同步" }));
    expect(screen.getByRole("switch", { name: "自动同步价格" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "保存新版本" })).toBeDisabled();
    const next = { ...configuredInventory(), table: { ...v7, version: 8 }, requires_apply: true,
      sync: { enabled: true, running: false, last_attempt_ms: 1788760800000, last_sync_ms: 1788760800000, errors: ["custom: no channel price"] } };
    await act(async () => resolveSync(next));
    expect(screen.getByText("price v8")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "自动同步价格" })).toBeChecked();
    expect(screen.getByText("custom: no channel price")).toBeInTheDocument();
    expect(screen.getByText(/价格已保存；运行中的代理仍需应用配置/)).toBeInTheDocument();
    expect(syncModelPrices).toHaveBeenCalledTimes(1);
  });

  it("persists opt-in and retains inventory on synchronization errors", async () => {
    const current = configuredInventory();
    vi.mocked(getPricingInventory).mockResolvedValue(current);
    vi.mocked(setPriceSyncEnabled).mockResolvedValue({ ...current, sync: { ...current.sync, enabled: true } });
    vi.mocked(syncModelPrices).mockRejectedValue(new Error("price sync unavailable"));
    const user = userEvent.setup();
    render(<ErrorToastProvider><PricingEditor /></ErrorToastProvider>);
    await user.click(await screen.findByRole("switch", { name: "自动同步价格" }));
    expect(setPriceSyncEnabled).toHaveBeenCalledWith(true);
    expect(screen.getByRole("switch", { name: "自动同步价格" })).toBeChecked();
    await user.click(screen.getByRole("button", { name: "立即同步" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("price sync unavailable");
    expect(screen.getByRole("table", { name: "已接入模型价格" })).toBeInTheDocument();
    expect(screen.getByText("price v7")).toBeInTheDocument();
  });

  it("surfaces an inventory load error and permits an explicit retry", async () => {
    vi.mocked(getPricingInventory).mockRejectedValueOnce(new Error("inventory unavailable")).mockResolvedValue(configuredInventory());
    const user = userEvent.setup();
    render(<PricingEditor />);
    expect(await screen.findByRole("alert")).toHaveTextContent("inventory unavailable");
    expect(screen.getByRole("button", { name: "保存新版本" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "刷新价格表" }));
    expect(await screen.findByRole("table", { name: "已接入模型价格" })).toBeInTheDocument();
  });

  it("observes an already running sync before enabling price actions", async () => {
    vi.useFakeTimers();
    try {
      const current = configuredInventory();
      vi.mocked(getPricingInventory)
        .mockResolvedValueOnce({ ...current, sync: { ...current.sync, running: true } })
        .mockResolvedValue({ ...current, table: { ...v7, version: 8 } });
      render(<PricingEditor />);
      await act(async () => {});
      expect(screen.getByRole("button", { name: "同步中…" })).toBeDisabled();
      expect(screen.getByRole("button", { name: "编辑 model-a" })).toBeDisabled();
      await act(async () => { await vi.advanceTimersByTimeAsync(1500); });
      expect(screen.getByText("price v8")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "立即同步" })).toBeEnabled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("polls an enabled idle scheduler and observes a job that starts later", async () => {
    vi.useFakeTimers();
    try {
      const current = configuredInventory();
      const enabled = { ...current, sync: { ...current.sync, enabled: true } };
      vi.mocked(getPricingInventory).mockResolvedValueOnce(current)
        .mockResolvedValueOnce({ ...enabled, sync: { ...enabled.sync, running: true } })
        .mockResolvedValue({ ...enabled, table: { ...v7, version: 8 } });
      vi.mocked(setPriceSyncEnabled).mockResolvedValue(enabled);
      render(<PricingEditor />);
      await act(async () => {});
      await act(async () => { fireEvent.click(screen.getByRole("switch", { name: "自动同步价格" })); });
      await act(async () => { await vi.advanceTimersByTimeAsync(14999); });
      expect(getPricingInventory).toHaveBeenCalledTimes(1);
      await act(async () => { await vi.advanceTimersByTimeAsync(1); });
      expect(screen.getByRole("button", { name: "同步中…" })).toBeDisabled();
      await act(async () => { await vi.advanceTimersByTimeAsync(1500); });
      expect(screen.getByText("price v8")).toBeInTheDocument();
      expect(screen.getByRole("button", { name: "立即同步" })).toBeEnabled();
    } finally { vi.useRealTimers(); }
  });

  it("ignores a pending idle poll after editing starts and keeps the draft version", async () => {
    vi.useFakeTimers();
    try {
      const current = configuredInventory();
      const enabled = { ...current, sync: { ...current.sync, enabled: true } };
      let finishPoll!: (value: PricingInventoryView) => void;
      vi.mocked(getPricingInventory).mockResolvedValueOnce(enabled)
        .mockImplementationOnce(() => new Promise((resolve) => { finishPoll = resolve; }));
      render(<PricingEditor />);
      await act(async () => {});
      await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
      fireEvent.click(screen.getByRole("button", { name: "编辑 model-a" }));
      fireEvent.change(screen.getByRole("spinbutton", { name: "输入价格" }), { target: { value: "9" } });
      await act(async () => finishPoll({ ...enabled, table: { ...v7, version: 8 } }));
      await act(async () => { await vi.advanceTimersByTimeAsync(45000); });
      expect(getPricingInventory).toHaveBeenCalledTimes(2);
      expect(screen.getByText("price v7")).toBeInTheDocument();
      expect(screen.getByRole("spinbutton", { name: "输入价格" })).toHaveValue(9);
      await act(async () => { fireEvent.click(screen.getByRole("button", { name: "保存新版本" })); });
      expect(setModelPrice).toHaveBeenCalledWith("model-a", expect.objectContaining({ input_per_mtok: 9000000 }), 7);
    } finally { vi.useRealTimers(); }
  });

  it("retries idle observation after a transient failure", async () => {
    vi.useFakeTimers();
    try {
      const current = configuredInventory();
      const enabled = { ...current, sync: { ...current.sync, enabled: true } };
      vi.mocked(getPricingInventory).mockResolvedValueOnce(enabled)
        .mockRejectedValueOnce(new Error("inventory unavailable"))
        .mockResolvedValue({ ...enabled, table: { ...v7, version: 8 } });
      render(<PricingEditor />);
      await act(async () => {});
      await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
      expect(screen.getByText("price v7")).toBeInTheDocument();
      expect(screen.getByRole("alert")).toHaveTextContent("inventory unavailable");
      await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
      expect(screen.getByText("price v8")).toBeInTheDocument();
      expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    } finally { vi.useRealTimers(); }
  });

  it("cancels an unchanged editor and reloads inventory without saving", async () => {
    vi.mocked(getPricingInventory).mockResolvedValueOnce(configuredInventory())
      .mockResolvedValue({ ...configuredInventory(), table: { ...v7, version: 8 } });
    const user = userEvent.setup();
    render(<PricingEditor />);
    await user.click(await screen.findByRole("button", { name: "编辑 model-a" }));
    expect(screen.getByRole("button", { name: "立即同步" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "取消编辑" }));
    expect(screen.getByRole("textbox", { name: "模型 ID" })).toHaveValue("");
    expect(screen.getByRole("spinbutton", { name: "输入价格" })).toHaveValue(0);
    expect(screen.getByText("price v8")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "立即同步" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "删除 model-a" })).toBeEnabled();
    expect(setModelPrice).not.toHaveBeenCalled();
    expect(removeModelPrice).not.toHaveBeenCalled();
  });

  it("discards a stale poll response after cancel reload succeeds", async () => {
    vi.useFakeTimers();
    try {
      const current = configuredInventory();
      const enabled = { ...current, sync: { ...current.sync, enabled: true } };
      let finishPoll!: (value: PricingInventoryView) => void;
      vi.mocked(getPricingInventory).mockResolvedValueOnce(enabled)
        .mockImplementationOnce(() => new Promise((resolve) => { finishPoll = resolve; }))
        .mockResolvedValue({ ...enabled, table: { ...v7, version: 9 } });
      render(<PricingEditor />);
      await act(async () => {});
      await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
      fireEvent.click(screen.getByRole("button", { name: "编辑 model-a" }));
      await act(async () => { fireEvent.click(screen.getByRole("button", { name: "取消编辑" })); });
      expect(screen.getByText("price v9")).toBeInTheDocument();
      await act(async () => finishPoll({ ...enabled, table: { ...v7, version: 8 } }));
      expect(screen.getByText("price v9")).toBeInTheDocument();
      expect(screen.getByRole("textbox", { name: "模型 ID" })).toHaveValue("");
      await act(async () => { await vi.advanceTimersByTimeAsync(15000); });
      expect(getPricingInventory).toHaveBeenCalledTimes(4);
    } finally { vi.useRealTimers(); }
  });

  it("serializes cancel reload and preserves the draft if that reload fails", async () => {
    let failReload!: (error: Error) => void;
    vi.mocked(getPricingInventory).mockResolvedValueOnce(configuredInventory())
      .mockImplementationOnce(() => new Promise((_resolve, reject) => { failReload = reject; }))
      .mockResolvedValue(configuredInventory());
    const user = userEvent.setup();
    render(<ErrorToastProvider><PricingEditor /></ErrorToastProvider>);
    await user.click(await screen.findByRole("button", { name: "编辑 model-a" }));
    await user.clear(screen.getByRole("spinbutton", { name: "输入价格" }));
    await user.type(screen.getByRole("spinbutton", { name: "输入价格" }), "9");
    await user.click(screen.getByRole("button", { name: "取消编辑" }));
    expect(screen.getByRole("button", { name: "取消编辑" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "保存新版本" })).toBeDisabled();
    expect(screen.getByRole("spinbutton", { name: "输入价格" })).toBeDisabled();
    await act(async () => failReload(new Error("inventory unavailable")));
    expect(screen.getByRole("spinbutton", { name: "输入价格" })).toHaveValue(9);
    expect(screen.getByText("price v7")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "立即同步" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "取消编辑" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "取消编辑" }));
    expect(screen.getByRole("textbox", { name: "模型 ID" })).toHaveValue("");
    expect(setModelPrice).not.toHaveBeenCalled();
  });
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
    expect(getPricingInventory).toHaveBeenCalledTimes(2);
    const viewport = screen.getByTestId("error-toast-viewport");
    expect(await within(viewport).findByText("已生成 price v8；正在运行的代理需重新应用配置。"))
      .toBeInTheDocument();
    expect(screen.queryByText("已生成 price v8；正在运行的代理需重新应用配置。", { selector: ".pricing-section .banner" }))
      .toBeNull();
  });

  it("accepts an explicit free price and deletes only the current model entry", async () => {
    vi.mocked(getPriceTable).mockResolvedValueOnce({ version: 0, models: {} });
    const freeTable = {
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
    };
    vi.mocked(setModelPrice).mockImplementationOnce(async () => {
      vi.mocked(getPriceTable).mockResolvedValue(freeTable);
      return freeTable;
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
    expect(getPricingInventory).toHaveBeenCalledTimes(3);
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

it("recovers a version conflict without replacing the entered price", async () => {
  vi.mocked(getPriceTable).mockResolvedValueOnce(v7).mockResolvedValue({ ...v7, version: 8 });
  vi.mocked(setModelPrice).mockRejectedValueOnce("定价表版本冲突：当前为 v8，页面基于 v7；请刷新后重试");
  const user = userEvent.setup();
  render(<ErrorToastProvider><PricingEditor /></ErrorToastProvider>);
  await user.click(await screen.findByRole("button", { name: "编辑 model-a" }));
  const input = screen.getByRole("spinbutton", { name: "输入价格" });
  await user.clear(input); await user.type(input, "3.5");
  await user.click(screen.getByRole("button", { name: "保存新版本" }));
  expect(await screen.findByText("price v8")).toBeInTheDocument();
  expect(input).toHaveValue(3.5);
  await user.click(screen.getByRole("button", { name: "保存新版本" }));
  await waitFor(() => expect(setModelPrice).toHaveBeenLastCalledWith("model-a", expect.objectContaining({ input_per_mtok: 3500000 }), 8));
});

it("preserves and explicitly clears TTL-specific cache prices", async () => {
  const price = { ...v7.models["model-a"], cache_write_5m_per_mtok: 6000000, cache_write_1h_per_mtok: 9000000 };
  vi.mocked(getPriceTable).mockResolvedValue({ version: 7, models: { "model-a": price } });
  const user = userEvent.setup();
  render(<PricingEditor />);
  await user.click(await screen.findByRole("button", { name: "编辑 model-a" }));
  expect(screen.getByRole("spinbutton", { name: "缓存写入价格（5 分钟）" })).toHaveValue(6);
  await user.clear(screen.getByRole("spinbutton", { name: "缓存写入价格（1 小时）" }));
  await user.click(screen.getByRole("button", { name: "保存新版本" }));
  await waitFor(() => expect(setModelPrice).toHaveBeenCalledWith("model-a", expect.objectContaining({
    cache_write_5m_per_mtok: 6000000, cache_write_1h_per_mtok: null,
  }), 7));
});
