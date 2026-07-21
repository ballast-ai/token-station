import { useEffect, useRef, useState } from "react";
import type { ServeView, SettingsView, StateView } from "../api";
import { useTheme, type Theme } from "../components/ThemeProvider";
import About from "./About";
import Plugins from "./Plugins";
import RouterTable from "./RouterTable";
import Settings from "./Settings";

type SettingsSection = "general" | "router" | "plugins" | "appearance" | "about";

const SECTIONS: Array<{ id: SettingsSection; label: string; description: string }> = [
  { id: "general", label: "通用", description: "代理、鉴权和环境" },
  { id: "router", label: "路由表", description: "只读决策结构" },
  { id: "plugins", label: "插件", description: "已加载的能力" },
  { id: "appearance", label: "外观", description: "明暗主题" },
  { id: "about", label: "关于", description: "版本与更新" },
];

function VirtualKeyCard({ serve }: { serve: ServeView }) {
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);
  const revealTimer = useRef<number | null>(null);
  const key = serve.virtual_key;

  useEffect(() => () => {
    if (revealTimer.current != null) window.clearTimeout(revealTimer.current);
  }, []);

  const reveal = () => {
    setRevealed(true);
    if (revealTimer.current != null) window.clearTimeout(revealTimer.current);
    revealTimer.current = window.setTimeout(() => setRevealed(false), 15_000);
  };

  const copy = async () => {
    if (!key) return;
    await navigator.clipboard.writeText(key);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1_500);
  };

  return (
    <section className="panel key-settings-card">
      <div className="panel-head split-heading">
        <div>
          <span className="eyebrow">LOCAL AUTH</span>
          <h2>虚拟 API Key</h2>
          <p className="sub">供本机 Agent 访问 Token Station。默认隐藏，复制时无需显示明文。</p>
        </div>
        <span className={`status-chip ${serve.running && key ? "success" : ""}`}>
          {serve.running && key ? "已生成" : "代理未运行"}
        </span>
      </div>
      <div className="secret-row">
        <code aria-label="虚拟 API Key">{key ? (revealed ? key : "ts-••••••••••••••••••••••••") : "启动代理后生成"}</code>
        <button className="btn tiny" type="button" disabled={!key} onClick={revealed ? () => setRevealed(false) : reveal}>
          {revealed ? "隐藏" : "显示 15 秒"}
        </button>
        <button className="btn tiny" type="button" disabled={!key} onClick={() => void copy()}>
          {copied ? "已复制" : "复制"}
        </button>
      </div>
    </section>
  );
}

function AppearancePanel() {
  const { theme, resolvedTheme, setTheme } = useTheme();
  const choices: Array<{ value: Theme; label: string; hint: string }> = [
    { value: "light", label: "浅色", hint: "稳定使用明亮界面" },
    { value: "dark", label: "深色", hint: "稳定使用暗色界面" },
    { value: "system", label: "跟随系统", hint: `当前系统为${resolvedTheme === "dark" ? "深色" : "浅色"}` },
  ];
  return (
    <section className="panel appearance-panel">
      <div className="panel-head">
        <span className="eyebrow">APPEARANCE</span>
        <h2>外观</h2>
        <p className="sub">主题会同步应用到主页、Agent、用量和设置中的所有页面。</p>
      </div>
      <div className="theme-options" role="radiogroup" aria-label="界面主题">
        {choices.map((choice) => (
          <button
            key={choice.value}
            className={`theme-option ${theme === choice.value ? "selected" : ""}`}
            type="button"
            role="radio"
            aria-checked={theme === choice.value}
            onClick={() => setTheme(choice.value)}
          >
            <span className={`theme-preview ${choice.value}`} aria-hidden="true"><i /><i /></span>
            <strong>{choice.label}</strong>
            <small>{choice.hint}</small>
          </button>
        ))}
      </div>
    </section>
  );
}

interface SettingsHubProps {
  settings: SettingsView;
  serve: ServeView;
  onSaved: (state: StateView) => void;
}

export default function SettingsHub({ settings, serve, onSaved }: SettingsHubProps) {
  const [section, setSection] = useState<SettingsSection>("general");
  return (
    <div className="settings-layout">
      <aside className="settings-nav" aria-label="设置分类">
        <div className="settings-nav-title">
          <span className="eyebrow">CONTROL ROOM</span>
          <h1>设置</h1>
        </div>
        {SECTIONS.map((item) => (
          <button
            key={item.id}
            className={section === item.id ? "active" : ""}
            type="button"
            onClick={() => setSection(item.id)}
          >
            <strong>{item.label}</strong>
            <small>{item.description}</small>
          </button>
        ))}
      </aside>
      <div className="settings-content">
        {section === "general" && (
          <>
            <VirtualKeyCard serve={serve} />
            <Settings settings={settings} serveRunning={serve.running} onSaved={onSaved} />
          </>
        )}
        {section === "router" && <RouterTable />}
        {section === "plugins" && <Plugins />}
        {section === "appearance" && <AppearancePanel />}
        {section === "about" && <About version={settings.version} />}
      </div>
    </div>
  );
}
