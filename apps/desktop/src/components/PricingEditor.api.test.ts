import { beforeEach, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { getPricingInventory, setPriceSyncEnabled, syncModelPrices } from "../api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

beforeEach(() => { vi.mocked(invoke).mockReset(); });

it("maps the pricing inventory and synchronization IPC contract", async () => {
  const result = { table: { version: 7, models: {} }, offerings: [], requires_apply: true,
    sync: { enabled: false, running: false, last_attempt_ms: null, last_sync_ms: null, errors: [] } };
  vi.mocked(invoke).mockResolvedValue(result);
  expect(await getPricingInventory()).toBe(result);
  expect(invoke).toHaveBeenLastCalledWith("get_pricing_inventory");
  expect(await syncModelPrices()).toBe(result);
  expect(invoke).toHaveBeenLastCalledWith("sync_model_prices");
  expect(await setPriceSyncEnabled(true)).toBe(result);
  expect(invoke).toHaveBeenLastCalledWith("set_price_sync_enabled", { enabled: true });
  await setPriceSyncEnabled(false);
  expect(invoke).toHaveBeenLastCalledWith("set_price_sync_enabled", { enabled: false });
});

it("propagates pricing inventory errors instead of replacing unavailable data with empty coverage", async () => {
  vi.mocked(invoke).mockRejectedValue(new Error("inventory unavailable"));
  await expect(getPricingInventory()).rejects.toThrow("inventory unavailable");
});
