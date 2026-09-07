import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { ProviderView, StateView } from "../api";
import { useLanguage } from "./LanguageProvider";
import { useErrorToast } from "./ErrorToast";
import { useDraftGuard } from "./DraftNavigation";
import { Button } from "./ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "./ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";

type Target = { upstream: string; model: string };

export default function WebSearchSettings({ providers, serveRunning, onSaved }: {
  providers: ProviderView[]; serveRunning: boolean; onSaved: (state: StateView) => void;
}) {
  const { copy: localizedCopy } = useLanguage();
  const copy = (value: { en: string; "zh-CN": string }) => localizedCopy(value.en, value["zh-CN"], value["zh-CN"], value.en);
  const { showError, showSuccess } = useErrorToast();
  const [saved, setSaved] = useState<Target | null>(null);
  const [upstream, setUpstream] = useState("");
  const [model, setModel] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let active = true;
    invoke<Target | null>("get_web_search_target").then((value) => {
      if (!active) return;
      setSaved(value); setUpstream(value?.upstream ?? ""); setModel(value?.model ?? "");
    }).catch((error) => { if (active) { setFailed(true); showError(String(error), "search-load"); } })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [showError]);
  const dirty = !loading && (upstream !== (saved?.upstream ?? "") || model !== (saved?.model ?? ""));
  useDraftGuard(dirty);
  const eligible = providers.filter((p) => !p.managed_route && ["anthropic", "openai-compatible"].includes(p.provider));
  const selected = eligible.find((p) => p.name === upstream);
  const save = async () => {
    setSaving(true);
    try {
      const state = await invoke<StateView>("set_web_search_target", { upstream, model });
      setSaved(upstream ? { upstream, model } : null);
      onSaved(state);
      showSuccess(copy({ en: serveRunning ? "Saved. Restart the proxy to apply." : "Saved.", "zh-CN": serveRunning ? "已保存，重启代理后生效。" : "已保存。" }), "search-save");
    } catch (error) { showError(String(error), "search-save"); }
    finally { setSaving(false); }
  };
  const providerLabel = copy({ en: "Search provider", "zh-CN": "搜索供应商" });
  const modelLabel = copy({ en: "Search model", "zh-CN": "搜索模型" });
  return <Card className="settings-card">
    <CardHeader><CardTitle>{copy({ en: "Web Search", "zh-CN": "联网搜索" })}</CardTitle>
      <p className="sub">{copy({ en: "Use one search backend across Claude Code chat models. Search requests and their context are sent to this provider; ordinary chat keeps its route.", "zh-CN": "Claude Code 切换对话模型后，仍可共用这个搜索后端。搜索请求及其上下文会发送给该供应商，普通对话保持原路由。" })}</p>
    </CardHeader>
    <CardContent className="settings-card-content">
      <div className="setting-row egress-settings">
        <label>{providerLabel}<Select disabled={loading || saving || failed} value={upstream || "__route__"} onValueChange={(value) => { setUpstream(value === "__route__" ? "" : value); setModel(""); }}>
          <SelectTrigger aria-label={providerLabel}><SelectValue /></SelectTrigger>
          <SelectContent><SelectItem value="__route__">{copy({ en: "Use the current route's native search", "zh-CN": "使用当前路由的原生搜索能力" })}</SelectItem>
            {eligible.map((p) => <SelectItem key={p.name} value={p.name}>{p.name}</SelectItem>)}
          </SelectContent>
        </Select></label>
        {upstream && <label>{modelLabel}<Select disabled={loading || saving} value={model} onValueChange={setModel}>
          <SelectTrigger aria-label={modelLabel}><SelectValue placeholder={copy({ en: "Select a model", "zh-CN": "选择模型" })} /></SelectTrigger>
          <SelectContent>{selected?.models.map((m) => <SelectItem key={m} value={m}>{m}</SelectItem>)}</SelectContent>
        </Select></label>}
        <p className="sub">{upstream ? copy({
          en: selected?.provider === "anthropic" ? "This declares Anthropic native Web Search support. Confirm that the provider and model support it; search may have separate charges." : "This declares native Responses Web Search support. A Chat Completions-only API is insufficient. Confirm support with your provider; search may have separate charges.",
          "zh-CN": selected?.provider === "anthropic" ? "保存后将声明该后端支持 Anthropic 原生搜索。请确认供应商及模型支持此能力，搜索可能单独收费。" : "保存后将声明该后端支持 Responses 原生搜索。仅支持普通聊天接口的供应商无法提供此能力，请先向供应商确认；搜索可能单独收费。",
        }) : copy({ en: "No automatic provider switching. If the current route has no native search, configure a search-capable provider above.", "zh-CN": "不会自动切换其他供应商。当前路由没有原生搜索能力时，请在上方配置支持搜索的后端。" })}</p>
      </div>
      <div className="panel-foot"><Button disabled={!dirty || saving || failed || Boolean(upstream && !model)} onClick={save}>{copy({ en: "Save search settings", "zh-CN": "保存搜索设置" })}</Button></div>
    </CardContent>
  </Card>;
}
