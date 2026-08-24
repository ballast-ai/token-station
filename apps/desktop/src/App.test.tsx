import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App, { configSaveStatus, firstProviderDefaultTarget, firstRunRouteApplyComplete } from "./App";
import { getStats } from "./api";
import type { AgentRouteView, AgentUiMetadataView, AgentView, ServeView, StateView } from "./api";
import {
  AGENT_VISIBILITY_STORAGE_KEY,
  SHOWN_UNDETECTED_AGENT_IDS_STORAGE_KEY,
} from "./components/AgentVisibilityPreferences";
import {
  FIRST_RUN_GUIDE_STORAGE_KEY,
  FIRST_RUN_GUIDE_VERSION,
  FIRST_RUN_TUTORIAL_CHOICE_STORAGE_KEY,
} from "./components/FirstRunGuide";
import { LANGUAGE_STORAGE_KEY } from "./components/LanguageProvider";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return { ...actual, getStats: vi.fn() };
});
const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);
const getStatsMock = vi.mocked(getStats);
type InvokeMockImplementation = Parameters<typeof invokeMock.mockImplementation>[0];

function mockInvokeImplementation(implementation: InvokeMockImplementation) {
  let fallbackRuntime = serveFixture();
  invokeMock.mockImplementation(async (...args) => {
    try {
      const result = await implementation(...args);
      if (result && typeof result === "object" && "serve" in result) {
        fallbackRuntime = (result as StateView).serve;
      }
      return result;
    } catch (error) {
      if (
        args[0] === "get_runtime_state"
        && error instanceof Error
        && error.message.startsWith("unexpected IPC command:")
      ) {
        return fallbackRuntime as never;
      }
      throw error;
    }
  });
}

const emptyRoute: AgentRouteView = {
  mode: "inherit",
  tiers: {
    high: { upstream: null, model: null },
    mid: { upstream: null, model: null },
    low: { upstream: null, model: null },
  },
  config_error: null,
  profile: null,
  routing_mode: "tiered",
};

const registryFixture: AgentUiMetadataView[] = [
  { agent_id: "claude-code", legacy_kind: "cc", display_name: "Claude Code", icon_key: "claude", admission: "supported", ui_order: 10, nav_mark: "C" },
  { agent_id: "claude-desktop", legacy_kind: null, display_name: "Claude Desktop", icon_key: "claude-desktop", admission: "supported", ui_order: 15, nav_mark: "CD" },
  { agent_id: "codex", legacy_kind: "codex", display_name: "Codex", icon_key: "codex", admission: "supported", ui_order: 20, nav_mark: "X" },
  { agent_id: "gemini-cli", legacy_kind: null, display_name: "Gemini CLI", icon_key: "gemini", admission: "supported", ui_order: 30, nav_mark: "G" },
  { agent_id: "opencode", legacy_kind: "opencode", display_name: "OpenCode", icon_key: "opencode", admission: "supported", ui_order: 40, nav_mark: "O" },
  { agent_id: "cursor", legacy_kind: null, display_name: "Cursor", icon_key: "cursor", admission: "discovery_only", ui_order: 45, nav_mark: "C" },
  { agent_id: "openclaw", legacy_kind: null, display_name: "OpenClaw", icon_key: "openclaw", admission: "supported", ui_order: 50, nav_mark: "OC" },
  { agent_id: "workbuddy", legacy_kind: null, display_name: "WorkBuddy", icon_key: "workbuddy", admission: "supported", ui_order: 55, nav_mark: "WB" },
  { agent_id: "nous-hermes-agent", legacy_kind: null, display_name: "Hermes Agent", icon_key: "hermes", admission: "supported", ui_order: 60, nav_mark: "H" },
  { agent_id: "future-agent", legacy_kind: null, display_name: "Future Agent", icon_key: "future", admission: "discovery_only" },
];
const supportedRegistryFixture = registryFixture.filter(
  (metadata) => metadata.admission === "supported" || metadata.agent_id === "cursor",
);
const agentIds = supportedRegistryFixture.map((metadata) => metadata.agent_id);
const agentDisplayNames = supportedRegistryFixture.map(
  (metadata) => metadata.display_name,
);
const agentNavigationNames = agentDisplayNames;

it("selects the first model as the default global route only for the first provider", () => {
  const next = stateFixture({
    providers: [{
      name: "deepseek",
      brand_id: "deepseek",
      provider: "openai-compatible",
      base_url: "https://api.deepseek.com/v1",
      models: ["deepseek-v4-flash", "deepseek-v4"],
      has_auth: true,
    }],
    direct_target: null,
  });

  expect(firstProviderDefaultTarget(0, next)).toEqual({
    upstream: "deepseek",
    model: "deepseek-v4-flash",
  });
  expect(firstProviderDefaultTarget(1, next)).toBeNull();
  expect(firstProviderDefaultTarget(0, { ...next, direct_target: { upstream: "kept", model: "model" } })).toBeNull();
});

const statsFixture = {
  total: {
    requests: 12,
    errors: 1,
    p50_latency_ms: 48,
    p95_latency_ms: 320,
    input_tokens: 2400,
    output_tokens: 800,
    cache_read_tokens: 0,
    cache_write_tokens: 0,
    reasoning_tokens: 0,
    cost_micros: null,
    priced_requests: 0,
    unpriced_requests: 12,
  },
  groups: [],
  by: null,
  empty: false,
};

const registryWithVirtualSupportedAgent: AgentUiMetadataView[] = [
  ...registryFixture,
  {
    agent_id: "virtual-agent",
    legacy_kind: null,
    display_name: "Virtual Agent",
    icon_key: "virtual",
    admission: "supported",
    ui_order: 999,
    nav_mark: "V",
  },
];

function serveFixture(overrides: Partial<ServeView> = {}): ServeView {
  return {
    phase: "stopped",
    app_runtime: "stopped",
    listener_reachable: false,
    agent_connected: false,
    running_revision: null,
    instance_id: null,
    listen: "127.0.0.1:8787",
    virtual_key: null,
    error: null,
    ...overrides,
  };
}

function stateFixture(overrides: Partial<StateView> = {}): StateView {
  return {
    providers: [],
    tiers: {
      high: { upstream: null, model: null },
      mid: { upstream: null, model: null },
      low: { upstream: null, model: null },
    },
    keywords: { high: [], mid: [], low: [] },
    agent_routes: Object.fromEntries(agentIds.map((id) => [id, structuredClone(emptyRoute)])),
    profiles: [],
    local_only: false,
    allow_cloud_fallback: false,
    routing_mode: "tiered",
    quota_accounts: [],
    serve: serveFixture(),
    draft_revision: 0,
    saved_revision: 0,
    config_dirty: false,
    config_error: null,
    settings: {
      listen: "127.0.0.1:8787",
      auth: true,
      metrics: true,
      data_dir: "/tmp/token-station-test/data",
      plugins_dir: "/tmp/token-station-test/plugins",
      agent: "test-agent",
      version: "test-version",
      egress_mode: "direct",
      egress_proxy_url: "",
      egress_no_proxy: [],
      egress_auth_username: "",
      egress_auth_slot: "",
    },
    ...overrides,
  };
}

const scannedClaude: AgentView = {
  metadata: registryFixture[0],
  installations: [{
    managed: false,
    connected: false,
    adapter_ready: true,
    discovery: {
      agent_id: "claude-code",
      executable_path: "/opt/claude",
      canonical_path: "/opt/claude",
      binary_source: "path",
      modified_at_ms: null,
      binary_sha256: null,
      upgrade_command: null,
      version_raw: "9.9.9",
      version_normalized: "9.9.9",
      environment: "macos",
      evidence: [{ source: "path", observed_path: "/opt/claude", is_path_default: true }],
      is_path_default: true,
      runnable: true,
      config_candidates: ["/tmp/settings.json"],
      config_fingerprint: null,
      conflict_group: null,
      diagnostics: [],
      scanned_at_ms: 1,
    },
    compatibility: {
      agent_id: "claude-code",
      installation_path: "/opt/claude",
      status: "DETECTED_VERIFIED",
      reason_code: "DefaultAdmission",
      message: "已通过只读预检，可以安全接入",
      matched_catalog_version: "fixture",
      connector_id: "claude-code-v1",
      allowed_actions: ["preview_connect"],
    },
  }],
  status: "DETECTED_VERIFIED",
  catalog_sequence: 1,
  catalog_expires_at_ms: null,
  catalog_source: "builtin",
  catalog_warning: null,
};

function defaultAdmittedClaude(): AgentView {
  const value = structuredClone(scannedClaude);
  value.installations[0].discovery.version_raw = "2.1.210";
  value.installations[0].discovery.version_normalized = "2.1.210";
  value.installations[0].compatibility.reason_code = "DefaultAdmission";
  value.installations[0].compatibility.message = "已通过只读预检，可以安全接入";
  value.installations[0].compatibility.allowed_actions = ["preview_connect"];
  return value;
}

const detectedAgentsFixture: AgentView[] = supportedRegistryFixture.map((metadata, index) => {
  const detected = structuredClone(scannedClaude);
  detected.metadata = metadata;
  detected.installations[0].discovery.agent_id = metadata.agent_id;
  detected.installations[0].discovery.executable_path = `/opt/${metadata.agent_id}`;
  detected.installations[0].discovery.canonical_path = `/opt/${metadata.agent_id}`;
  detected.installations[0].compatibility.agent_id = metadata.agent_id;
  detected.installations[0].compatibility.installation_path = `/opt/${metadata.agent_id}`;
  detected.catalog_sequence = index + 1;
  return detected;
});

function projectionPlan(
  operationId: string,
  confirmationToken: string,
  intent: "connect" | "disconnect" = "connect",
) {
  return {
    operation_id: operationId,
    confirmation_token: confirmationToken,
    intent,
    target_config_path: "/tmp/settings.json",
    related_config_paths: [],
    human_diff: "~ /env/ANTHROPIC_BASE_URL: <设置受管值>",
    changes: [{
      operation: "replace",
      path: { segments: ["env", "ANTHROPIC_BASE_URL"] },
      sensitive: false,
      summary: "<设置受管值>",
    }],
  };
}

function navigation() {
  return within(screen.getByRole("navigation", { name: /主导航|Main navigation/ }));
}

async function openRouting(user: ReturnType<typeof userEvent.setup>) {
  await user.click((await screen.findByRole("navigation", { name: /主导航|Main navigation/ })).querySelector<HTMLButtonElement>('button[aria-label="路由"], button[aria-label="Routing"]')!);
  await user.click(await screen.findByRole("button", { name: /全局路由|Global routing/ }));
  await screen.findByRole("region", { name: /路由模式|Routing mode/ });
  expect(screen.queryByRole("heading", { name: /全局路由|Global routing/ })).toBeNull();
}

async function openAgents(user: ReturnType<typeof userEvent.setup>) {
  await user.click((await screen.findByRole("navigation", { name: /主导航|Main navigation/ })).querySelector<HTMLButtonElement>('button[aria-label="Agent"]')!);
  await screen.findByRole("heading", { name: /Agent 接入|Agent connections/ });
}

async function openAgent(user: ReturnType<typeof userEvent.setup>, name: string) {
  await openAgents(user);
  await user.click(screen.getByRole("button", { name }));
  await screen.findByRole("heading", { name });
}

async function openAgentRoute(user: ReturnType<typeof userEvent.setup>, name: string) {
  await openRouting(user);
  const scopes = screen.getByRole("region", { name: /路由范围|Routing scopes/ });
  const disclosure = within(scopes).getByRole("button", { name: /Agent 路由|Agent routes/ });
  if (disclosure.getAttribute("aria-expanded") !== "true") {
    await user.click(disclosure);
  }
  await user.click(within(scopes).getByRole("button", { name }));
  await screen.findByRole("heading", { name });
}

async function openAgentVisibility(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: "设置" }));
  await user.click(screen.getByRole("button", { name: /Agent 显示/ }));
  await screen.findByRole("heading", { name: "Agent 显示" });
}

async function continueFromOverview(user: ReturnType<typeof userEvent.setup>) {
  const overviewCoachmark = await screen.findByRole("dialog", { name: "从这里随时回到主页" });
  expect(screen.getByRole("heading", { name: "概览" })).toBeInTheDocument();
  expect(overviewCoachmark).toHaveTextContent("点击顶部“主页”都能返回主页");
  await user.click(within(overviewCoachmark).getByRole("button", { name: "知道了，开始配置" }));
}

beforeEach(() => {
  window.localStorage.setItem(LANGUAGE_STORAGE_KEY, "zh-CN");
  window.localStorage.setItem(FIRST_RUN_GUIDE_STORAGE_KEY, FIRST_RUN_GUIDE_VERSION);
  window.localStorage.setItem(FIRST_RUN_TUTORIAL_CHOICE_STORAGE_KEY, "started");
  window.localStorage.removeItem(AGENT_VISIBILITY_STORAGE_KEY);
  window.localStorage.removeItem(SHOWN_UNDETECTED_AGENT_IDS_STORAGE_KEY);
  listenMock.mockReset();
  listenMock.mockResolvedValue(vi.fn());
  getStatsMock.mockReset();
  getStatsMock.mockResolvedValue(statsFixture);
  const initial = stateFixture();
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return initial;
    // App polls runtime state every 500 ms. Keep the default mock complete so
    // slower coverage runners do not turn an unrelated test into a poll error.
    if (command === "get_runtime_state") return initial.serve;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return detectedAgentsFixture;
    if (command === "get_request_receipts") return { items: [], total: 0, page: 1, page_size: 20 };
    throw new Error(`unexpected IPC command: ${command}`);
  });
});

it("每次启动完成后都进入主页", async () => {
  render(<App />);

  expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
  expect(navigation().getByRole("button", { name: "主页" }))
    .toHaveAttribute("aria-current", "page");
  expect(navigation().getByRole("button", { name: "Agent" }))
    .not.toHaveAttribute("aria-current");
});

