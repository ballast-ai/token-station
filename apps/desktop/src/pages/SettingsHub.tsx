import { useEffect, useRef, useState } from "react";
import type {
  AgentUiMetadataView,
  ServeView,
  SettingsView,
  StateView,
} from "../api";
import { AgentIcon } from "../brandIcons";
import {
  LanguageBoundary,
  useLanguage,
  type Language,
  type TranslationKey,
} from "../components/LanguageProvider";
import { useTheme, type Theme } from "../components/ThemeProvider";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Switch } from "../components/ui/switch";
import { useErrorToast } from "../components/ErrorToast";
import About from "./About";
import Settings from "./Settings";

type SettingsSection =
  | "general"
  | "agent-visibility"
  | "appearance"
  | "language"
  | "about";

const SECTIONS: Array<{
  id: SettingsSection;
  label: TranslationKey;
  description: TranslationKey;
}> = [
  { id: "general", label: "settings.general", description: "settings.generalHint" },
  {
    id: "agent-visibility",
    label: "settings.agentVisibility",
    description: "settings.agentVisibilityHint",
  },
  { id: "appearance", label: "settings.appearance", description: "settings.appearanceHint" },
  { id: "language", label: "settings.language", description: "settings.languageHint" },
  { id: "about", label: "settings.about", description: "settings.aboutHint" },
];

function VirtualKeyCard({ serve }: { serve: ServeView }) {
  const { copy: localizedCopy, t } = useLanguage();
  const { showError } = useErrorToast();
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
    try {
      await navigator.clipboard.writeText(key);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1_500);
    } catch {
      showError(
        localizedCopy(
          "Could not copy the virtual API key. Check the system clipboard permission and try again.",
          "无法复制虚拟 API Key。请检查系统剪贴板权限，然后重试。",
        ),
        "copy-virtual-key",
      );
    }
  };

  return (
    <Card className="settings-card key-settings-card">
      <CardHeader className="panel-head split-heading">
        <div>
          <span className="eyebrow">{t("key.eyebrow")}</span>
          <CardTitle><h2>{t("key.title")}</h2></CardTitle>
          <p className="sub">{t("key.description")}</p>
        </div>
        <span className={`status-chip ${runtimeHealthy && key ? "success" : ""}`}>
          {runtimeHealthy && key ? t("key.generated") : t("key.proxyStopped")}
        </span>
      </CardHeader>
      <CardContent className="secret-row">
        <code aria-label={t("key.ariaLabel")}>
          {key ? (revealed ? key : "ts-••••••••••••••••••••••••") : t("key.startToGenerate")}
        </code>
        <Button variant="outline" size="sm" type="button" disabled={!key} onClick={revealed ? () => setRevealed(false) : reveal}>
          {revealed ? t("key.hide") : t("key.show")}
        </Button>
        <Button variant="outline" size="sm" type="button" disabled={!key} onClick={() => void copy()}>
          {copied ? t("key.copied") : t("key.copy")}
        </Button>
      </CardContent>
    </Card>
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
    <Card className="settings-card appearance-panel">
      <CardHeader className="panel-head">
        <span className="eyebrow">{t("appearance.eyebrow")}</span>
        <CardTitle><h2>{t("appearance.title")}</h2></CardTitle>
        <p className="sub">{t("appearance.description")}</p>
      </CardHeader>
      <CardContent className="theme-options" role="radiogroup" aria-label={t("appearance.groupLabel")}>
        {choices.map((choice) => (
          <Button
            key={choice.value}
            className={`theme-option ${theme === choice.value ? "selected" : ""}`}
            variant="ghost"
            type="button"
            role="radio"
            aria-checked={theme === choice.value}
            onClick={() => setTheme(choice.value)}
          >
            <span className={`theme-preview ${choice.value}`} aria-hidden="true"><i /><i /></span>
            <strong>{choice.label}</strong>
            <small>{choice.hint}</small>
          </Button>
        ))}
      </CardContent>
    </Card>
  );
}

