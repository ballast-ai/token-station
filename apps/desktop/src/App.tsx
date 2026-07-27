import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addKeyword,
  applyHomeRouteToAllAgents,
  getRuntimeState,
  getState,
  listAgentRegistry,
  listFreeProviderPresets,
  listenServeState,
  removeKeyword,
  removeProvider,
  restoreProvider,
  scanAgents,
  serveStart,
  serveStop,
  setAdminEndpoint,
  setLocalRouting,
  setTier,
  type AgentRouteView,
  type AgentUiMetadataView,
  type AgentView,
  type FreeProviderPresetView,
  type ServeView,
  type StateView,
  type TierSlot,
} from "./api";
import AppShell, { type AppView } from "./components/AppShell";
import { LanguageBoundary } from "./components/LanguageProvider";
import AddProviderPage, {
  type FreeCatalogFilters,
  type ProviderCatalogMode,
  type RegularCatalogFilters,
} from "./pages/AddProviderPage";
import AgentRoutePage from "./pages/AgentRoutePage";
import FreeProviderConfigPage from "./pages/FreeProviderConfigPage";
import HomePage from "./pages/HomePage";
import SettingsHub from "./pages/SettingsHub";
import Stats from "./pages/Stats";
import "./App.css";

function errorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object") {
    const value = error as { message?: unknown; code?: unknown };
    return [value.message, value.code && `code=${value.code}`].filter(Boolean).map(String).join(" · ");
  }
  return String(error);
}

function hasErrorCode(error: unknown, code: string): boolean {
  return Boolean(error && typeof error === "object" && (error as { code?: unknown }).code === code);
}

function emptyAgentRoute(state: StateView): AgentRouteView {
  return { mode: "inherit", tiers: state.tiers, config_error: null, profile: null };
}

export function configSaveStatus(state: StateView): string {
  if (state.config_dirty) return "有未保存更改";
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  if (runtimeHealthy && state.serve.running_revision !== state.saved_revision) {
    return "已保存尚未应用";
  }
  if (runtimeHealthy && state.serve.running_revision === state.saved_revision) {
    return `运行中 revision ${state.saved_revision}`;
  }
  return "无改动";
}

