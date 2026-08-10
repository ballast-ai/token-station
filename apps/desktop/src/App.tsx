import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addKeyword,
  applyHomeRouteToAllAgents,
  deleteProfile,
  getRuntimeState,
  getState,
  listAgentRegistry,
  listFreeProviderPresets,
  listenServeState,
  removeKeyword,
  removeProvider,
  restoreProvider,
  scanAgents,
  saveHomeRouteAsProfile,
  serveStart,
  serveStop,
  setAdminEndpoint,
  setLocalRouting,
  setQuotaAccounts,
  setQuotaPlan,
  setRoutingMode,
  setTier,
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
  markFirstRunGuideDismissed,
  shouldOpenFirstRunGuide,
} from "./components/FirstRunGuide";
import {
  readHiddenAgentIds,
  updateHiddenAgentIds,
  writeHiddenAgentIds,
} from "./components/AgentVisibilityPreferences";
import {
  LanguageBoundary,
  useLanguage,
  type Language,
} from "./components/LanguageProvider";
import { ThemeBoundary } from "./components/ThemeProvider";
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
import "./App.css";
import { humanizeAppError } from "./errors";

function errorText(error: unknown): string {
  return humanizeAppError(error);
}

function hasErrorCode(error: unknown, code: string): boolean {
  return Boolean(error && typeof error === "object" && (error as { code?: unknown }).code === code);
}

function emptyAgentRoute(state: StateView): AgentRouteView {
  return {
    mode: "inherit",
    tiers: state.tiers,
    config_error: null,
    profile: null,
    routing_mode: state.routing_mode,
  };
}

export function configSaveStatus(state: StateView, language: Language = "en"): string {
  const chinese = language === "zh-CN";
  if (state.config_dirty) return chinese ? "有未保存更改" : "Unsaved changes";
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  if (runtimeHealthy && state.serve.running_revision !== state.saved_revision) {
    return chinese ? "已保存尚未应用" : "Saved, not applied";
  }
  if (runtimeHealthy && state.serve.running_revision === state.saved_revision) {
    return chinese
      ? `运行中 revision ${state.saved_revision}`
      : `Running revision ${state.saved_revision}`;
  }
  return chinese ? "无改动" : "No changes";
}

