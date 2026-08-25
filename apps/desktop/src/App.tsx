import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addKeyword,
  addManagedEnterpriseRoute,
  applyHomeRouteToAllAgents,
  deleteProfile,
  getCachedAgentViews,
  getRuntimeState,
  getState,
  listAgentRegistry,
  listFreeProviderPresets,
  listenServeState,
  listenStatusMenuNavigate,
  removeKeyword,
  removeProvider,
  restoreProvider,
  restartAgentRoute,
  scanAgents,
  saveHomeRouteAsProfile,
  serveStart,
  serveStop,
  setAdminEndpoint,
  setDirectRoute,
  setLocalRouting,
  setQuotaAccounts,
  setQuotaPlan,
  setRoutingMode,
  setTier,
  verifyEnterpriseRoute,
  type AgentRouteView,
  type AgentUiMetadataView,
  type AgentView,
  type FreeProviderPresetView,
  type QuotaAccount,
  type ServeView,
  type StateView,
  type TierSlot,
} from "./api";
import AppShell, { type AppView } from "./components/AppShell";
import FirstRunGuide, {
  FirstRunCompletionDialog,
  FirstRunTutorialPrompt,
  markFirstRunGuideDismissed,
  markFirstRunTutorialChoice,
  readFirstRunTutorialChoice,
  shouldOpenFirstRunGuide,
  shouldShowFirstRunTutorialPrompt,
  type FirstRunMicroStep,
  type FirstRunSetupStep,
} from "./components/FirstRunGuide";
import {
  readHiddenAgentIds,
  readShownUndetectedAgentIds,
  updateHiddenAgentIds,
  writeHiddenAgentIds,
  writeShownUndetectedAgentIds,
} from "./components/AgentVisibilityPreferences";
import {
  LanguageBoundary,
  localizedCopy,
  useLanguage,
  type Language,
} from "./components/LanguageProvider";
import { ThemeBoundary } from "./components/ThemeProvider";
import { ErrorToastBoundary, useErrorToast } from "./components/ErrorToast";
import AddProviderPage, {
  type FreeCatalogFilters,
  type ProviderCatalogMode,
  type RegularCatalogFilters,
} from "./pages/AddProviderPage";
import AgentsPage from "./pages/AgentsPage";
import AgentRoutePage from "./pages/AgentRoutePage";
import FreeProviderConfigPage from "./pages/FreeProviderConfigPage";
import HomePage from "./pages/HomePage";
import OverviewPage from "./pages/OverviewPage";
import ProvidersPage from "./pages/ProvidersPage";
import QuotaUsagePage from "./pages/QuotaUsagePage";
import SettingsHub from "./pages/SettingsHub";
import UsageWorkspace from "./pages/UsageWorkspace";
import BudgetPricingPage from "./pages/BudgetPricingPage";
import "./App.css";
import { humanizeAppError } from "./errors";
import { resolveStatusMenuNavigation } from "./statusMenuNavigation";

function errorText(error: unknown): string {
  return humanizeAppError(error);
}

function emptyAgentRoute(state: StateView): AgentRouteView {
  return {
    mode: "inherit",
    tiers: state.tiers,
    config_error: null,
    profile: null,
    routing_mode: state.routing_mode,
    direct_target: state.direct_target ?? null,
  };
}

function hasConnectedAgent(agents: AgentView[]): boolean {
  return agents.some((agent) =>
    agent.status === "CONNECTED"
      || agent.installations.some((installation) => installation.connected),
  );
}

const AGENT_REVEAL_CLEAR_MS = 520;

function firstIncompleteSetupStep(state: StateView, agents: AgentView[]): FirstRunSetupStep {
  if (!state.providers.some((provider) => provider.models.length > 0)) return "provider";
  const routeConfigured = state.routing_mode === "quota_first"
    ? (state.quota_accounts ?? []).some((account) => account.upstream && account.model)
    : state.routing_mode === "direct"
      ? Boolean(state.direct_target?.upstream && state.direct_target.model)
      : Object.values(state.tiers).every((tier) => tier.upstream && tier.model);
  const routeReady = routeConfigured
    && state.serve.app_runtime === "running"
    && state.serve.listener_reachable
    && state.serve.error === null
    && state.serve.running_revision === state.saved_revision
    && !state.config_dirty
    && state.config_error === null;
  if (!routeReady) return "route";
  return hasConnectedAgent(agents) ? "complete" : "agent";
}

export function firstRunRouteApplyComplete(
  state: StateView,
  targetRevision: number,
): boolean {
  return state.serve.phase === "running"
    && state.serve.app_runtime === "running"
    && state.serve.listener_reachable
    && state.serve.running_revision === state.saved_revision
    && state.saved_revision === targetRevision
    && state.serve.error === null
    && !state.config_dirty
    && state.config_error === null;
}

export function configSaveStatus(state: StateView, language: Language = "en"): string {
  if (state.config_dirty) return localizedCopy(
    language,
    "Unsaved changes",
    "有未保存更改",
    "有未儲存的變更",
    "未保存の変更があります",
  );
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  if (runtimeHealthy && state.serve.running_revision !== state.saved_revision) {
    return localizedCopy(
      language,
      "Saved, not applied",
      "已保存尚未应用",
      "已儲存但尚未套用",
      "保存済み、未適用",
    );
  }
  if (runtimeHealthy && state.serve.running_revision === state.saved_revision) {
    return localizedCopy(
      language,
      `Running revision ${state.saved_revision}`,
      `运行中 revision ${state.saved_revision}`,
      `執行中 revision ${state.saved_revision}`,
      `実行中の revision ${state.saved_revision}`,
    );
  }
  return localizedCopy(language, "No changes", "无改动", "沒有變更", "変更はありません");
}

export function firstProviderDefaultTarget(
  previousProviderCount: number,
  next: StateView,
): { upstream: string; model: string } | null {
  if (previousProviderCount !== 0 || next.direct_target?.upstream || next.direct_target?.model) return null;
  const provider = next.providers[0];
  const model = provider?.models[0];
  return provider && model ? { upstream: provider.name, model } : null;
}