function StationApp() {
  const [state, setState] = useState<StateView | null>(null);
  const [view, setView] = useState<AppView>("home");
  const [registry, setRegistry] = useState<AgentUiMetadataView[]>([]);
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [scanBusy, setScanBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [serveBusy, setServeBusy] = useState(false);
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
  const prevPhaseRef = useRef<string | null>(null);
  const runtimeReadyRef = useRef<boolean | null>(null);

  const orderedRegistry = useMemo(
    () => registry
      .map((metadata, index) => ({ metadata, index }))
      .filter(({ metadata }) => metadata.admission === "supported")
      .sort((left, right) =>
        (left.metadata.ui_order ?? Number.MAX_SAFE_INTEGER)
          - (right.metadata.ui_order ?? Number.MAX_SAFE_INTEGER)
        || left.index - right.index)
      .map(({ metadata }) => metadata),
    [registry],
  );

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
        setError(`代理状态监听失败：${errorText(caught)}`);
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

  // Runtime state drives the Save and apply banner, not a one-time success message. Apply is asynchronous.
  // Setting Applying immediately when serveStart returns breaks alignment with the real lifecycle. See UX feedback.
  // When runtime state changes from starting (=Applying) to running, this version is active. Replace it with
  // Automatically hide the brief Applied notice.
  useEffect(() => {
    const phase = state?.serve.phase;
    const previous = prevPhaseRef.current;
    prevPhaseRef.current = phase ?? null;
    if (previous === "starting" && phase === "running" && state) {
      setMessage(`配置已应用 · revision ${state.saved_revision}`);
      const timer = window.setTimeout(
        () => setMessage((current) => (current.startsWith("配置已应用") ? "" : current)),
        2600,
      );
      return () => window.clearTimeout(timer);
    }
    return undefined;
  }, [state?.serve.phase]);

  // Rescan when runtime state changes from not ready to ready. The first app scan can occur before the gateway starts,
  // That scan_agents call got runtime=None, so all installations had connected=false. Managed
  // can incorrectly show Repair required. The 500 ms top-bar poll corrects itself, but scan results do not refresh. When runtime state
  // Scan once when ready to align cards with actual runtime state. rescanAgents has deduplication and queue protection.
  useEffect(() => {
    if (!state) return;
    const ready = state.serve.app_runtime === "running" && Boolean(state.serve.listener_reachable);
    const wasReady = runtimeReadyRef.current;
    runtimeReadyRef.current = ready;
    // The first observation (null) is not a transition to ready. If already ready, the initial load() scan includes runtime.
    // Scan only on an actual not-ready (false) to ready (true) transition. This fixes the startup race.
    if (wasReady === false && ready) {
      void rescanAgents();
    }
  }, [state?.serve.app_runtime, state?.serve.listener_reachable, rescanAgents]);

  const showState = (next: StateView, nextMessage?: string) => {
    setState(next);
    setError("");
    if (nextMessage) setMessage(nextMessage);
  };

  const run = async (action: () => Promise<StateView>, ok?: string) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      showState(await action(), ok);
    } catch (caught) {
      setError(errorText(caught));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const toggleServe = async () => {
    if (!state || serveBusy) return;
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
    if (next === view) return;
    if (next === "usage" || next === "settings" || next === "add-provider") {
      viewHistoryRef.current.push(view);
    } else {
      viewHistoryRef.current = [];
    }
    setView(next);
    setMessage("");
    setError("");
  };

  const navigateBack = () => {
    setView(viewHistoryRef.current.pop() ?? "home");
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
        <strong>{error ? "无法加载 Token Station" : "正在进入 Token Station"}</strong>
        {error && <><p>{error}</p><button className="btn" type="button" onClick={() => window.location.reload()}>重试</button></>}
      </div>
    );
  }

  const agentId = view.startsWith("agent:") ? view.slice("agent:".length) : null;
  const metadata = agentId ? orderedRegistry.find((item) => item.agent_id === agentId) : undefined;
  const agent = agentId ? agents.find((item) => item.metadata.agent_id === agentId) : undefined;
  const route = agentId ? (state.agent_routes?.[agentId] ?? emptyAgentRoute(state)) : undefined;
  const runtimeHealthy = state.serve.app_runtime === "running" && state.serve.listener_reachable;
  const saveStatus = configSaveStatus(state);

  return (
    <AppShell
      view={view}
      serve={state.serve}
      registry={orderedRegistry}
      agents={agents}
      scanBusy={scanBusy}
      commandBusy={serveBusy || busy}
      onNavigate={navigate}
      onRescan={() => void rescanAgents()}
      onToggleServe={() => void toggleServe()}
    >
      {state.serve.phase === "starting" && !error && <div className="banner ok global-banner">正在应用配置…</div>}
      {message && state.serve.phase !== "starting" && <div className="banner ok global-banner">{message}</div>}
      {error && <div className="banner err global-banner">{error}</div>}
      {state.serve.phase === "error" && state.serve.error && <div className="banner err global-banner">{state.serve.error}</div>}

      {view === "home" && (
        <HomePage
          providers={state.providers}
          deletedProviders={state.deleted_providers ?? []}
          providerRecoveryError={state.provider_recovery_error ?? null}
          tiers={state.tiers}
          agentRoutes={state.agent_routes ?? {}}
          profiles={state.profiles ?? []}
          registry={orderedRegistry}
          agents={agents}
          serveRunning={runtimeHealthy}
          busy={busy}
          applying={state.serve.phase === "starting"}
          configError={state.config_error}
          keywords={state.keywords}
          saveStatus={saveStatus}
          localOnly={state.local_only}
          allowCloudFallback={state.allow_cloud_fallback}
          onSetLocalRouting={(localOnly, allowCloudFallback) => void run(() => setLocalRouting(localOnly, allowCloudFallback))}
          onTierChange={(slot: TierSlot, upstream, model) => void run(() => setTier(slot, upstream, model))}
          onAddKeyword={(slot, keyword) => void run(() => addKeyword(slot, keyword))}
          onRemoveKeyword={(slot, keyword) => void run(() => removeKeyword(slot, keyword))}
          onSave={() => void run(serveStart)}
          onApplyAll={() => void run(applyHomeRouteToAllAgents, runtimeHealthy ? "全部 Agent 已恢复跟随主页 · 尚待应用" : "全部 Agent 已恢复跟随主页")}
          onOpenAgent={(id) => navigate(`agent:${id}`)}
          onRemoveProvider={(name) => void run(() => removeProvider(name), "供应商已删除")}
          onRestoreProvider={(name) => void run(() => restoreProvider(name), "供应商已从回收站恢复")}
          onStateChange={showState}
        />
      )}

      {metadata && route && (
        <AgentRoutePage
          // Key by agent_id and remount when the Agent changes. Per-Agent transient state, such as first-connection cards
          // notices and selected installations do not leak to another Agent page.
          key={metadata.agent_id}
          metadata={metadata}
          agent={agent}
          route={route}
          profiles={state.profiles ?? []}
          providers={state.providers}
          serveRunning={runtimeHealthy}
          onStateChange={showState}
          onRescan={rescanAgents}
        />
      )}

      {agentId && !metadata && (
        <section className="panel"><div className="panel-head"><h2>未知 Agent</h2><p className="sub">该 Agent 不在当前 Registry 的受支持列表中。</p></div></section>
      )}

      {view === "usage" && <Stats onBack={navigateBack} />}
      {view === "settings" && (
        <SettingsHub
          settings={state.settings}
          serve={state.serve}
          onSaved={showState}
          onBack={navigateBack}
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
            onAdded={(next, nextMessage) => {
              showState(next, nextMessage);
              setView("home");
            }}
          />
        ) : (
          <section className="panel free-catalog-state">
            <strong>免费供应商不存在或目录已更新</strong>
            <button className="btn" type="button" onClick={() => setView("add-provider")}>返回目录</button>
          </section>
        );
      })()}
    </AppShell>
  );
}

export default function App() {
  return (
    <LanguageBoundary>
      <StationApp />
    </LanguageBoundary>
  );
}
