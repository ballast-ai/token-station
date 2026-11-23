import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getRequestReceipts } from "../api";
import RequestLogsPage from "./RequestLogsPage";

vi.mock("../api", async (loadOriginal) => ({
  ...await loadOriginal<typeof import("../api")>(),
  getRequestReceipts: vi.fn(),
}));

beforeEach(() => {
  vi.mocked(getRequestReceipts).mockReset();
  vi.mocked(getRequestReceipts).mockResolvedValue({ items: [], total: 0, page: 1, page_size: 20 });
});

describe("Request log page controls", () => {
  it.each([true, false])("keeps every time range and refresh usable when embedded=%s", async (embedded) => {
    const user = userEvent.setup();
    render(<RequestLogsPage embedded={embedded} />);
    await waitFor(() => expect(getRequestReceipts).toHaveBeenLastCalledWith(expect.objectContaining({ since: "24h" })));
    for (const [label, since] of [["近 7 天", "7d"], ["近 30 天", "30d"], ["全部历史", "all"], ["近 24 小时", "24h"]]) {
      await user.click(screen.getByRole("combobox", { name: "日志时间范围" }));
      await user.click(screen.getByRole("option", { name: label }));
      await waitFor(() => expect(getRequestReceipts).toHaveBeenLastCalledWith(expect.objectContaining({ since })));
    }
    const before = vi.mocked(getRequestReceipts).mock.calls.length;
    await user.click(screen.getByRole("button", { name: "刷新" }));
    await waitFor(() => expect(getRequestReceipts).toHaveBeenCalledTimes(before + 1));
    if (embedded) expect(screen.queryByRole("heading", { name: "请求日志", level: 1 })).toBeNull();
    else expect(screen.getByRole("heading", { name: "请求日志", level: 1 })).toBeInTheDocument();
  });
});