it("新用户首次打开先询问是否需要教程，暂不需要后不再自动询问", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  window.localStorage.removeItem(FIRST_RUN_TUTORIAL_CHOICE_STORAGE_KEY);
  const user = userEvent.setup();
  const firstSession = render(<App />);

  const prompt = await screen.findByRole("dialog", { name: "需要新手教程吗？" });
  expect(document.querySelector(".overview-page h1")).toHaveTextContent("概览");
  expect(document.querySelector('[aria-label="主页"]')).toHaveAttribute("aria-current", "page");
  expect(document.body).not.toHaveAttribute("data-first-run-guide-active");
  await user.click(within(prompt).getByRole("button", { name: "暂不需要" }));
  expect(window.localStorage.getItem(FIRST_RUN_TUTORIAL_CHOICE_STORAGE_KEY)).toBe("declined");

  firstSession.unmount();
  render(<App />);
  await screen.findByRole("heading", { name: "概览" });
  expect(screen.queryByRole("dialog", { name: "需要新手教程吗？" })).toBeNull();
});

it("新用户选择开始教程后高亮主页菜单", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  window.localStorage.removeItem(FIRST_RUN_TUTORIAL_CHOICE_STORAGE_KEY);
  const user = userEvent.setup();
  render(<App />);

  const prompt = await screen.findByRole("dialog", { name: "需要新手教程吗？" });
  await user.click(within(prompt).getByRole("button", { name: "开始教程" }));

  expect(await screen.findByRole("dialog", { name: "从这里随时回到主页" })).toBeInTheDocument();
  expect(navigation().getByRole("button", { name: "主页" }))
    .toHaveAttribute("data-onboarding-active", "true");
  expect(window.localStorage.getItem(FIRST_RUN_TUTORIAL_CHOICE_STORAGE_KEY)).toBe("started");
});

it("teaches overview first, then spotlights the real add-provider button", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();

  render(<App />);

  const overviewCoachmark = await screen.findByRole("dialog", { name: "从这里随时回到主页" });
  expect(screen.getByRole("heading", { name: "概览" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "主页" }))
    .toHaveAttribute("data-onboarding-active", "true");
  expect(screen.getByRole("region", { name: "概览页" }))
    .not.toHaveAttribute("data-onboarding-active");
  expect(overviewCoachmark).toHaveTextContent("点击顶部“主页”都能返回主页");
  await user.click(within(overviewCoachmark).getByRole("button", { name: "知道了，开始配置" }));

  const coachmark = await screen.findByRole("dialog", { name: "添加你的第一个模型" });
  expect(document.body).toHaveAttribute("data-first-run-guide-active", "true");
  expect(coachmark).toHaveTextContent("添加模型 · 1/5");
  expect(screen.queryByRole("button", { name: "下一步" })).toBeNull();
  const addProvider = screen.getByRole("button", { name: "添加模型" });
  expect(addProvider).toHaveAttribute("data-onboarding-active", "true");
  expect(addProvider).toHaveAccessibleDescription("点击这里进入模型配置，完成操作后引导会自动继续。");
  expect(addProvider).toHaveFocus();

  await user.tab();
  expect(within(coachmark).getByRole("button", { name: "稍后继续" })).toHaveFocus();
  await user.tab({ shift: true });
  expect(addProvider).toHaveFocus();

  const workspace = document.querySelector<HTMLElement>(".station-content");
  expect(workspace).not.toBeNull();
  workspace!.scrollTop = 240;
  await user.click(addProvider);

  expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();
  expect(await screen.findByRole("dialog", { name: "选择一个模型供应商" })).toBeInTheDocument();
  expect(workspace).toHaveProperty("scrollTop", 0);
});

it("walks through provider configuration and advances only after a real save", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const added = stateFixture({
    providers: [{
      name: "openai",
      provider: "openai-compatible",
      base_url: "https://api.openai.com/v1",
      models: ["gpt-5.1"],
      has_auth: true,
    }],
    draft_revision: 1,
    config_dirty: true,
  });
  let saveAttempts = 0;
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return stateFixture();
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [];
    if (command === "preview_provider_endpoints") {
      return {
        chat: "https://api.openai.com/v1/chat/completions",
        responses: "https://api.openai.com/v1/responses",
        messages: "https://api.openai.com/v1/messages",
        loopback: false,
      };
    }
    if (command === "add_provider_with_credential") {
      saveAttempts += 1;
      if (saveAttempts === 1) throw new Error("credential rejected");
      return added;
    }
    if (command === "set_routing_mode" || command === "set_direct_route") return added;
    if (command === "serve_start") return {
      ...added,
      routing_mode: "direct",
      direct_target: { upstream: "openai", model: "gpt-5.1" },
    };
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);

  await continueFromOverview(user);
  await screen.findByRole("dialog", { name: "添加你的第一个模型" });
  await user.click(screen.getByRole("button", { name: "添加模型" }));
  expect(await screen.findByRole("dialog", { name: "选择一个模型供应商" })).toBeInTheDocument();
  expect(screen.getByRole("list", { name: "常规供应商列表" }))
    .toHaveAttribute("data-onboarding-active", "true");
  await user.click(screen.getByPlaceholderText("搜索供应商、模型或标签…"));
  expect(screen.getByRole("dialog", { name: "选择一个模型供应商" })).toBeInTheDocument();
  await user.click(screen.getByText("OpenAI", { selector: ".provider-catalog-card-title strong" }));

  const credentialCoachmark = await screen.findByRole("dialog", { name: "填写供应商凭据" });
  expect(credentialCoachmark).toHaveTextContent("添加模型 · 3/5");
  expect(screen.getByRole("group", { name: "供应商凭据" }))
    .toHaveAttribute("data-onboarding-active", "true");
  await user.type(screen.getByLabelText("API Key"), "secret-test");
  await user.click(within(credentialCoachmark).getByRole("button", { name: "下一项：选择模型" }));

  const modelCoachmark = await screen.findByRole("dialog", { name: "选择至少一个模型" });
  expect(screen.getByRole("group", { name: "供应商模型" }))
    .toHaveAttribute("data-onboarding-active", "true");
  await user.click(within(modelCoachmark).getByRole("button", { name: "配置好了，去保存" }));

  const saveCoachmark = await screen.findByRole("dialog", { name: "保存供应商" });
  await user.click(within(saveCoachmark).getByRole("button", { name: "返回选择模型" }));
  const revisitedModels = await screen.findByRole("dialog", { name: "选择至少一个模型" });
  await user.click(within(revisitedModels).getByRole("button", { name: "配置好了，去保存" }));
  await screen.findByRole("dialog", { name: "保存供应商" });
  const saveProvider = screen.getByRole("button", { name: "添加供应商" });
  expect(saveProvider).toHaveAttribute("data-onboarding-active", "true");
  await user.click(saveProvider);

  expect(await screen.findByText(
    "凭据无法使用。请检查 API Key 及其权限，然后重试。",
  )).toBeInTheDocument();
  expect(screen.getByRole("dialog", { name: "保存供应商" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "OpenAI" })).toBeInTheDocument();

  await user.click(saveProvider);

  expect(await screen.findByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
  expect(screen.queryByRole("dialog", { name: "选择路由模式" })).toBeNull();
  expect(invokeMock).toHaveBeenCalledWith("set_routing_mode", { mode: "direct", agentId: null });
  expect(invokeMock).toHaveBeenCalledWith("set_direct_route", { upstream: "openai", model: "gpt-5.1", agentId: null });
});

it("spotlights routing controls and advances only when the requested revision is really running", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  let emitServe: ((serve: ServeView) => void) | undefined;
  listenMock.mockImplementation(async (_eventName, handler) => {
    emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
    return () => undefined;
  });
  const provider = {
    name: "openai",
    provider: "openai-compatible",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.1"],
    has_auth: true,
  };
  const initial = stateFixture({
    providers: [provider],
    tiers: {
      high: { upstream: "openai", model: "gpt-5.1" },
      mid: { upstream: "openai", model: "gpt-5.1" },
      low: { upstream: "openai", model: "gpt-5.1" },
    },
    draft_revision: 1,
    saved_revision: 1,
    config_dirty: false,
  });
  const applying = stateFixture({
    providers: [provider],
    tiers: {
      high: { upstream: "openai", model: "gpt-5.1" },
      mid: { upstream: "openai", model: "gpt-5.1" },
      low: { upstream: "openai", model: "gpt-5.1" },
    },
    draft_revision: 2,
    saved_revision: 2,
    config_dirty: false,
    serve: serveFixture({ phase: "starting" }),
  });
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return initial;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [];
    if (command === "serve_start") return applying;
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);
  await continueFromOverview(user);
  const entryCoachmark = await screen.findByRole("dialog", { name: "打开路由配置" });
  expect(entryCoachmark).toHaveTextContent("配置路由 · 1/4");
  const routingNav = screen.getByRole("button", { name: "全局路由" });
  expect(routingNav).toHaveAttribute("data-onboarding-active", "true");
  await user.click(routingNav);

  const modeCoachmark = await screen.findByRole("dialog", { name: "选择路由模式" });
  expect(screen.getByRole("region", { name: "路由模式" }))
    .toHaveAttribute("data-onboarding-active", "true");
  await user.click(within(modeCoachmark).getByRole("button", { name: "沿用当前模式" }));

  const configCoachmark = await screen.findByRole("dialog", { name: "配置模型路由" });
  expect(screen.getByRole("group", { name: "三档模型配置" }))
    .toHaveAttribute("data-onboarding-active", "true");
  await user.click(within(configCoachmark).getByRole("button", { name: "配置好了，去应用" }));

  await screen.findByRole("dialog", { name: "保存并应用路由" });
  const saveAndApply = screen.getByRole("button", { name: "保存并应用" });
  expect(saveAndApply).toHaveAttribute("data-onboarding-active", "true");
  await user.click(saveAndApply);

  expect(await within(screen.getByTestId("error-toast-viewport")).findByText("正在应用配置…"))
    .toBeInTheDocument();
  expect(document.querySelector(".global-banner")).toBeNull();
  expect(screen.getByRole("dialog", { name: "保存并应用路由" })).toBeInTheDocument();
  act(() => emitServe?.(serveFixture({
    phase: "running",
    app_runtime: "running",
    listener_reachable: true,
    running_revision: 1,
  })));
  expect(screen.getByRole("region", { name: "路由模式" })).toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "全局路由" })).toBeNull();

  act(() => emitServe?.(serveFixture({
    phase: "running",
    app_runtime: "running",
    listener_reachable: false,
    running_revision: 2,
  })));
  expect(screen.getByRole("region", { name: "路由模式" })).toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "全局路由" })).toBeNull();

  act(() => emitServe?.(serveFixture({
    phase: "running",
    app_runtime: "running",
    listener_reachable: true,
    running_revision: 2,
  })));

  expect(await screen.findByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
  expect(await screen.findByRole("dialog", { name: "未检测到可接入 Agent" })).toBeInTheDocument();
});

it("requires the saved and running revisions to match the requested revision", () => {
  const mismatched = stateFixture({
    saved_revision: 3,
    config_dirty: false,
    config_error: null,
    serve: serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 2,
      error: null,
    }),
  });
  expect(firstRunRouteApplyComplete(mismatched, 2)).toBe(false);

  expect(firstRunRouteApplyComplete({ ...mismatched, saved_revision: 2 }, 2)).toBe(true);
});

it("pins each route teaching target and restores main scrolling when paused", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const provider = {
    name: "openai",
    provider: "openai-compatible",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.1"],
    has_auth: true,
  };
  const initial = stateFixture({
    providers: [provider],
    tiers: {
      high: { upstream: "openai", model: "gpt-5.1" },
      mid: { upstream: "openai", model: "gpt-5.1" },
      low: { upstream: "openai", model: "gpt-5.1" },
    },
    draft_revision: 1,
    saved_revision: 1,
    config_dirty: false,
  });
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return initial;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);
  await continueFromOverview(user);
  await screen.findByRole("dialog", { name: "打开路由配置" });
  await user.click(screen.getByRole("button", { name: "全局路由" }));

  const workspace = document.querySelector<HTMLElement>(".station-content");
  expect(workspace).not.toBeNull();
  const expectScrollLocked = (attemptedTop: number) => {
    const wheelEvent = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
    act(() => {
      workspace!.dispatchEvent(wheelEvent);
    });
    expect(wheelEvent.defaultPrevented).toBe(true);
    const overlayWheel = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
    const blocker = document.querySelector<HTMLElement>(".first-run-spotlight-blocker");
    expect(blocker).not.toBeNull();
    act(() => {
      blocker!.dispatchEvent(overlayWheel);
    });
    expect(overlayWheel.defaultPrevented).toBe(true);
    const lockedTop = workspace!.scrollTop;
    act(() => {
      workspace!.scrollTop = attemptedTop;
      workspace!.dispatchEvent(new Event("scroll"));
    });
    expect(workspace!.scrollTop).toBe(lockedTop);
  };

  const modeCoachmark = await screen.findByRole("dialog", { name: "选择路由模式" });
  expectScrollLocked(180);
  await user.click(within(modeCoachmark).getByRole("button", { name: "沿用当前模式" }));

  const configCoachmark = await screen.findByRole("dialog", { name: "配置模型路由" });
  expectScrollLocked(360);
  const highProvider = screen.getByRole("combobox", { name: "上档供应商" });
  await user.click(highProvider);
  const optionWheel = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
  act(() => {
    screen.getByRole("listbox").dispatchEvent(optionWheel);
  });
  expect(optionWheel.defaultPrevented).toBe(false);
  await user.click(highProvider);
  await user.click(within(configCoachmark).getByRole("button", { name: "配置好了，去应用" }));

  await screen.findByRole("dialog", { name: "保存并应用路由" });
  expectScrollLocked(540);
  await user.keyboard("{Escape}");

  expect(screen.queryByRole("dialog")).toBeNull();
  const wheelAfterPause = new WheelEvent("wheel", { deltaY: 120, cancelable: true });
  act(() => {
    workspace!.dispatchEvent(wheelAfterPause);
  });
  expect(wheelAfterPause.defaultPrevented).toBe(false);
  act(() => {
    workspace!.scrollTop = 720;
    workspace!.dispatchEvent(new Event("scroll"));
  });
  expect(workspace!.scrollTop).toBe(720);
});

