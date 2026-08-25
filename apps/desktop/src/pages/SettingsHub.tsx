import { useEffect, useRef, useState } from "react";
import {
  Bot,
  Globe,
  Info,
  KeyRound,
  Palette,
  ScrollText,
  ServerCog,
  Settings2,
  type LucideIcon,
} from "lucide-react";
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
import RequestLogsPage from "./RequestLogsPage";

type SettingsSection =
  | "general"
  | "api-key"
  | "runtime"
  | "agent-visibility"
  | "appearance"
  | "language"
  | "request-logs"
  | "about";

const SECTIONS: Array<{
  id: SettingsSection;
  label: TranslationKey;
  description: TranslationKey;
  englishLabel?: string;
  chineseLabel?: string;
  englishDescription?: string;
  chineseDescription?: string;
  traditionalLabel?: string;
  japaneseLabel?: string;
  traditionalDescription?: string;
  japaneseDescription?: string;
  icon: LucideIcon;
}> = [
  {
    id: "general",
    label: "settings.general",
    description: "settings.generalHint",
    icon: Settings2,
  },
  {
    id: "api-key",
    label: "settings.apiKey",
    description: "settings.apiKeyHint",
    icon: KeyRound,
  },
  {
    id: "runtime",
    label: "settings.runtime",
    description: "settings.runtimeHint",
    icon: ServerCog,
  },
  {
    id: "agent-visibility",
    label: "settings.agentVisibility",
    description: "settings.agentVisibilityHint",
    icon: Bot,
  },
  {
    id: "appearance",
    label: "settings.appearance",
    description: "settings.appearanceHint",
    icon: Palette,
  },
  {
    id: "language",
    label: "settings.language",
    description: "settings.languageHint",
    icon: Globe,
  },
  {
    id: "request-logs",
    label: "settings.general",
    description: "settings.generalHint",
    englishLabel: "Request logs",
    chineseLabel: "请求日志",
    traditionalLabel: "請求記錄",
    japaneseLabel: "リクエストログ",
    englishDescription: "Routing outcomes and local receipts",
    chineseDescription: "路由结果与本地回执",
    traditionalDescription: "路由結果與本機回執",
    japaneseDescription: "ルーティング結果とローカルレシート",
    icon: ScrollText,
  },
  {
    id: "about",
    label: "settings.about",
    description: "settings.aboutHint",
    icon: Info,
  },
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
          "無法複製虛擬 API Key。請檢查系統剪貼簿權限，然後再試一次。",
          "仮想 API Key をコピーできませんでした。システムのクリップボード権限を確認して再試行してください。",
        ),
        "copy-virtual-key",
      );
    }
  };

  return (
    <Card className="settings-card key-settings-card">
      <CardHeader className="panel-head split-heading">
        <div>
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

function RuntimeInformationCard({ settings }: { settings: SettingsView }) {
  const { t } = useLanguage();
  return (
    <Card className="settings-card runtime-information-card">
      <CardHeader className="panel-head">
        <CardTitle><h2>{t("runtime.title")}</h2></CardTitle>
        <p className="sub">{t("runtime.description")}</p>
      </CardHeader>
      <CardContent>
        <div className="kv-grid">
          <div className="kv-k">{t("general.listen")}</div>
          <div className="kv-v mono">{settings.listen}</div>
          <div className="kv-k">{t("general.dataDir")}</div>
          <div className="kv-v mono">{settings.data_dir || "—"}</div>
          <div className="kv-k">{t("general.pluginsDir")}</div>
          <div className="kv-v mono">{settings.plugins_dir || "—"}</div>
          <div className="kv-k">{t("general.adapter")}</div>
          <div className="kv-v mono">{settings.agent || "—"}</div>
          <div className="kv-k">{t("general.coreVersion")}</div>
          <div className="kv-v mono">{settings.version}</div>
        </div>
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
  visibleAgentIds,
  onVisibilityChange,
}: {
  registry: AgentUiMetadataView[];
  visibleAgentIds: ReadonlySet<string>;
  onVisibilityChange: (agentId: string, visible: boolean) => void;
}) {
  const { t } = useLanguage();
  const visibleCount = registry.reduce(
    (count, metadata) => count + Number(visibleAgentIds.has(metadata.agent_id)),
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
            const visible = visibleAgentIds.has(metadata.agent_id);
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
  { value: "zh-TW", label: "繁體中文", mark: "繁", hint: "language.zhTWHint" },
  { value: "ja", label: "日本語", mark: "日", hint: "language.jaHint" },
];

function LanguagePanel() {
  const { language, setLanguage, t } = useLanguage();
  return (
    <Card className="settings-card language-panel">
      <CardHeader className="panel-head">
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
  visibleAgentIds: ReadonlySet<string>;
  onAgentVisibilityChange: (agentId: string, visible: boolean) => void;
  onOpenFirstRunGuide: () => void;
  onSaved: (state: StateView) => void;
  initialSection?: SettingsSection;
}

function SettingsHubContent({
  settings,
  serve,
  registry,
  visibleAgentIds,
  onAgentVisibilityChange,
  onOpenFirstRunGuide,
  onSaved,
  initialSection = "general",
}: SettingsHubProps) {
  const [section, setSection] = useState<SettingsSection>(initialSection);
  const [navigationInputMode, setNavigationInputMode] = useState<"pointer" | "keyboard">("pointer");
  const navigationRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const contentRef = useRef<HTMLElement | null>(null);
  const { t, copy } = useLanguage();
  const runtimeHealthy = serve.app_runtime === "running" && serve.listener_reachable;
  const resetScroll = () => {
    if (!contentRef.current) return;
    contentRef.current.scrollTop = 0;
    const workspace = contentRef.current.closest<HTMLElement>(".station-content-settings");
    if (workspace) workspace.scrollTop = 0;
  };
  useEffect(() => {
    setSection(initialSection);
    resetScroll();
  }, [initialSection]);

  const activateSection = (index: number) => {
    const nextIndex = (index + SECTIONS.length) % SECTIONS.length;
    navigationRefs.current[nextIndex]?.focus({ preventScroll: true });
    resetScroll();
    setSection(SECTIONS[nextIndex].id);
  };
  const activeSection = SECTIONS.find((item) => item.id === section) ?? SECTIONS[0];
  const activeSectionDescription = activeSection.englishDescription ? copy(
    activeSection.englishDescription,
    activeSection.chineseDescription ?? activeSection.englishDescription,
    activeSection.traditionalDescription ?? activeSection.englishDescription,
    activeSection.japaneseDescription ?? activeSection.englishDescription,
  ) : t(activeSection.description);

  return (
    <div className="page-stack settings-page">
      <aside className="settings-sidebar">
        <header className="overview-heading settings-heading">
          <div>
            <h1>{t("settings.title")}</h1>
            <p>{activeSectionDescription}</p>
          </div>
        </header>
        <nav
          className="settings-subnav"
          aria-label={t("settings.navLabel")}
          data-input-mode={navigationInputMode}
          onPointerDown={() => setNavigationInputMode("pointer")}
          onPointerMove={() => setNavigationInputMode("pointer")}
        >
          {SECTIONS.map((item, index) => {
            const Icon = item.icon;
            return (
              <Button
                key={item.id}
                ref={(node) => { navigationRefs.current[index] = node; }}
                className="settings-subnav-item"
                variant={section === item.id ? "secondary" : "ghost"}
                type="button"
                aria-current={section === item.id ? "page" : undefined}
                onClick={(event) => {
                  if (event.detail === 0) setNavigationInputMode("keyboard");
                  activateSection(index);
                }}
                onKeyDown={(event) => {
                  if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
                    setNavigationInputMode("keyboard");
                  }
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    activateSection(index + 1);
                  } else if (event.key === "ArrowUp") {
                    event.preventDefault();
                    activateSection(index - 1);
                  } else if (event.key === "Home") {
                    event.preventDefault();
                    activateSection(0);
                  } else if (event.key === "End") {
                    event.preventDefault();
                    activateSection(SECTIONS.length - 1);
                  }
                }}
              >
                <Icon className="settings-subnav-icon" aria-hidden="true" />
                <span>
                  <strong>{item.englishLabel ? copy(
                    item.englishLabel,
                    item.chineseLabel ?? item.englishLabel,
                    item.traditionalLabel ?? item.englishLabel,
                    item.japaneseLabel ?? item.englishLabel,
                  ) : t(item.label)}</strong>
                  <small>{item.englishDescription ? copy(
                    item.englishDescription,
                    item.chineseDescription ?? item.englishDescription,
                    item.traditionalDescription ?? item.englishDescription,
                    item.japaneseDescription ?? item.englishDescription,
                  ) : t(item.description)}</small>
                </span>
              </Button>
            );
          })}
        </nav>
      </aside>
      <main ref={contentRef} className="settings-content">
        {section === "general" && (
          <Settings settings={settings} serveRunning={runtimeHealthy} onSaved={onSaved} mode="general" />
        )}
        {section === "api-key" && (
          <>
            <VirtualKeyCard serve={serve} />
            <Settings settings={settings} serveRunning={runtimeHealthy} onSaved={onSaved} mode="api-key" />
          </>
        )}
        {section === "runtime" && <RuntimeInformationCard settings={settings} />}
        {section === "agent-visibility" && (
          <AgentVisibilityPanel
            registry={registry}
            visibleAgentIds={visibleAgentIds}
            onVisibilityChange={onAgentVisibilityChange}
          />
        )}
        {section === "appearance" && <AppearancePanel />}
        {section === "language" && <LanguagePanel />}
        {section === "request-logs" && (
          <section className="settings-request-logs">
            <header className="overview-heading">
              <div>
                <h1>{copy("Request logs", "请求日志", "請求日誌", "リクエストログ")}</h1>
                <p>{copy(
                  "Inspect routing outcomes, failures, and locally retained plaintext bodies.",
                  "查看路由结果、失败原因和本地保留的明文正文。", "檢視路由結果、失敗原因和本機保留的明文主體", "ルーティング結果、失敗原因およびローカルに保持された平文本文を確認"
                )}</p>
              </div>
            </header>
            <RequestLogsPage embedded />
          </section>
        )}
        {section === "about" && (
          <About
            desktopVersion={settings.desktop_version ?? settings.version}
            coreVersion={settings.core_version ?? settings.version}
            onOpenFirstRunGuide={onOpenFirstRunGuide}
          />
        )}
      </main>
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
