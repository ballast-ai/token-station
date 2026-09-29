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
interface Settings { enabled: boolean; engine: Engine }
interface Status { settings: Settings; chrome_available: boolean; busy: boolean }
interface Result { results: Array<{title: string; url: string; snippet: string}>; elapsed_ms: number }

export default function SearchSettingsPanel() {
  const { copy: translate } = useLanguage();
  const copy = (en: string, zh: string) => translate(en, zh, zh, en);
  const [status, setStatus] = useState<Status | null>(null);
  const [query, setQuery] = useState("Python official documentation");
  const [result, setResult] = useState<Result | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => { let active = true; invoke<Status>("get_search_status").then(value => { if (active) setStatus(value); }).catch(reason => { if (active) setError(String(reason)); }); return () => { active = false; }; }, []);
  const save = async (settings: Settings) => {
    setBusy(true); setError("");
    try { setStatus(await invoke<Status>("save_search_settings", { settings })); }
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
    <CardHeader><CardTitle>{copy("Browser search · Preview", "浏览器搜索 · 试用")}</CardTitle></CardHeader>
    <CardContent className="space-y-6">
      <p className="text-sm text-muted-foreground">{copy("Use a separate headless Chrome to search. No visible windows, personal tabs, or search API charges. Model tokens still apply.", "使用独立的后台 Chrome 搜索，不弹出窗口、不操作个人标签页，不产生搜索 API 费用。模型 Token 仍正常计费。")}</p>
      <div className="flex items-center justify-between gap-4">
        <label htmlFor="browser-search-enabled" className="text-sm font-medium">{copy("Handle native web search", "接管原生联网搜索")}</label>
        <Switch id="browser-search-enabled" checked={status?.settings.enabled ?? false} disabled={!status || busy || (!status.chrome_available && !status.settings.enabled)} onCheckedChange={enabled => { if (status) void save({ ...status.settings, enabled }); }} />
      </div>
      <p className="text-sm text-muted-foreground">{copy("Off restores the existing search path. Changes apply to new requests. The preview buffers model rounds before returning results.", "关闭即可恢复原有搜索路径，对新请求生效。试用版会等待模型和搜索完成，再返回结果。")}</p>
      <div className="space-y-2">
        <label id="search-engine-label" className="text-sm font-medium">{copy("Search engine", "搜索引擎")}</label>
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
      <p className="text-xs text-muted-foreground">{copy("Preview: up to 3 searches per request and 5 snippets per search. Domain filters, geographic targeting, cached-only search, and page reading are not supported. CAPTCHA is reported as an error. Chrome uses the system network settings.", "试用范围：每个请求最多搜索 3 次，每次最多返回 5 条摘要。不支持域名过滤、地理定位、仅缓存搜索和正文读取。验证码会明确报错。Chrome 使用系统网络设置。")}</p>
      <p className="text-xs text-muted-foreground">{copy("For Codex, enable live web search in the client. Token Station does not change your active client configuration here.", "Codex 需要在客户端启用实时联网搜索。这一开关不会修改你正在使用的客户端配置。")}</p>
    </CardContent>
  </Card>;
}
