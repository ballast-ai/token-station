import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderView, TierView } from "../api";
import { cancelModelTestChat, testModelChatStream } from "../api";
import { LANGUAGE_STORAGE_KEY, LanguageProvider } from "./LanguageProvider";
import ModelTestConsole, {
  buildModelTestRequestMessages,
  trimModelTestTranscript,
  type TranscriptItem,
} from "./ModelTestConsole";

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
    models: ["deepseek-chat", "deepseek-reasoner", "deepseek-v4"],
    has_auth: true,
  },
];

const directTarget: TierView = { upstream: "openai-main", model: "gpt-5.6-terra" };

function renderConsole(
  target: TierView | null = directTarget,
  onOpenChange = vi.fn(),
  open = true,
) {
  return render(
    <LanguageProvider>
      <ModelTestConsole
        open={open}
        onOpenChange={onOpenChange}
        providers={providers}
        initialTarget={target}
      />
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
    const targetButton = screen.getByRole("button", { name: /选择模型/ });
    expect(targetButton).toHaveTextContent("gpt-5.6-terra");
    expect(targetButton).toHaveTextContent("openai-main");
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

  it("uses one button to select a Provider first and then one of its models", async () => {
    const user = userEvent.setup();
    renderConsole();

    const targetButton = screen.getByRole("button", { name: /选择模型/ });
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    await user.click(targetButton);

    expect(screen.getByText("选择供应商")).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "openai-main" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "deepseek-main" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /gpt-5.6/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /deepseek-v4/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("menuitem", { name: "deepseek-main" }));

    expect(screen.getByText("选择模型")).toBeInTheDocument();
    expect(screen.getByText("deepseek-main")).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "deepseek-chat" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "deepseek-reasoner" })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "deepseek-v4" })).toBeInTheDocument();
    expect(screen.queryByRole("menuitem", { name: /gpt-5.6/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole("menuitem", { name: "deepseek-reasoner" }));

    expect(targetButton).toHaveTextContent("deepseek-reasoner");
    expect(targetButton).toHaveTextContent("deepseek-main");
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole("textbox", { name: "消息" })).toHaveFocus());
  });

  it("shows the official Provider avatar once and keeps model options text-only", async () => {
    const user = userEvent.setup();
    renderConsole();

    expect(document.querySelectorAll('[data-provider-brand="openai"]')).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: /选择模型/ }));
    await user.click(screen.getByRole("menuitem", { name: "deepseek-main" }));
    expect(screen.getAllByRole("menuitem")).toHaveLength(4);
    expect(document.querySelector('[data-slot="dropdown-menu-content"] [data-provider-brand="openai"]')).toBeNull();
    document.querySelectorAll(".model-test-model-name").forEach((modelName) => {
      expect(modelName.closest('[data-slot="dropdown-menu-item"]')?.querySelector("[data-provider-brand]")).toBeNull();
    });
  });

  it("keeps the failed prompt available, blocks duplicate sends, and retries one prompt", async () => {
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

    vi.mocked(testModelChatStream).mockResolvedValue({
      content: "鉴权恢复",
      first_token_ms: 40,
      latency_ms: 80,
    });
    await user.keyboard("{Enter}");
    await waitFor(() => expect(testModelChatStream).toHaveBeenCalledTimes(2));
    expect(vi.mocked(testModelChatStream).mock.calls[1]?.[2]).toEqual([
      { role: "user", content: "测试鉴权" },
    ]);
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

    vi.mocked(testModelChatStream).mockResolvedValue({
      content: "新回答",
      first_token_ms: 30,
      latency_ms: 60,
    });
    await user.type(composer, "新问题");
    await user.keyboard("{Enter}");
    await waitFor(() => expect(testModelChatStream).toHaveBeenCalledTimes(2));
    expect(vi.mocked(testModelChatStream).mock.calls[1]?.[2]).toEqual([
      { role: "user", content: "新问题" },
    ]);
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

    await user.click(screen.getByRole("button", { name: /选择模型/ }));
    await user.click(screen.getByRole("menuitem", { name: "deepseek-main" }));
    await user.click(screen.getByRole("menuitem", { name: "deepseek-chat" }));

    expect(cancelModelTestChat).toHaveBeenCalledWith(expect.any(String));
    expect(screen.queryByText("旧模型回复")).not.toBeInTheDocument();
    expect(screen.getByText("每次发送都会产生一次真实的模型请求，可能计入供应商用量。")).toBeInTheDocument();
  });

  it("rejects a prompt that exceeds the UTF-8 byte limit", async () => {
    const user = userEvent.setup();
    renderConsole();

    const composer = screen.getByRole("textbox", { name: "消息" });
    fireEvent.change(composer, { target: { value: "界".repeat(5_334) } });
    await user.click(screen.getByRole("button", { name: "发送消息" }));

    expect(testModelChatStream).not.toHaveBeenCalled();
    expect(screen.getByRole("alert")).toHaveTextContent("消息不能超过 16,000 个 UTF-8 字节");
  });

  it("keeps a request active when cancellation fails and continues rendering deltas", async () => {
    const user = userEvent.setup();
    let pushDelta: ((delta: string) => void) | undefined;
    vi.mocked(cancelModelTestChat).mockRejectedValue(new Error("cancel transport failed"));
    vi.mocked(testModelChatStream).mockImplementation((_upstream, _model, _messages, requestId, onDelta) => {
      pushDelta = (delta) => onDelta({ request_id: requestId, delta, first_token_ms: 42 });
      pushDelta("第一段");
      return new Promise(() => undefined);
    });
    renderConsole();

    await user.type(screen.getByRole("textbox", { name: "消息" }), "继续生成");
    await user.keyboard("{Enter}");
    expect(await screen.findByText("第一段")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "停止生成" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("无法停止请求，生成仍在继续");
    expect(screen.getByRole("button", { name: "停止生成" })).toBeInTheDocument();
    pushDelta?.("第二段");
    expect(await screen.findByText("第一段第二段")).toBeInTheDocument();
  });

  it("does not close while cancellation is unresolved or after cancellation fails", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    let rejectCancellation: ((reason?: unknown) => void) | undefined;
    vi.mocked(cancelModelTestChat).mockImplementation(() => new Promise((_, reject) => {
      rejectCancellation = reject;
    }));
    vi.mocked(testModelChatStream).mockImplementation(() => new Promise(() => undefined));
    renderConsole(directTarget, onOpenChange);

    await user.type(screen.getByRole("textbox", { name: "消息" }), "保持打开");
    await user.keyboard("{Enter}");
    await user.click(screen.getByRole("button", { name: "Close" }));

    expect(cancelModelTestChat).toHaveBeenCalledTimes(1);
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(screen.getByRole("dialog", { name: "测试模型" })).toBeInTheDocument();
    rejectCancellation?.(new Error("cancel failed"));
    expect(await screen.findByRole("alert")).toHaveTextContent("无法停止请求，生成仍在继续");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(screen.getByRole("dialog", { name: "测试模型" })).toBeInTheDocument();
  });

  it("lets an acknowledged cancellation win when the stream rejects first", async () => {
    const user = userEvent.setup();
    let resolveCancellation: (() => void) | undefined;
    let rejectRequest: ((reason?: unknown) => void) | undefined;
    vi.mocked(cancelModelTestChat).mockImplementation(() => new Promise((resolve) => {
      resolveCancellation = resolve;
    }));
    vi.mocked(testModelChatStream).mockImplementation((_upstream, _model, _messages, requestId, onDelta) => {
      onDelta({ request_id: requestId, delta: "保留片段", first_token_ms: 18 });
      return new Promise((_, reject) => {
        rejectRequest = reject;
      });
    });
    renderConsole();

    await user.type(screen.getByRole("textbox", { name: "消息" }), "停止竞态");
    await user.keyboard("{Enter}");
    expect(await screen.findByText("保留片段")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "停止生成" }));
    rejectRequest?.(new Error("stream closed first"));
    resolveCancellation?.();

    expect(await screen.findByText("已停止")).toBeInTheDocument();
    expect(screen.queryByText("stream closed first")).not.toBeInTheDocument();
  });

  it("clears an externally closed request even if the console reopens before cancellation resolves", async () => {
    const user = userEvent.setup();
    let resolveCancellation: (() => void) | undefined;
    vi.mocked(cancelModelTestChat).mockImplementation(() => new Promise((resolve) => {
      resolveCancellation = resolve;
    }));
    vi.mocked(testModelChatStream).mockImplementation((_upstream, _model, _messages, requestId, onDelta) => {
      onDelta({ request_id: requestId, delta: "旧回复", first_token_ms: 21 });
      return new Promise(() => undefined);
    });
    const onOpenChange = vi.fn();
    const view = renderConsole(directTarget, onOpenChange);

    await user.type(screen.getByRole("textbox", { name: "消息" }), "旧问题");
    await user.keyboard("{Enter}");
    expect(await screen.findByText("旧回复")).toBeInTheDocument();
    view.rerender(
      <LanguageProvider>
        <ModelTestConsole
          open={false}
          onOpenChange={onOpenChange}
          providers={providers}
          initialTarget={directTarget}
        />
      </LanguageProvider>,
    );
    view.rerender(
      <LanguageProvider>
        <ModelTestConsole
          open
          onOpenChange={onOpenChange}
          providers={providers}
          initialTarget={directTarget}
        />
      </LanguageProvider>,
    );
    resolveCancellation?.();

    expect(await screen.findByText("每次发送都会产生一次真实的模型请求，可能计入供应商用量。")).toBeInTheDocument();
    expect(screen.queryByText("旧回复")).not.toBeInTheDocument();
  });
});

