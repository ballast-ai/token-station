import { useEffect, useMemo, useRef, useState } from "react";
import {
  StateView,
  TierSlot,
  AgentKind,
  getState,
  addProvider,
  removeProvider,
  setTier,
  saveConfig,
  serveStart,
  serveStop,
  connectAgent,
  discoverProviderModels,
  ModelDiscoveryView,
  setAdminEndpoint,
} from "./api";
import { PROVIDER_CATALOG, CUSTOM_ID, ProviderPreset } from "./catalog";
import ModelPicker, { CatalogStatus } from "./components/ModelPicker";
import ProviderModelManager from "./components/ProviderModelManager";
import RouterTable from "./pages/RouterTable";
import Stats from "./pages/Stats";
import Plugins from "./pages/Plugins";
import Settings from "./pages/Settings";
import About from "./pages/About";
import "./App.css";

type Tab = "home" | "router" | "stats" | "plugins" | "settings" | "about";
const TABS: { id: Tab; label: string }[] = [
  { id: "home", label: "主页" },
  { id: "router", label: "路由表" },
  { id: "stats", label: "用量" },
  { id: "plugins", label: "插件" },
  { id: "settings", label: "设置" },
  { id: "about", label: "关于" },
];

const TIER_META: { slot: TierSlot; label: string; hint: string }[] = [
  { slot: "high", label: "上档", hint: "最强模型 · 难任务升到这里" },
  { slot: "mid", label: "中档", hint: "中等复杂度" },
  { slot: "low", label: "下档", hint: "便宜快模型 · 简单任务兜底" },
];

const AGENTS: { kind: AgentKind; label: string; icon: string }[] = [
  { kind: "cc", label: "Claude Code", icon: "🅰" },
  { kind: "codex", label: "Codex", icon: "◎" },
  { kind: "opencode", label: "opencode", icon: "▣" },
];