it("does not treat a running but unconfigured route as onboarding-complete", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const provider = {
    name: "openai",
    provider: "openai-compatible",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.1"],
    has_auth: true,
  };
  const unconfigured = stateFixture({
    providers: [provider],
    serve: serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 0,
    }),
  });
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return unconfigured;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);

  await continueFromOverview(user);
  expect(await screen.findByRole("dialog", { name: "打开路由配置" })).toBeInTheDocument();
  expect(screen.queryByRole("dialog", { name: "打开 Agent 管理" })).toBeNull();
});

it("启动未扫到 Agent 时可直接完成基础设置并回到主页", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const provider = {
    name: "openai",
    provider: "openai-compatible",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.1"],
    has_auth: true,
  };
  const ready = stateFixture({
    providers: [provider],
    tiers: {
      high: { upstream: "openai", model: "gpt-5.1" },
      mid: { upstream: "openai", model: "gpt-5.1" },
      low: { upstream: "openai", model: "gpt-5.1" },
    },
    draft_revision: 1,
    saved_revision: 1,
    config_dirty: false,
    config_error: null,
    serve: serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
    }),
  });
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return ready;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);

  await continueFromOverview(user);
  const emptyCoachmark = await screen.findByRole("dialog", { name: "未检测到可接入 Agent" });
  expect(screen.getByRole("button", { name: "重新扫描" })).toBeEnabled();
  expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  await user.click(within(emptyCoachmark).getByRole("button", { name: "暂不接入，完成设置" }));

  expect(await screen.findByRole("dialog", { name: "基础设置完成" })).toBeInTheDocument();
  expect(screen.getByText("供应商和路由已就绪，Agent 尚未接入。"))
    .toBeInTheDocument();
  expect(screen.getByRole("dialog", { name: "基础设置完成" }))
    .toHaveTextContent("设置 → 关于 → 重新查看新手引导");
  expect(window.localStorage.getItem(FIRST_RUN_GUIDE_STORAGE_KEY)).toBe(FIRST_RUN_GUIDE_VERSION);
  await user.click(screen.getByRole("button", { name: "前往 Agent" }));
  expect(await screen.findByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
});

it("一键接入后只通过缓存状态刷新确认 CONNECTED", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const provider = {
    name: "openai",
    provider: "openai-compatible",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.1"],
    has_auth: true,
  };
  const ready = stateFixture({
    providers: [provider],
    tiers: {
      high: { upstream: "openai", model: "gpt-5.1" },
      mid: { upstream: "openai", model: "gpt-5.1" },
      low: { upstream: "openai", model: "gpt-5.1" },
    },
    saved_revision: 1,
    draft_revision: 1,
    serve: serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
      instance_id: "instance-ready",
      virtual_key: "vk-test",
    }),
  });
  let resolveConnectedCache: ((agents: AgentView[]) => void) | undefined;
  const connectedCache = new Promise<AgentView[]>((resolve) => {
    resolveConnectedCache = resolve;
  });
  const connectedAgent = structuredClone(scannedClaude);
  connectedAgent.status = "CONNECTED";
  connectedAgent.installations[0].managed = true;
  connectedAgent.installations[0].connected = true;
  connectedAgent.installations[0].compatibility.status = "CONNECTED";
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return ready;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [scannedClaude];
    if (command === "ensure_serve_running") return ready;
    if (command === "get_cached_agent_views") return connectedCache;
    if (command === "plan_agent_connection") return projectionPlan("op-onboarding", "token-onboarding");
    if (command === "apply_agent_plan") {
      return { operation_id: "op-onboarding", maintenance_warning: null };
    }
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);
  await continueFromOverview(user);
  await screen.findByRole("dialog", { name: "打开 Agent 管理" });
  await user.click(screen.getByRole("button", { name: "Agent" }));

  const discoveryScope = await screen.findByRole("dialog", {
    name: "这里仅显示扫描到的 Agent",
  });
  expect(discoveryScope).toHaveTextContent("Token Station 支持的全部 Agent");
  expect(discoveryScope).toHaveTextContent("设置 → Agent 显示");
  await user.click(within(discoveryScope).getByRole("button", {
    name: "知道了，选择 Agent",
  }));

  await screen.findByRole("dialog", { name: "选择一个 Agent" });
  expect(screen.getByRole("region", { name: "Agent 选择列表" }))
    .toHaveAttribute("data-onboarding-active", "true");
  await user.click(screen.getByRole("button", { name: "Claude Code" }));

  const connectCoachmark = await screen.findByRole("dialog", { name: "一键接入 Agent" });
  const connect = screen.getByRole("button", { name: "一键接入" });
  expect(connect).toHaveAttribute("data-onboarding-active", "true");
  expect(connectCoachmark).toHaveTextContent("接入 Agent · 4/4");
  await user.click(connect);

  await waitFor(() => expect(invokeMock).toHaveBeenCalledWith(
    "apply_agent_plan",
    { operationId: "op-onboarding", confirmationToken: "token-onboarding" },
  ));
  expect(screen.queryByRole("dialog", { name: "首次设置完成" })).toBeNull();
  resolveConnectedCache?.([connectedAgent]);
  expect(await screen.findByRole("dialog", { name: "首次设置完成" })).toBeInTheDocument();
  expect(screen.getByText("供应商、路由和 Agent 均已就绪，Token Station 现在可以接管模型请求。"))
    .toBeInTheDocument();
  expect(window.localStorage.getItem(FIRST_RUN_GUIDE_STORAGE_KEY)).toBe(FIRST_RUN_GUIDE_VERSION);
});

it("requires an exact installation choice before connecting a multi-installation Agent", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const provider = {
    name: "openai",
    provider: "openai-compatible",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.1"],
    has_auth: true,
  };
  const ready = stateFixture({
    providers: [provider],
    tiers: {
      high: { upstream: "openai", model: "gpt-5.1" },
      mid: { upstream: "openai", model: "gpt-5.1" },
      low: { upstream: "openai", model: "gpt-5.1" },
    },
    saved_revision: 1,
    draft_revision: 1,
    serve: serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
      instance_id: "instance-ready",
    }),
  });
  const multiInstall = structuredClone(scannedClaude);
  const secondInstall = structuredClone(multiInstall.installations[0]);
  secondInstall.discovery.executable_path = "/opt/claude-preview";
  secondInstall.discovery.canonical_path = "/opt/claude-preview";
  secondInstall.discovery.version_raw = "10.0.0";
  secondInstall.discovery.version_normalized = "10.0.0";
  secondInstall.compatibility.installation_path = "/opt/claude-preview";
  multiInstall.installations.push(secondInstall);
  multiInstall.status = "MULTIPLE_INSTALLATIONS";
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return ready;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [multiInstall];
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);
  await continueFromOverview(user);
  await screen.findByRole("dialog", { name: "打开 Agent 管理" });
  await user.click(screen.getByRole("button", { name: "Agent" }));
  const discoveryScope = await screen.findByRole("dialog", {
    name: "这里仅显示扫描到的 Agent",
  });
  await user.click(within(discoveryScope).getByRole("button", {
    name: "知道了，选择 Agent",
  }));
  await screen.findByRole("dialog", { name: "选择一个 Agent" });
  await user.click(screen.getByRole("button", { name: "Claude Code" }));

  expect(await screen.findByRole("dialog", { name: "选择要接管的安装" })).toBeInTheDocument();
  const picker = screen.getByRole("button", { name: "选择版本" });
  expect(picker).toHaveAttribute("data-onboarding-active", "true");
  expect(screen.queryByRole("dialog", { name: "一键接入 Agent" })).toBeNull();

  await user.click(picker);
  await user.tab();
  expect(screen.getByRole("option", { name: "claude · v9.9.9" })).toHaveFocus();
  await user.click(screen.getByRole("option", { name: "claude-preview · v10.0.0" }));

  expect(await screen.findByRole("dialog", { name: "一键接入 Agent" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "一键接入" }))
    .toHaveAttribute("data-onboarding-active", "true");
});

it("shows an already-complete summary without asking for repeated setup", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const provider = {
    name: "openai",
    provider: "openai-compatible",
    base_url: "https://api.openai.com/v1",
    models: ["gpt-5.1"],
    has_auth: true,
  };
  const ready = stateFixture({
    providers: [provider],
    tiers: {
      high: { upstream: "openai", model: "gpt-5.1" },
      mid: { upstream: "openai", model: "gpt-5.1" },
      low: { upstream: "openai", model: "gpt-5.1" },
    },
    saved_revision: 1,
    draft_revision: 1,
    serve: serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
      instance_id: "instance-ready",
    }),
  });
  const connectedAgent = structuredClone(scannedClaude);
  connectedAgent.status = "CONNECTED";
  connectedAgent.installations[0].managed = true;
  connectedAgent.installations[0].connected = true;
  connectedAgent.installations[0].compatibility.status = "CONNECTED";
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return ready;
    if (command === "list_agent_registry") return registryFixture;
    if (command === "scan_agents") return [connectedAgent];
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);

  await continueFromOverview(user);
  expect(await screen.findByRole("dialog", { name: "首次设置已完成" })).toBeInTheDocument();
  expect(screen.getByRole("dialog", { name: "首次设置已完成" }))
    .toHaveTextContent("设置 → 关于 → 重新查看新手引导");
  await user.click(screen.getByRole("button", { name: "前往 Agent" }));

  expect(screen.queryByRole("dialog")).toBeNull();
  expect(await screen.findByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
  expect(window.localStorage.getItem(FIRST_RUN_GUIDE_STORAGE_KEY)).toBe(FIRST_RUN_GUIDE_VERSION);
});

it("persists a skipped guide and does not show it on the next App session", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const firstSession = render(<App />);

  await screen.findByRole("dialog", { name: "从这里随时回到主页" });
  await user.click(screen.getByRole("button", { name: "不再提示" }));

  expect(window.localStorage.getItem(FIRST_RUN_GUIDE_STORAGE_KEY)).toBe(
    FIRST_RUN_GUIDE_VERSION,
  );
  firstSession.unmount();
  render(<App />);
  await screen.findByRole("heading", { name: "概览" });
  expect(screen.queryByRole("dialog")).toBeNull();
});

it("reopens the guide from the About page without clearing the dismissed version", async () => {
  const user = userEvent.setup();
  render(<App />);

  await user.click(await screen.findByRole("button", { name: "设置" }));
  expect(screen.queryByRole("button", { name: "重新查看新手引导" })).toBeNull();
  await user.click(screen.getByRole("button", { name: /关于/ }));
  await user.click(screen.getByRole("button", { name: "重新查看新手引导" }));

  expect(await screen.findByRole("dialog", { name: "从这里随时回到主页" })).toBeInTheDocument();
  expect(screen.getByRole("heading", { name: "概览" })).toBeInTheDocument();
  expect(window.localStorage.getItem(FIRST_RUN_GUIDE_STORAGE_KEY)).toBe(
    FIRST_RUN_GUIDE_VERSION,
  );
});

it("treats Escape as a pause, restores focus, and offers setup next session", async () => {
  window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
  const user = userEvent.setup();
  const firstSession = render(<App />);

  await screen.findByRole("dialog", { name: "从这里随时回到主页" });
  await user.keyboard("{Escape}");

  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.body).not.toHaveAttribute("data-first-run-guide-active");
  expect(window.localStorage.getItem(FIRST_RUN_GUIDE_STORAGE_KEY)).toBeNull();
  await waitFor(() => {
    expect(screen.getByRole("button", { name: "主页" })).toHaveFocus();
  });

  firstSession.unmount();
  render(<App />);
  expect(await screen.findByRole("dialog", { name: "从这里随时回到主页" }))
    .toBeInTheDocument();
});

it("不显示仅存在于注册表但启动扫描未发现安装的 Agent", async () => {
  const user = userEvent.setup();
  mockInvokeImplementation(async (command) => {
    if (command === "get_state") return stateFixture();
    if (command === "list_agent_registry") return registryWithVirtualSupportedAgent;
    if (command === "scan_agents") return [];
    throw new Error(`unexpected IPC command: ${command}`);
  });

  render(<App />);
  await openAgents(user);
  expect(screen.queryByRole("button", { name: "Virtual Agent" })).toBeNull();
});

