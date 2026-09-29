import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import AgentModelGuide from "./AgentModelGuide";
import { LanguageProvider, useLanguage } from "./LanguageProvider";
import { ErrorToastProvider } from "./ErrorToast";
import AgentRoutePage from "../pages/AgentRoutePage";
import registry from "../../src-tauri/agent-registry/builtin-agents.json";

vi.mock("../api", async (importOriginal) => ({
  ...await importOriginal<typeof import("../api")>(),
  getAgentBackupDirectory: vi.fn().mockResolvedValue("/test/backups"),
  getAgentDrift: vi.fn().mockResolvedValue([]),
}));

const metadata = { agent_id: "opencode", display_name: "OpenCode", admission: "supported" as const, legacy_kind: null, icon_key: "opencode" };

beforeEach(() => {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn().mockResolvedValue(undefined) } });
});

afterEach(() => vi.useRealTimers());

function deferredCopy() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

describe("Agent model selection guide", () => {
  it("ignores an older copy failure after the newest copy succeeds", async () => {
    const first = deferredCopy();
    vi.mocked(navigator.clipboard.writeText).mockReturnValueOnce(first.promise).mockResolvedValueOnce(undefined);
    render(<ErrorToastProvider><AgentModelGuide metadata={metadata} connected /></ErrorToastProvider>);
    fireEvent.click(screen.getByRole("button", { name: "复制模型名称" }));
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "复制模型名称" })); });
    expect(screen.getByRole("status")).toHaveTextContent("已复制");
    await act(async () => { first.reject(new Error("late denial")); });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("已复制");
  });

  it("does not claim success when an older copy completes after the newest copy fails", async () => {
    const first = deferredCopy();
    vi.mocked(navigator.clipboard.writeText).mockReturnValueOnce(first.promise).mockRejectedValueOnce(new Error("denied"));
    render(<ErrorToastProvider><AgentModelGuide metadata={metadata} connected /></ErrorToastProvider>);
    fireEvent.click(screen.getByRole("button", { name: "复制模型名称" }));
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "复制模型名称" })); });
    expect(screen.getByRole("alert")).toBeInTheDocument();
    await act(async () => { first.resolve(); });
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
  });

  it("uses the current language when a pending copy fails after a language change", async () => {
    function LanguageSwitch() {
      const { setLanguage } = useLanguage();
      return <button onClick={() => setLanguage("en")}>English</button>;
    }
    const pending = deferredCopy();
    vi.mocked(navigator.clipboard.writeText).mockReturnValueOnce(pending.promise);
    render(<ErrorToastProvider><LanguageProvider><LanguageSwitch /><AgentModelGuide metadata={metadata} connected /></LanguageProvider></ErrorToastProvider>);
    fireEvent.click(screen.getByRole("button", { name: "复制模型名称" }));
    fireEvent.click(screen.getByRole("button", { name: "English" }));
    await act(async () => { pending.reject(new Error("denied")); });
    expect(screen.getByRole("alert")).toHaveTextContent("Unable to copy the model name. Select and copy it manually.");
  });

  it("ignores a late copy failure after the guide unmounts", async () => {
    const pending = deferredCopy();
    vi.mocked(navigator.clipboard.writeText).mockReturnValueOnce(pending.promise);
    const { rerender } = render(<ErrorToastProvider><AgentModelGuide metadata={metadata} connected /></ErrorToastProvider>);
    fireEvent.click(screen.getByRole("button", { name: "复制模型名称" }));
    rerender(<ErrorToastProvider><div>Another page</div></ErrorToastProvider>);
    await act(async () => { pending.reject(new Error("late denial")); });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("ignores a pending copy result after selecting another Agent", async () => {
    const pending = deferredCopy();
    vi.mocked(navigator.clipboard.writeText).mockReturnValueOnce(pending.promise);
    const { rerender } = render(<ErrorToastProvider><AgentModelGuide metadata={metadata} connected /></ErrorToastProvider>);
    fireEvent.click(screen.getByRole("button", { name: "复制模型名称" }));
    rerender(<ErrorToastProvider><AgentModelGuide metadata={{ agent_id: "codex", display_name: "Codex" }} connected /></ErrorToastProvider>);
    await act(async () => { pending.reject(new Error("late denial")); });
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
  });

  it("keeps feedback visible for 1600 milliseconds after the latest successful copy", async () => {
    vi.useFakeTimers();
    render(<AgentModelGuide metadata={metadata} connected />);
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "复制模型名称" })); });
    act(() => vi.advanceTimersByTime(1000));
    await act(async () => { fireEvent.click(screen.getByRole("button", { name: "复制模型名称" })); });
    act(() => vi.advanceTimersByTime(600));
    expect(screen.getByRole("status")).toHaveTextContent("已复制");
    act(() => vi.advanceTimersByTime(1000));
    expect(screen.getByRole("status")).toBeEmptyDOMElement();
  });

  it.each(registry.agents.filter((agent) => agent.admission === "supported" || agent.agent_id === "cursor"))(
    "provides guidance for the registered $agent_id identity",
    (agent) => {
      render(<AgentModelGuide metadata={agent} connected />);
      expect(screen.getByRole("region", { name: `在 ${agent.display_name} 中使用` })).toBeInTheDocument();
    },
  );

  it("keeps the OpenCode picker names visible before and after connection", () => {
    const { rerender } = render(<AgentModelGuide metadata={metadata} connected={false} />);
    expect(screen.getByRole("heading", { name: "接入后在 OpenCode 中选择" })).toBeInTheDocument();
    expect(screen.getByText("token-station")).toBeInTheDocument();
    expect(screen.getByText("auto (智能路由)")).toBeInTheDocument();
    rerender(<AgentModelGuide metadata={metadata} connected />);
    expect(screen.getByRole("heading", { name: "在 OpenCode 中使用" })).toBeInTheDocument();
    expect(screen.getByText("auto (智能路由)")).toBeInTheDocument();
  });

  it.each([
    ["codex", "Codex", "Token Station Auto"],
    ["claude-code", "Claude Code", "Token Station Auto"],
    ["kimi-code", "Kimi Code", "tokenstation-auto"],
    ["nous-hermes-agent", "Hermes Agent", "auto"],
  ])("shows the entry written by the %s connector", (agent_id, display_name, model) => {
    render(<AgentModelGuide metadata={{ ...metadata, agent_id, display_name }} connected />);
    expect(screen.getByText(model, { selector: "dd strong" })).toBeInTheDocument();
    expect(screen.queryByText("OpenAI")).not.toBeInTheDocument();
  });

  it("keeps instructions out of the layout and reveals them with keyboard focus", async () => {
    const user = userEvent.setup();
    render(<AgentModelGuide metadata={{ ...metadata, agent_id: "claude-code", display_name: "Claude Code" }} connected />);
    expect(screen.queryByText(/\/model/)).not.toBeInTheDocument();
    await user.tab();
    await user.tab();
    expect(screen.getByRole("button", { name: "查看使用说明" })).toHaveFocus();
    expect(await screen.findByRole("tooltip")).toHaveTextContent("/model");
    expect(screen.getByRole("tooltip")).toHaveTextContent("上下文");
    expect(screen.queryByText(/200K|1M/)).not.toBeInTheDocument();
    expect(screen.queryByText("供应商")).not.toBeInTheDocument();
  });

  it.each(["gemini-cli", "claude-desktop"])("does not invent a model or copy action for %s", (agent_id) => {
    render(<AgentModelGuide metadata={{ ...metadata, agent_id }} connected />);
    expect(screen.getByText("沿用原有模型选项")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "复制模型名称" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "查看使用说明" })).toBeInTheDocument();
  });

  it("does not guess an entry for an unknown Agent", () => {
    const { container } = render(<AgentModelGuide metadata={{ ...metadata, agent_id: "unknown" }} connected />);
    expect(container).toBeEmptyDOMElement();
  });

  it("copies the searchable model name with the keyboard and resets on Agent change", async () => {
    const user = userEvent.setup();
    const clipboard = vi.spyOn(navigator.clipboard, "writeText");
    const { rerender } = render(<AgentModelGuide metadata={metadata} connected />);
    await user.tab();
    expect(screen.getByRole("button", { name: "复制模型名称" })).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(clipboard).toHaveBeenCalledWith("auto (智能路由)");
    expect(screen.getByRole("status")).toHaveTextContent("已复制");
    rerender(<AgentModelGuide metadata={{ ...metadata, agent_id: "codex", display_name: "Codex" }} connected />);
    expect(screen.getByRole("status")).not.toHaveTextContent("已复制");
  });

  it("reports clipboard errors without claiming success", async () => {
    vi.mocked(navigator.clipboard.writeText).mockRejectedValue(new Error("denied"));
    render(<ErrorToastProvider><AgentModelGuide metadata={metadata} connected /></ErrorToastProvider>);
    fireEvent.click(screen.getByRole("button", { name: "复制模型名称" }));
    await waitFor(() => expect(screen.getByText("无法复制模型名称，请手动选中并复制。" )).toBeInTheDocument());
    expect(screen.getByRole("status")).not.toHaveTextContent("已复制");
  });
});

it("places the guide before connection details and omits it from routing", () => {
  const props = {
    metadata,
    route: { mode: "inherit" as const, tiers: { high: { upstream: null, model: null }, mid: { upstream: null, model: null }, low: { upstream: null, model: null } }, config_error: null, profile: null, routing_mode: "direct" as const },
    providers: [], profiles: [], quotaAccounts: [], serveRunning: false, applying: false,
    onStateChange: vi.fn(), onRefreshAgents: vi.fn(), onSaveQuota: vi.fn(), onSaveQuotaPlan: vi.fn(), onViewQuotaUsage: vi.fn(),
  };
  const { rerender } = render(<AgentRoutePage {...props} pageMode="connection" />);
  const guide = screen.getByRole("region", { name: "接入后在 OpenCode 中选择" });
  const details = screen.getByRole("region", { name: "Agent 接入详情" });
  expect(guide.compareDocumentPosition(details) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(within(guide).queryByText("发现路径")).not.toBeInTheDocument();
  rerender(<AgentRoutePage {...props} pageMode="routing" />);
  expect(screen.queryByText("auto (智能路由)")).not.toBeInTheDocument();
});