function App() {
  const [state, setState] = useState<StateView | null>(null);
  const [msg, setMsg] = useState<string>("");
  const [err, setErr] = useState<string>("");
  const [tab, setTab] = useState<Tab>("home");
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);

  // Add-provider form
  const [presetId, setPresetId] = useState<string>("");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [key, setKey] = useState("");
  const [picked, setPicked] = useState<string[]>([]);
  const [extraModels, setExtraModels] = useState<string[]>([]);
  const [discoveredModels, setDiscoveredModels] = useState<string[]>([]);
  const [discovery, setDiscovery] = useState<ModelDiscoveryView | null>(null);
  const [discovering, setDiscovering] = useState(false);
  const [autoDiscoveryDone, setAutoDiscoveryDone] = useState(false);
  const [managedProvider, setManagedProvider] = useState<string | null>(null);

  const refresh = async () => {
    setErr("");
    try {
      setState(await getState());
    } catch (e) {
      setErr(String(e));
    }
  };
  useEffect(() => {
    refresh();
  }, []);
  // Synchronize the data-plane endpoint after every state write, including serve start and stop. Data pages prefer local HTTP.
  useEffect(() => {
    if (state) setAdminEndpoint(state.serve);
  }, [state]);

  const run = async (fn: () => Promise<StateView | string>, okMsg?: string) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setErr("");
    setMsg("");
    try {
      const r = await fn();
      if (typeof r === "string") setMsg(r);
      else setState(r);
      if (okMsg) setMsg(okMsg);
    } catch (e) {
      setErr(String(e));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  const preset: ProviderPreset | null = useMemo(
    () => PROVIDER_CATALOG.find((p) => p.id === presetId) ?? null,
    [presetId],
  );
  const isCustom = presetId === CUSTOM_ID;
  const needsKey = isCustom ? true : preset?.needsKey ?? true;
  const catalogModels = preset?.models ?? [];
  const allModels = [...new Set([...catalogModels, ...discoveredModels, ...extraModels])];

  const addCatalogStatus: CatalogStatus = discovering
    ? { label: "正在获取…", tone: "loading" }
    : discovery?.source === "live"
      ? {
          label: `已同步 ${discovery.models.length} 个`,
          tone: "live",
          warning: discovery.warning,
        }
      : discovery?.source === "cache"
        ? {
            label: `使用缓存 · ${discovery.models.length} 个`,
            tone: "cache",
            warning: discovery.warning,
          }
        : discovery
          ? { label: "获取失败", tone: "error", warning: discovery.warning }
          : { label: `内置建议 · ${catalogModels.length} 个`, tone: "idle" };

  const onPreset = (id: string) => {
    setPresetId(id);
    const p = PROVIDER_CATALOG.find((x) => x.id === id);
    if (p) {
      setName(p.id);
      setUrl(p.baseUrl);
      setPicked([...p.models]); // Select all recommended models by default
    } else {
      setName("");
      setUrl("");
      setPicked([]);
    }
    setExtraModels([]);
    setDiscoveredModels([]);
    setDiscovery(null);
    setAutoDiscoveryDone(false);
  };

  const toggleModel = (m: string) =>
    setPicked((s) => (s.includes(m) ? s.filter((x) => x !== m) : [...s, m]));

  const discoverNewProviderModels = async () => {
    if (discovering || busy) return;
    if (!name.trim() || !url.trim()) {
      setDiscovery({
        models: [],
        source: "none",
        fetched_at_ms: null,
        warning: "请先填写供应商名称和 Base URL",
      });
      return;
    }
    if (needsKey && !key.trim()) {
      setDiscovery({
        models: [],
        source: "none",
        fetched_at_ms: null,
        warning: "填写 API Key 后才能读取该厂商的模型目录",
      });
      return;
    }
    setDiscovering(true);
    setErr("");
    try {
      const result = await discoverProviderModels(name.trim(), url.trim(), needsKey ? key : null);
      setDiscovery(result);
      setDiscoveredModels((current) => [...new Set([...current, ...result.models])]);
    } catch (caught) {
      setDiscovery({ models: [], source: "none", fetched_at_ms: null, warning: String(caught) });
    } finally {
      setDiscovering(false);
    }
  };

  const resetForm = () => {
    setPresetId("");
    setName("");
    setUrl("");
    setKey("");
    setPicked([]);
    setExtraModels([]);
    setDiscoveredModels([]);
    setDiscovery(null);
    setAutoDiscoveryDone(false);
  };

  const onAddProvider = () => {
    if (!presetId) return setErr("请先选择供应商");
    if (picked.length === 0) return setErr("请至少勾选一个模型");
    return run(async () => {
      const r = await addProvider(name.trim(), url.trim(), picked, needsKey ? key : null);
      resetForm();
      return r;
    }, "供应商已添加");
  };

  if (!state) {
    return (
      <div className="loading">
        {err ? (
          <>
            <div className="banner err">{err}</div>
            <button className="btn" disabled={busy} onClick={refresh}>重试</button>
          </>
        ) : "加载中…"}
      </div>
    );
  }

  const { providers, tiers, serve, config_error } = state;

  const onTierProvider = (slot: TierSlot, upstream: string) => {
    if (!upstream) return run(() => setTier(slot, null, null));
    const p = providers.find((x) => x.name === upstream);
    return run(() => setTier(slot, upstream, p?.models[0] ?? null));
  };

  // Show a mild prompt only when a provider exists but configuration is incomplete. Do not show an error for a new empty state.
  const showHint = providers.length > 0 && !!config_error;

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">token-station</div>
        <div className={`serve-pill ${serve.running ? "on" : "off"}`}>
          <span className="dot" />
          {serve.running ? `运行中 · ${serve.listen}` : "已停止"}
        </div>
        {serve.running ? (
          <button className="btn" disabled={busy} onClick={() => run(() => serveStop())}>
            停止
          </button>
        ) : (
          <button className="btn primary" disabled={busy} onClick={() => run(() => serveStart())}>
            启动代理
          </button>
        )}
        <div className="agentbar">
          {AGENTS.map((a) => (
            <button
              key={a.kind}
              className={`agent ${serve.running ? "ready" : "idle"}`}
              disabled={busy}
              title={serve.running ? `接入 ${a.label}` : "点此会提示先启动代理"}
              onClick={() => run(() => connectAgent(a.kind))}
            >
              <span className="ai">{a.icon}</span> {a.label}
            </button>
          ))}
        </div>
      </header>

      {serve.running && serve.virtual_key && (
        <div className="keybar">
          <span>虚拟 Key</span>
          <code>{serve.virtual_key}</code>
          <button className="btn tiny" onClick={() => navigator.clipboard.writeText(serve.virtual_key!)}>
            复制
          </button>
        </div>
      )}

      {msg && <div className="banner ok">{msg}</div>}
      {err && <div className="banner err">{err}</div>}

      <nav className="tabs">
        {TABS.map((t) => (
          <button
            key={t.id}
            className={`tab ${tab === t.id ? "active" : ""}`}
            onClick={() => {
              setTab(t.id);
              setMsg("");
              setErr("");
            }}
          >
            {t.label}
          </button>
        ))}
      </nav>

      {tab === "router" && <RouterTable />}
      {tab === "stats" && <Stats />}
      {tab === "plugins" && <Plugins />}
      {tab === "settings" && (
        <Settings settings={state.settings} serveRunning={serve.running} onSaved={setState} />
      )}
      {tab === "about" && <About version={state.settings.version} />}

      {tab === "home" && (
        <>
      {/* Three-tier routing */}
      <section className="panel">
        <div className="panel-head">
          <h2>智能路由 · 三档</h2>
          <p className="sub">请求按复杂度自动落到某一档。你只定「谁在上、谁在下」,分档交给路由。</p>
        </div>

        <div className="tier-grid">
          <div className="tier-col-head" />
          <div className="tier-col-head">供应商</div>
          <div className="tier-col-head">模型</div>

          {TIER_META.map(({ slot, label, hint }) => {
            const t = tiers[slot];
            const provider = providers.find((p) => p.name === t.upstream);
            return (
              <div className="tier-row" key={slot}>
                <div className={`tier-badge ${slot}`}>
                  <div className="tier-label">{label}</div>
                  <div className="tier-hint">{hint}</div>
                </div>
                <select className="select" disabled={busy} value={t.upstream ?? ""} onChange={(e) => onTierProvider(slot, e.target.value)}>
                  <option value="">— 未选 —</option>
                  {providers.map((p) => (
                    <option key={p.name} value={p.name}>
                      {p.name}
                    </option>
                  ))}
                </select>
                <select
                  className="select"
                  value={t.model ?? ""}
                  disabled={busy || !t.upstream}
                  onChange={(e) => run(() => setTier(slot, t.upstream!, e.target.value))}
                >
                  <option value="">— 模型 —</option>
                  {provider?.models.map((m) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ))}
                </select>
              </div>
            );
          })}
        </div>

        <div className="panel-foot">
          <button className="btn primary" disabled={busy} onClick={() => run(() => saveConfig(), "已保存并校验")}>
            保存并应用
          </button>
          {providers.length === 0 && <span className="foot-hint">先在下面添加供应商,再给三档各选一个模型</span>}
          {showHint && <span className="foot-hint">还差一步:给三档各选好模型再保存</span>}
        </div>
      </section>

      {/* Provider */}
      <section className="panel">
        <div className="panel-head">
          <h2>供应商</h2>
          <p className="sub">选主流供应商 → URL 自动带出,你只填 API Key。Key 存入系统钥匙串,不落配置文件。</p>
        </div>

        <div className="provider-list">
          {providers.length === 0 && <div className="empty">还没有供应商,在下面选一个添加。</div>}
          {providers.map((p) => (
            <div className={`provider-card ${managedProvider === p.name ? "expanded" : ""}`} key={p.name}>
              <div className="provider-card-head">
                <div className="provider-main">
                  <div className="provider-name">{p.name}</div>
                  <div className="provider-url">{p.base_url}</div>
                  <div className="provider-models">
                    {p.models.map((m) => (
                      <span className="chip" key={m}>
                        {m}
                      </span>
                    ))}
                  </div>
                </div>
                <div className="provider-side">
                  <span className={`auth ${p.has_auth ? "yes" : "no"}`}>
                    {p.has_auth ? "● Key 已就绪" : "○ 无鉴权"}
                  </span>
                  <button
                    className="btn tiny"
                    type="button"
                    disabled={busy}
                    onClick={() => setManagedProvider((current) => (current === p.name ? null : p.name))}
                  >
                    {managedProvider === p.name ? "收起" : "管理模型"}
                  </button>
                  <button className="btn tiny danger" disabled={busy} onClick={() => run(() => removeProvider(p.name))}>
                    删除
                  </button>
                </div>
              </div>
              {managedProvider === p.name && (
                <ProviderModelManager
                  provider={p}
                  serveRunning={serve.running}
                  disabled={busy}
                  onSaved={(next) => {
                    setState(next);
                    setMsg(`${p.name} 的模型已保存`);
                  }}
                />
              )}
            </div>
          ))}
        </div>

        {/* Add provider: preset-driven */}
        <div className="add-panel">
          <div className="add-row">
            <select
              className="select grow"
              value={presetId}
              disabled={busy || discovering}
              onChange={(e) => onPreset(e.target.value)}
            >
              <option value="">— 选择供应商 —</option>
              {PROVIDER_CATALOG.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
                </option>
              ))}
              <option value={CUSTOM_ID}>自定义…</option>
            </select>

            {isCustom ? (
              <>
                <input
                  className="input"
                  placeholder="名称"
                  value={name}
                  disabled={busy || discovering}
                  onChange={(e) => {
                    setName(e.target.value);
                    setDiscovery(null);
                    setDiscoveredModels([]);
                    setAutoDiscoveryDone(false);
                  }}
                />
                <input
                  className="input grow"
                  placeholder="Base URL"
                  value={url}
                  disabled={busy || discovering}
                  onChange={(e) => {
                    setUrl(e.target.value);
                    setDiscovery(null);
                    setDiscoveredModels([]);
                    setAutoDiscoveryDone(false);
                  }}
                />
              </>
            ) : (
              preset && <span className="url-tag">{url}</span>
            )}
          </div>

          {presetId && (
            <>
              <ModelPicker
                models={allModels}
                selected={picked}
                status={addCatalogStatus}
                refreshing={discovering}
                disabled={busy}
                onRefresh={discoverNewProviderModels}
                onToggle={toggleModel}
                onAdd={(model) => {
                  if (!allModels.includes(model)) setExtraModels((current) => [...current, model]);
                  if (!picked.includes(model)) setPicked((current) => [...current, model]);
                }}
              />

              <div className="add-row">
                {needsKey ? (
                  <input
                    className="input grow"
                    type="password"
                    placeholder="API Key"
                    value={key}
                    disabled={busy || discovering}
                    onChange={(e) => {
                      setKey(e.target.value);
                      setAutoDiscoveryDone(false);
                    }}
                    onBlur={() => {
                      if (!autoDiscoveryDone && key.trim()) {
                        setAutoDiscoveryDone(true);
                        void discoverNewProviderModels();
                      }
                    }}
                  />
                ) : (
                  <span className="url-tag">本地模型 · 免 Key</span>
                )}
                <button className="btn primary" disabled={busy || discovering} onClick={onAddProvider}>
                  添加
                </button>
                <button className="btn" disabled={busy || discovering} onClick={resetForm}>
                  取消
                </button>
              </div>
            </>
          )}
        </div>
      </section>
        </>
      )}
    </div>
  );
}

export default App;
