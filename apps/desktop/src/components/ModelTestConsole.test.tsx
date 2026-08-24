import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderView, TierView } from "../api";
import { cancelModelTestChat, testModelChatStream } from "../api";
import { LANGUAGE_STORAGE_KEY, LanguageProvider } from "./LanguageProvider";
import ModelTestConsole from "./ModelTestConsole";

vi.mock("../api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../api")>();
  return {
    ...actual,
    cancelModelTestChat: vi.fn(),
    testModelChatStream: vi.fn(),
  };
});

const providers: ProviderView[] = [
  {
    name: "openai-main",
    brand_id: "openai",
    provider: "openai-compatible",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.6-sol", "gpt-5.6-terra"],
    has_auth: true,
  },
  {
    name: "deepseek-main",
    brand_id: "deepseek",
    provider: "openai-compatible",
    base_url: "https://api.deepseek.com/v1",
    models: ["deepseek-v4"],
    has_auth: true,
  },
];

const directTarget: TierView = { upstream: "openai-main", model: "gpt-5.6-terra" };

function renderConsole(target: TierView | null = directTarget) {
  return render(
    <LanguageProvider>
      <ModelTestConsole open onOpenChange={vi.fn()} providers={providers} initialTarget={target} />
    </LanguageProvider>,
  );
}

beforeEach(() => {
  window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "zh-CN");
  vi.mocked(cancelModelTestChat).mockReset();
  vi.mocked(cancelModelTestChat).mockResolvedValue();
  vi.mocked(testModelChatStream).mockReset();
});

describe("ModelTestConsole", () => {
  it("starts with the saved direct target and a normal chat composer", async () => {
    renderConsole();

    expect(screen.getByRole("dialog", { name: "测试模型" })).toBeInTheDocument();
    expect(screen.getByRole("combobox", { name: "测试模型" })).toHaveTextContent("gpt-5.6-terra");
    expect(screen.getByRole("combobox", { name: "测试模型" })).toHaveTextContent("openai-main");
    await waitFor(() => expect(screen.getByRole("textbox", { name: "消息" })).toHaveFocus());
    expect(screen.getByText("每次发送都会产生一次真实的模型请求，可能计入供应商用量。")).toBeInTheDocument();
  });

  it("renders real deltas before completion and shows first-text and total latency", async () => {
    const user = userEvent.setup();
    let finishRequest: ((reply: { content: string; first_token_ms: number; latency_ms: number }) => void) | undefined;
    vi.mocked(testModelChatStream).mockImplementation((_upstream, _model, _messages, requestId, onDelta) => {
      onDelta({ request_id: requestId, delta: "连接", first_token_ms: 126 });
      return new Promise((resolve) => {
        finishRequest = resolve;
      });
    });
    renderConsole();

    const composer = screen.getByRole("textbox", { name: "消息" });
    await user.type(composer, "只回复：连接正常");
    await user.keyboard("{Enter}");

    expect(testModelChatStream).toHaveBeenCalledWith(
      "openai-main",
      "gpt-5.6-terra",
      [{ role: "user", content: "只回复：连接正常" }],
      expect.any(String),
      expect.any(Function),
    );
    expect(await screen.findByText("连接")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "停止生成" })).toBeInTheDocument();

    finishRequest?.({ content: "连接正常。", first_token_ms: 126, latency_ms: 842 });
    expect(await screen.findByText("连接正常。")).toBeInTheDocument();
    expect(screen.getByText("首字 126 ms · 总计 842 ms")).toBeInTheDocument();
    expect(composer).toHaveValue("");
  });

  it("returns to the composer after selecting another exact target", async () => {
    const user = userEvent.setup();
    renderConsole();

    await user.click(screen.getByRole("combobox", { name: "测试模型" }));
    await user.click(screen.getByRole("option", { name: /deepseek-v4/ }));

    expect(screen.getByRole("combobox", { name: "测试模型" })).toHaveTextContent("deepseek-v4");
    expect(screen.getByRole("combobox", { name: "测试模型" })).toHaveTextContent("deepseek-main");
    await waitFor(() => expect(screen.getByRole("textbox", { name: "消息" })).toHaveFocus());
  });

  it("keeps the failed prompt available and blocks duplicate sends", async () => {
    const user = userEvent.setup();
    let rejectRequest: ((reason?: unknown) => void) | undefined;
    vi.mocked(testModelChatStream).mockImplementation(() => new Promise((_, reject) => {
      rejectRequest = reject;
    }));
    renderConsole();

    const composer = screen.getByRole("textbox", { name: "消息" });
    await user.type(composer, "测试鉴权");
    await user.keyboard("{Enter}");
    await user.keyboard("{Enter}");
    expect(testModelChatStream).toHaveBeenCalledTimes(1);

    rejectRequest?.(new Error("Provider authentication failed"));
    expect(await screen.findByRole("alert")).toHaveTextContent("Provider authentication failed");
    expect(composer).toHaveValue("测试鉴权");
  });

  it("stops an active stream, keeps partial text, and ignores later deltas", async () => {
    const user = userEvent.setup();
    let rejectRequest: ((reason?: unknown) => void) | undefined;
    let pushDelta: ((delta: string) => void) | undefined;
    vi.mocked(testModelChatStream).mockImplementation((_upstream, _model, _messages, requestId, onDelta) => {
      pushDelta = (delta) => onDelta({ request_id: requestId, delta, first_token_ms: 90 });
      pushDelta("部分回答");
      return new Promise((_, reject) => {
        rejectRequest = reject;
      });
    });
    renderConsole();

    const composer = screen.getByRole("textbox", { name: "消息" });
    await user.type(composer, "输出一个较长回答");
    await user.keyboard("{Enter}");
    expect(await screen.findByText("部分回答")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "停止生成" }));
    expect(cancelModelTestChat).toHaveBeenCalledWith(expect.any(String));
    expect(screen.getByText("已停止")).toBeInTheDocument();

    pushDelta?.("不应出现");
    rejectRequest?.(new Error("Model test cancelled"));
    await waitFor(() => expect(screen.queryByText(/不应出现/)).not.toBeInTheDocument());
    expect(composer).toHaveValue("");
  });

  it("cancels the active request and clears partial text when the target changes", async () => {
    const user = userEvent.setup();
    vi.mocked(testModelChatStream).mockImplementation((_upstream, _model, _messages, requestId, onDelta) => {
      onDelta({ request_id: requestId, delta: "旧模型回复", first_token_ms: 75 });
      return new Promise(() => undefined);
    });
    renderConsole();

    await user.type(screen.getByRole("textbox", { name: "消息" }), "开始测试");
    await user.keyboard("{Enter}");
    expect(await screen.findByText("旧模型回复")).toBeInTheDocument();

    await user.click(screen.getByRole("combobox", { name: "测试模型" }));
    await user.click(screen.getByRole("option", { name: /deepseek-v4/ }));

    expect(cancelModelTestChat).toHaveBeenCalledWith(expect.any(String));
    expect(screen.queryByText("旧模型回复")).not.toBeInTheDocument();
    expect(screen.getByText("每次发送都会产生一次真实的模型请求，可能计入供应商用量。")).toBeInTheDocument();
  });
});