describe("desktop station navigation", () => {
  it("keeps destination content and the fixed shell stable without entrance motion", async () => {
    const user = userEvent.setup();
    const cancel = vi.fn();
    const animate = vi.fn().mockReturnValue({ cancel } as unknown as Animation);
    const originalAnimate = HTMLElement.prototype.animate;
    Object.defineProperty(HTMLElement.prototype, "animate", {
      configurable: true,
      value: animate,
    });

    try {
      render(<App />);
      await openAgents(user);
      animate.mockClear();

      await user.click(screen.getByRole("button", { name: "模型" }));
      expect(screen.getByRole("dialog", { name: "选择模型接入方式" })).toBeInTheDocument();
      await user.keyboard("{Escape}");
      expect(await screen.findByRole("heading", { name: "模型" })).toBeInTheDocument();

      expect(animate).not.toHaveBeenCalled();
      expect(document.querySelector(".station-header")).toBeInTheDocument();
    } finally {
      if (originalAnimate) {
        Object.defineProperty(HTMLElement.prototype, "animate", {
          configurable: true,
          value: originalAnimate,
        });
      } else {
        Reflect.deleteProperty(HTMLElement.prototype, "animate");
      }
    }
  });

  it("reveals startup Agent rows in stable order with a capped stagger", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgents(user);

    const firstAgent = await screen.findByRole("button", { name: "Claude Code" });
    const lastAgent = await screen.findByRole("button", { name: "Hermes Agent" });
    expect(firstAgent).toHaveClass("agent-master-item-revealing");
    expect(firstAgent).toHaveStyle({ animationDelay: "0ms" });
    expect(lastAgent).toHaveClass("agent-master-item-revealing");
    expect(lastAgent).toHaveStyle({ animationDelay: "300ms" });

    await waitFor(() => {
      expect(firstAgent).not.toHaveClass("agent-master-item-revealing");
      expect(lastAgent).not.toHaveClass("agent-master-item-revealing");
    }, { timeout: 1_000 });
  });

  it("等启动扫描完成后一次性显示合并主页与已发现 Agent", async () => {
    window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    let resolveScan!: (agents: AgentView[]) => void;
    const startupScan = new Promise<AgentView[]>((resolve) => {
      resolveScan = resolve;
    });
    let runtime = serveFixture();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return startupScan;
      if (command === "get_runtime_state") return runtime;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    const startupStatus = await screen.findByRole("status", { name: "正在检查本机 Agent" });
    expect(startupStatus).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
    expect(screen.getByText("发现 Agents")).toBeInTheDocument();
    const startupNavigation = within(screen.getByLabelText("主导航"));
    for (const name of ["主页", "Agent", "路由", "模型", "用量"]) {
      expect(startupNavigation.getByRole("button", { name })).toBeDisabled();
    }
    expect(screen.getByRole("button", { name: "设置" })).toBeDisabled();
    expect(screen.queryByRole("button", { name: "切换颜色主题" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Claude Code" })).toBeNull();
    expect(screen.queryByRole("dialog", { name: "添加你的第一个模型" })).toBeNull();
    expect(screen.queryByTestId("agent-runtime-connection")).toBeNull();
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
    await waitFor(() => expect(emitServe).toBeTypeOf("function"));
    act(() => {
      runtime = serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 0,
        instance_id: "startup-instance",
      });
      emitServe?.(runtime);
    });
    expect(await screen.findByText("代理运行中")).toBeInTheDocument();

    await act(async () => resolveScan([scannedClaude]));

    await continueFromOverview(user);
    expect(await screen.findByRole("heading", { name: "模型" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "不再提示" }));
    await user.click(navigation().getByRole("button", { name: "Agent" }));
    expect(await screen.findByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
    const nav = within(screen.getByLabelText("主导航"));
    expect(nav.getAllByRole("button").map((item) => item.getAttribute("aria-label"))).toEqual([
      "主页",
      "Agent",
      "路由",
      "模型",
      "用量",
    ]);
    expect(screen.queryByRole("button", { name: "全局路由" })).toBeNull();
    expect(screen.getByRole("button", { name: "Claude Code" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Codex" })).toBeNull();
    expect(screen.getByText("代理运行中")).toBeInTheDocument();
    expect(screen.getByTestId("agent-runtime-connection")).toHaveTextContent("0 / 1 个已接管");
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
    expect(screen.queryByRole("dialog", { name: "添加你的第一个模型" })).toBeNull();
  });

  it("serve 事件早于 get_state 返回时保留最新运行代次并只扫描一次", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    let resolveState!: (state: StateView) => void;
    const pendingState = new Promise<StateView>((resolve) => {
      resolveState = resolve;
    });
    const latestRuntime = serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 9,
      instance_id: "runtime-latest",
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return pendingState;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      if (command === "get_cached_agent_views") return [scannedClaude];
      if (command === "get_runtime_state") return latestRuntime;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await waitFor(() => expect(emitServe).toBeTypeOf("function"));
    act(() => emitServe?.(latestRuntime));

    await act(async () => resolveState(stateFixture({
      serve: serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 1,
        instance_id: "runtime-stale-state",
      }),
    })));

    expect(await screen.findByText("代理运行中")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /代理运行中.*rev 9/ }))
      .toHaveAttribute("title", expect.stringContaining("rev 9"));
    await openAgents(user);
    expect(screen.getByRole("button", { name: "Claude Code" })).toBeInTheDocument();
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);

    // Repeating the latest instance must not look like a replacement. A truly
    // newer instance does refresh the cached Agent overlay exactly once.
    act(() => emitServe?.(latestRuntime));
    expect(invokeMock.mock.calls.filter(([command]) => command === "get_cached_agent_views"))
      .toHaveLength(0);
    act(() => emitServe?.({ ...latestRuntime, instance_id: "runtime-replacement" }));
    await waitFor(() => expect(
      invokeMock.mock.calls.filter(([command]) => command === "get_cached_agent_views"),
    ).toHaveLength(1));
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  });

  it("启动扫描失败时保留主页壳并提供明确的重新进入操作", async () => {
    window.localStorage.removeItem(FIRST_RUN_GUIDE_STORAGE_KEY);
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") throw new Error("agent discovery failed");
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);

    const startupStatus = await screen.findByRole("status", { name: "无法检查本机 Agent" });
    expect(startupStatus).toHaveAttribute("aria-busy", "false");
    expect(screen.getByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
    expect(screen.getByText("启动检查未完成，当前不会把失败结果当作空 Agent 列表。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新进入 Token Station" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Claude Code" })).toBeNull();
    expect(screen.queryByRole("dialog", { name: "添加你的第一个模型" })).toBeNull();
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  });

  it("启动扫描成功为空时进入正常主页而不是失败状态", async () => {
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);

    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "全局路由" })).toBeNull();
    expect(screen.queryByRole("status", { name: /检查本机 Agent/ })).toBeNull();
    expect(screen.queryByRole("button", { name: "重新进入 Token Station" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Claude Code" })).toBeNull();
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  });

  it("shows Home, Agent, and Routing as separate primary navigation entries", async () => {
    render(<App />);

    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    const nav = within(screen.getByLabelText("主导航"));
    for (const name of ["主页", "Agent", "路由", "模型", "用量"]) {
      expect(nav.getByRole("button", { name })).toBeInTheDocument();
    }
    expect(nav.queryByRole("button", { name: "设置" })).toBeNull();
    expect(screen.getByRole("button", { name: "设置" })).toHaveClass("station-settings-button");
    expect(nav.getByRole("button", { name: "主页" })).toHaveAttribute("aria-current", "page");
    expect(nav.getByRole("button", { name: "路由" })).toBeInTheDocument();
    expect(nav.getByRole("button", { name: "Agent" })).not.toHaveAttribute("aria-current");
    expect(screen.getByLabelText("主导航").querySelector(".station-nav-alert")).toBeNull();
    expect(nav.queryByRole("button", { name: "日志" })).toBeNull();

    expect(nav.getByRole("button", { name: "主页" })).toHaveAttribute("aria-current", "page");
    const snapshot = screen.getByRole("region", { name: "路由概览" });
    expect(within(snapshot).queryByTestId("revision-chain")).toBeNull();
    expect(within(snapshot).getByText("全局路由")).toBeInTheDocument();
    for (const tier of ["上档", "中档", "下档"]) {
      expect(within(snapshot).getByText(tier)).toBeInTheDocument();
    }
    expect(within(snapshot).queryByText("简单路由")).toBeNull();
    expect(within(snapshot).queryByText("额度优先")).toBeNull();
    expect(await screen.findByText(/成功率 91\.7% · P95 320ms/)).toBeInTheDocument();
    expect(getStatsMock).toHaveBeenCalledWith("24h", null);
  });

  it("returns to the last opened Agent after visiting another primary page", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgent(user, "Codex");
    await user.click(navigation().getByRole("button", { name: "主页" }));
    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();

    await user.click(navigation().getByRole("button", { name: "Agent" }));
    expect(await screen.findByRole("heading", { name: "Codex", level: 2 })).toBeInTheDocument();
    expect(within(screen.getByRole("navigation", { name: "发现 Agent 列表" }))
      .getByRole("button", { name: "Codex" })).toHaveAttribute("aria-current", "page");
  });

  it("returns Home after cancelling a provider flow opened from the status menu", async () => {
    let emitStatusMenuNavigate: ((target: string) => void) | undefined;
    listenMock.mockImplementation(async (eventName, handler) => {
      if (eventName === "status-menu-navigate") {
        emitStatusMenuNavigate = (target) => handler({ payload: target } as Parameters<typeof handler>[0]);
      }
      return () => undefined;
    });
    const user = userEvent.setup();
    render(<App />);

    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    await waitFor(() => expect(emitStatusMenuNavigate).toBeTypeOf("function"));
    act(() => emitStatusMenuNavigate?.("add-provider"));
    expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "返回" }));
    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    expect(navigation().getByRole("button", { name: "主页" }))
      .toHaveAttribute("aria-current", "page");
  });

  it("概览的单独路由快照展示已应用目标而不是三档", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture({
        routing_mode: "direct",
        direct_target: { upstream: "team-openai", model: "gpt-5.6" },
      });
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Token Station 主页" }));
    const snapshot = screen.getByRole("region", { name: "路由概览" });

    expect(within(snapshot).getByText("简单路由")).toBeInTheDocument();
    expect(within(snapshot).getByText("gpt-5.6")).toBeInTheDocument();
    expect(within(snapshot).getByText("team-openai")).toBeInTheDocument();
    expect(within(snapshot).queryByText("上档")).toBeNull();
    expect(within(snapshot).queryByText("未配置")).toBeNull();
  });

  it("概览的单独路由目标缺失时明确提示待选择", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture({
        routing_mode: "direct",
        direct_target: null,
      });
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Token Station 主页" }));
    const snapshot = screen.getByRole("region", { name: "路由概览" });
    expect(within(snapshot).getByText("待选择供应商")).toBeInTheDocument();
    expect(within(snapshot).getByText("待选择模型")).toBeInTheDocument();
  });

  it("概览的额度优先快照展示账户数与实际目标", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture({
        routing_mode: "quota_first",
        quota_accounts: [
          { upstream: "deepseek", model: "deepseek-chat" },
          { upstream: "team-openai", model: "gpt-5.6" },
        ],
      });
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await user.click(await screen.findByRole("button", { name: "Token Station 主页" }));
    const snapshot = screen.getByRole("region", { name: "路由概览" });
    expect(within(snapshot).getByText("额度优先")).toBeInTheDocument();
    expect(within(snapshot).getByText("2 个账户")).toBeInTheDocument();
    expect(within(snapshot).getByText("deepseek/deepseek-chat · team-openai/gpt-5.6"))
      .toBeInTheDocument();
    expect(within(snapshot).queryByText("上档")).toBeNull();
  });

  it("将操作错误放入左下可关闭 toast，不替换当前主页", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") throw new Error("proxy start failed");
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await user.click(await screen.findByRole("button", { name: /启动代理/ }));

    const alert = await screen.findByRole("alert");
    expect(alert.parentElement).toHaveClass("error-toast-viewport");
    expect(screen.getByRole("heading", { name: "概览" })).toBeInTheDocument();
    await user.click(within(alert).getByRole("button", { name: "关闭提示" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  });

  it("makes cost primary and exposes the three fixed workspace summaries", async () => {
    getStatsMock.mockResolvedValueOnce({
      ...statsFixture,
      total: {
        ...statsFixture.total,
        cost_micros: 2_340_000,
        priced_requests: 12,
        unpriced_requests: 0,
      },
    });

    render(<App />);

    await screen.findByRole("heading", { name: "概览" });

    const systemSummary = await screen.findByRole("region", { name: "系统摘要" });
    const costLabel = await within(systemSummary).findByText("近 24 小时成本");
    const costCard = costLabel.closest('[data-slot="card"]');
    expect(costCard).toHaveTextContent("近 24 小时成本$2.3412 次请求");
    expect(costCard).toHaveTextContent("成功率 91.7% · P95 320ms");
    expect(getStatsMock).toHaveBeenCalledWith("24h", null);
    expect(within(systemSummary).queryByText("今日请求")).toBeNull();
    expect(screen.queryByText("快捷键")).toBeNull();
    expect(screen.getByRole("region", { name: "Agent 概览" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "路由概览" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "模型概览" })).toBeInTheDocument();
    for (const name of ["打开 Agent", "打开路由", "打开模型"]) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    expect(screen.queryByText("下一步处理")).toBeNull();
    expect(screen.queryByRole("button", { name: "管理供应商" })).toBeNull();
    expect(screen.queryByRole("button", { name: "重新扫描 Agent" })).toBeNull();
    expect(screen.getByRole("button", { name: /启动代理.*127\.0\.0\.1:8787/ }))
      .toHaveAttribute("data-variant", "outline");
  });

  it("keeps Request logs in Settings and opens Usage management as a dedicated page", async () => {
    const user = userEvent.setup();
    render(<App />);

    await openRouting(user);
    await openAgents(user);
    await user.click(navigation().getByRole("button", { name: "模型" }));
    expect(screen.getByRole("dialog", { name: "选择模型接入方式" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(await screen.findByRole("heading", { name: "模型" })).toBeInTheDocument();
    expect(screen.queryByText("UPSTREAM CATALOG")).toBeNull();
    await user.click(navigation().getByRole("button", { name: "用量" }));
    expect(screen.queryByText("LOCAL RECEIPT LEDGER")).toBeNull();
    expect(screen.queryByRole("tablist", { name: "用量视图" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "预算与定价" }));
    expect(await screen.findByRole("heading", { name: "预算与定价管理" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /返回/ })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "设置" }));
    const settingsNavigation = screen.getByRole("navigation", { name: "设置分类" });
    await user.click(within(settingsNavigation).getByRole("button", { name: /请求日志/ }));
    expect(await screen.findByRole("heading", { name: "请求日志", level: 1 })).toBeInTheDocument();
    expect(screen.queryByText("LOCAL RECEIPTS")).toBeNull();
    expect(await screen.findByText("当前筛选范围没有请求日志。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "设置" })).toHaveAttribute("aria-current", "page");
  });

  it("keeps the Agent list visible while editing the selected Agent on the right", async () => {
    const user = userEvent.setup();
    render(<App />);

    await openAgents(user);
    const agentNavigation = screen.getByRole("navigation", { name: "发现 Agent 列表" });
    const claudeCodeButton = within(agentNavigation).getByRole("button", { name: "Claude Code" });
    expect(screen.queryByText("AGENT FLEET")).toBeNull();
    expect(screen.getByText("选择一个已发现的 Agent，查看详情并管理接入。")).toBeInTheDocument();
    expect(within(agentNavigation).queryByRole("button", { name: "全局路由" })).toBeNull();
    expect(within(claudeCodeButton).queryByText("claude-code")).toBeNull();
    expect(claudeCodeButton).toHaveAttribute("aria-current", "page");
    const centeredBrand = claudeCodeButton.querySelector('[data-agent-brand="claude-code"]');
    expect(centeredBrand).toBeInTheDocument();
    expect(centeredBrand?.querySelector("svg")).toBeInTheDocument();
    expect(claudeCodeButton.querySelector('[style*="background"]')).toBeNull();
    await user.click(claudeCodeButton);
    expect(await screen.findByRole("heading", { name: "Claude Code", level: 2 })).toBeInTheDocument();
    expect(screen.queryByText("AGENT ROUTE")).toBeNull();
    expect(screen.queryByText("DIRECT · ONE PROVIDER")).toBeNull();

    const updatedAgentNavigation = screen.getByRole("navigation", { name: "发现 Agent 列表" });
    await user.click(within(updatedAgentNavigation).getByRole("button", { name: "Codex" }));
    expect(await screen.findByRole("heading", { name: "Codex", level: 2 })).toBeInTheDocument();
    expect(within(screen.getByRole("navigation", { name: "发现 Agent 列表" })).getByRole("button", { name: "Codex" })).toHaveAttribute("aria-current", "page");
  });

  it("changes the selected Agent routing strategy from the detail workspace", async () => {
    const user = userEvent.setup();
    const initial = stateFixture();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return initial;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      if (command === "set_routing_mode") return {
        ...initial,
        agent_routes: {
          ...initial.agent_routes,
          "claude-code": { ...initial.agent_routes["claude-code"], routing_mode: "quota_first" },
        },
      };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgentRoute(user, "Claude Code");
    const strategyTabs = screen.getByRole("tablist", { name: "Agent 路由策略" });
    await user.click(within(strategyTabs).getByRole("tab", { name: "额度优先" }));

    expect(invokeMock).toHaveBeenCalledWith("set_routing_mode", { mode: "quota_first", agentId: "claude-code" });
    expect(within(strategyTabs).getByRole("tab", { name: "额度优先" })).toHaveAttribute("aria-selected", "true");
  });

  it("puts the real global routing-mode switch below the routing title", async () => {
    const user = userEvent.setup();
    const initial = stateFixture();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return initial;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      if (command === "set_routing_mode") return { ...initial, routing_mode: "quota_first" };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    expect(screen.queryByRole("tablist", { name: "路由模式" })).toBeNull();
    await openRouting(user);
    const modeTabs = screen.getByRole("tablist", { name: "路由模式" });
    await user.click(within(modeTabs).getByRole("tab", { name: "额度优先" }));

    expect(invokeMock).toHaveBeenCalledWith("set_routing_mode", { mode: "quota_first", agentId: null });
    expect(within(modeTabs).getByRole("tab", { name: "额度优先" })).toHaveAttribute("aria-selected", "true");
  });

  it("connects and applies a server-managed enterprise route in one action", async () => {
    const user = userEvent.setup();
    const account = { upstream: "kimi", model: "kimi-k3" };
    const initial = stateFixture({
      providers: [{
        name: "kimi",
        brand_id: "kimi",
        provider: "openai-compatible",
        base_url: "https://api.moonshot.cn/v1",
        models: ["kimi-k3"],
        has_auth: true,
      }],
      quota_accounts: [account],
    });
    const enterpriseProvider = {
      name: "enterprise_main",
      provider: "openai-compatible",
      base_url: "https://enterprise.example.com/v1",
      models: ["auto"],
      has_auth: true,
    };
    const added = stateFixture({
      ...initial,
      providers: [...initial.providers, enterpriseProvider],
    });
    const routed = stateFixture({
      ...added,
      routing_mode: "direct",
      direct_target: { upstream: "enterprise_main", model: "auto" },
    });
    const applying = stateFixture({
      ...routed,
      config_dirty: false,
      saved_revision: routed.draft_revision,
      serve: serveFixture({ phase: "starting" }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return initial;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      if (command === "verify_enterprise_route") {
        return {
          models: ["enterprise-chat", "enterprise-reasoner"],
          source: "live",
          fetched_at_ms: 1,
          warning: null,
        };
      }
      if (command === "add_managed_enterprise_route") return routed;
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    const scopes = screen.getByRole("region", { name: "路由范围" });
    await user.click(within(scopes).getByRole("button", { name: "企业路由" }));

    expect(await screen.findByRole("heading", { name: "企业路由" })).toBeInTheDocument();
    expect(screen.queryByRole("tablist", { name: "路由模式" })).toBeNull();
    expect(screen.getByRole("textbox", { name: "Base URL" })).toBeInTheDocument();
    expect(screen.getByLabelText("API Key")).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "Base URL" }), enterpriseProvider.base_url);
    await user.type(screen.getByLabelText("API Key"), "secret-key");
    await user.type(screen.getByRole("textbox", { name: "账户名称" }), enterpriseProvider.name);
    await user.click(screen.getByRole("button", { name: "接入并使用" }));

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("serve_start"));
    expect(invokeMock).toHaveBeenCalledWith("add_managed_enterprise_route", {
      name: enterpriseProvider.name,
      baseUrl: enterpriseProvider.base_url,
      apiKey: "secret-key",
    });
    expect(invokeMock).toHaveBeenCalledWith("verify_enterprise_route", {
      name: enterpriseProvider.name,
      baseUrl: enterpriseProvider.base_url,
      apiKey: "secret-key",
    });
    expect(invokeMock).not.toHaveBeenCalledWith("set_routing_mode", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("set_direct_route", expect.anything());
    expect(screen.getByText("企业路由已接入，正在应用配置…")).toBeInTheDocument();
    expect(screen.queryByText("额度优先")).toBeNull();
    expect(screen.queryByRole("button", { name: "保存并应用" })).toBeNull();
  });

  it("rejects cached enterprise verification and reloads authoritative state", async () => {
    const user = userEvent.setup();
    const initial = stateFixture();
    let stateReads = 0;
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") {
        stateReads += 1;
        return initial;
      }
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      if (command === "verify_enterprise_route") {
        return {
          models: ["private-model"],
          source: "cache",
          fetched_at_ms: 1,
          warning: "Provider rejected the API key",
        };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    const scopes = screen.getByRole("region", { name: "路由范围" });
    await user.click(within(scopes).getByRole("button", { name: "企业路由" }));
    await user.type(screen.getByRole("textbox", { name: "Base URL" }), "https://enterprise.example.com/v1");
    await user.type(screen.getByLabelText("API Key"), "invalid-key");
    await user.click(screen.getByRole("button", { name: "接入并使用" }));

    await waitFor(() => expect(stateReads).toBe(2));
    expect(invokeMock).not.toHaveBeenCalledWith("add_managed_enterprise_route", expect.anything());
    expect(invokeMock).not.toHaveBeenCalledWith("serve_start");
    expect(screen.getByText("接入未完成，请查看错误后重试。")).toBeInTheDocument();
    expect(screen.getByLabelText("API Key")).toHaveValue("invalid-key");
  });

  it("maps the four revision relationships to stable save copy", () => {
    expect(configSaveStatus(stateFixture(), "zh-CN")).toBe("无改动");
    expect(configSaveStatus(stateFixture({ config_dirty: true, draft_revision: 2, saved_revision: 1 }), "zh-CN")).toBe("有未保存更改");
    expect(configSaveStatus(stateFixture({
      saved_revision: 2,
      serve: serveFixture({ app_runtime: "running", listener_reachable: true, running_revision: 1 }),
    }), "zh-CN")).toBe("已保存尚未应用");
    expect(configSaveStatus(stateFixture({
      saved_revision: 2,
      serve: serveFixture({ app_runtime: "running", listener_reachable: true, running_revision: 2 }),
    }), "zh-CN")).toBe("运行中 revision 2");
    expect(configSaveStatus(stateFixture({ config_dirty: true }), "zh-TW")).toBe("有未儲存的變更");
    expect(configSaveStatus(stateFixture({ config_dirty: true }), "ja")).toBe("未保存の変更があります");
    expect(configSaveStatus(stateFixture(), "zh-TW")).toBe("沒有變更");
    expect(configSaveStatus(stateFixture(), "ja")).toBe("変更はありません");
  });

  it("shows the revision save state and makes 保存并应用 start a real apply", async () => {
    const user = userEvent.setup();
    const dirty = stateFixture({ draft_revision: 2, saved_revision: 1, config_dirty: true });
    const applying = stateFixture({
      draft_revision: 2,
      saved_revision: 2,
      config_dirty: false,
      serve: serveFixture({ phase: "starting" }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return dirty;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    expect(await screen.findByTestId("config-save-status")).toHaveTextContent("有未保存更改");
    await user.click(screen.getByRole("button", { name: "保存并应用" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("serve_start"));
    expect(invokeMock).not.toHaveBeenCalledWith("save_config");
  });

  it.each([
    [
      "the old running revision",
      serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 1,
      }),
      null,
    ],
    [
      "an apply error",
      serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 2,
        error: "已保存尚未应用：gateway_init: fixture failure",
      }),
      "已保存尚未应用：gateway_init: fixture failure",
    ],
  ])("does not report configuration applied when starting returns to %s", async (_case, terminal, expectedError) => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const dirty = stateFixture({ draft_revision: 2, saved_revision: 1, config_dirty: true });
    const applying = stateFixture({
      draft_revision: 2,
      saved_revision: 2,
      config_dirty: false,
      serve: serveFixture({ phase: "starting" }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return dirty;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "保存并应用" }));
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("正在应用配置…"))
      .toBeInTheDocument();
    expect(document.querySelector(".global-banner")).toBeNull();
    act(() => emitServe?.(terminal));

    if (expectedError) {
      await waitFor(() => expect(
        within(screen.getByTestId("error-toast-viewport")).queryByText("正在应用配置…"),
      ).toBeNull());
    } else {
      expect(within(screen.getByTestId("error-toast-viewport")).getByText("正在应用配置…"))
        .toBeInTheDocument();
    }
    expect(screen.queryByText(/配置已应用/)).toBeNull();
    if (expectedError) {
      expect(await within(screen.getByTestId("error-toast-viewport")).findByText(
        "操作未能完成。请重试；如果仍然失败，请从自救模式打开本地日志。",
      )).toBeInTheDocument();
    }
  });

  it("reports configuration applied only when the requested revision is running without an error", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const dirty = stateFixture({ draft_revision: 2, saved_revision: 1, config_dirty: true });
    const applying = stateFixture({
      draft_revision: 2,
      saved_revision: 2,
      config_dirty: false,
      serve: serveFixture({ phase: "starting" }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return dirty;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "保存并应用" }));
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("正在应用配置…"))
      .toBeInTheDocument();
    expect(document.querySelector(".global-banner")).toBeNull();
    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 2,
    })));

    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("配置已应用 · revision 2"))
      .toBeInTheDocument();
  });

  it("keeps waiting when a stale runtime poll arrives before the requested revision", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const dirty = stateFixture({ draft_revision: 2, saved_revision: 1, config_dirty: true });
    const applying = stateFixture({
      draft_revision: 2,
      saved_revision: 2,
      config_dirty: false,
      serve: serveFixture({ phase: "starting" }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return dirty;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return applying;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "保存并应用" }));
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("正在应用配置…"))
      .toBeInTheDocument();
    expect(document.querySelector(".global-banner")).toBeNull();
    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
    })));
    expect(within(screen.getByTestId("error-toast-viewport")).getByText("正在应用配置…"))
      .toBeInTheDocument();
    expect(screen.queryByText(/配置已应用/)).toBeNull();

    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 2,
    })));
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("配置已应用 · revision 2"))
      .toBeInTheDocument();
  });

  it("does not treat an ordinary first proxy startup as a completed configuration apply", async () => {
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") {
        return stateFixture({
          saved_revision: 1,
          serve: serveFixture({ phase: "starting" }),
        });
      }
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    expect(within(screen.getByTestId("error-toast-viewport")).queryByText("正在应用配置…"))
      .toBeNull();
    expect(document.querySelector(".global-banner")).toBeNull();
    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
    })));

    await waitFor(() => expect(screen.queryByText("正在应用配置…")).toBeNull());
    expect(screen.queryByText(/配置已应用/)).toBeNull();
  });

  it("refreshes the independent Agent runtime fact within the 500ms poll", async () => {
    const connected = stateFixture({
      serve: serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        agent_connected: true,
        running_revision: 1,
        instance_id: "runtime-a",
      }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return connected;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "get_runtime_state") {
        return serveFixture({
          phase: "running",
          app_runtime: "running",
          listener_reachable: true,
          agent_connected: false,
          running_revision: 1,
          instance_id: "runtime-a",
        });
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    expect(await screen.findByTestId("agent-runtime-connection")).toHaveTextContent("Agent：已连接");
    await waitFor(
      () => expect(screen.getByTestId("agent-runtime-connection")).toHaveTextContent("Agent：未连接"),
      { timeout: 1_500 },
    );
  });

  it("运行态轮询连续失败时用稳定错误弹窗去重", async () => {
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "get_runtime_state") throw new Error("runtime poll failed");
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    expect(await screen.findByRole("heading", { name: "概览" })).toBeInTheDocument();
    await waitFor(() => expect(
      invokeMock.mock.calls.filter(([command]) => command === "get_runtime_state").length,
    ).toBeGreaterThanOrEqual(2), { timeout: 1_800 });

    const alerts = within(screen.getByTestId("error-toast-viewport")).getAllByRole("alert");
    expect(alerts).toHaveLength(1);
    expect(alerts[0]).toHaveTextContent("操作未能完成");
  });

  it("运行时从未就绪转为就绪时只刷新缓存且不重复扫描", async () => {
    const notReady = stateFixture({
      serve: serveFixture({
        phase: "starting",
        app_runtime: "stopped",
        listener_reachable: false,
        running_revision: 1,
        instance_id: "runtime-a",
      }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return notReady;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      if (command === "get_cached_agent_views") return [scannedClaude];
      if (command === "get_runtime_state") {
        return serveFixture({
          phase: "running",
          app_runtime: "running",
          listener_reachable: true,
          running_revision: 1,
          instance_id: "runtime-a",
        });
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1));
    expect(await screen.findByRole("button", { name: /运行中.*停止/ })).toBeInTheDocument();
    await waitFor(() => expect(
      invokeMock.mock.calls.filter(([command]) => command === "get_cached_agent_views"),
    ).toHaveLength(1));
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  });

  it("运行实例替换时仍保持启动扫描只调用一次", async () => {
    const oldRuntime = stateFixture({
      serve: serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 1,
        instance_id: "runtime-old",
      }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return oldRuntime;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      if (command === "get_cached_agent_views") return [scannedClaude];
      if (command === "get_runtime_state") {
        return serveFixture({
          phase: "running",
          app_runtime: "running",
          listener_reachable: true,
          running_revision: 2,
          instance_id: "runtime-new",
        });
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await waitFor(() => expect(
      invokeMock.mock.calls.filter(([command]) => command === "scan_agents"),
    ).toHaveLength(1));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("get_runtime_state"));
    await waitFor(() => expect(
      invokeMock.mock.calls.filter(([command]) => command === "get_cached_agent_views"),
    ).toHaveLength(1));
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  });

  it("代理停止后用缓存清除启动时的已接入态且不重新扫描", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const running = stateFixture({
      serve: serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        instance_id: "runtime-a",
      }),
    });
    const connected = structuredClone(scannedClaude);
    connected.status = "CONNECTED";
    connected.installations[0].managed = true;
    connected.installations[0].connected = true;
    connected.installations[0].compatibility.status = "CONNECTED";
    const disconnected = structuredClone(scannedClaude);
    let runtime = running.serve;
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [connected];
      if (command === "get_cached_agent_views") return [disconnected];
      if (command === "get_runtime_state") return runtime;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgents(user);
    const agentButton = await screen.findByRole("button", { name: "Claude Code" });
    expect(agentButton).toHaveAttribute("title", "Claude Code · 已接入");

    act(() => {
      runtime = serveFixture();
      emitServe?.(runtime);
    });
    await waitFor(() => expect(agentButton).toHaveAttribute("title", "Claude Code · 可接入"));
    expect(invokeMock.mock.calls.filter(([command]) => command === "get_cached_agent_views"))
      .toHaveLength(1);
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  });

  it("可从主页手动重新扫描本机 Agent，并在失败时保留上次结果", async () => {
    const user = userEvent.setup();
    const initial = stateFixture();
    const scannedGemini = detectedAgentsFixture.find(
      (agent) => agent.metadata.agent_id === "gemini-cli",
    )!;
    let scanCalls = 0;
    let resolveRescan!: (agents: AgentView[]) => void;
    const pendingRescan = new Promise<AgentView[]>((resolve) => {
      resolveRescan = resolve;
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return initial;
      if (command === "get_runtime_state") return initial.serve;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") {
        scanCalls += 1;
        if (scanCalls === 1) return [scannedClaude];
        if (scanCalls === 2) return pendingRescan;
        throw new Error("manual rescan failed");
      }
      if (command === "get_request_receipts") {
        return { items: [], total: 0, page: 1, page_size: 20 };
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgents(user);

    const claudeButton = screen.getByRole("button", { name: "Claude Code" });
    expect(claudeButton).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Gemini CLI" })).toBeNull();
    await waitFor(() => expect(claudeButton).not.toHaveClass("agent-master-item-revealing"));

    await user.click(screen.getByRole("button", { name: "重新扫描" }));
    const scanningButton = screen.getByRole("button", { name: "扫描中…" });
    expect(scanningButton).toBeDisabled();
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(2);

    await act(async () => resolveRescan([scannedClaude, scannedGemini]));
    const geminiButton = await screen.findByRole("button", { name: "Gemini CLI" });
    expect(geminiButton).toHaveClass("agent-master-item-revealing");
    expect(geminiButton).toHaveStyle({ animationDelay: "0ms" });
    expect(claudeButton).not.toHaveClass("agent-master-item-revealing");
    expect(screen.getByRole("button", { name: "重新扫描" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "重新扫描" }));
    expect(await within(screen.getByTestId("error-toast-viewport")).findByRole("alert"))
      .toHaveTextContent("操作未能完成");
    expect(screen.getByRole("button", { name: "Gemini CLI" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "重新扫描" })).toBeEnabled();
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(3);
  });

  it("lets the user hide and restore a sidebar Agent from Settings without rescanning", async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole("heading", { name: "概览" });
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1));

    await openAgentVisibility(user);

    expect(screen.queryByText("AGENT NAVIGATION")).not.toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Agent 显示选项" })).toBeInTheDocument();
    for (const name of agentDisplayNames) {
      expect(screen.getByRole("switch", { name, checked: true })).toBeInTheDocument();
    }
    expect(screen.getByRole("status")).toHaveTextContent(`${agentIds.length} / ${agentIds.length} 已显示`);

    await user.click(screen.getByRole("switch", { name: "Codex", checked: true }));

    expect(screen.getByRole("switch", { name: "Codex", checked: false })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(`${agentIds.length - 1} / ${agentIds.length} 已显示`);
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify(["codex"]),
    );
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
    expect(invokeMock.mock.calls.filter(([command]) => [
      "set_settings",
      "serve_start",
      "serve_stop",
      "set_agent_route_mode",
      "plan_agent_connection",
      "plan_agent_disconnect",
      "apply_agent_plan",
    ].includes(String(command)))).toHaveLength(0);

    await user.click(screen.getByRole("switch", { name: "Codex", checked: false }));

    expect(screen.getByRole("switch", { name: "Codex", checked: true })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(`${agentIds.length} / ${agentIds.length} 已显示`);
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify([]),
    );
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents")).toHaveLength(1);
  });

  it("defaults undetected Agents off and lets the user show their honest empty state", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgentVisibility(user);

    expect(screen.getByRole("switch", { name: "Claude Code", checked: true }))
      .toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Gemini CLI", checked: false }))
      .toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(`1 / ${agentIds.length} 已显示`);

    await user.click(screen.getByRole("switch", { name: "Gemini CLI", checked: false }));
    expect(screen.getByRole("switch", { name: "Gemini CLI", checked: true }))
      .toBeInTheDocument();
    expect(window.localStorage.getItem(SHOWN_UNDETECTED_AGENT_IDS_STORAGE_KEY)).toBe(
      JSON.stringify(["gemini-cli"]),
    );
    expect(invokeMock.mock.calls.filter(([command]) => command === "scan_agents"))
      .toHaveLength(1);

    await user.click(navigation().getByRole("button", { name: "Agent" }));
    const gemini = await screen.findByRole("button", { name: "Gemini CLI" });
    expect(gemini).toHaveAttribute("title", "Gemini CLI · 未检测");
    expect(within(gemini).getByText("未检测")).toBeInTheDocument();

    await user.click(gemini);
    expect(await screen.findByRole("heading", { name: "Gemini CLI" })).toBeInTheDocument();
    expect(screen.getByText("没有在本机发现可管理的安装。")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "一键接入" })).toBeDisabled();
  });

  it("restores an explicit undetected Agent after remount", async () => {
    const user = userEvent.setup();
    window.localStorage.setItem(
      SHOWN_UNDETECTED_AGENT_IDS_STORAGE_KEY,
      JSON.stringify(["gemini-cli"]),
    );
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [scannedClaude];
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgents(user);

    expect(screen.getByRole("button", { name: "Gemini CLI" }))
      .toHaveAttribute("title", "Gemini CLI · 未检测");
  });

  it("restores hidden Agent preferences after remount while newly detected Agents remain visible", async () => {
    const user = userEvent.setup();
    const first = render(<App />);
    await openAgentVisibility(user);
    await user.click(screen.getByRole("switch", { name: "Codex", checked: true }));
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify(["codex"]),
    );
    first.unmount();

    const detectedVirtual = structuredClone(scannedClaude);
    detectedVirtual.metadata = registryWithVirtualSupportedAgent[
      registryWithVirtualSupportedAgent.length - 1
    ];
    detectedVirtual.installations[0].discovery.agent_id = "virtual-agent";
    detectedVirtual.installations[0].compatibility.agent_id = "virtual-agent";
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryWithVirtualSupportedAgent;
      if (command === "scan_agents") return [...detectedAgentsFixture, detectedVirtual];
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await openAgents(user);
    expect(screen.queryByRole("button", { name: "Codex" })).toBeNull();
    expect(screen.getByRole("button", { name: "Virtual Agent" })).toBeInTheDocument();
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify(["codex"]),
    );
  });

  it("applies the stored preference before the first navigation render", async () => {
    const user = userEvent.setup();
    window.localStorage.setItem(
      AGENT_VISIBILITY_STORAGE_KEY,
      JSON.stringify(["codex"]),
    );
    const container = document.createElement("div");
    document.body.append(container);
    let codexWasAdded = false;
    const observer = new MutationObserver((records) => {
      for (const record of records) {
        for (const node of record.addedNodes) {
          if (!(node instanceof Element)) continue;
          const buttons = node.matches("button")
            ? [node]
            : [...node.querySelectorAll("button")];
          if (buttons.some((button) => button.textContent?.trim() === "Codex")) {
            codexWasAdded = true;
          }
        }
      }
    });
    observer.observe(container, { childList: true, subtree: true });

    render(<App />, { container });
    await openAgents(user);
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("scan_agents"));
    observer.disconnect();
    expect(screen.queryByRole("button", { name: "Codex" })).toBeNull();
    expect(screen.getByRole("button", { name: "Claude Code" })).toBeInTheDocument();
    expect(codexWasAdded).toBe(false);
  });

  it("falls back to showing every supported Agent when the stored preference is invalid", async () => {
    const user = userEvent.setup();
    window.localStorage.setItem(AGENT_VISIBILITY_STORAGE_KEY, "{not-json");

    render(<App />);

    await openAgents(user);
    for (const name of agentNavigationNames) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    expect(screen.queryByRole("button", { name: /Future/i })).toBeNull();
  });

  it("does not overwrite an existing preference when the initial storage read fails", async () => {
    const user = userEvent.setup();
    window.localStorage.setItem(
      AGENT_VISIBILITY_STORAGE_KEY,
      JSON.stringify(["codex"]),
    );
    const originalGetItem = Storage.prototype.getItem;
    const setItemSpy = vi.spyOn(Storage.prototype, "setItem");
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(function (
      this: Storage,
      key: string,
    ) {
      if (key === AGENT_VISIBILITY_STORAGE_KEY) throw new Error("storage denied");
      return originalGetItem.call(this, key);
    });

    render(<App />);

    await openAgents(user);
    expect(screen.getByRole("button", { name: "Codex" })).toBeInTheDocument();
    expect(setItemSpy).not.toHaveBeenCalledWith(
      AGENT_VISIBILITY_STORAGE_KEY,
      expect.any(String),
    );
    expect(originalGetItem.call(
      window.localStorage,
      AGENT_VISIBILITY_STORAGE_KEY,
    )).toBe(JSON.stringify(["codex"]));
  });

  it("keeps the current-session visibility change when preference persistence fails", async () => {
    const user = userEvent.setup();
    render(<App />);
    await openAgentVisibility(user);
    vi.spyOn(Storage.prototype, "setItem").mockImplementationOnce(() => {
      throw new Error("storage denied");
    });

    await user.click(screen.getByRole("switch", { name: "Codex", checked: true }));

    expect(screen.getByRole("switch", { name: "Codex", checked: false })).toBeInTheDocument();
    await waitFor(
      () => expect(invokeMock).toHaveBeenCalledWith("get_runtime_state"),
      { timeout: 1_000 },
    );
    expect(within(screen.getByTestId("error-toast-viewport")).getByRole("alert"))
      .toHaveTextContent("Agent 显示已在本次会话生效，但无法保存到下次启动。");
    expect(document.querySelector(".settings-hub .banner.err")).toBeNull();
  });

  it("keeps a hidden Agent closed and returns to Home", async () => {
    const user = userEvent.setup();
    render(<App />);

    await openAgent(user, "Codex");

    await openAgentVisibility(user);
    await user.click(screen.getByRole("switch", { name: "Codex", checked: true }));
    await user.click(navigation().getByRole("button", { name: "Agent" }));

    expect(await screen.findByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "Codex" })).toBeNull();
  });

  it("supports toggling Agent visibility with the Space key", async () => {
    const user = userEvent.setup();
    render(<App />);
    await openAgentVisibility(user);

    const codexToggle = screen.getByRole("switch", { name: "Codex", checked: true });
    codexToggle.focus();
    expect(codexToggle).toHaveFocus();
    await user.keyboard("[Space]");

    expect(screen.getByRole("switch", { name: "Codex", checked: false })).toHaveFocus();
    await user.keyboard("[Enter]");

    expect(screen.getByRole("switch", { name: "Codex", checked: true })).toHaveFocus();
    expect(window.localStorage.getItem(AGENT_VISIBILITY_STORAGE_KEY)).toBe(
      JSON.stringify([]),
    );
  });

  it("keeps core navigation and the recovery controls available when every Agent is hidden", async () => {
    const user = userEvent.setup();
    render(<App />);
    await openAgentVisibility(user);

    for (const name of agentDisplayNames) {
      await user.click(screen.getByRole("switch", { name, checked: true }));
    }

    for (const name of agentNavigationNames) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
    expect(navigation().getByRole("button", { name: "Agent" })).toBeInTheDocument();
    expect(navigation().getByRole("button", { name: "路由" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "用量" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(`0 / ${agentIds.length} 已显示`);
    expect(screen.getByRole("switch", { name: "Claude Code", checked: false })).toBeInTheDocument();

    await user.click(screen.getByRole("switch", { name: "Claude Code", checked: false }));
    expect(screen.getByRole("status")).toHaveTextContent(`1 / ${agentIds.length} 已显示`);
  });

  it("keeps usage independent and exposes only user-facing categories inside Settings", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "get_stats") return { total: {}, groups: [], by: null, empty: true };
      if (command === "get_router_table") return { default_pool: "", assumed_context_window: 8192, threshold: null, rules: [], hint_routes: [], bands: [], pools: [] };
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await screen.findByRole("heading", { name: "概览" });

    const primaryNavigation = navigation();
    expect(primaryNavigation.queryByRole("button", { name: "设置" })).toBeNull();
    expect(screen.getByRole("button", { name: "设置" })).toHaveClass("station-settings-button");

    await user.click(screen.getByRole("button", { name: "用量" }));
    expect(await screen.findByRole("heading", { name: "用量统计" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "设置" }));
    expect(await screen.findByRole("heading", { name: "设置", level: 1 })).toBeInTheDocument();
    const settingsNavigation = screen.getByRole("navigation", { name: "设置分类" });
    expect(within(settingsNavigation).getByRole("button", { name: /通用/ })).toHaveAttribute("aria-current", "page");
    expect(within(settingsNavigation).queryByRole("button", { name: /路由表/ })).toBeNull();
    expect(within(settingsNavigation).queryByRole("button", { name: /插件/ })).toBeNull();
    expect(within(settingsNavigation).getByRole("button", { name: /Agent 显示/ })).toBeInTheDocument();
    expect(within(settingsNavigation).getByRole("button", { name: /外观/ })).toBeInTheDocument();
    expect(within(settingsNavigation).getByRole("button", { name: /语言/ })).toBeInTheDocument();
    expect(within(settingsNavigation).getByRole("button", { name: /请求日志/ })).toBeInTheDocument();
    expect(within(settingsNavigation).getByRole("button", { name: /关于/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /返回/ })).toBeNull();
    expect(screen.queryByRole("button", { name: /用量/ })).not.toBeNull();

    await user.click(navigation().getByRole("button", { name: "用量" }));
    expect(await screen.findByRole("heading", { name: "用量统计" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "Agent" }));
    expect(await screen.findByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
  });

  it("renders the primary desktop surface in English by default", async () => {
    const user = userEvent.setup();
    window.localStorage.removeItem(LANGUAGE_STORAGE_KEY);
    render(<App />);

    expect(await screen.findByRole("heading", { name: "Overview" })).toBeInTheDocument();
    await openRouting(user);
    expect(screen.getByRole("heading", { name: "Smart routing" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Save and apply" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "Models" }));
    expect(screen.getByRole("dialog", { name: "Choose how to add a model" })).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(await screen.findByRole("heading", { name: "Models", level: 1 })).toBeInTheDocument();
    expect(navigation().queryByRole("button", { name: "Add model" })).toBeNull();
    expect(screen.getByRole("button", { name: "Add model" })).toBeInTheDocument();
    expect(screen.queryByText("主页路由")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Usage" }));
    expect(await screen.findByRole("heading", { name: "Usage" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(await screen.findByRole("heading", { name: "Settings", level: 1 })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Agent visibility/ }));
    expect(await screen.findByRole("heading", { name: "Agent visibility" })).toBeInTheDocument();
    expect(screen.getByText(
      "Choose which Agents appear on Home. Undetected Agents are off by default.",
    )).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(`${agentIds.length} / ${agentIds.length} visible`);
    expect(screen.getByRole("group", { name: "Agent visibility options" })).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Codex", checked: true })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "Models" }));
    await user.click(within(screen.getByRole("dialog", { name: "Choose how to add a model" }))
      .getByRole("button", { name: "Choose provider first" }));
    expect(await screen.findByRole("heading", { name: "Add provider" })).toBeInTheDocument();
    expect(screen.getByText("MiniMax (China)")).toBeInTheDocument();
    expect(screen.queryByText("MiniMax（中国）")).not.toBeInTheDocument();
  });

  it("switches the whole interface to Simplified Chinese and persists the choice", async () => {
    const user = userEvent.setup();
    window.localStorage.removeItem(LANGUAGE_STORAGE_KEY);
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Settings" }));
    const languageButton = screen.getByRole("button", { name: /Language/ });
    expect(languageButton.querySelector(".lucide-globe")).not.toBeNull();
    expect(languageButton.querySelector(".lucide-languages")).toBeNull();
    await user.click(languageButton);
    expect(screen.getByRole("heading", { name: "Interface language" })).toBeInTheDocument();
    expect(screen.getAllByRole("radio")).toHaveLength(4);
    expect(screen.getByRole("radio", { name: /繁體中文/ })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /日本語/ })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /English/ })).toHaveAttribute("aria-checked", "true");

    await user.click(screen.getByRole("radio", { name: /简体中文/ }));

    expect(screen.getByRole("heading", { name: "设置", level: 1 })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "设置" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /简体中文/ })).toHaveAttribute("aria-checked", "true");
    await user.click(screen.getByRole("button", { name: /Agent 显示/ }));
    expect(await screen.findByRole("heading", { name: "Agent 显示" })).toBeInTheDocument();
    expect(screen.getByText(
      "选择显示在主页列表中的 Agent；未检测到的 Agent 默认关闭。",
    )).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(`${agentIds.length} / ${agentIds.length} 已显示`);
    expect(screen.getByRole("group", { name: "Agent 显示选项" })).toBeInTheDocument();
    await user.click(navigation().getByRole("button", { name: "Agent" }));
    expect(await screen.findByRole("heading", { name: "Agent 接入" })).toBeInTheDocument();
    expect(window.localStorage.getItem(LANGUAGE_STORAGE_KEY)).toBe("zh-CN");
    expect(document.documentElement).toHaveAttribute("lang", "zh-CN");
  });

  it("moves virtual key to Settings, masks it, and starts or stops the proxy from the top bar", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText");
    const running = stateFixture({
      serve: serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, running_revision: 1, instance_id: "instance", virtual_key: "vk-test-secret" }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      if (command === "serve_start") return running;
      if (command === "serve_stop") return stateFixture();
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await user.click(await screen.findByRole("button", { name: /启动/ }));
    expect(await screen.findByText("代理运行中")).toBeInTheDocument();
    const feedback = screen.getByTestId("error-toast-viewport");
    expect(within(feedback).getByRole("status")).toHaveTextContent("代理已启动");
    expect(document.querySelector(".global-banner")).toBeNull();
    expect(screen.queryByText("vk-test-secret")).toBeNull();
    await user.click(screen.getByRole("button", { name: "设置" }));
    expect(screen.getByLabelText("虚拟 API Key")).toHaveTextContent("••••");
    await user.click(screen.getByRole("button", { name: "复制" }));
    expect(writeText).toHaveBeenCalledWith("vk-test-secret");
    expect(await screen.findByRole("button", { name: "已复制" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "显示 15 秒" }));
    expect(screen.getByText("vk-test-secret")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /停止/ }));
    expect(await screen.findByRole("button", { name: /启动/ })).toBeInTheDocument();
    expect(within(feedback).getByRole("status")).toHaveTextContent("代理已停止");
    expect(document.querySelector(".global-banner")).toBeNull();
  });

  it("keeps an explicit proxy-start progress toast in the bottom-left until runtime is reachable", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const starting = stateFixture({
      serve: serveFixture({ phase: "starting" }),
    });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "serve_start") return starting;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await user.click(await screen.findByRole("button", { name: /启动/ }));

    const feedback = screen.getByTestId("error-toast-viewport");
    expect(within(feedback).getByRole("status")).toHaveTextContent("正在启动代理…");
    expect(document.querySelector(".global-banner")).toBeNull();
    expect(screen.queryByText("正在应用配置…")).toBeNull();

    act(() => emitServe?.(serveFixture({
      phase: "running",
      app_runtime: "running",
      listener_reachable: true,
      running_revision: 1,
      instance_id: "runtime-ready",
    })));

    expect(await within(feedback).findByText("代理已启动")).toBeInTheDocument();
    expect(document.querySelector(".global-banner")).toBeNull();
  });

  it("applies the home route to all Agents with a dedicated command", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (["get_state", "apply_home_route_to_all_agents"].includes(command)) return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "应用到全部 Agent" }));
    expect(invokeMock).toHaveBeenCalledWith("apply_home_route_to_all_agents");
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("全部 Agent 已恢复跟随全局路由"))
      .toBeInTheDocument();
  });

  it("lets one Agent switch to an independent route using the same tier selects", async () => {
    const user = userEvent.setup();
    const provider = { name: "deepseek", provider: "openai-compatible", base_url: "https://example.test/v1", models: ["deepseek-v4-pro"], has_auth: true };
    let current = stateFixture({ providers: [provider] });
    mockInvokeImplementation(async (command, args) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      if (command === "set_agent_route_mode") {
        const { agentId, mode } = args as { agentId: string; mode: "inherit" | "custom" };
        current = { ...current, agent_routes: { ...current.agent_routes, [agentId]: { ...current.agent_routes[agentId], mode } } };
        return current;
      }
      if (command === "set_agent_tier") return current;
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await openAgentRoute(user, "Codex");
    await user.click(screen.getByRole("radio", { name: "自定义三档" }));
    await user.click(screen.getByLabelText("上档供应商"));
    await user.click(screen.getByRole("option", { name: "deepseek" }));
    expect(invokeMock).toHaveBeenCalledWith("set_agent_tier", {
      agentId: "codex",
      slot: "high",
      upstream: "deepseek",
      model: "deepseek-v4-pro",
    });
  });

  it("returns home without promoting or discarding an incomplete Agent editor", async () => {
    const user = userEvent.setup();
    let current = stateFixture();
    mockInvokeImplementation(async (command, args) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      if (command === "set_agent_route_mode") {
        const { agentId, mode } = args as { agentId: string; mode: "inherit" | "custom" };
        current = {
          ...current,
          agent_routes: {
            ...current.agent_routes,
            [agentId]: {
              ...current.agent_routes[agentId],
              mode,
              config_error: mode === "custom"
                ? "Agent `codex` 的 high 档缺少供应商和模型"
                : null,
            },
          },
        };
        return current;
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);

    await openAgentRoute(user, "Codex");
    await user.click(screen.getByRole("radio", { name: "自定义三档" }));
    expect(screen.getByText("有一个路由尚未配置完整。请同时选择供应商和模型，然后重新保存。")).toBeInTheDocument();

    await user.click(navigation().getByRole("button", { name: "路由" }));

    expect(await screen.findByRole("region", { name: "路由模式" })).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "全局路由" })).toBeNull();
    expect(screen.queryByText(/配置结构不合法/)).toBeNull();
    expect(invokeMock).not.toHaveBeenCalledWith("save_agent_routes");

    await openAgentRoute(user, "Codex");
    expect(screen.getByRole("radio", { name: "自定义三档" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("button", { name: "保存并重启" })).toBeDisabled();
  });

  it("saves the home route as a reusable profile and mounts it from an Agent page", async () => {
    const user = userEvent.setup();
    const provider = { name: "deepseek", provider: "openai-compatible", base_url: "https://example.test/v1", models: ["deepseek-chat"], has_auth: true };
    const configuredTiers = {
      high: { upstream: "deepseek", model: "deepseek-chat" },
      mid: { upstream: "deepseek", model: "deepseek-chat" },
      low: { upstream: "deepseek", model: "deepseek-chat" },
    };
    let current = stateFixture({ providers: [provider], tiers: configuredTiers });
    mockInvokeImplementation(async (command, args) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      if (command === "save_home_route_as_profile") {
        current = { ...current, profiles: [(args as { name: string }).name], config_dirty: true, draft_revision: 1 };
        return current;
      }
      if (command === "mount_agent_profile") {
        const { agentId, profile } = args as { agentId: string; profile: string };
        current = {
          ...current,
          agent_routes: {
            ...current.agent_routes,
            [agentId]: { ...current.agent_routes[agentId], mode: "profile", profile, tiers: configuredTiers },
          },
          draft_revision: 2,
        };
        return current;
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "存为策略" }));
    await user.type(await screen.findByLabelText("策略组名称"), "日常开发");
    await user.click(screen.getByRole("button", { name: "保存策略" }));
    expect(invokeMock).toHaveBeenCalledWith("save_home_route_as_profile", { name: "日常开发" });
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("策略组“日常开发”已加入草稿，请保存并应用。"))
      .toBeInTheDocument();

    await openAgentRoute(user, "Codex");
    await user.click(screen.getByRole("radio", { name: "挂载策略组" }));
    expect(invokeMock).toHaveBeenCalledWith("mount_agent_profile", { agentId: "codex", profile: "日常开发" });
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("已挂载策略组「日常开发」· 尚待保存并应用"))
      .toBeInTheDocument();
    expect(screen.getByLabelText("当前策略组")).toHaveTextContent("日常开发");
  });

  it("在主页公开管理区删除未挂载策略组", async () => {
    const user = userEvent.setup();
    let current = stateFixture({ profiles: ["已停用"] });
    mockInvokeImplementation(async (command, args) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "delete_profile") {
        expect(args).toEqual({ name: "已停用" });
        current = { ...current, profiles: [] };
        return current;
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(screen.getByRole("button", { name: "删除策略组 已停用" }));

    expect(invokeMock).toHaveBeenCalledWith("delete_profile", { name: "已停用" });
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("策略组“已停用”已从草稿删除，请保存并应用。"))
      .toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "删除策略组 已停用" })).toBeNull();
  });

  it("serializes profile mutations with the rest of the home route commands", async () => {
    const user = userEvent.setup();
    let finishSave: ((value: ReturnType<typeof stateFixture>) => void) | undefined;
    const current = stateFixture();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return current;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "save_home_route_as_profile") {
        return new Promise((resolve) => {
          finishSave = resolve;
        });
      }
      if (command === "serve_start") return current;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openRouting(user);
    await user.click(await screen.findByRole("button", { name: "存为策略" }));
    await user.type(screen.getByLabelText("策略组名称"), "并发保护");
    await user.click(screen.getByRole("button", { name: "保存策略" }));

    expect(screen.getByRole("button", { name: "保存并应用" })).toBeDisabled();
    await user.click(screen.getByRole("button", { name: "保存并应用" }));
    expect(invokeMock).not.toHaveBeenCalledWith("serve_start");

    finishSave?.({ ...current, profiles: ["并发保护"] });
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("策略组“并发保护”已加入草稿，请保存并应用。"))
      .toBeInTheDocument();
  });

  it("opens Add Provider as a separate page and returns to the source page after saving", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (["get_state", "add_provider_with_credential"].includes(command)) return stateFixture();
      if (command === "preview_provider_endpoints") {
        return {
          chat: "https://api.openai.com/v1/chat/completions",
          responses: "https://api.openai.com/v1/responses",
          messages: "https://api.openai.com/v1/messages",
        };
      }
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return detectedAgentsFixture;
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await openAgents(user);
    await user.click(navigation().getByRole("button", { name: "模型" }));
    await user.click(within(screen.getByRole("dialog", { name: "选择模型接入方式" }))
      .getByRole("button", { name: "先选供应商" }));
    expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "添加供应商" })).toBeNull();
    // The provider picker is a brand-card catalog; click by visible label instead of selecting an option.
    await user.click(screen.getByText("OpenAI", { selector: ".provider-catalog-card-title strong" }));
    expect(screen.getByRole("button", { name: "添加供应商" })).toBeInTheDocument();
    await user.type(screen.getByLabelText("API Key"), "secret-test");
    await user.click(screen.getByRole("button", { name: "添加供应商" }));
    expect(await screen.findByRole("heading", { name: "模型" })).toBeInTheDocument();
    expect(within(screen.getByTestId("error-toast-viewport")).getByText("供应商已添加"))
      .toBeInTheDocument();
  }, 15_000);

  it("uses one provider catalog for regular and free APIs and restores the selected mode", async () => {
    const user = userEvent.setup();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [];
      if (command === "list_free_provider_presets") {
        return [{
          id: "nvidia",
          upstream_name: "nvidia_free",
          label: "NVIDIA API Catalog",
          short_label: "NV",
          base_url: "https://integrate.api.nvidia.com/v1",
          offer_kind: "recurring",
          region: "global",
          tags: ["长期免费", "全球平台", "开发用途"],
          free_note: "build.nvidia.com 托管 API",
          key_instruction: "打开模型页面并点击 Get API Key。",
          application_url: "https://build.nvidia.com/",
          docs_url: "https://docs.example.com/nvidia",
          verified_at: "2026-07-27",
          overage_policy: "rate_limited",
          models: [{
            id: "openai/gpt-oss-120b",
            label: "GPT-OSS 120B",
            tool: "declared",
            vision: "unknown",
            json_schema: "declared",
            context_window: 131072,
          }],
        }];
      }
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await user.click((await screen.findByRole("navigation", { name: "主导航" })).querySelector<HTMLButtonElement>('button[aria-label="模型"]')!);
    await user.click(within(screen.getByRole("dialog", { name: "选择模型接入方式" }))
      .getByRole("button", { name: "先选供应商" }));
    expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /常规 API/ })).toHaveAttribute("aria-pressed", "true");

    await user.click(screen.getByRole("button", { name: /免费 API/ }));
    expect(screen.getByRole("button", { name: /免费 API/ })).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByText("NVIDIA API Catalog")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /NVIDIA API Catalog/ }));
    expect(await screen.findByRole("heading", { name: "NVIDIA API Catalog" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "返回" }));
    expect(await screen.findByRole("heading", { name: "添加供应商" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /免费 API/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("NVIDIA API Catalog")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /常规 API/ }));
    expect(screen.getByRole("button", { name: /常规 API/ })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText("OpenAI", { selector: ".provider-catalog-card-title strong" })).toBeInTheDocument();
  }, 15_000);

  it("ensure 后 plan 失败仍只刷新缓存覆盖且不重新扫描", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const stopped = stateFixture();
    const running = stateFixture({
      serve: serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 1,
        instance_id: "instance-after-ensure",
      }),
    });
    let scans = 0;
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stopped;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") {
        scans += 1;
        return [scannedClaude];
      }
      if (command === "ensure_serve_running") {
        emitServe?.(running.serve);
        return running;
      }
      if (command === "plan_agent_connection") throw new Error("plan failed");
      if (command === "get_cached_agent_views") return [scannedClaude];
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await waitFor(() => expect(scans).toBe(1));
    await openAgent(user, "Claude Code");
    await user.click(await screen.findByRole("button", { name: "一键接入" }));
    await waitFor(() => expect(
      invokeMock.mock.calls.filter(([command]) => command === "get_cached_agent_views"),
    ).toHaveLength(1));

    expect(invokeMock.mock.calls
      .map(([command]) => command)
      .filter((command) => [
        "ensure_serve_running",
        "plan_agent_connection",
        "apply_agent_plan",
        "get_cached_agent_views",
      ].includes(command)))
      .toEqual([
        "ensure_serve_running",
        "plan_agent_connection",
        "get_cached_agent_views",
      ]);
    expect(scans).toBe(1);
  });

  it("applies the Connector plan directly on 一键接入", async () => {
    const user = userEvent.setup();
    let emitServe: ((serve: ServeView) => void) | undefined;
    listenMock.mockImplementation(async (_eventName, handler) => {
      emitServe = (serve) => handler({ payload: serve } as Parameters<typeof handler>[0]);
      return () => undefined;
    });
    const stopped = stateFixture();
    const running = stateFixture({ serve: serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, running_revision: 1, instance_id: "instance", virtual_key: "vk-test" }) });
    let scans = 0;
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stopped;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") { scans += 1; return [scannedClaude]; }
      if (command === "ensure_serve_running") {
        emitServe?.(running.serve);
        return running;
      }
      if (command === "get_cached_agent_views") return [scannedClaude];
      if (command === "plan_agent_connection") return {
        operation_id: "op-1",
        confirmation_token: "token-1",
        intent: "connect",
        target_config_path: "/tmp/settings.json",
        related_config_paths: [],
        human_diff: "~ /env/ANTHROPIC_BASE_URL: <设置受管值>\n~ /env/ANTHROPIC_AUTH_TOKEN: <敏感值已隐藏>",
        changes: [
          { operation: "replace", path: { segments: ["env", "ANTHROPIC_BASE_URL"] }, sensitive: false, summary: "<设置受管值>" },
          { operation: "replace", path: { segments: ["env", "ANTHROPIC_AUTH_TOKEN"] }, sensitive: true, summary: "<敏感值已隐藏>" },
        ],
      };
      if (command === "apply_agent_plan") return { operation_id: "op-1", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });
    render(<App />);
    await waitFor(() => expect(scans).toBe(1));
    await openAgent(user, "Claude Code");
    expect(screen.queryByRole("button", { name: /选择版本/ })).toBeNull();
    await user.click(await screen.findByRole("button", { name: "一键接入" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
      expectedVersion: "9.9.9",
    }));
    // There is no separate write-confirmation step; apply immediately after planning.
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", { operationId: "op-1", confirmationToken: "token-1" }));
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("Agent 已接入"))
      .toBeInTheDocument();
    expect(scans).toBe(1);
    expect(invokeMock.mock.calls.filter(([command]) => command === "get_cached_agent_views"))
      .toHaveLength(1);
    expect(invokeMock.mock.calls
      .map(([command]) => command)
      .filter((command) => [
        "ensure_serve_running",
        "plan_agent_connection",
        "apply_agent_plan",
        "get_cached_agent_views",
      ].includes(command)))
      .toEqual([
        "ensure_serve_running",
        "plan_agent_connection",
        "apply_agent_plan",
        "get_cached_agent_views",
      ]);
  });

  it("applies directly for an admitted state", async () => {
    const user = userEvent.setup();
    const running = stateFixture({ serve: serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, running_revision: 1, instance_id: "instance", virtual_key: "vk-test" }) });
    const admitted = defaultAdmittedClaude();
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [admitted];
      if (command === "ensure_serve_running") return running;
      if (command === "get_cached_agent_views") return [admitted];
      if (command === "plan_agent_connection") return projectionPlan("op-admitted", "token-admitted");
      if (command === "apply_agent_plan") return { operation_id: "op-admitted", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgent(user, "Claude Code");
    expect(await screen.findByText("可接入")).toBeInTheDocument();
    expect(screen.queryByText(/未经验证|试验性/)).toBeNull();
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
      expectedVersion: "2.1.210",
    }));
    // Apply immediately after planning without a confirmation step.
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("apply_agent_plan", {
      operationId: "op-admitted",
      confirmationToken: "token-admitted",
    }));
  });

  it("strips injected fields to the official config on 恢复官方配置并断开", async () => {
    const user = userEvent.setup();
    const connected = structuredClone(scannedClaude);
    connected.installations[0].managed = true;
    connected.installations[0].connected = true;
    connected.installations[0].compatibility.status = "CONNECTED";
    connected.status = "CONNECTED";
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return stateFixture();
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [
        connected,
        detectedAgentsFixture.find((agent) => agent.metadata.agent_id === "opencode")!,
      ];
      if (command === "get_cached_agent_views") return [
        connected,
        detectedAgentsFixture.find((agent) => agent.metadata.agent_id === "opencode")!,
      ];
      if (command === "get_agent_drift") return [];
      if (command === "force_forget_agent") return null;
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgent(user, "Claude Code");
    await user.click(await screen.findByRole("button", { name: "恢复官方配置并断开" }));
    // Restoring official config uses force_forget to remove injected fields without planning or confirming encrypted snapshot restoration.
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("force_forget_agent", {
      agentId: "claude-code",
      installationPath: "/opt/claude",
    }));
    expect(invokeMock).not.toHaveBeenCalledWith("plan_agent_disconnect", expect.anything());
    expect(await within(screen.getByTestId("error-toast-viewport")).findByText("已恢复官方配置并断开。"))
      .toBeInTheDocument();

    await openAgent(user, "OpenCode");
    expect(within(screen.getByTestId("error-toast-viewport")).getByText("已恢复官方配置并断开。"))
      .toBeInTheDocument();
    expect(document.querySelector(".agent-route-page > .banner.ok")).toBeNull();
  });

  it("keeps the selected Claude Code version after switching to another Agent and back", async () => {
    const user = userEvent.setup();
    const running = stateFixture({
      serve: serveFixture({
        phase: "running",
        app_runtime: "running",
        listener_reachable: true,
        running_revision: 1,
        instance_id: "instance",
        virtual_key: "vk-test",
      }),
    });
    const multipleClaude = structuredClone(scannedClaude);
    const preview = structuredClone(multipleClaude.installations[0]);
    preview.discovery.executable_path = "/opt/claude-preview";
    preview.discovery.canonical_path = "/opt/claude-preview";
    preview.discovery.version_raw = "10.0.0";
    preview.discovery.version_normalized = "10.0.0";
    preview.compatibility.installation_path = "/opt/claude-preview";
    multipleClaude.installations.push(preview);
    multipleClaude.status = "MULTIPLE_INSTALLATIONS";
    const detectedCodex = detectedAgentsFixture.find((agent) => agent.metadata.agent_id === "codex");
    expect(detectedCodex).toBeDefined();

    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "get_runtime_state") return running.serve;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [multipleClaude, detectedCodex];
      if (command === "get_request_receipts") return { items: [], total: 0, page: 1, page_size: 20 };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgents(user);
    const navigation = screen.getByRole("navigation", { name: "发现 Agent 列表" });
    await user.click(within(navigation).getByRole("button", { name: "Claude Code" }));
    await user.click(await screen.findByRole("button", { name: "选择版本" }));
    await user.click(screen.getByRole("option", { name: "claude-preview · v10.0.0" }));

    expect(screen.getByRole("button", { name: "一键接入" })).toBeEnabled();
    await user.click(within(screen.getByRole("navigation", { name: "发现 Agent 列表" })).getByRole("button", { name: "Codex" }));
    expect(await screen.findByRole("heading", { name: "Codex", level: 2 })).toBeInTheDocument();
    await user.click(within(screen.getByRole("navigation", { name: "发现 Agent 列表" })).getByRole("button", { name: "Claude Code" }));
    expect(await screen.findByRole("heading", { name: "Claude Code", level: 2 })).toBeInTheDocument();

    await user.click(await screen.findByRole("button", { name: "选择版本" }));
    expect(screen.getByRole("option", { name: "claude-preview · v10.0.0" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("button", { name: "一键接入" })).toBeEnabled();
  });

  it("selects an exact installation and plans against its path", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const secondInstallation = structuredClone(scannedClaude.installations[0]);
    secondInstallation.discovery.executable_path = "/Users/x/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe";
    secondInstallation.discovery.canonical_path = "/Users/x/.local/lib/node_modules/@anthropic-ai/claude-code/bin/claude.exe";
    secondInstallation.discovery.version_raw = "10.0.0";
    secondInstallation.discovery.version_normalized = "10.0.0";
    secondInstallation.discovery.binary_source = "npm_global";
    secondInstallation.discovery.upgrade_command = "npm install --global @anthropic-ai/claude-code@latest";
    secondInstallation.compatibility.installation_path = secondInstallation.discovery.canonical_path;
    const multipleClaude: AgentView = {
      ...scannedClaude,
      status: "MULTIPLE_INSTALLATIONS",
      installations: [scannedClaude.installations[0], secondInstallation],
    };
    const running = stateFixture({ serve: serveFixture({ phase: "running", app_runtime: "running", listener_reachable: true, running_revision: 1, instance_id: "instance", virtual_key: "vk-test" }) });
    mockInvokeImplementation(async (command) => {
      if (command === "get_state") return running;
      if (command === "list_agent_registry") return registryFixture;
      if (command === "scan_agents") return [multipleClaude];
      if (command === "ensure_serve_running") return running;
      if (command === "get_cached_agent_views") return [multipleClaude];
      if (command === "plan_agent_connection") return projectionPlan("op-2", "token-2");
      if (command === "apply_agent_plan") return { operation_id: "op-2", maintenance_warning: null };
      throw new Error(`unexpected IPC command: ${command}`);
    });

    render(<App />);
    await openAgent(user, "Claude Code");
    expect(await screen.findByRole("button", { name: "一键接入" })).toBeDisabled();
    expect(screen.getByText("检测到多份安装，请先选择要接管的精确路径。")).toBeInTheDocument();
    await user.click(await screen.findByRole("button", { name: /选择版本/ }));
    await user.click(screen.getByRole("option", { name: "claude.exe · v10.0.0" }));
    expect(screen.queryByRole("listbox")).toBeNull();
    await user.click(screen.getByRole("button", { name: "一键接入" }));
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith("plan_agent_connection", {
      agentId: "claude-code",
      installationPath: secondInstallation.discovery.canonical_path,
      expectedVersion: "10.0.0",
    }));
  });
});
