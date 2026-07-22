import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  addKeyword,
  applyHomeRouteToAllAgents,
  getRuntimeState,
  getState,
  listAgentRegistry,
  listenServeState,
  removeKeyword,
  removeProvider,
  restoreProvider,
  scanAgents,
  serveStart,
  serveStop,
  setAdminEndpoint,
  setTier,
  type AgentRouteView,
  type AgentUiMetadataView,
  type AgentView,
  type ServeView,
  type StateView,
  type TierSlot,
} from "./api";
import AppShell, { type AppView } from "./components/AppShell";
import AddProviderPage from "./pages/AddProviderPage";
import AgentRoutePage from "./pages/AgentRoutePage";
import HomePage from "./pages/HomePage";
import SettingsHub from "./pages/SettingsHub";
import Stats from "./pages/Stats";
import "./App.css";

const AGENT_ORDER = ["claude-code", "codex", "opencode", "openclaw", "nous-hermes-agent"];

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

export default function App() {
  const [state, setState] = useState<StateView | null>(null);
  const [view, setView] = useState<AppView>("home");
  const [returnView, setReturnView] = useState<AppView>("home");
  const [registry, setRegistry] = useState<AgentUiMetadataView[]>([]);
  const [agents, setAgents] = useState<AgentView[]>([]);
  const [scanBusy, setScanBusy] = useState(false);
  const [busy, setBusy] = useState(false);
  const [serveBusy, setServeBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const busyRef = useRef(false);
  const scanRef = useRef(false);
  const scanQueuedRef = useRef(false);
  const scanGenerationRef = useRef(0);
  const pendingServeRef = useRef<ServeView | null>(null);

  const orderedRegistry = useMemo(
    () => AGENT_ORDER.flatMap((id) => {
      const metadata = registry.find((item) => item.agent_id === id && item.admission === "supported");
      return metadata ? [metadata] : [];
    }),
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
    if (next === "add-provider") setReturnView(view);
    setView(next);
    setMessage("");
    setError("");
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
      {message && <div className="banner ok global-banner">{message}</div>}
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
          configError={state.config_error}
          keywords={state.keywords}
          saveStatus={saveStatus}
          onTierChange={(slot: TierSlot, upstream, model) => void run(() => setTier(slot, upstream, model))}
          onAddKeyword={(slot, keyword) => void run(() => addKeyword(slot, keyword))}
          onRemoveKeyword={(slot, keyword) => void run(() => removeKeyword(slot, keyword))}
          onSave={() => void run(serveStart, "正在保存并应用配置")}
          onApplyAll={() => void run(applyHomeRouteToAllAgents, runtimeHealthy ? "全部 Agent 已恢复跟随主页 · 尚待应用" : "全部 Agent 已恢复跟随主页")}
          onOpenAgent={(id) => navigate(`agent:${id}`)}
          onRemoveProvider={(name) => void run(() => removeProvider(name), "供应商已删除")}
          onRestoreProvider={(name) => void run(() => restoreProvider(name), "供应商已从回收站恢复")}
          onStateChange={showState}
        />
      )}

      {metadata && route && (
        <AgentRoutePage
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
        <section className="panel"><div className="panel-head"><h2>未知 Agent</h2><p className="sub">该 Agent 不在当前受支持的五个客户端中。</p></div></section>
      )}

      {view === "usage" && <Stats />}
      {view === "settings" && <SettingsHub settings={state.settings} serve={state.serve} onSaved={showState} />}
      {view === "add-provider" && (
        <AddProviderPage
          existingNames={state.providers.map((provider) => provider.name)}
          onCancel={() => navigate(returnView)}
          onAdded={(next, message) => {
            showState(next, message);
            setView(returnView);
          }}
        />
      )}
    </AppShell>
  );
}
