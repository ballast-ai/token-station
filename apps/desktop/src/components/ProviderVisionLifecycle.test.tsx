import { act, render, renderHook, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, expect, it, vi } from "vitest";
import type { ProviderView, StateView, VisionVerificationView } from "../api";
import { getStats, previewProviderEndpoints, verifyProviderModelVision } from "../api";
import ProviderList from "./ProviderList";
import { useVisionVerificationTasks } from "./useVisionVerificationTasks";

vi.mock("../api", async (loadOriginal) => ({
  ...await loadOriginal<typeof import("../api")>(),
  getStats: vi.fn(), previewProviderEndpoints: vi.fn(), verifyProviderModelVision: vi.fn(),
}));

const provider: ProviderView = {
  name: "fixture", provider: "openai-compatible", base_url: "https://fixture.invalid/v1",
  models: ["model-a", "model-b"], has_auth: true, credential_source: "store",
};
const response = (): VisionVerificationView => ({
  outcome: "blocked", reason: "rate_limit", http_status: 429, detail: "fixture rate limit",
  state: { providers: [provider] } as StateView,
});
const listProps = {
  providers: [provider], deletedProviders: [], recoveryError: null, serveRunning: false,
  busy: false, onRemove: vi.fn(), onRestore: vi.fn(), onStateChange: vi.fn(),
};

beforeEach(() => {
  vi.mocked(getStats).mockResolvedValue({ groups: [] } as never);
  vi.mocked(previewProviderEndpoints).mockResolvedValue({ chat: provider.base_url, responses: provider.base_url, messages: provider.base_url, loopback: false });
  vi.mocked(verifyProviderModelVision).mockReset();
});

it("retains the running model and blocked result across closing and reopening management", async () => {
  let finish!: (result: VisionVerificationView) => void;
  vi.mocked(verifyProviderModelVision).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const user = userEvent.setup();
  render(<ProviderList {...listProps} />);
  await user.click(screen.getByRole("button", { name: "管理" }));
  await user.click(screen.getByRole("button", { name: "验证 model-a 的视觉能力" }));
  await user.click(within(screen.getByRole("dialog")).getByRole("button", { name: "关闭" }));
  await user.click(screen.getByRole("button", { name: "管理" }));
  expect(screen.getByRole("button", { name: "验证 model-b 的视觉能力" })).toBeDisabled();
  expect(screen.getByText(/仅验证 model-a/)).toBeInTheDocument();
  await act(async () => finish(response()));
  expect(await screen.findByText(/供应商限流.*HTTP 429/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "验证 model-b 的视觉能力" })).toBeEnabled();
  expect(verifyProviderModelVision).toHaveBeenCalledTimes(1);
});

it("shows command errors that arrive while management is closed and clears them on retry", async () => {
  let fail!: (error: string) => void;
  vi.mocked(verifyProviderModelVision).mockImplementationOnce(() => new Promise((_, reject) => { fail = reject; }));
  const user = userEvent.setup();
  render(<ProviderList {...listProps} />);
  await user.click(screen.getByRole("button", { name: "管理" }));
  await user.click(screen.getByRole("button", { name: "验证 model-a 的视觉能力" }));
  await user.keyboard("{Escape}");
  await act(async () => fail("The Provider changed during verification. Run verification again."));
  await user.click(screen.getByRole("button", { name: "管理" }));
  expect(screen.getByText(/验证未能完成/)).toBeInTheDocument();
  vi.mocked(verifyProviderModelVision).mockResolvedValueOnce(response());
  await user.click(screen.getByRole("button", { name: "验证 model-a 的视觉能力" }));
  await waitFor(() => expect(screen.queryByText(/验证未能完成/)).not.toBeInTheDocument());
  expect(await screen.findByText(/供应商限流.*HTTP 429/)).toBeInTheDocument();
});

