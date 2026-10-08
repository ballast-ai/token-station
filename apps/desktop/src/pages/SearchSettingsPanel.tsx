import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useLanguage } from "../components/LanguageProvider";
import { Button } from "../components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "../components/ui/card";
import { Input } from "../components/ui/input";
import { Switch } from "../components/ui/switch";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "../components/ui/select";

type Engine = "bing" | "duckduckgo";
type Mode = "auto" | "native" | "local";
interface Settings { enabled: boolean; mode: Mode; engine: Engine }
interface Status { settings: Settings; chrome_available: boolean; busy: boolean }
interface Quality { expected_source: string; source_count: number; target_found: boolean; cited_source_count: number }
interface Activation { status: Status; verified: boolean; managed_codex_updated: number; execution?: "native" | "local"; quality?: Quality | null }
interface Result { results: Array<{title: string; url: string; snippet: string}>; elapsed_ms: number }

export default function SearchSettingsPanel() {
  const { copy: translate } = useLanguage();
  const copy = (en: string, zh: string) => translate(en, zh, zh, en);
  const [status, setStatus] = useState<Status | null>(null);
  const [query, setQuery] = useState("Python official documentation");
  const [result, setResult] = useState<Result | null>(null);
  const [activation, setActivation] = useState<Activation | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => { let active = true; invoke<Status>("get_search_status").then(value => { if (active) setStatus({ ...value, settings: { ...value.settings, mode: value.settings.mode ?? "auto" } }); }).catch(reason => { if (active) setError(String(reason)); }); return () => { active = false; }; }, []);
  const save = async (settings: Settings) => {
    setBusy(true); setError(""); setActivation(null); setResult(null);
    try { const value = await invoke<Activation>("save_search_settings", { settings }); setStatus(value.status); setActivation(value); }
    catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };
  const test = async () => {
    setBusy(true); setError(""); setResult(null);
    try { setResult(await invoke<Result>("test_browser_search", { query })); }
    catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };
  return <Card>
    <CardHeader><CardTitle>{copy("Web search · Preview", "联网搜索 · 试用")}</CardTitle></CardHeader>
    <CardContent className="space-y-6">
      <p className="text-sm text-muted-foreground">{copy("Auto prefers the provider’s native search. Local Chrome supplements explicit unsupported-search failures. Native provider charges and model tokens still apply.", "自动模式优先使用上游原生搜索，明确不支持时由本地 Chrome 补充。本地搜索无额外搜索 API 费用；原生服务费用及模型 Token 按上游计费。")}</p>
      <div className="space-y-2">
        <label htmlFor="search-mode" className="text-sm font-medium">{copy("Search mode", "搜索模式")}</label>
        <select id="search-mode" className="flex h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={status?.settings.mode ?? "auto"} disabled={!status || busy} onChange={event => { if (status) void save({ ...status.settings, mode: event.target.value as Mode }); }}>
          <option value="auto">{copy("Auto · Native first", "自动 · 原生优先")}</option>
          <option value="native">{copy("Native only", "仅原生搜索")}</option>
          <option value="local">{copy("Local browser only", "仅本地浏览器")}</option>
        </select>
        <p className="text-xs text-muted-foreground">{copy("Auto uses local search only for explicit capability refusals. Authentication, quota, network errors, and empty results do not trigger a retry.", "自动模式仅在明确不支持原生搜索时使用本地搜索。鉴权、额度、网络故障及空结果不会触发补充搜索。")}</p>
      </div>
      <div className="flex items-center justify-between gap-4">
        <label htmlFor="browser-search-enabled" className="text-sm font-medium">{copy("Enable web search", "启用联网搜索")}</label>
        <Switch id="browser-search-enabled" checked={status?.settings.enabled ?? false} disabled={!status || busy || (!status.chrome_available && status.settings.mode === "local" && !status.settings.enabled)} onCheckedChange={enabled => { if (status) void save({ ...status.settings, enabled }); }} />
      </div>
      <p className="text-sm text-muted-foreground">{copy("Enabling runs a real search through the Codex model route (up to 120 seconds). It uses model tokens. Failed verification restores your previous settings.", "开启时会通过 Codex 模型路由执行一次真实搜索（最长约 120 秒），消耗少量模型 Token。验证失败会恢复原设置。")}</p>
      {status?.settings.enabled && <Button disabled={busy || (status.settings.mode === "local" && !status.chrome_available)} onClick={() => void save(status.settings)}>{copy("Verify again", "重新验证")}</Button>}
      <div role="status" aria-live="polite" className="text-sm space-y-2">
        {busy && <p>{copy("Checking… Keep this panel open.", "正在检查，请保持此页面打开…")}</p>}
        {activation?.verified ? <><p>{copy("Gateway search verified: execution completed. Verify again after changing the model or network.", "搜索链路已完成：上次检查的 Codex 路由执行了搜索。更换模型或网络后请重新验证。")}</p>
          {activation.execution && <p>{activation.execution === "native" ? copy("Last check used native search.", "上次验证使用：原生搜索。") : copy("Last check used local browser search.", "上次验证使用：本地浏览器搜索。")}</p>}
          {activation.quality ? <div className="space-y-1 border-l-2 pl-3">
            <p>{activation.quality.target_found ? copy("Target source found", "已命中目标来源") : copy("Target source not found", "未命中目标来源")}</p>
            <p className="text-xs text-muted-foreground break-all">{copy("Expected source", "预期来源")}：{activation.quality.expected_source}</p>
            <p>{copy(`Retrieved sources: ${activation.quality.source_count}`, `检索来源：${activation.quality.source_count}`)}</p>
            <p>{activation.quality.cited_source_count > 0
              ? copy(`Retrieved sources cited: ${activation.quality.cited_source_count} / ${activation.quality.source_count}`, `已引用检索来源：${activation.quality.cited_source_count} / ${activation.quality.source_count}`)
              : copy("No retrieved source cited", "回答未引用检索来源")}</p>
            <p className="text-xs text-muted-foreground">{copy("This check observes source URLs. It does not establish relevance, freshness, or factual support for the answer.", "此检查仅核对来源 URL，不能证明回答的相关性、时效性或事实依据。")}</p>
          </div> : <p>{copy("Source quality was not evaluated in this check.", "此次检查未评估来源质量。")}</p>}
          <p>{activation.managed_codex_updated > 0 ? copy("Managed Codex search configuration updated. Start a new session in Codex.", "已同步受管 Codex 的搜索配置，请在 Codex 中新建会话。") : copy("Connect Codex on the Agents page to use this route. Unmanaged configuration was not changed.", "请在 Agent 页面接入 Codex 后使用此路由。未接入的客户端配置不会被改动。")}</p></>
          : <p>{copy("This panel has not verified the current model route yet. Browser tests alone do not verify model compatibility.", "此页面尚未验证当前模型路由。仅浏览器测试成功不代表模型适配通过。")}</p>}
      </div>
      <div className="space-y-2">
        <label id="search-engine-label" className="text-sm font-medium">{copy("Local search engine", "本地搜索引擎")}</label>
        <Select value={status?.settings.engine ?? "bing"} disabled={!status || busy} onValueChange={engine => { if (status) void save({ ...status.settings, engine: engine as Engine }); }}>
          <SelectTrigger aria-labelledby="search-engine-label"><SelectValue /></SelectTrigger>
          <SelectContent><SelectItem value="bing">Bing</SelectItem><SelectItem value="duckduckgo">DuckDuckGo</SelectItem></SelectContent>
        </Select>
      </div>
      <p className="text-sm">{status ? (status.chrome_available ? copy("Chrome detected. Search runs in a temporary, isolated profile.", "已检测到 Chrome。搜索使用临时独立资料，不复用个人登录态。") : copy("Install Google Chrome before testing browser search.", "请先安装 Google Chrome，再测试浏览器搜索。")) : copy("Checking Chrome…", "正在检测 Chrome…")}</p>
      <form className="space-y-3" onSubmit={event => { event.preventDefault(); void test(); }}>
        <label htmlFor="search-test-query" className="text-sm font-medium">{copy("Test a real search", "测试真实搜索")}</label>
        <Input id="search-test-query" maxLength={500} value={query} onChange={event => setQuery(event.target.value)} />
        <Button type="submit" disabled={busy || !status?.chrome_available || !query.trim()}>{busy ? copy("Working…", "处理中…") : copy("Test search", "测试搜索")}</Button>
        <p className="text-xs text-muted-foreground">{copy("Testing works while the switch is off. Only this query is sent to the search engine. No model call is made.", "开关关闭时也可测试。测试仅将该关键词发给搜索引擎，不调用模型。")}</p>
      </form>
      <div aria-live="polite" className="space-y-4">
        {error && <p role="alert" className="text-sm text-destructive break-words">{error}</p>}
        {result && <><p className="text-sm">{copy("Search snippets", "搜索摘要")} · {result.results.length} · {(result.elapsed_ms / 1000).toFixed(1)} s</p>
          {result.results.map(item => <div key={item.url} className="space-y-1 border-t pt-3">
            <button className="text-left text-sm font-medium underline break-words" onClick={() => void openUrl(item.url).catch(reason => setError(String(reason)))}>{item.title}</button>
            <p className="text-xs text-muted-foreground break-all">{item.url}</p><p className="text-sm break-words">{item.snippet}</p>
          </div>)}
        </>}
      </div>
      <p className="text-xs text-muted-foreground">{copy("Local preview: up to 3 searches per request and 5 snippets per search. One browser runs at a time. Up to 8 requests can wait for 60 seconds. Domain allow or block lists support up to 20 domains. Filters apply to returned source URLs and include subdomains. Geographic targeting, cached-only search, and page reading are not supported. CAPTCHA is reported as an error.", "本地补充范围：每个请求最多搜索 3 次，每次最多返回 5 条摘要。同时运行 1 个浏览器，最多 8 个请求排队，等待上限 60 秒。域名白名单或黑名单最多 20 个域名，过滤返回的来源 URL，包含子域名。不支持地理定位、仅缓存搜索和正文读取。验证码会明确报错。")}</p>
      <p className="text-xs text-muted-foreground">{copy("Managed Codex uses live search in all enabled modes and disables it when off. If the proxy is stopped, configuration sync waits until it restarts. Changing models or networks requires a new check. Claude Code and page fetching need separate validation.", "已接入的 Codex 在三种启用模式下均发送联网搜索请求，关闭总开关则禁用。代理停止时，配置将在代理重启后同步。更换模型或网络后请重新验证。Claude Code 和网页正文读取需要单独验证。")}</p>
    </CardContent>
  </Card>;
}