describe("model test conversation bounds", () => {
  const completedTurn = (turn: number): TranscriptItem[] => [
    { id: turn * 2, turnId: `turn-${turn}`, role: "user", content: `question-${turn}` },
    { id: turn * 2 + 1, turnId: `turn-${turn}`, role: "assistant", content: `answer-${turn}` },
  ];

  it("keeps complete recent turns and never starts history with an assistant", () => {
    const items = Array.from({ length: 10 }, (_, index) => completedTurn(index + 1)).flat();

    const result = buildModelTestRequestMessages(items, "question-11");

    expect(result.error).toBeNull();
    expect(result.messages).toHaveLength(19);
    expect(result.messages[0]).toEqual({ role: "user", content: "question-2" });
    expect(result.messages[result.messages.length - 1]).toEqual({
      role: "user",
      content: "question-11",
    });
  });

  it("excludes failed, stopped, streaming, and orphaned turns from request history", () => {
    const items: TranscriptItem[] = [
      ...completedTurn(1),
      { id: 4, turnId: "failed", role: "user", content: "failed question" },
      { id: 5, turnId: "failed", role: "assistant", content: "", errorMessage: "failed" },
      { id: 6, turnId: "stopped", role: "user", content: "stopped question" },
      { id: 7, turnId: "stopped", role: "assistant", content: "partial", stopped: true },
      { id: 8, turnId: "streaming", role: "user", content: "streaming question" },
      { id: 9, turnId: "streaming", role: "assistant", content: "partial", streaming: true },
      { id: 10, turnId: "orphan", role: "user", content: "orphaned question" },
    ];

    expect(buildModelTestRequestMessages(items, "next").messages).toEqual([
      { role: "user", content: "question-1" },
      { role: "assistant", content: "answer-1" },
      { role: "user", content: "next" },
    ]);
  });

  it("bounds the visible transcript by complete request turns", () => {
    const items = Array.from({ length: 11 }, (_, index) => completedTurn(index + 1)).flat();

    const trimmed = trimModelTestTranscript(items);

    expect(trimmed).toHaveLength(20);
    expect(trimmed[0]?.turnId).toBe("turn-2");
    expect(trimmed[trimmed.length - 1]?.turnId).toBe("turn-11");
  });
});