function AgentVisibilityPanel({
  registry,
  hiddenAgentIds,
  onVisibilityChange,
}: {
  registry: AgentUiMetadataView[];
  hiddenAgentIds: ReadonlySet<string>;
  onVisibilityChange: (agentId: string, visible: boolean) => void;
}) {
  const { t } = useLanguage();
  const visibleCount = registry.reduce(
    (count, metadata) => count + Number(!hiddenAgentIds.has(metadata.agent_id)),
    0,
  );

  return (
    <Card className="settings-card agent-visibility-panel">
      <CardHeader className="panel-head">
        <CardTitle><h2>{t("agentVisibility.title")}</h2></CardTitle>
        <p className="sub">{t("agentVisibility.description")}</p>
        <span
          className="agent-visibility-status"
          role="status"
          aria-live="polite"
          aria-atomic="true"
        >
          {t("agentVisibility.count", {
            visible: visibleCount,
            total: registry.length,
          })}
        </span>
      </CardHeader>

      {registry.length > 0 ? (
        <CardContent
          className="agent-visibility-list"
          role="group"
          aria-label={t("agentVisibility.groupLabel")}
        >
          {registry.map((metadata) => {
            const visible = !hiddenAgentIds.has(metadata.agent_id);
            return (
              <div
                key={metadata.agent_id}
                className="agent-visibility-row"
              >
                <span className="agent-visibility-icon" aria-hidden="true">
                  <AgentIcon
                    id={metadata.agent_id}
                    fallback={metadata.nav_mark ?? metadata.display_name.slice(0, 1)}
                    size={22}
                  />
                </span>
                <span className="agent-visibility-name">
                  {metadata.display_name}
                </span>
                <Switch
                  aria-label={metadata.display_name}
                  checked={visible}
                  onCheckedChange={(checked) => onVisibilityChange(metadata.agent_id, checked)}
                />
              </div>
            );
          })}
        </CardContent>
      ) : (
        <p className="agent-visibility-empty">{t("agentVisibility.empty")}</p>
      )}
    </Card>
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
    <Card className="settings-card language-panel">
      <CardHeader className="panel-head">
        <span className="eyebrow">{t("language.eyebrow")}</span>
        <CardTitle><h2>{t("language.title")}</h2></CardTitle>
        <p className="sub">{t("language.description")}</p>
      </CardHeader>
      <CardContent className="language-options" role="radiogroup" aria-label={t("language.groupLabel")}>
        {LANGUAGE_OPTIONS.map((option) => (
          <Button
            key={option.value}
            className={`language-option ${language === option.value ? "selected" : ""}`}
            variant="ghost"
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
          </Button>
        ))}
      </CardContent>
    </Card>
  );
}

interface SettingsHubProps {
  settings: SettingsView;
  serve: ServeView;
  registry: AgentUiMetadataView[];
  hiddenAgentIds: ReadonlySet<string>;
  onAgentVisibilityChange: (agentId: string, visible: boolean) => void;
  onOpenFirstRunGuide: () => void;
  onSaved: (state: StateView) => void;
}

function SettingsHubContent({
  settings,
  serve,
  registry,
  hiddenAgentIds,
  onAgentVisibilityChange,
  onOpenFirstRunGuide,
  onSaved,
}: SettingsHubProps) {
  const [section, setSection] = useState<SettingsSection>("general");
  const { t } = useLanguage();
  const runtimeHealthy = serve.app_runtime === "running" && serve.listener_reachable;
  return (
    <div className="page-stack settings-page">
      <header className="overview-heading settings-heading">
        <div>
          <span className="page-eyebrow">{t("settings.controlRoom")}</span>
          <h1>{t("settings.title")}</h1>
          <p>{t("settings.generalHint")}</p>
        </div>
      </header>
      <nav className="settings-subnav" aria-label={t("settings.navLabel")}>
        {SECTIONS.map((item) => (
          <Button
            key={item.id}
            className="settings-subnav-item"
            variant={section === item.id ? "secondary" : "ghost"}
            type="button"
            aria-current={section === item.id ? "page" : undefined}
            onClick={() => setSection(item.id)}
          >
            <span>
              <strong>{t(item.label)}</strong>
              <small>{t(item.description)}</small>
            </span>
          </Button>
        ))}
      </nav>
      <div className="settings-content">
        {section === "general" && (
          <>
            <VirtualKeyCard serve={serve} />
            <Settings settings={settings} serveRunning={runtimeHealthy} onSaved={onSaved} />
          </>
        )}
        {section === "agent-visibility" && (
          <AgentVisibilityPanel
            registry={registry}
            hiddenAgentIds={hiddenAgentIds}
            onVisibilityChange={onAgentVisibilityChange}
          />
        )}
        {section === "appearance" && <AppearancePanel />}
        {section === "language" && <LanguagePanel />}
        {section === "about" && (
          <About
            desktopVersion={settings.desktop_version ?? settings.version}
            coreVersion={settings.core_version ?? settings.version}
            onOpenFirstRunGuide={onOpenFirstRunGuide}
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
