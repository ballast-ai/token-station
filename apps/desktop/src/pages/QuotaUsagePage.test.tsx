import { act, render, screen, within } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { getQuotaSnapshot, type QuotaAccountSnapshot } from "../api";
import QuotaUsagePage from "./QuotaUsagePage";
import { ErrorToastProvider } from "../components/ErrorToast";

vi.mock("../api", async (loadOriginal) => {
  const original = await loadOriginal<typeof import("../api")>();
  return { ...original, getQuotaSnapshot: vi.fn() };
});

function account(over: Partial<QuotaAccountSnapshot>): QuotaAccountSnapshot {
  return {
    upstream: "deepseek",
    windows: [],
    rate_headroom_permille: 1000,
    rate_pressured: false,
    inflight: 0,
    exhausted: false,
    cooling_ms_remaining: 0,
    source: "none",
    ...over,
  };
}

beforeEach(() => {
  vi.mocked(getQuotaSnapshot).mockReset();
});

it("shows an authoritative window with its remaining percent and reset", async () => {
  vi.mocked(getQuotaSnapshot).mockResolvedValue({
    now_ms: 0,
    accounts: [
      account({
        source: "authoritative",
        windows: [
          { len_ms: 0, limit: 1000, used: 250, remaining_permille: 750, ms_until_reset: 3_600_000 },
        ],
      }),
    ],
  });

  render(<QuotaUsagePage providers={[]} onBack={vi.fn()} />);

  expect(await screen.findByText("deepseek")).toBeInTheDocument();
  // 750‰ → 75% of the window remaining.
  expect(await screen.findByText("75%")).toBeInTheDocument();
});

it("shows a cooling account even with no window data", async () => {
  vi.mocked(getQuotaSnapshot).mockResolvedValue({
    now_ms: 0,
    accounts: [account({ exhausted: true, cooling_ms_remaining: 45_000 })],
  });

  render(<QuotaUsagePage providers={[]} onBack={vi.fn()} />);

  expect(await screen.findByText("deepseek")).toBeInTheDocument();
  // The cooling flag renders "…· 45s"; assert on the language-independent duration.
  expect(await screen.findByText(/45s/)).toBeInTheDocument();
});

it("surfaces the error when the proxy is not running", async () => {
  vi.mocked(getQuotaSnapshot).mockRejectedValue(
    "代理未运行——启动代理后可查看实时额度",
  );

  render(<QuotaUsagePage providers={[]} onBack={vi.fn()} />);

  expect(await screen.findByText("本地代理尚未运行。请先启动代理，然后重试。")).toBeInTheDocument();
});

it("已有额度快照时轮询失败保留旧卡片并只显示错误弹窗", async () => {
  vi.useFakeTimers();
  vi.mocked(getQuotaSnapshot)
    .mockResolvedValueOnce({
      now_ms: 0,
      accounts: [account({
        source: "authoritative",
        windows: [
          { len_ms: 0, limit: 1000, used: 250, remaining_permille: 750, ms_until_reset: 3_600_000 },
        ],
      })],
    })
    .mockRejectedValueOnce("代理未运行——启动代理后可查看实时额度");
  const view = render(
    <ErrorToastProvider>
      <QuotaUsagePage providers={[]} onBack={vi.fn()} />
    </ErrorToastProvider>,
  );

  try {
    await act(async () => {
      await Promise.resolve();
    });
    expect(screen.getByText("75%")).toBeInTheDocument();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(5_000);
    });

    expect(getQuotaSnapshot).toHaveBeenCalledTimes(2);
    expect(screen.getByText("75%")).toBeInTheDocument();
    expect(screen.queryByText("暂时拿不到实时额度")).toBeNull();
    expect(within(screen.getByTestId("error-toast-viewport")).getByRole("alert"))
      .toHaveTextContent("本地代理尚未运行");
  } finally {
    view.unmount();
    vi.useRealTimers();
  }
});
