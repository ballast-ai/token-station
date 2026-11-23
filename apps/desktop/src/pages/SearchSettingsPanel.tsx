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
type Backend = "browser" | "tavily";
interface Settings { enabled: boolean; mode: Mode; engine: Engine; backend?: Backend; browser_fallback?: boolean }
interface Status { settings: Settings; chrome_available: boolean; busy: boolean; tavily_configured?: boolean }
interface Quality { expected_source: string; source_count: number; target_found: boolean; cited_source_count: number }
interface Activation { status: Status; verified: boolean; managed_codex_updated: number; execution?: "native" | "local"; quality?: Quality | null }
interface Result { results: Array<{title: string; url: string; snippet: string}>; elapsed_ms: number; source?: string; credits?: number; fallback_reason?: string }

export default function SearchSettingsPanel() {
  const { copy: translate } = useLanguage();
  const copy = (en: string, zh: string) => translate(en, zh, zh, en);
  const [status, setStatus] = useState<Status | null>(null);
  const [query, setQuery] = useState("Python official documentation");
  const [result, setResult] = useState<Result | null>(null);
  const [activation, setActivation] = useState<Activation | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [apiKey, setApiKey] = useState("");
  const backend = status?.settings.backend ?? "browser";
  const backendReady = backend === "tavily" ? status?.tavily_configured : status?.chrome_available;
  const fallbackLabel = result?.fallback_reason === "rate_limited" ? copy("API rate limit", "API 限流")
    : result?.fallback_reason === "timeout" ? copy("API timeout", "API 超时") : copy("Temporary API failure", "API 临时故障");
  const saveKey = async (key: string) => {
    setBusy(true); setError(""); setActivation(null); setResult(null);
    try { setStatus(await invoke<Status>("save_search_api_key", { key })); setApiKey(""); }
    catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };
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
  return <Card className="shrink-0">
    <CardHeader><CardTitle>{copy("Web search · Preview", "联网搜索 · 试用")}</CardTitle></CardHeader>
    <CardContent className="space-y-6">
      <p className="text-sm text-muted-foreground">{copy("Auto prefers native search. The selected backend supplements unsupported native search. API credits and model tokens may apply.", "自动模式优先原生搜索，不支持时由所选后端补充。Tavily 按搜索额度计费，模型 Token 仍按上游计费。")}</p>
      <div className="space-y-2">
        <label htmlFor="search-mode" className="text-sm font-medium">{copy("Search mode", "搜索模式")}</label>
        <select id="search-mode" className="flex h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={status?.settings.mode ?? "auto"} disabled={!status || busy} onChange={event => { if (status) void save({ ...status.settings, mode: event.target.value as Mode }); }}>
          <option value="auto">{copy("Auto · Native first", "自动 · 原生优先")}</option>
          <option value="native">{copy("Native only", "仅原生搜索")}</option>
          <option value="local">{copy("Managed search only", "仅使用所选搜索后端")}</option>
        </select>
        <p className="text-xs text-muted-foreground">{copy("Auto supplements native search only when its capability or protocol is unsupported. Provider authentication, quota, network failures, and empty results do not trigger managed search.", "自动模式仅在原生能力或协议不支持时补充搜索。上游模型的鉴权、额度、网络故障及空结果不会触发补充搜索。")}</p>
      </div>
      <div className="flex items-center justify-between gap-4">
        <label htmlFor="browser-search-enabled" className="text-sm font-medium">{copy("Enable web search", "启用联网搜索")}</label>
        <Switch id="browser-search-enabled" checked={status?.settings.enabled ?? false} disabled={!status || busy || (!backendReady && status.settings.mode === "local" && !status.settings.enabled)} onCheckedChange={enabled => { if (status) void save({ ...status.settings, enabled }); }} />
      </div>
      <p className="text-sm text-muted-foreground">{copy("Enabling runs a real search through the Codex model route (up to 120 seconds). It uses model tokens. Failed verification restores your previous settings.", "开启时会通过 Codex 模型路由执行一次真实搜索（最长约 120 秒），消耗少量模型 Token。验证失败会恢复原设置。")}</p>
      {status?.settings.enabled && <Button disabled={busy || (status.settings.mode === "local" && !backendReady)} onClick={() => void save(status.settings)}>{copy("Verify again", "重新验证")}</Button>}
      <div role="status" aria-live="polite" className="text-sm space-y-2">
        {busy && <p>{copy("Checking… Keep this panel open.", "正在检查，请保持此页面打开…")}</p>}
        {activation?.verified ? <><p>{copy("Gateway search verified: execution completed. Verify again after changing the model or network.", "搜索链路已完成：上次检查的 Codex 路由执行了搜索。更换模型或网络后请重新验证。")}</p>
          {activation.execution && <p>{activation.execution === "native" ? copy("Last check used native search.", "上次验证使用：原生搜索。") : copy("Last check used managed search.", "上次验证使用：所选后端的补充搜索。")}</p>}
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
        <label htmlFor="search-backend" className="text-sm font-medium">{copy("Managed search backend", "补充搜索后端")}</label>
        <select id="search-backend" className="flex h-10 w-full rounded-md border border-input bg-background px-3 text-sm" value={backend} disabled={!status || busy} onChange={event => { if (status) void save({ ...status.settings, backend: event.target.value as Backend }); }}>
          <option value="browser">{copy("Browser · Bing / DuckDuckGo", "浏览器 · Bing / DuckDuckGo")}</option>
          <option value="tavily">Tavily API</option>
        </select>
        <p className="text-xs text-muted-foreground">{copy("Tavily receives the query and domain filters, without conversation history. Basic search uses API credits. Native-only mode ignores this backend.", "Tavily 仅接收查询词和域名条件，不接收对话历史。Basic 搜索消耗 API 额度。仅原生模式不使用此后端。")}</p>
        {backend === "tavily" && <label className="flex gap-2 text-sm"><input type="checkbox" checked={status?.settings.browser_fallback ?? false} disabled={!status || busy} onChange={event => { if (status) void save({ ...status.settings, browser_fallback: event.target.checked }); }} />{copy("Allow one browser fallback for rate limits, timeouts, or temporary failures", "限流、超时或临时故障时，允许回退浏览器一次")}</label>}
      </div>
      <div className="space-y-2">
        <label htmlFor="tavily-api-key" className="text-sm font-medium">{copy("Tavily API key", "Tavily API 密钥")}</label>
        <Input id="tavily-api-key" type="password" autoComplete="off" maxLength={512} value={apiKey} disabled={!status || busy} onChange={event => setApiKey(event.target.value)} />
        <div className="flex flex-wrap gap-2">
          <Button disabled={!status || busy || !apiKey.trim()} onClick={() => void saveKey(apiKey)}>{copy("Save key", "保存密钥")}</Button>
          <Button variant="outline" disabled={!status?.tavily_configured || busy} onClick={() => void saveKey("")}>{copy("Remove key", "移除密钥")}</Button>
        </div>
        <p role="status" className="text-xs text-muted-foreground">{status?.tavily_configured ? copy("Tavily key saved", "已保存 Tavily 密钥") : copy("No Tavily key saved", "尚未保存 Tavily 密钥")}</p>
        <p className="text-xs text-muted-foreground">{copy("Stored in the private local secrets file. Saving a key does not enable search.", "密钥保存在本机私有凭据文件中。保存密钥不会自动启用搜索。")}</p>
      </div>
      <div className="space-y-2">
        <label id="search-engine-label" className="text-sm font-medium">{copy("Local search engine", "本地搜索引擎")}</label>
        <Select value={status?.settings.engine ?? "bing"} disabled={!status || busy} onValueChange={engine => { if (status) void save({ ...status.settings, engine: engine as Engine }); }}>
          <SelectTrigger aria-labelledby="search-engine-label"><SelectValue /></SelectTrigger>
          <SelectContent><SelectItem value="bing">Bing</SelectItem><SelectItem value="duckduckgo">DuckDuckGo</SelectItem></SelectContent>
        </Select>
      </div>
      <p className="text-sm">{status ? (status.chrome_available ? copy("Chrome detected. Search runs in a temporary, isolated profile.", "已检测到 Chrome。搜索使用临时独立资料，不复用个人登录态。") : copy("Chrome is unavailable. Only the browser backend requires it.", "未检测到 Chrome，仅浏览器后端需要安装。")) : copy("Checking Chrome…", "正在检测 Chrome…")}</p>
      <form className="space-y-3" onSubmit={event => { event.preventDefault(); void test(); }}>
        <label htmlFor="search-test-query" className="text-sm font-medium">{copy("Test a real search", "测试真实搜索")}</label>
        <Input id="search-test-query" maxLength={500} value={query} onChange={event => setQuery(event.target.value)} />
        <Button type="submit" disabled={busy || !backendReady || !query.trim()}>{busy ? copy("Working…", "处理中…") : copy("Test search", "测试搜索")}</Button>
        <p className="text-xs text-muted-foreground">{copy("Tests the selected backend, even with search disabled. Sends only this query. Tavily tests use API credits; no model call is made.", "开关关闭时也可测试所选后端。仅发送此查询词，不调用模型；Tavily 测试消耗 API 额度。")}</p>
      </form>
      <div aria-live="polite" className="space-y-4">
        {error && <p role="alert" className="text-sm text-destructive break-words">{error}</p>}
        {result && <>{result.source && <p className="text-sm">{copy("Backend", "实际后端")}：{result.source === "tavily" ? "Tavily API" : copy("Browser search", "浏览器搜索")}</p>}{result.credits != null && <p className="text-sm">{copy(`Tavily credits: ${result.credits}`, `Tavily 搜索额度：${result.credits}`)}</p>}{result.fallback_reason && <p className="text-sm">{copy("Browser fallback reason", "浏览器回退原因")}：{fallbackLabel}</p>}<p className="text-sm">{copy("Search snippets", "搜索摘要")} · {result.results.length} · {(result.elapsed_ms / 1000).toFixed(1)} s</p>
          {result.results.map(item => <div key={item.url} className="space-y-1 border-t pt-3">
            <button className="text-left text-sm font-medium underline break-words" onClick={() => void openUrl(item.url).catch(reason => setError(String(reason)))}>{item.title}</button>
            <p className="text-xs text-muted-foreground break-all">{item.url}</p><p className="text-sm break-words">{item.snippet}</p>
          </div>)}
        </>}
      </div>
      <p className="text-xs text-muted-foreground">{copy("Managed search: up to 3 search or page-read operations per request and 5 snippets per search. Browser and API searches have separate bounded queues. Search admission and fallback share a 35-second window; page reads can wait up to 60 seconds. Domain allow or block lists support up to 20 domains. Filters apply to returned source URLs and include subdomains. Page reading opens only sources returned in the same request. It supports static HTML and UTF-8 text, up to 2 MiB and 24,000 returned characters. Literal find applies to returned text. Geographic targeting, cached-only search, PDF, and JavaScript rendering are not supported.", "补充搜索范围：每个请求最多执行 3 次搜索或正文读取，每次搜索最多返回 5 条摘要。浏览器与 API 分开排队；搜索排队与回退共享 35 秒时限，正文读取最多等待 60 秒。域名白名单或黑名单最多 20 个域名，过滤返回的来源 URL，包含子域名。正文读取仅限本次搜索返回的来源，支持静态 HTML 和 UTF-8 文本，下载上限 2 MiB、返回上限 24,000 字符，可在返回文本中查找字面内容。不支持地理定位、仅缓存搜索、PDF 和 JavaScript 渲染。")}</p>
      <p className="text-xs text-muted-foreground">{copy("Managed Codex uses live search in all enabled modes and disables it when off. If the proxy is stopped, configuration sync waits until it restarts. Changing models or networks requires a new check. Native search bridges support text and direct functions in both directions. Some source metadata is finalized at stream completion. Verify each provider separately.", "已接入的 Codex 在三种启用模式下均发送联网搜索请求，关闭总开关则禁用。代理停止时，配置将在代理重启后同步。更换模型或网络后请重新验证。原生搜索双向桥接支持文本和直接函数调用，部分来源信息会在流结束时补齐。各供应商须分别验证。")}</p>
    </CardContent>
  </Card>;
}