function StartupHome({ error, onReload }: { error: string; onReload: () => void }) {
  const { copy } = useLanguage();
  const failed = Boolean(error);
  const statusLabel = failed
    ? copy("Unable to check local Agents", "无法检查本机 Agent", "無法檢查本機 Agent", "ローカルのエージェントを確認できません")
    : copy("Checking local Agents", "正在检查本机 Agent", "正在檢查本機 Agent", "ローカルのエージェントを確認中");

  return (
    <div className="startup-home agent-workspace-page">
      <header className="startup-heading">
        <div>
          <h1>{copy("Agent connection", "Agent 接入", "Agent 連線", "エージェント接続")}</h1>
          <p>{copy(
            "Discovering local Agents and preparing connection details.",
            "正在发现本机 Agent 并准备接入详情。", "正在發現本機 Agent 並準備連線細節。", "ローカルのエージェントを検出中で、接続の詳細を準備しています。"
          )}</p>
        </div>
      </header>

      <div className="startup-layout">
        <section
          className="startup-agent-card"
          aria-label={copy("Detected Agents", "发现 Agents", "檢測到的 Agent", "検出されたエージェント")}
        >
          <header className="startup-agent-card-header">
            <div>
              <h2>{copy("Detected Agents", "发现 Agents", "檢測到的 Agent", "検出されたエージェント")}</h2>
            </div>
            <span className="startup-count" aria-hidden="true">—</span>
          </header>
          <div className="startup-agent-card-body">
            <p className="startup-agent-pending-copy">{copy(
              "Detected Agents appear together when this startup check finishes.",
              "启动检查完成后，已发现的 Agent 会一次性出现。", "此啟動檢查完成後，檢測到的 Agent 會一次性出現。", "この起動チェックが完了すると、検出されたエージェントが一括して表示されます。"
            )}</p>
          </div>
        </section>

        <section
          className={`startup-status-panel${failed ? " startup-status-panel-failed" : ""}`}
          role="status"
          aria-live="polite"
          aria-busy={!failed}
          aria-label={statusLabel}
        >
          <h2>{statusLabel}</h2>
          <p>{failed
            ? copy(
                "The startup check did not complete. No empty Agent result has been applied.",
                "启动检查未完成，当前不会把失败结果当作空 Agent 列表。", "啟動檢查未完成，當前不會把失敗結果當作空 Agent 清單。", "起動チェックが完了していません。現在、失敗結果を空のエージェントリストとして扱いません。"
              )
            : copy(
                "Verifying installation locations, versions, and local configuration. This runs once per launch.",
                "正在核对安装位置、版本与本地配置；每次启动只执行一次。", "正在核對安裝位置、版本與本地配置；每次啟動只執行一次。", "インストール場所、バージョン、ローカル設定を確認しています。起動ごとに1回実行します。"
              )}</p>
          {!failed && (
            <div className="startup-discovery-track" aria-hidden="true">
              <span>{copy("ENTRY", "入口", "入口", "エントリ")}</span>
              <i><b /></i>
              <span>AGENT</span>
            </div>
          )}
          {failed && (
            <div className="startup-failure-actions">
              <p className="startup-error-detail">{error}</p>
              <button className="startup-reload-button" type="button" onClick={onReload}>
                {copy("Reload Token Station", "重新进入 Token Station", "重新進入 Token Station", "Token Station に再接続")}
              </button>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

function StationApp() {
  const { language, copy } = useLanguage();
  const { dismissToast, showError, showInfo, showSuccess } = useErrorToast();
  const [state, setState] = useState<StateView | null>(null);
  const [view, setView] = useState<AppView>("overview");
  const [modelEntryOpen, setModelEntryOpen] = useState(false);
  const [hiddenAgentIds, setHiddenAgentIds] = useState<Set<string>>(readHiddenAgentIds);
  const hiddenAgentIdsRef = useRef(hiddenAgentIds);
  const [shownUndetectedAgentIds, setShownUndetectedAgentIds] = useState<Set<string>>(
    readShownUndetectedAgentIds,
  );
  const shownUndetectedAgentIdsRef = useRef(shownUndetectedAgentIds);
  const [registry, setRegistry] = useState<AgentUiMetadataView[]>([]);
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [revealingAgentIds, setRevealingAgentIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [selectedInstallationPaths, setSelectedInstallationPaths] = useState<Record<string, string>>({});
  const [scanBusy, setScanBusy] = useState(false);
  const [scanSucceeded, setScanSucceeded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [serveBusy, setServeBusy] = useState(false);
  const [freeProviderBusy, setFreeProviderBusy] = useState(false);
  const [error, setError] = useState("");
  const [freePresets, setFreePresets] = useState<FreeProviderPresetView[]>([]);
  const [freeCatalogLoading, setFreeCatalogLoading] = useState(false);
  const [freeCatalogError, setFreeCatalogError] = useState("");
  const [freeCatalogFilters, setFreeCatalogFilters] = useState<FreeCatalogFilters>({
    query: "",
    offer: "all",
    region: "all",
  });
  const [providerCatalogMode, setProviderCatalogMode] = useState<ProviderCatalogMode>("regular");
  const [firstRunTutorialPromptOpen, setFirstRunTutorialPromptOpen] = useState(false);
  const [firstRunGuideOpen, setFirstRunGuideOpen] = useState(false);
  const [firstRunSetupStep, setFirstRunSetupStep] = useState<FirstRunSetupStep | null>(null);
  const [firstRunMicroStep, setFirstRunMicroStep] = useState<FirstRunMicroStep | null>(null);
  const [firstRunCompletion, setFirstRunCompletion] = useState<"connected" | "skipped" | null>(null);
  const [regularCatalogFilters, setRegularCatalogFilters] = useState<RegularCatalogFilters>({
    query: "",
    region: "all",
  });
  const busyRef = useRef(false);
  const scanBusyRef = useRef(false);
  const detectedAgentIdsRef = useRef<Set<string>>(new Set());
  const cachedAgentRefreshGenerationRef = useRef(0);
  const observedServeRef = useRef<{ ready: boolean; instanceId: string | null } | null>(null);
  const agentConnectInFlightRef = useRef(false);
  const pendingServeRef = useRef<ServeView | null>(null);
  const viewRef = useRef(view);
  const lastConnectionAgentIdRef = useRef<string | null>(null);
  const viewHistoryRef = useRef<AppView[]>([]);
  const pendingApplyRevisionRef = useRef<number | null>(null);
  const pendingServeActionRef = useRef<"start" | "stop" | null>(null);
  const firstRunGuideCheckedRef = useRef(false);
  const startupLoadStartedRef = useRef(false);
  const agentRevealTimerRef = useRef<number | null>(null);

  const revealAgents = useCallback((agentIds: string[]) => {
    if (agentIds.length === 0) return;
    if (agentRevealTimerRef.current !== null) {
      window.clearTimeout(agentRevealTimerRef.current);
    }
    setRevealingAgentIds(new Set(agentIds));
    agentRevealTimerRef.current = window.setTimeout(() => {
      agentRevealTimerRef.current = null;
      setRevealingAgentIds(new Set());
    }, AGENT_REVEAL_CLEAR_MS);
  }, []);

  useEffect(() => () => {
    if (agentRevealTimerRef.current !== null) {
      window.clearTimeout(agentRevealTimerRef.current);
    }
  }, []);

  const orderedRegistry = useMemo(
    () => registry
      .map((metadata, index) => ({ metadata, index }))
      // Cursor has a working OpenAI-compatible Agent route, but its private
      // settings file is not stable enough for an automatic connector. Keep it
      // visible so the user can open the route and configure Cursor manually.
      .filter(({ metadata }) =>
        metadata.admission === "supported" || metadata.agent_id === "cursor",
      )
      .sort((left, right) =>
        (left.metadata.ui_order ?? Number.MAX_SAFE_INTEGER)
          - (right.metadata.ui_order ?? Number.MAX_SAFE_INTEGER)
        || left.index - right.index)
      .map(({ metadata }) => metadata),
    [registry],
  );
  viewRef.current = view;
  const visibleAgentIds = useMemo(
    () => new Set(orderedRegistry
      .filter((metadata) => !hiddenAgentIds.has(metadata.agent_id) && (
        detectedAgentIdsRef.current.has(metadata.agent_id)
        || shownUndetectedAgentIds.has(metadata.agent_id)
      ))
      .map((metadata) => metadata.agent_id)),
    [agents, hiddenAgentIds, orderedRegistry, shownUndetectedAgentIds],
  );
  const visibleRegistry = useMemo(
    () => orderedRegistry.filter((metadata) => visibleAgentIds.has(metadata.agent_id)),
    [orderedRegistry, visibleAgentIds],
  );
  const statusMenuAgentIdsRef = useRef<ReadonlySet<string>>(visibleAgentIds);
  statusMenuAgentIdsRef.current = visibleAgentIds;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenStatusMenuNavigate((target) => {
      if (disposed) return;
      const next = resolveStatusMenuNavigation(target, statusMenuAgentIdsRef.current);
      if (!next) return;
      viewHistoryRef.current = next === "add-provider" ? [viewRef.current] : [];
      setView(next);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch((caught) => {
      if (!disposed) showError(errorText(caught), "status-menu-navigation-listener");
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [showError]);

  useEffect(() => {
    if (!scanSucceeded || !view.startsWith("agent:")) return;
    const selectedId = view.slice("agent:".length);
    if (!visibleRegistry.some((metadata) => metadata.agent_id === selectedId)) {
      viewHistoryRef.current = [];
      setView("agents");
    }
  }, [scanSucceeded, view, visibleRegistry]);

  useEffect(() => {
    if (view.startsWith("agent:")) {
      lastConnectionAgentIdRef.current = view.slice("agent:".length);
    }
  }, [view]);

  const setAgentVisible = useCallback((agentId: string, visible: boolean) => {
    const detected = detectedAgentIdsRef.current.has(agentId);
    const currentHidden = hiddenAgentIdsRef.current;
    const currentShown = shownUndetectedAgentIdsRef.current;
    const currentlyVisible = !currentHidden.has(agentId) && (
      detected || currentShown.has(agentId)
    );
    if (currentlyVisible === visible) return;
    const nextHidden = updateHiddenAgentIds(
      currentHidden,
      agentId,
      !visible && detected,
    );
    const nextShown = updateHiddenAgentIds(
      currentShown,
      agentId,
      visible && !detected,
    );
    hiddenAgentIdsRef.current = nextHidden;
    shownUndetectedAgentIdsRef.current = nextShown;
    setHiddenAgentIds(nextHidden);
    setShownUndetectedAgentIds(nextShown);
    const hiddenSaved = writeHiddenAgentIds(nextHidden);
    const shownSaved = writeShownUndetectedAgentIds(nextShown);
    if (!hiddenSaved || !shownSaved) {
      showError(copy(
        "Agent visibility changed for this session, but it could not be saved for the next launch.",
        "Agent 显示已在本次会话生效，但无法保存到下次启动。", "Agent 顯示已在本次會話生效，但無法儲存到下次啟動。", "エージェントの表示はこのセッションで有効ですが、次回起動には保存できません。"
      ), "agent-visibility-storage");
    }
  }, [copy, showError]);

  useEffect(() => {
    setSelectedInstallationPaths((current) => {
      const next = Object.fromEntries(Object.entries(current).filter(([agentId, path]) => (
        agents.some((agent) => (
          agent.metadata.agent_id === agentId
          && agent.installations.some((installation) => installation.discovery.canonical_path === path)
        ))
      )));
      return Object.keys(next).length === Object.keys(current).length ? current : next;
    });
  }, [agents]);

  const rescanAgents = useCallback(async () => {
    if (scanBusyRef.current) return;
    scanBusyRef.current = true;
    setScanBusy(true);
    cachedAgentRefreshGenerationRef.current += 1;
    try {
      const scannedAgents = await scanAgents();
      const detectedAgents = scannedAgents.filter((agent) => agent.installations.length > 0);
      const previousDetectedIds = detectedAgentIdsRef.current;
      const newlyDetectedIds = detectedAgents
        .filter((agent) => !previousDetectedIds.has(agent.metadata.agent_id))
        .map((agent) => agent.metadata.agent_id);
      detectedAgentIdsRef.current = new Set(
        detectedAgents.map((agent) => agent.metadata.agent_id),
      );
      setAgents(detectedAgents);
      setScanSucceeded(true);
      revealAgents(newlyDetectedIds);
    } catch (caught) {
      showError(errorText(caught), "agent-rescan");
    } finally {
      scanBusyRef.current = false;
      setScanBusy(false);
    }
  }, [revealAgents, showError]);

  const refreshCachedAgents = useCallback(async () => {
    const refreshGeneration = ++cachedAgentRefreshGenerationRef.current;
    try {
      const cached = await getCachedAgentViews();
      if (refreshGeneration !== cachedAgentRefreshGenerationRef.current) return;
      const detectedIds = detectedAgentIdsRef.current;
      setAgents(cached.filter((agent) => detectedIds.has(agent.metadata.agent_id)));
    } catch (caught) {
      if (refreshGeneration !== cachedAgentRefreshGenerationRef.current) return;
      showError(errorText(caught), "cached-agent-overlay");
    }
  }, [showError]);

  const observeServeRuntime = useCallback((serve: ServeView) => {
    const next = {
      ready: serve.app_runtime === "running" && serve.listener_reachable,
      instanceId: serve.instance_id,
    };
    const previous = observedServeRef.current;
    observedServeRef.current = next;

    if (detectedAgentIdsRef.current.size === 0 || !previous) return;
    if (!next.ready) {
      if (previous.ready && !agentConnectInFlightRef.current) {
        void refreshCachedAgents();
      }
      return;
    }

    const becameReady = !previous.ready;
    const instanceChanged = previous.ready && previous.instanceId !== next.instanceId;
    if ((becameReady || instanceChanged) && !agentConnectInFlightRef.current) {
      void refreshCachedAgents();
    }
  }, [refreshCachedAgents]);

  useEffect(() => {
    if (startupLoadStartedRef.current) return undefined;
    startupLoadStartedRef.current = true;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const load = async () => {
      const discoveryLoad = Promise.all([
        listAgentRegistry(),
        scanAgents(),
      ]).then(
        (value) => ({ ok: true as const, value }),
        (caught: unknown) => ({ ok: false as const, caught }),
      );
      try {
        const nextState = await getState();
        if (disposed) return;
        const effectiveServe = pendingServeRef.current ?? nextState.serve;
        observedServeRef.current = {
          ready: effectiveServe.app_runtime === "running" && effectiveServe.listener_reachable,
          instanceId: effectiveServe.instance_id,
        };
        setState(pendingServeRef.current ? { ...nextState, serve: effectiveServe } : nextState);

        const discoveryResult = await discoveryLoad;
        if (disposed) return;
        if (!discoveryResult.ok) {
          setError(errorText(discoveryResult.caught));
          return;
        }
        const [nextRegistry, scannedAgents] = discoveryResult.value;
        const detectedAgents = scannedAgents.filter((agent) => agent.installations.length > 0);
        detectedAgentIdsRef.current = new Set(
          detectedAgents.map((agent) => agent.metadata.agent_id),
        );
        setRegistry(nextRegistry);
        setAgents(detectedAgents);
        setScanSucceeded(true);
        revealAgents(detectedAgents.map((agent) => agent.metadata.agent_id));
      } catch (caught) {
        if (!disposed) setError(errorText(caught));
      }
    };

    void load();
    void listenServeState((serve) => {
      pendingServeRef.current = serve;
      observeServeRuntime(serve);
      if (!disposed) setState((current) => current ? { ...current, serve } : current);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch((caught) => {
      if (!disposed) showError(errorText(caught), "serve-state-listener");
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [observeServeRuntime, revealAgents, showError]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      void getRuntimeState()
        .then((serve) => {
          pendingServeRef.current = serve;
          observeServeRuntime(serve);
          setState((current) => current ? { ...current, serve } : current);
        })
        .catch((caught) => {
          showError(errorText(caught), "runtime-state-poll");
        });
    }, 500);
    return () => window.clearInterval(timer);
  }, [observeServeRuntime, showError]);

  useEffect(() => {
    if (state) setAdminEndpoint(state.serve);
  }, [state]);

  useEffect(() => {
    if (!state?.serve.error) return;
    const toastId = pendingServeActionRef.current
      ? "serve-toggle"
      : pendingApplyRevisionRef.current != null
        ? "config-apply"
        : `serve-runtime:${state.serve.error}`;
    if (pendingServeActionRef.current) pendingServeActionRef.current = null;
    showError(
      humanizeAppError(state.serve.error, language),
      toastId,
    );
  }, [language, showError, state?.serve.error]);

  useEffect(() => {
    const action = pendingServeActionRef.current;
    if (!state || !action || state.serve.error) return;
    if (
      action === "start"
      && state.serve.app_runtime === "running"
      && state.serve.listener_reachable
    ) {
      pendingServeActionRef.current = null;
      showSuccess(copy("Proxy started", "代理已启动", "代理已啟動", "プロキシが起動しました"), "serve-toggle");
    } else if (
      action === "stop"
      && state.serve.phase === "stopped"
      && state.serve.app_runtime === "stopped"
    ) {
      pendingServeActionRef.current = null;
      showSuccess(copy("Proxy stopped", "代理已停止", "代理已停止", "プロキシが停止しました"), "serve-toggle");
    }
  }, [copy, showSuccess, state?.serve.app_runtime, state?.serve.error, state?.serve.listener_reachable, state?.serve.phase]);

  useEffect(() => {
    if (!state || !scanSucceeded || firstRunGuideCheckedRef.current) return;
    firstRunGuideCheckedRef.current = true;
    if (shouldShowFirstRunTutorialPrompt()) {
      viewHistoryRef.current = [];
      setView("overview");
      setFirstRunTutorialPromptOpen(true);
    } else if (
      readFirstRunTutorialChoice() === "started"
      && shouldOpenFirstRunGuide()
    ) {
      viewHistoryRef.current = [];
      setView("overview");
      setFirstRunSetupStep(null);
      setFirstRunMicroStep("overview");
      setFirstRunGuideOpen(true);
    }
  }, [scanSucceeded, state]);

  useEffect(() => {
    if (firstRunSetupStep !== "agent" || !hasConnectedAgent(agents)) return;
    markFirstRunGuideDismissed();
    setFirstRunGuideOpen(false);
    setFirstRunSetupStep(null);
    setFirstRunMicroStep(null);
    setFirstRunCompletion("connected");
  }, [agents, firstRunSetupStep]);

  useEffect(() => {
    if (
      firstRunSetupStep === "agent"
      && firstRunMicroStep === "agent-scan-empty"
      && !hasConnectedAgent(agents)
      && agents.some((item) => item.installations.length > 0)
    ) {
      setFirstRunMicroStep("agent-select");
    }
  }, [agents, firstRunMicroStep, firstRunSetupStep]);

  // Only an explicit Save and Apply records a target revision. A normal first
  // startup transition from starting to running is not a successful config apply.
  useEffect(() => {
    const phase = state?.serve.phase;
    if (!state) return undefined;
    const targetRevision = pendingApplyRevisionRef.current;
    if (targetRevision == null || phase === "starting") return undefined;

    if (firstRunRouteApplyComplete(state, targetRevision)) {
      pendingApplyRevisionRef.current = null;
      if (firstRunSetupStep === "route") {
        viewHistoryRef.current = [];
        setFirstRunSetupStep("agent");
        setFirstRunMicroStep(
          agents.some((item) => item.installations.length > 0)
            ? "agent-select"
            : "agent-scan-empty",
        );
        setView(agents[0] ? `agent:${agents[0].metadata.agent_id}` : "agents");
        showSuccess(copy(
          "Routing is running. Connect an Agent next.",
          "路由已运行，接下来接入 Agent。", "路由已執行，接下來接入 Agent。", "ルーティングが実行中です。次にエージェントを接続してください。"
        ), "config-apply");
      } else {
        showSuccess(copy(
          `Configuration applied · revision ${targetRevision}`,
          `配置已应用 · revision ${targetRevision}`, `配置已應用 · revision ${targetRevision}`, `設定が適用されました · revision ${targetRevision}`
        ), "config-apply");
      }
      return undefined;
    }
    // Falling back to the old instance after a failure also reports a running
    // phase; error is the authoritative failure signal. A late old
    // running_revision without an error may only be a reordered 500 ms poll, so
    // keep waiting for the target revision.
    if (state.serve.error !== null) {
      pendingApplyRevisionRef.current = null;
    } else if (phase !== "running") {
      pendingApplyRevisionRef.current = null;
      dismissToast("config-apply");
    }
    return undefined;
  }, [
    agents,
    copy,
    dismissToast,
    firstRunSetupStep,
    showSuccess,
    state?.config_dirty,
    state?.config_error,
    state?.saved_revision,
    state?.serve.app_runtime,
    state?.serve.error,
    state?.serve.listener_reachable,
    state?.serve.phase,
    state?.serve.running_revision,
  ]);

  const showState = (next: StateView, nextMessage?: string) => {
    setState(next);
    if (nextMessage) showSuccess(nextMessage);
  };

  const run = async (
    action: () => Promise<StateView>,
    ok?: string,
    recordApplyTarget = false,
    reloadOnFailure = false,
  ): Promise<boolean> => {
    if (busyRef.current) return false;
    busyRef.current = true;
    setBusy(true);
    if (recordApplyTarget) {
      showInfo(copy("Applying configuration…", "正在应用配置…", "正在應用配置…", "設定を適用中…"), "config-apply");
    }
    try {
      const next = await action();
      if (recordApplyTarget) {
        pendingApplyRevisionRef.current = next.saved_revision;
      }
      showState(next, ok);
      return true;
    } catch (caught) {
      if (recordApplyTarget) pendingApplyRevisionRef.current = null;
      if (reloadOnFailure) {
        try {
          showState(await getState());
        } catch {
          // Keep the operation error authoritative. Normal polling can retry the reload.
        }
      }
      showError(errorText(caught), recordApplyTarget ? "config-apply" : undefined);
      return false;
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const toggleServe = async () => {
    if (!state || serveBusy) return;
    pendingApplyRevisionRef.current = null;
    dismissToast("config-apply");
    setServeBusy(true);
    const active = state.serve.app_runtime === "running" || state.serve.phase === "starting";
    const action = active ? "stop" : "start";
    pendingServeActionRef.current = action;
    showInfo(
      action === "start"
        ? copy("Starting proxy…", "正在启动代理…", "開始代理中…", "プロキシを開始中…")
        : copy("Stopping proxy…", "正在停止代理…", "停止代理中…", "プロキシを停止中…"),
      "serve-toggle",
    );
    try {
      const next = await (active ? serveStop() : serveStart());
      showState(next);
      const ready = next.serve.app_runtime === "running" && next.serve.listener_reachable;
      observedServeRef.current = { ready, instanceId: next.serve.instance_id };
      if (detectedAgentIdsRef.current.size > 0 && (active || ready)) {
        await refreshCachedAgents();
      }
    } catch (caught) {
      pendingServeActionRef.current = null;
      showError(errorText(caught), "serve-toggle");
    } finally {
      setServeBusy(false);
    }
  };

  const navigate = (next: AppView) => {
    if (freeProviderBusy) return;
    const lastConnectionAgentId = lastConnectionAgentIdRef.current;
    const target = next === "agents"
      && lastConnectionAgentId
      && visibleAgentIds.has(lastConnectionAgentId)
      ? `agent:${lastConnectionAgentId}` as AppView
      : next;
    if (
      target === "providers"
      && !firstRunGuideOpen
      && (state?.providers.reduce((total, provider) => total + provider.models.length, 0) ?? 0) === 0
    ) {
      setModelEntryOpen(true);
    }
    if (target === view) return;
    if (
      target === "usage" ||
      target === "usage-management" ||
      target === "quota-usage" ||
      target === "settings" ||
      target === "logs" ||
      target === "add-provider" ||
      target === "add-model"
    ) {
      viewHistoryRef.current.push(view);
    } else {
      viewHistoryRef.current = [];
    }
    setView(target);
  };

  const navigateBack = () => {
    const previous = viewHistoryRef.current.pop() ?? "overview";
    if (
      previous.startsWith("agent:")
      && !visibleAgentIds.has(previous.slice("agent:".length))
    ) {
      viewHistoryRef.current = [];
      setView("agents");
    } else {
      setView(previous);
    }
  };

  const loadFreeCatalog = async () => {
    setFreeCatalogLoading(true);
    setFreeCatalogError("");
    try {
      setFreePresets(await listFreeProviderPresets());
    } catch (caught) {
      setFreeCatalogError(errorText(caught));
    } finally {
      setFreeCatalogLoading(false);
    }
  };

  if (!state) {
    return (
      <div
        className="loading-screen"
        role="status"
        aria-live="polite"
        aria-busy={!error}
        aria-label={error
          ? copy("Unable to load Token Station", "无法加载 Token Station", "無法載入 Token Station", "Token Station を読み込めません")
          : copy("Opening Token Station", "正在进入 Token Station", "開啟 Token Station", "Token Station を開きます")}
      >
        <span className="loading-mark" aria-hidden="true"><i /><i /><i /></span>
        <strong>{error
          ? copy("Unable to load Token Station", "无法加载 Token Station", "無法載入 Token Station", "Token Station を読み込めません")
          : copy("Opening Token Station", "正在进入 Token Station", "開啟 Token Station", "Token Station を開きます")}</strong>
        {error && (
          <>
            <p>{error}</p>
            <button className="btn" type="button" onClick={() => window.location.reload()}>
              {copy("Retry", "重试", "重試", "再試行")}
            </button>
          </>
        )}
      </div>
    );
  }

  if (!scanSucceeded) {
    return (
      <AppShell
        view="home"
        serve={state.serve}
        registry={[]}
        agents={[]}
        commandBusy
        discoveryPending
        modelCount={0}
        modelEntryOpen={false}
        suppressModelEntryAutoOpen
        onModelEntryOpenChange={() => undefined}
        onNavigate={() => undefined}
        onToggleServe={() => undefined}
      >
        <StartupHome error={error} onReload={() => window.location.reload()} />
      </AppShell>
    );
  }

  const connectionAgentId = view.startsWith("agent:") ? view.slice("agent:".length) : null;
  const routeAgentId = view.startsWith("agent-route:") ? view.slice("agent-route:".length) : null;
  const selectedAgentId = connectionAgentId
    ?? routeAgentId
    ?? (view === "agents" ? visibleRegistry[0]?.agent_id : undefined);
  const agentWorkspaceMode = view === "home" || view === "enterprise-routing" || routeAgentId ? "routing" : "connections";
  const showAgentWorkspace = view === "home"
    || view === "enterprise-routing"
    || view === "agents"
    || Boolean(connectionAgentId)
    || Boolean(routeAgentId);
  const metadata = selectedAgentId ? orderedRegistry.find((item) => item.agent_id === selectedAgentId) : undefined;
  const agent = selectedAgentId ? agents.find((item) => item.metadata.agent_id === selectedAgentId) : undefined;
  const route = selectedAgentId ? (state.agent_routes?.[selectedAgentId] ?? emptyAgentRoute(state)) : undefined;
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  const saveStatus = configSaveStatus(state, language);
  const recommendedFirstRunStep = firstIncompleteSetupStep(state, agents);
  const activeFirstRunStep = firstRunSetupStep ?? recommendedFirstRunStep;
  const agentDetected = agents.some((item) => item.installations.length > 0);
  const activeFirstRunMicroStep = firstRunMicroStep
    ?? (activeFirstRunStep === "provider"
      ? "provider-entry"
      : activeFirstRunStep === "route"
        ? "route-entry"
        : activeFirstRunStep === "agent"
          ? (agentDetected ? "agent-entry" : "agent-scan-empty")
          : "complete");

  // Quota-first Save and Apply persists the account list before restarting the proxy.
  const saveQuota = (accounts: QuotaAccount[]) =>
    void run(async () => {
      await setQuotaAccounts(accounts);
      return serveStart();
    }, undefined, true);

  // Store provider quota plans in the draft for local estimates; the next Save and Apply activates them.
  const saveQuotaPlan = (
    upstream: string,
    lenMs: number,
    limit: number,
    unit: "tokens" | "requests",
  ) => void run(() => setQuotaPlan(upstream, lenMs, limit, unit, null));

  const handleProviderAdded = (next: StateView, message: string) => {
    const defaultTarget = firstProviderDefaultTarget(state.providers.length, next);
    if (!defaultTarget) {
      if (firstRunSetupStep === "provider") {
        showState(next, copy(
          "Provider added. Configure routing next.",
          "供应商已添加，接下来配置路由。", "供應商已加入。請配置路由。", "プロバイダーが追加されました。ルーティングを設定してください。"
        ));
        viewHistoryRef.current = [];
        setFirstRunSetupStep("route");
        setFirstRunMicroStep("route-mode");
        setView("home");
      } else {
        showState(next, message);
        setView(viewHistoryRef.current.pop() ?? "home");
      }
      return;
    }

    showState(next);
    void run(async () => {
      await setRoutingMode("direct");
      await setDirectRoute(defaultTarget.upstream, defaultTarget.model);
      return serveStart();
    }, copy(
      `Provider added. Global routing now uses ${defaultTarget.upstream} / ${defaultTarget.model}.`,
      `供应商已添加，全局路由已默认使用 ${defaultTarget.upstream} / ${defaultTarget.model}。`, `供應商已加入。全域性路由已預設使用 ${defaultTarget.upstream} / ${defaultTarget.model}。`, `プロバイダーが追加されました。グローバルルーティングは ${defaultTarget.upstream} / ${defaultTarget.model} がデフォルトです。`
    ), true);
    viewHistoryRef.current = [];
    if (firstRunSetupStep === "provider") {
      setFirstRunSetupStep("agent");
      setFirstRunMicroStep(agentDetected ? "agent-select" : "agent-scan-empty");
    }
    setView("agents");
  };

  return (
    <AppShell
      view={view}
      serve={state.serve}
      registry={visibleRegistry}
      agents={agents}
      commandBusy={serveBusy || busy || freeProviderBusy}
      modelCount={state.providers.reduce((total, provider) => total + provider.models.length, 0)}
      modelEntryOpen={modelEntryOpen}
      suppressModelEntryAutoOpen={firstRunGuideOpen}
      onModelEntryOpenChange={setModelEntryOpen}
      onNavigate={navigate}
      onToggleServe={() => void toggleServe()}
    >
      {view === "overview" && (
        <OverviewPage
          state={state}
          registry={visibleRegistry}
          agents={agents}
          onNavigate={navigate}
        />
      )}

      {showAgentWorkspace && (
        <AgentsPage
          mode={agentWorkspaceMode}
          registry={visibleRegistry}
          agents={agents}
          revealingAgentIds={revealingAgentIds}
          selectedAgentId={selectedAgentId}
          homeSelected={view === "home"}
          enterpriseSelected={view === "enterprise-routing"}
          scanBusy={scanBusy}
          onOpenHome={() => navigate("home")}
          onOpenEnterprise={() => navigate("enterprise-routing")}
          onRescan={() => void rescanAgents()}
          onOpenAgent={(id) => {
            navigate(agentWorkspaceMode === "routing" ? `agent-route:${id}` : `agent:${id}`);
            if (agentWorkspaceMode === "connections" && firstRunGuideOpen && activeFirstRunMicroStep === "agent-select") {
              const selected = agents.find((item) => item.metadata.agent_id === id);
              setFirstRunMicroStep(
                (selected?.installations.length ?? 0) > 1
                  ? "agent-installation"
                  : "agent-connect",
              );
            }
          }}
        >
          {agentWorkspaceMode === "routing" && !routeAgentId && (
            <HomePage
              providers={state.providers}
              tiers={state.tiers}
              profiles={state.profiles ?? []}
              routingMode={state.routing_mode}
              directTarget={state.direct_target ?? null}
              onSetRoutingMode={(mode) => void run(() => setRoutingMode(mode))}
              onApplyDirect={(upstream, model) => void run(async () => {
                await setDirectRoute(upstream, model);
                return serveStart();
              }, undefined, true)}
              quotaAccounts={state.quota_accounts ?? []}
              onSaveQuota={saveQuota}
              onSaveQuotaPlan={saveQuotaPlan}
              onViewQuotaUsage={() => navigate("quota-usage")}
              busy={busy}
              applying={state.serve.phase === "starting"}
              configError={state.config_error ? humanizeAppError(state.config_error, language) : null}
              keywords={state.keywords}
              saveStatus={saveStatus}
              localOnly={state.local_only}
              allowCloudFallback={state.allow_cloud_fallback}
              onSetLocalRouting={(localOnly, allowCloudFallback) => void run(() => setLocalRouting(localOnly, allowCloudFallback))}
              onTierChange={(slot: TierSlot, upstream, model) => void run(() => setTier(slot, upstream, model))}
              onSaveProfile={(name) => run(
                () => saveHomeRouteAsProfile(name),
                copy(
                  `Profile "${name}" added to the draft. Save and apply to activate it.`,
                  `策略组“${name}”已加入草稿，请保存并应用。`, `策略組「${name}」已加入草稿。請儲存並應用。`, `プロファイル「${name}」が下書きに追加されました。保存して適用してください。`
                ),
              )}
              onDeleteProfile={(name) => run(
                () => deleteProfile(name),
                copy(
                  `Profile "${name}" removed from the draft. Save and apply to activate the change.`,
                  `策略组“${name}”已从草稿删除，请保存并应用。`, `策略組「${name}」已從草稿刪除。請儲存並應用。`, `プロファイル「${name}」が下書きから削除されました。保存して適用してください。`
                ),
              )}
              onAddKeyword={(slot, keyword) => void run(() => addKeyword(slot, keyword))}
              onRemoveKeyword={(slot, keyword) => void run(() => removeKeyword(slot, keyword))}
              onSave={() => void run(serveStart, undefined, true)}
              onApplyAll={() => void run(
                applyHomeRouteToAllAgents,
                runtimeHealthy
                  ? copy(
                      "All Agents now follow global routing · pending apply",
                      "全部 Agent 已恢复跟随全局路由 · 尚待应用", "全部 Agent 已恢復跟隨全域性路由 · 尚待應用", "すべての Agent がグローバルルーティングに従うようになりました · 適用を待機中"
                    )
                  : copy("All Agents now follow global routing", "全部 Agent 已恢复跟随全局路由", "全部 Agent 已恢復跟隨全域性路由", "すべての Agent がグローバルルーティングに従うようになりました"),
              )}
              onEnterpriseConnect={(connection) => run(async () => {
                  const discovery = await verifyEnterpriseRoute(
                    connection.name,
                    connection.baseUrl,
                    connection.apiKey,
                  );
                  if (discovery.source !== "live") {
                    throw new Error(discovery.warning ?? "Live credential verification failed");
                  }
                  await addManagedEnterpriseRoute(
                    connection.name,
                    connection.baseUrl,
                    connection.apiKey,
                  );
                  return serveStart();
                }, undefined, true, true)}
              embedded
              scope={view === "enterprise-routing" ? "enterprise" : "global"}
            />
          )}
          {metadata && route && (
            <AgentRoutePage
              key={metadata.agent_id}
              metadata={metadata}
              agent={agent}
              route={route}
              profiles={state.profiles ?? []}
              providers={state.providers}
              quotaAccounts={state.quota_accounts ?? []}
              serveRunning={runtimeHealthy}
              applying={state.serve.phase === "starting"}
              onStateChange={showState}
              onRefreshAgents={refreshCachedAgents}
              onConnectInFlightChange={(inFlight) => {
                agentConnectInFlightRef.current = inFlight;
              }}
              onSaveQuota={saveQuota}
              onSaveQuotaPlan={saveQuotaPlan}
              onViewQuotaUsage={() => navigate("quota-usage")}
              onSetRoutingMode={(mode) => void run(() => setRoutingMode(mode, metadata.agent_id))}
              onApplyDirect={(upstream, model) => run(async () => {
                await setDirectRoute(upstream, model, metadata.agent_id);
                return restartAgentRoute(metadata.agent_id);
              })}
              onDeleteProfile={(name) => run(
                () => deleteProfile(name),
                copy(`Profile "${name}" deleted`, `已删除策略组“${name}”`, `已刪除策略組「${name}」`, `プロファイル「${name}」が削除されました`),
              )}
              selectedInstallationPath={(() => {
                const selectedPath = selectedInstallationPaths[metadata.agent_id];
                if (agent?.installations.some((installation) => installation.discovery.canonical_path === selectedPath)) {
                  return selectedPath;
                }
                return agent?.installations.length === 1
                  ? agent.installations[0].discovery.canonical_path
                  : "";
              })()}
              onInstallationPathChange={(path) => {
                setSelectedInstallationPaths((current) => ({ ...current, [metadata.agent_id]: path }));
              }}
              onInstallationSelected={() => {
                if (agentWorkspaceMode === "connections" && firstRunGuideOpen && activeFirstRunMicroStep === "agent-installation") {
                  setFirstRunMicroStep("agent-connect-multiple");
                }
              }}
              pageMode={agentWorkspaceMode === "connections" ? "connection" : "routing"}
              embedded
            />
          )}
          {!metadata && (agentWorkspaceMode === "connections" || Boolean(routeAgentId)) && (
            <section className="panel agent-master-empty">
              <div className="panel-head">
                <h2>{copy("No Agent selected", "未选择 Agent", "未選擇 Agent", "Agent を選択していません")}</h2>
                <p className="sub">{copy("Choose a visible Agent.", "请选择一个可见 Agent。", "請選擇一個可見 Agent。", "表示されている Agent を選択してください。")}</p>
              </div>
            </section>
          )}
        </AgentsPage>
      )}

      {view === "providers" && (
        <ProvidersPage
          providers={state.providers}
          deletedProviders={state.deleted_providers ?? []}
          recoveryError={state.provider_recovery_error ?? null}
          serveRunning={runtimeHealthy}
          busy={busy}
          onRemove={(name) => run(
            () => removeProvider(name),
            copy("Provider deleted", "供应商已删除", "供應商已刪除", "プロバイダーが削除されました"),
          )}
          onRestore={(name) => void run(
            () => restoreProvider(name),
            copy("Provider restored from the recycle bin", "供应商已从回收站恢复", "供應商已從回收站恢復", "プロバイダーがゴミ箱から復元されました"),
          )}
          onStateChange={showState}
          onAddProvider={() => {
            if (firstRunGuideOpen) navigate("add-provider");
            else setModelEntryOpen(true);
          }}
        />
      )}

      {view === "usage" && (
        <UsageWorkspace onOpenManagement={() => navigate("usage-management")} />
      )}
      {view === "usage-management" && (
        <BudgetPricingPage onBack={navigateBack} />
      )}
      {view === "quota-usage" && (
        <QuotaUsagePage providers={state.providers} onBack={navigateBack} />
      )}
      {(view === "settings" || view === "logs") && (
        <SettingsHub
          settings={state.settings}
          serve={state.serve}
          registry={orderedRegistry}
          visibleAgentIds={visibleAgentIds}
          onAgentVisibilityChange={setAgentVisible}
          onOpenFirstRunGuide={() => {
            viewHistoryRef.current = [];
            setView("overview");
            setFirstRunSetupStep(null);
            setFirstRunMicroStep("overview");
            setFirstRunGuideOpen(true);
          }}
          onSaved={showState}
          initialSection={view === "logs" ? "request-logs" : "general"}
        />
      )}
      {(view === "add-provider" || view === "add-model") && (
        <AddProviderPage
          existingNames={state.providers.map((provider) => provider.name)}
          onCancel={navigateBack}
          catalogMode={providerCatalogMode}
          onCatalogModeChange={setProviderCatalogMode}
          regularFilters={regularCatalogFilters}
          onRegularFiltersChange={setRegularCatalogFilters}
          freePresets={freePresets}
          freeLoading={freeCatalogLoading}
          freeError={freeCatalogError}
          freeFilters={freeCatalogFilters}
          onFreeFiltersChange={setFreeCatalogFilters}
          onLoadFree={() => void loadFreeCatalog()}
          onProviderSelected={() => {
            if (firstRunGuideOpen && activeFirstRunMicroStep === "provider-choice") {
              setFirstRunMicroStep("provider-credential");
            }
          }}
          onSelectFree={(preset) => setView(`free-provider:${preset.id}`)}
          onAdded={handleProviderAdded}
          onStateChanged={showState}
          entryMode={view === "add-model" ? "model-first" : "provider-first"}
        />
      )}
      {view.startsWith("free-provider:") && (() => {
        const presetId = view.slice("free-provider:".length);
        const preset = freePresets.find((item) => item.id === presetId);
        return preset ? (
          <FreeProviderConfigPage
            key={preset.id}
            preset={preset}
            onBack={() => setView("add-provider")}
            onBusyChange={setFreeProviderBusy}
            onAdded={handleProviderAdded}
          />
        ) : (
          <section className="panel free-catalog-state">
            <strong>{copy(
              "This free provider is no longer available or the catalog has changed.",
              "免费供应商不存在或目录已更新。", "此免費供應商已無法使用或目錄已更新。", "この無料プロバイダーは利用できなくなりましたか、またはカタログが変更されました。"
            )}</strong>
            <button className="btn" type="button" onClick={() => setView("add-provider")}>
              {copy("Back to catalog", "返回目录", "返回目錄", "カタログに戻る")}
            </button>
          </section>
        );
      })()}
      <FirstRunTutorialPrompt
        open={firstRunTutorialPromptOpen}
        onStart={() => {
          markFirstRunTutorialChoice("started");
          setFirstRunTutorialPromptOpen(false);
          viewHistoryRef.current = [];
          setView("overview");
          setFirstRunSetupStep(null);
          setFirstRunMicroStep("overview");
          setFirstRunGuideOpen(true);
        }}
        onDecline={() => {
          markFirstRunTutorialChoice("declined");
          setFirstRunTutorialPromptOpen(false);
        }}
      />
      <FirstRunGuide
        open={firstRunGuideOpen}
        microStep={activeFirstRunMicroStep}
        canSkipAgent={scanSucceeded && !scanBusy && !agentDetected}
        onBack={() => {
          if (activeFirstRunMicroStep === "provider-models") {
            setFirstRunMicroStep("provider-credential");
          } else if (activeFirstRunMicroStep === "provider-save") {
            setFirstRunMicroStep("provider-models");
          } else if (activeFirstRunMicroStep === "route-config") {
            setFirstRunMicroStep("route-mode");
          } else if (activeFirstRunMicroStep === "route-apply") {
            setFirstRunMicroStep("route-config");
          } else if (activeFirstRunMicroStep === "agent-installation") {
            setFirstRunMicroStep("agent-select");
          } else if (activeFirstRunMicroStep === "agent-connect") {
            setFirstRunMicroStep("agent-select");
          } else if (activeFirstRunMicroStep === "agent-connect-multiple") {
            setFirstRunMicroStep("agent-installation");
          }
        }}
        onTargetAction={() => {
          if (activeFirstRunMicroStep === "overview") {
            viewHistoryRef.current = [];
            setView(
              recommendedFirstRunStep === "provider"
                ? "providers"
                : recommendedFirstRunStep === "agent"
                  ? "agents"
                  : "home",
            );
            setFirstRunSetupStep(null);
            setFirstRunMicroStep(null);
          } else if (activeFirstRunMicroStep === "provider-entry") {
            setFirstRunSetupStep("provider");
            setFirstRunMicroStep("provider-choice");
          } else if (activeFirstRunMicroStep === "provider-credential") {
            setFirstRunMicroStep("provider-models");
          } else if (activeFirstRunMicroStep === "provider-models") {
            setFirstRunMicroStep("provider-save");
          } else if (activeFirstRunMicroStep === "route-entry") {
            setFirstRunSetupStep("route");
            setFirstRunMicroStep("route-mode");
          } else if (activeFirstRunMicroStep === "route-mode") {
            setFirstRunMicroStep("route-config");
          } else if (activeFirstRunMicroStep === "route-config") {
            setFirstRunMicroStep("route-apply");
          } else if (activeFirstRunMicroStep === "agent-entry") {
            setFirstRunSetupStep("agent");
            setFirstRunMicroStep(agentDetected ? "agent-discovery-scope" : "agent-scan-empty");
          } else if (activeFirstRunMicroStep === "agent-discovery-scope") {
            setFirstRunMicroStep("agent-select");
          } else if (activeFirstRunMicroStep === "complete") {
            markFirstRunGuideDismissed();
            setFirstRunGuideOpen(false);
            setFirstRunSetupStep(null);
            setFirstRunMicroStep(null);
            viewHistoryRef.current = [];
            setView("agents");
          }
        }}
        onSkipAgent={() => {
          markFirstRunGuideDismissed();
          setFirstRunGuideOpen(false);
          setFirstRunSetupStep(null);
          setFirstRunMicroStep(null);
          setFirstRunCompletion("skipped");
        }}
        onPause={() => {
          setFirstRunGuideOpen(false);
          setFirstRunSetupStep(null);
          setFirstRunMicroStep(null);
          window.requestAnimationFrame(() => {
            document.querySelector<HTMLElement>("[data-onboarding-return-focus]")?.focus();
          });
        }}
        onDismiss={() => {
          markFirstRunGuideDismissed();
          setFirstRunGuideOpen(false);
          setFirstRunSetupStep(null);
          setFirstRunMicroStep(null);
        }}
      />
      <FirstRunCompletionDialog
        open={firstRunCompletion !== null}
        agentSkipped={firstRunCompletion === "skipped"}
        onFinish={() => {
          setFirstRunCompletion(null);
          viewHistoryRef.current = [];
          setView("agents");
        }}
      />
    </AppShell>
  );
}

export default function App() {
  return (
    <ErrorToastBoundary>
      <LanguageBoundary>
        <ThemeBoundary>
          <StationApp />
        </ThemeBoundary>
      </LanguageBoundary>
    </ErrorToastBoundary>
  );
}
