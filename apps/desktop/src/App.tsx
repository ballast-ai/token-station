import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  applyHomeRouteToAllAgents,
  getState,
  listAgentRegistry,
  listenServeState,
  removeProvider,
  saveConfig,
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

function emptyAgentRoute(state: StateView): AgentRouteView {
  return { mode: "inherit", tiers: state.tiers, config_error: null };
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
  const pendingServeRef = useRef<ServeView | null>(null);

  const orderedRegistry = useMemo(
    () => AGENT_ORDER.flatMap((id) => {
      const metadata = registry.find((item) => item.agent_id === id && item.admission === "supported");
      return metadata ? [metadata] : [];
    }),
    [registry],
  );

  const rescanAgents = useCallback(async () => {
    if (scanRef.current) return;
    scanRef.current = true;
    setScanBusy(true);
    try {
      setAgents(await scanAgents());
    } catch (caught) {
      setError(errorText(caught));
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
      showState(await (state.serve.running || state.serve.phase === "starting" ? serveStop() : serveStart()));
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
          tiers={state.tiers}
          agentRoutes={state.agent_routes ?? {}}
          registry={orderedRegistry}
          agents={agents}
          serveRunning={state.serve.running}
          busy={busy}
          configError={state.config_error}
          onTierChange={(slot: TierSlot, upstream, model) => void run(() => setTier(slot, upstream, model))}
          onSave={() => void run(saveConfig, state.serve.running ? "主页路由已保存 · 重启代理后生效" : "主页路由已保存")}
          onApplyAll={() => void run(applyHomeRouteToAllAgents, state.serve.running ? "全部 Agent 已恢复跟随主页 · 重启代理后生效" : "全部 Agent 已恢复跟随主页")}
          onOpenAgent={(id) => navigate(`agent:${id}`)}
          onRemoveProvider={(name) => void run(() => removeProvider(name), "供应商已删除")}
          onStateChange={showState}
        />
      )}

      {metadata && route && (
        <AgentRoutePage
          metadata={metadata}
          agent={agent}
          route={route}
          providers={state.providers}
          serveRunning={state.serve.running}
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
          onCancel={() => navigate(returnView)}
          onAdded={(next) => {
            showState(next, "供应商已添加");
            setView(returnView);
          }}
        />
      )}
    </AppShell>
  );
}
