import { render, screen, within } from "@testing-library/react";
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

// 供应商选择器是品牌图标卡片网格(role=radiogroup),不是 <select>;按可见标签点卡片。
const pickPreset = (user: ReturnType<typeof userEvent.setup>, label: string) =>
  user.click(screen.getByText(label, { selector: ".preset-card-label" }));

describe("AddProviderPage", () => {
  it("shows the endpoint and credential boundary for a catalog preset", async () => {
    const user = userEvent.setup();
    render(<AddProviderPage existingNames={[]} onCancel={vi.fn()} onAdded={vi.fn()} />);

    await pickPreset(user, "MiniMax（中国）");

    expect(screen.getByDisplayValue("https://api.minimaxi.com/v1")).toBeDisabled();
    expect(screen.getByText("中国开放平台；与国际站 Key 不通用。")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "官方接入文档" })).toHaveAttribute(
      "href",
      "https://platform.minimaxi.com/docs/api-reference/text-openai-api",
    );
    expect(screen.getByText("MiniMax-M3")).toBeInTheDocument();
    expect(await screen.findByText("https://api.minimaxi.com/v1/chat/completions")).toBeInTheDocument();
    expect(screen.getByText("https://api.minimaxi.com/v1/responses")).toBeInTheDocument();
    expect(screen.getByText("https://api.minimaxi.com/v1/messages")).toBeInTheDocument();
  });

  it("offers official and self-hosted presets as cards and omits non-default aggregators", () => {
    render(<AddProviderPage existingNames={[]} onCancel={vi.fn()} onAdded={vi.fn()} />);

    const grid = screen.getByRole("radiogroup", { name: "选择供应商" });
    // 官方/按量 API 与本机/自托管两组的代表都以卡片呈现。
    expect(within(grid).getByText("MiniMax（中国）", { selector: ".preset-card-label" })).toBeInTheDocument();
    expect(within(grid).getByText("本地 Ollama", { selector: ".preset-card-label" })).toBeInTheDocument();
    // 聚合候选(AGGREGATOR_CANDIDATES)不进默认目录,不作为卡片提供。
    expect(screen.queryByText(/OpenRouter/, { selector: ".preset-card-label" })).toBeNull();
  });

  it("frames an existing provider as a safe update", async () => {
    const user = userEvent.setup();
    render(<AddProviderPage existingNames={["deepseek"]} onCancel={vi.fn()} onAdded={vi.fn()} />);

    await pickPreset(user, "DeepSeek");

    expect(screen.getByText(/已经存在/)).toBeInTheDocument();
    expect(screen.getByText(/不会重复创建/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "更新供应商" })).toBeInTheDocument();
  });
});
