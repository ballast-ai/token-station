import { useEffect, useRef, useState } from "react";
import type { ServeView, SettingsView, StateView } from "../api";
import {
  LanguageBoundary,
  useLanguage,
  type Language,
  type TranslationKey,
} from "../components/LanguageProvider";
import PageBackButton from "../components/PageBackButton";
import { useTheme, type Theme } from "../components/ThemeProvider";
import About from "./About";
import Plugins from "./Plugins";
import RouterTable from "./RouterTable";
import Settings from "./Settings";

type SettingsSection = "general" | "router" | "plugins" | "appearance" | "language" | "about";

const SECTIONS: Array<{
  id: SettingsSection;
  label: TranslationKey;
  description: TranslationKey;
}> = [
  { id: "general", label: "settings.general", description: "settings.generalHint" },
  { id: "router", label: "settings.router", description: "settings.routerHint" },
  { id: "plugins", label: "settings.plugins", description: "settings.pluginsHint" },
  { id: "appearance", label: "settings.appearance", description: "settings.appearanceHint" },
  { id: "language", label: "settings.language", description: "settings.languageHint" },
  { id: "about", label: "settings.about", description: "settings.aboutHint" },
];

function VirtualKeyCard({ serve }: { serve: ServeView }) {
  const { t } = useLanguage();
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);
  const revealTimer = useRef<number | null>(null);
  const key = serve.virtual_key;
  const runtimeHealthy = serve.app_runtime === "running" && serve.listener_reachable;

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
          <span className="eyebrow">{t("key.eyebrow")}</span>
          <h2>{t("key.title")}</h2>
          <p className="sub">{t("key.description")}</p>
        </div>
        <span className={`status-chip ${runtimeHealthy && key ? "success" : ""}`}>
          {runtimeHealthy && key ? t("key.generated") : t("key.proxyStopped")}
        </span>
      </div>
      <div className="secret-row">
        <code aria-label={t("key.ariaLabel")}>
          {key ? (revealed ? key : "ts-••••••••••••••••••••••••") : t("key.startToGenerate")}
        </code>
        <button className="btn tiny" type="button" disabled={!key} onClick={revealed ? () => setRevealed(false) : reveal}>
          {revealed ? t("key.hide") : t("key.show")}
        </button>
        <button className="btn tiny" type="button" disabled={!key} onClick={() => void copy()}>
          {copied ? t("key.copied") : t("key.copy")}
        </button>
      </div>
    </section>
  );
}

function AppearancePanel() {
  const { theme, resolvedTheme, setTheme } = useTheme();
  const { t } = useLanguage();
  const choices: Array<{ value: Theme; label: string; hint: string }> = [
    { value: "light", label: t("appearance.light"), hint: t("appearance.lightHint") },
    { value: "dark", label: t("appearance.dark"), hint: t("appearance.darkHint") },
    {
      value: "system",
      label: t("appearance.system"),
      hint: t("appearance.systemHint", {
        theme: t(resolvedTheme === "dark" ? "appearance.systemDark" : "appearance.systemLight"),
      }),
    },
  ];
  return (
    <section className="panel appearance-panel">
      <div className="panel-head">
        <span className="eyebrow">{t("appearance.eyebrow")}</span>
        <h2>{t("appearance.title")}</h2>
        <p className="sub">{t("appearance.description")}</p>
      </div>
      <div className="theme-options" role="radiogroup" aria-label={t("appearance.groupLabel")}>
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

const LANGUAGE_OPTIONS: Array<{
  value: Language;
  label: string;
  mark: string;
  hint: TranslationKey;
}> = [
  { value: "en", label: "English", mark: "EN", hint: "language.enHint" },
  { value: "zh-CN", label: "简体中文", mark: "简", hint: "language.zhCNHint" },
];

function LanguagePanel() {
  const { language, setLanguage, t } = useLanguage();
  return (
    <section className="panel language-panel">
      <div className="panel-head">
        <span className="eyebrow">{t("language.eyebrow")}</span>
        <h2>{t("language.title")}</h2>
        <p className="sub">{t("language.description")}</p>
      </div>
      <div className="language-options" role="radiogroup" aria-label={t("language.groupLabel")}>
        {LANGUAGE_OPTIONS.map((option) => (
          <button
            key={option.value}
            className={`language-option ${language === option.value ? "selected" : ""}`}
            type="button"
            role="radio"
            aria-checked={language === option.value}
            onClick={() => setLanguage(option.value)}
          >
            <span className="language-mark" aria-hidden="true">{option.mark}</span>
            <span>
              <strong>{option.label}</strong>
              <small>{t(option.hint)}</small>
            </span>
            <i className="language-selected-dot" aria-hidden="true" />
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
  onBack?: () => void;
}

function SettingsHubContent({ settings, serve, onSaved, onBack }: SettingsHubProps) {
  const [section, setSection] = useState<SettingsSection>("general");
  const { t } = useLanguage();
  const runtimeHealthy = serve.app_runtime === "running" && serve.listener_reachable;
  return (
    <div className="settings-layout">
      <aside className="settings-nav" aria-label={t("settings.navLabel")}>
        <div className="settings-nav-title">
          {onBack && <PageBackButton onClick={onBack} />}
          <span className="eyebrow">{t("settings.controlRoom")}</span>
          <h1>{t("settings.title")}</h1>
        </div>
        {SECTIONS.map((item) => (
          <button
            key={item.id}
            className={section === item.id ? "active" : ""}
            type="button"
            onClick={() => setSection(item.id)}
          >
            <strong>{t(item.label)}</strong>
            <small>{t(item.description)}</small>
          </button>
        ))}
      </aside>
      <div className="settings-content">
        {section === "general" && (
          <>
            <VirtualKeyCard serve={serve} />
            <Settings settings={settings} serveRunning={runtimeHealthy} onSaved={onSaved} />
          </>
        )}
        {section === "router" && <RouterTable />}
        {section === "plugins" && <Plugins />}
        {section === "appearance" && <AppearancePanel />}
        {section === "language" && <LanguagePanel />}
        {section === "about" && (
          <About
            desktopVersion={settings.desktop_version ?? settings.version}
            coreVersion={settings.core_version ?? settings.version}
          />
        )}
      </div>
    </div>
  );
}

export default function SettingsHub(props: SettingsHubProps) {
  return (
    <LanguageBoundary>
      <SettingsHubContent {...props} />
    </LanguageBoundary>
  );
}