function StationApp() {
  const { language, copy } = useLanguage();
  const [state, setState] = useState<StateView | null>(null);
  const [view, setView] = useState<AppView>("overview");
  const [hiddenAgentIds, setHiddenAgentIds] = useState<Set<string>>(readHiddenAgentIds);
  const hiddenAgentIdsRef = useRef(hiddenAgentIds);
  const [registry, setRegistry] = useState<AgentUiMetadataView[]>([]);
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [scanBusy, setScanBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [serveBusy, setServeBusy] = useState(false);
  const [freeProviderBusy, setFreeProviderBusy] = useState(false);
  const [message, setMessage] = useState("");
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
  const [firstRunGuideOpen, setFirstRunGuideOpen] = useState(false);
  const [regularCatalogFilters, setRegularCatalogFilters] = useState<RegularCatalogFilters>({
    query: "",
    region: "all",
  });
  const busyRef = useRef(false);
  const scanRef = useRef(false);
  const scanQueuedRef = useRef(false);
  const scanGenerationRef = useRef(0);
  const pendingServeRef = useRef<ServeView | null>(null);
  const viewHistoryRef = useRef<AppView[]>([]);
  const pendingApplyRevisionRef = useRef<number | null>(null);
  const firstRunGuideCheckedRef = useRef(false);
  const runtimeObservationRef = useRef<{
    ready: boolean;
    instanceId: string | null;
  } | null>(null);

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
  const visibleRegistry = useMemo(
    () => orderedRegistry.filter((metadata) => !hiddenAgentIds.has(metadata.agent_id)),
    [hiddenAgentIds, orderedRegistry],
  );

  const setAgentVisible = useCallback((agentId: string, visible: boolean) => {
    const current = hiddenAgentIdsRef.current;
    const currentlyVisible = !current.has(agentId);
    if (currentlyVisible === visible) return;
    const next = updateHiddenAgentIds(current, agentId, !visible);
    hiddenAgentIdsRef.current = next;
    setHiddenAgentIds(next);
    writeHiddenAgentIds(next);
  }, []);

  const rescanAgents = useCallback(async () => {
    const requestedGeneration = ++scanGenerationRef.current;
    if (scanRef.current) {
      scanQueuedRef.current = true;
      return;
    }
    scanRef.current = true;
    setScanBusy(true);
    try {
      let generation = requestedGeneration;
      for (;;) {
        scanQueuedRef.current = false;
        try {
          const nextAgents = await scanAgents();
          if (generation === scanGenerationRef.current) setAgents(nextAgents);
        } catch (caught) {
          if (
            generation === scanGenerationRef.current
            && !hasErrorCode(caught, "scan_in_progress")
          ) {
            setError(errorText(caught));
          }
        }
        if (!scanQueuedRef.current) break;
        generation = scanGenerationRef.current;
      }
    } finally {
      scanRef.current = false;
      setScanBusy(false);
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const load = async () => {
      try {
        const [nextState, nextRegistry] = await Promise.all([getState(), listAgentRegistry()]);
        if (disposed) return;
        setState(pendingServeRef.current ? { ...nextState, serve: pendingServeRef.current } : nextState);
        setRegistry(nextRegistry);
        void rescanAgents();
      } catch (caught) {
        if (!disposed) setError(errorText(caught));
      }
    };

    void listenServeState((serve) => {
      pendingServeRef.current = serve;
      if (!disposed) setState((current) => current ? { ...current, serve } : current);
    }).then((stop) => {
      if (disposed) stop();
      else {
        unlisten = stop;
        void load();
      }
    }).catch((caught) => {
      if (!disposed) {
        setError(copy(
          `Failed to listen for proxy status: ${errorText(caught)}`,
          `代理状态监听失败：${errorText(caught)}`,
        ));
        void load();
      }
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [rescanAgents]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      void getRuntimeState()
        .then((serve) => {
          pendingServeRef.current = serve;
          setState((current) => current ? { ...current, serve } : current);
        })
        .catch(() => undefined);
    }, 500);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (state) setAdminEndpoint(state.serve);
  }, [state]);

  useEffect(() => {
    if (!state || firstRunGuideCheckedRef.current) return;
    firstRunGuideCheckedRef.current = true;
    if (shouldOpenFirstRunGuide()) setFirstRunGuideOpen(true);
  }, [state]);

  // Record a target revision only for an explicit Save and Apply. A normal
  // initial transition from starting to running is not a successful config apply.
  useEffect(() => {
    const phase = state?.serve.phase;
    if (!state) return undefined;
    const targetRevision = pendingApplyRevisionRef.current;
    if (targetRevision == null || phase === "starting") return undefined;

    if (
      phase === "running"
      && state.serve.running_revision === targetRevision
      && state.serve.error === null
    ) {
      pendingApplyRevisionRef.current = null;
      setMessage(copy(
        `Configuration applied · revision ${targetRevision}`,
        `配置已应用 · revision ${targetRevision}`,
      ));
      const timer = window.setTimeout(
        () => setMessage((current) => (
          current.startsWith("Configuration applied") || current.startsWith("配置已应用")
            ? ""
            : current
        )),
        2600,
      );
      return () => window.clearTimeout(timer);
    }
    // Falling back to the old instance after a failure also reports a running
    // phase; error is the authoritative failure signal. A late old
    // running_revision without an error may only be a reordered 500 ms poll, so
    // keep waiting for the target revision.
    if (state.serve.error !== null || phase !== "running") {
      pendingApplyRevisionRef.current = null;
    }
    return undefined;
  }, [state?.serve.error, state?.serve.phase, state?.serve.running_revision]);

  // Rescan once when runtime changes from not ready to ready. The initial app
  // scan can run before the gateway starts, so scan_agents receives runtime=None
  // and marks every installation disconnected, incorrectly showing managed
  // Agents as needing repair. The 500 ms header poll corrects runtime but not the
  // scan results, so rescan when ready to align cards with reality. rescanAgents
  // already deduplicates and queues requests.
  useEffect(() => {
    if (!state) return;
    const ready = state.serve.app_runtime === "running" && Boolean(state.serve.listener_reachable);
    const observation = {
      ready,
      instanceId: state.serve.instance_id,
    };
    const previous = runtimeObservationRef.current;
    runtimeObservationRef.current = observation;
    // The first observation (null) is not a ready transition. If runtime is
    // already ready then load() included it in the first scan. Rescan after a
    // real not-ready-to-ready transition or when the serving instance changes
    // while ready. The latter prevents Agent adapter readiness from staying on
    // the old instance after Applying.old -> Running(new).
    const becameReady = previous?.ready === false && ready;
    const servingInstanceChanged = Boolean(
      previous?.ready
        && ready
        && previous.instanceId
        && observation.instanceId
        && previous.instanceId !== observation.instanceId,
    );
    if (becameReady || servingInstanceChanged) {
      void rescanAgents();
    }
  }, [
    state?.serve.app_runtime,
    state?.serve.instance_id,
    state?.serve.listener_reachable,
    rescanAgents,
  ]);

  const showState = (next: StateView, nextMessage?: string) => {
    setState(next);
    setError("");
    if (nextMessage) setMessage(nextMessage);
  };

  const run = async (
    action: () => Promise<StateView>,
    ok?: string,
    recordApplyTarget = false,
  ): Promise<boolean> => {
    if (busyRef.current) return false;
    busyRef.current = true;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      const next = await action();
      if (recordApplyTarget) {
        pendingApplyRevisionRef.current = next.saved_revision;
      }
      showState(next, ok);
      return true;
    } catch (caught) {
      if (recordApplyTarget) pendingApplyRevisionRef.current = null;
      setError(errorText(caught));
      return false;
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const toggleServe = async () => {
    if (!state || serveBusy) return;
    pendingApplyRevisionRef.current = null;
    setServeBusy(true);
    setError("");
    setMessage("");
    try {
      const active = state.serve.app_runtime === "running" || state.serve.phase === "starting";
      showState(await (active ? serveStop() : serveStart()));
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      setServeBusy(false);
    }
  };

  const navigate = (next: AppView) => {
    if (freeProviderBusy) return;
    if (next === view) return;
    if (
      next === "usage" ||
      next === "quota-usage" ||
      next === "settings" ||
      next === "add-provider"
    ) {
      viewHistoryRef.current.push(view);
    } else {
      viewHistoryRef.current = [];
    }
    setView(next);
    setMessage("");
    setError("");
  };

  const navigateBack = () => {
    const previous = viewHistoryRef.current.pop() ?? "overview";
    if (
      previous.startsWith("agent:")
      && hiddenAgentIds.has(previous.slice("agent:".length))
    ) {
      viewHistoryRef.current = [];
      setView("overview");
    } else {
      setView(previous);
    }
    setError("");
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
      <div className="loading-screen">
        <span className="loading-mark" aria-hidden="true"><i /><i /><i /></span>
        <strong>{error
          ? copy("Unable to load Token Station", "无法加载 Token Station")
          : copy("Opening Token Station", "正在进入 Token Station")}</strong>
        {error && (
          <>
            <p>{error}</p>
            <button className="btn" type="button" onClick={() => window.location.reload()}>
              {copy("Retry", "重试")}
            </button>
          </>
        )}
      </div>
    );
  }

  const agentId = view.startsWith("agent:") ? view.slice("agent:".length) : null;
  const selectedAgentId = agentId ?? (view === "agents" ? visibleRegistry[0]?.agent_id : undefined);
  const metadata = selectedAgentId ? orderedRegistry.find((item) => item.agent_id === selectedAgentId) : undefined;
  const agent = selectedAgentId ? agents.find((item) => item.metadata.agent_id === selectedAgentId) : undefined;
  const route = selectedAgentId ? (state.agent_routes?.[selectedAgentId] ?? emptyAgentRoute(state)) : undefined;
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  const saveStatus = configSaveStatus(state, language);

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

  return (
    <AppShell
      view={view}
      serve={state.serve}
      registry={visibleRegistry}
      agents={agents}
      scanBusy={scanBusy}
      commandBusy={serveBusy || busy || freeProviderBusy}
      onNavigate={navigate}
      onRescan={() => void rescanAgents()}
      onToggleServe={() => void toggleServe()}
    >
      {state.serve.phase === "starting" && !error && (
        <div className="banner ok global-banner">
          {copy("Applying configuration…", "正在应用配置…")}
        </div>
      )}
      {message && state.serve.phase !== "starting" && <div className="banner ok global-banner">{message}</div>}
      {error && <div className="banner err global-banner">{error}</div>}
      {state.serve.error && <div className="banner err global-banner">{humanizeAppError(state.serve.error, language)}</div>}

      {view === "overview" && (
        <OverviewPage
          state={state}
          registry={visibleRegistry}
          agents={agents}
          onNavigate={navigate}
        />
      )}

      {view === "home" && (
        <HomePage
          providers={state.providers}
          tiers={state.tiers}
          profiles={state.profiles ?? []}
          routingMode={state.routing_mode}
          onSetRoutingMode={(mode) => void run(() => setRoutingMode(mode))}
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
              `策略组“${name}”已加入草稿，请保存并应用。`,
            ),
          )}
          onDeleteProfile={(name) => run(
            () => deleteProfile(name),
            copy(
              `Profile "${name}" removed from the draft. Save and apply to activate the change.`,
              `策略组“${name}”已从草稿删除，请保存并应用。`,
            ),
          )}
          onAddKeyword={(slot, keyword) => void run(() => addKeyword(slot, keyword))}
          onRemoveKeyword={(slot, keyword) => void run(() => removeKeyword(slot, keyword))}
          onSave={() => void run(serveStart, undefined, true)}
          onApplyAll={() => void run(
            applyHomeRouteToAllAgents,
            runtimeHealthy
              ? copy(
                  "All Agents now follow Home · pending apply",
                  "全部 Agent 已恢复跟随主页 · 尚待应用",
                )
              : copy("All Agents now follow Home", "全部 Agent 已恢复跟随主页"),
          )}
        />
      )}

      {(view === "agents" || agentId) && (
        <AgentsPage
          registry={visibleRegistry}
          agents={agents}
          selectedAgentId={selectedAgentId}
          scanBusy={scanBusy}
          onRescan={() => void rescanAgents()}
          onOpenAgent={(id) => navigate(`agent:${id}`)}
        >
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
              onRescan={rescanAgents}
              onSaveQuota={saveQuota}
              onSaveQuotaPlan={saveQuotaPlan}
              onViewQuotaUsage={() => navigate("quota-usage")}
              onSetRoutingMode={(mode) => void run(() => setRoutingMode(mode, metadata.agent_id))}
              embedded
            />
          )}
          {!metadata && (
            <section className="panel agent-master-empty">
              <div className="panel-head">
                <h2>{copy("No Agent selected", "未选择 Agent")}</h2>
                <p className="sub">{copy("Choose a visible Agent to manage its connection and route.", "请选择一个可见 Agent 管理接入和路由。")}</p>
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
          onRemove={(name) => void run(
            () => removeProvider(name),
            copy("Provider deleted", "供应商已删除"),
          )}
          onRestore={(name) => void run(
            () => restoreProvider(name),
            copy("Provider restored from the recycle bin", "供应商已从回收站恢复"),
          )}
          onStateChange={showState}
        />
      )}

      {(view === "usage" || view === "logs") && (
        <UsageWorkspace
          section={view === "logs" ? "logs" : "overview"}
          onSectionChange={(section) => navigate(section === "logs" ? "logs" : "usage")}
        />
      )}
      {view === "quota-usage" && (
        <QuotaUsagePage providers={state.providers} onBack={navigateBack} />
      )}
      {view === "settings" && (
        <SettingsHub
          settings={state.settings}
          serve={state.serve}
          registry={orderedRegistry}
          hiddenAgentIds={hiddenAgentIds}
          onAgentVisibilityChange={setAgentVisible}
          onOpenFirstRunGuide={() => {
            viewHistoryRef.current = [];
            setView("overview");
            setMessage("");
            setError("");
            setFirstRunGuideOpen(true);
          }}
          onSaved={showState}
        />
      )}
      {view === "add-provider" && (
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
          onSelectFree={(preset) => setView(`free-provider:${preset.id}`)}
          onAdded={(next, message) => {
            showState(next, message);
            setView(viewHistoryRef.current.pop() ?? "home");
          }}
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
            onAdded={(next, nextMessage) => {
              showState(next, nextMessage);
              setView("home");
            }}
          />
        ) : (
          <section className="panel free-catalog-state">
            <strong>{copy(
              "This free provider is no longer available or the catalog has changed.",
              "免费供应商不存在或目录已更新。",
            )}</strong>
            <button className="btn" type="button" onClick={() => setView("add-provider")}>
              {copy("Back to catalog", "返回目录")}
            </button>
          </section>
        );
      })()}
      <FirstRunGuide
        open={firstRunGuideOpen}
        onDismiss={() => {
          markFirstRunGuideDismissed();
          setFirstRunGuideOpen(false);
        }}
      />
    </AppShell>
  );
}

export default function App() {
  return (
    <LanguageBoundary>
      <ThemeBoundary>
        <StationApp />
      </ThemeBoundary>
    </LanguageBoundary>
  );
}
