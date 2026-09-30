# Browser search preview

Open **Settings > Web search** in the desktop App.
Install Google Chrome before using this preview.
Select Bing or DuckDuckGo. Enter a query. Select **Test search**.
The test sends the query to the search engine. It does not call a model.

Enable **Handle native web search** to execute supported hosted search requests through the local browser.
The switch is off by default. Disable it to restore the existing native provider path.
Preferences are stored in `search-settings.json` under the Token Station data directory.
Model routing and provider credentials do not change.

## Supported behavior

- Anthropic Messages direct hosted search and OpenAI Responses live web search.
- Function-capable models on the existing provider route.
- Up to three search attempts per request and five snippets per search.
- Shared tool handling for function-capable models. No model-name overrides.
- Long or incompatible client function names use internal short aliases. Returned calls keep the original client names.
- Parallel search calls use the same request budget. Excess calls receive tool errors.
- Mixed search and client tool calls return completed search records and the original client calls.
- Browser failures return tool errors. The model can explain the failure or use available results.
- A separate headless Chrome profile. No visible window or personal browser tabs.
- A single active browser search per process. Concurrent searches receive a busy error.
- Bounded execution, cancellation, and cleanup of the owned browser process group on Unix.
- Search snippets and source links. No full-page reading.

When the budget is exhausted, search history becomes labeled text before the search declaration is removed.
This avoids invalid historical tool references on strict providers. Other client tools remain available.
The request log includes `browser_searches` with operation order, elapsed time, and a fixed outcome code.
These records contain no query, page content, or raw browser error. They are not yet shown in the request-history UI.

A single-tool forced choice can use one equivalent-format retry after an explicit initial HTTP 400 refusal.
The retry stays on the same model and preserves forced tool use. Both attempts appear in the request log.

The preview buffers model rounds. It returns protocol events after the model completes.
Domain filters, location constraints, cached-only search, and dynamic code-execution search are not supported.
Unsupported constraints produce an error before search execution.
Local-only routes cannot use browser search.
Chrome uses the system network settings. It does not inherit Token Station's model-provider proxy settings.
CAPTCHA and unsupported page layouts produce errors. The preview does not bypass verification.

## Web page reading

This feature returns snippets. It does not replace Claude Code WebFetch or read full pages.
Client domain checks, tool permissions, website restrictions, and model transport failures remain separate failure paths.
A successful model response does not prove a successful search. Check the hosted tool result and browser search outcome.

## Codex

Codex must declare live web search for the gateway to receive it.
The existing Token Station connector can set `web_search = "disabled"` in the Codex configuration.
For a temporary trial, launch Codex with `codex -c 'web_search="live"'`.
Keep the Token Station provider selected. This command does not persist the search override.
The switch in Token Station does not rewrite an active client configuration.

## Cost and rollback

There is no paid search API fallback in this preview. Model token charges still apply.
There is no automatic MCP installation. These remain separate future capabilities.
Disable the preview to restore the previous search behavior without removing model configuration.

### Search evidence and custom tools

Search-only requests require a browser call unless the client explicitly disables tools.
If the model omits a required search, the gateway returns an error instead of an unsupported answer.
Search-only responses contain retrieved titles, URLs, snippets, and structured errors instead of an inner model summary.
These snippets do not verify publication dates, relevance, or the full article.
General Agent requests with multiple tools retain their tool selection behavior.
Custom tool instructions remain readable before their preserved JSON metadata.
The gateway does not execute or repair model-generated client code.