it("discards an old endpoint's completion without overwriting the new Provider state", async () => {
  let finish!: (result: VisionVerificationView) => void;
  vi.mocked(verifyProviderModelVision).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const onStateChange = vi.fn();
  const user = userEvent.setup();
  const view = render(<ProviderList {...listProps} onStateChange={onStateChange} />);
  await user.click(screen.getByRole("button", { name: "管理" }));
  await user.click(screen.getByRole("button", { name: "验证 model-a 的视觉能力" }));
  await user.keyboard("{Escape}");
  view.rerender(<ProviderList {...listProps} providers={[{ ...provider, base_url: "https://new.fixture.invalid/v1" }]} onStateChange={onStateChange} />);
  await act(async () => finish(response()));
  await user.click(screen.getByRole("button", { name: "管理" }));
  expect(screen.queryByText(/供应商限流/)).not.toBeInTheDocument();
  expect(onStateChange).not.toHaveBeenCalled();
});

it("bounds completed feedback without retaining configuration snapshots", async () => {
  const models = Array.from({ length: 17 }, (_, index) => `model-${index}`);
  const providers = Array.from({ length: 33 }, (_, index) => ({ ...provider, name: `provider-${index}`, models }));
  const { result } = renderHook(() => useVisionVerificationTasks(providers, vi.fn()));
  vi.mocked(verifyProviderModelVision).mockResolvedValue(response());
  for (const model of models) {
    await act(async () => result.current.verify(providers[0], model));
  }
  const feedback = result.current.snapshot(providers[0]);
  expect(Object.keys(feedback.results)).toHaveLength(16);
  expect(feedback.results[models[0]]).toBeUndefined();
  expect(feedback.results[models[16]]).not.toHaveProperty("state");
  for (const item of providers.slice(1)) {
    await act(async () => result.current.verify(item, models[0]));
  }
  expect(result.current.snapshot(providers[0]).results).toEqual({});
  expect(result.current.snapshot(providers[32]).results[models[0]].outcome).toBe("blocked");
});

it("retains active checks at the concurrency bound and suppresses completions after removal", async () => {
  const providers = Array.from({ length: 9 }, (_, index) => ({ ...provider, name: `provider-${index}` }));
  const pending: Array<(view: VisionVerificationView) => void> = [];
  vi.mocked(verifyProviderModelVision).mockImplementation(() => new Promise(resolve => { pending.push(resolve); }));
  const onSaved = vi.fn();
  const { result, rerender } = renderHook(({ items }) => useVisionVerificationTasks(items, onSaved), { initialProps: { items: providers } });
  act(() => { for (const item of providers) void result.current.verify(item, "model-a"); });
  expect(verifyProviderModelVision).toHaveBeenCalledTimes(8);
  expect(result.current.snapshot(providers[0]).checking).toBe("model-a");
  expect(result.current.snapshot(providers[8]).errors["model-a"]).toMatch(/Wait/);
  rerender({ items: [] });
  await act(async () => { for (const finish of pending) finish(response()); });
  expect(onSaved).not.toHaveBeenCalled();
  rerender({ items: providers });
  expect(result.current.snapshot(providers[0]).checking).toBeNull();
  expect(result.current.snapshot(providers[0]).results).toEqual({});
});

it("keeps a successful result on its Provider when another Provider is managed", async () => {
  const other = { ...provider, name: "other" };
  let finish!: (view: VisionVerificationView) => void;
  vi.mocked(verifyProviderModelVision).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const user = userEvent.setup();
  render(<ProviderList {...listProps} providers={[provider, other]} />);
  await user.click(within(screen.getByRole("group", { name: "fixture 供应商" })).getByRole("button", { name: "管理" }));
  await user.click(screen.getByRole("button", { name: "验证 model-a 的视觉能力" }));
  await user.keyboard("{Escape}");
  await user.click(within(screen.getByRole("group", { name: "other 供应商" })).getByRole("button", { name: "管理" }));
  expect(screen.getByRole("button", { name: "验证 model-a 的视觉能力" })).toBeEnabled();
  await act(async () => finish({ ...response(), outcome: "verified", reason: null, http_status: 200, detail: "" }));
  expect(screen.queryByText(/九个随机色块全部识别正确/)).not.toBeInTheDocument();
  await user.keyboard("{Escape}");
  await user.click(within(screen.getByRole("group", { name: "fixture 供应商" })).getByRole("button", { name: "管理" }));
  expect(screen.getByText(/九个随机色块全部识别正确/)).toBeInTheDocument();
});
