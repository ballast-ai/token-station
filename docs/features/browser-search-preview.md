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
- Up to three searches per request and five snippets per search.
- A separate headless Chrome profile. No visible window or personal browser tabs.
- A single active browser search per process. Concurrent searches receive a busy error.
- Bounded execution, cancellation, and cleanup of the owned browser process group on Unix.
- Search snippets and source links. No full-page reading.

The preview buffers model rounds. It returns protocol events after the model completes.
Domain filters, location constraints, cached-only search, and dynamic code-execution search are not supported.
Unsupported constraints produce an error before search execution.
Local-only routes cannot use browser search.
Chrome uses the system network settings. It does not inherit Token Station's model-provider proxy settings.
CAPTCHA and unsupported page layouts produce errors. The preview does not bypass verification.

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
