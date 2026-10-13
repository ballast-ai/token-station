# Web search preview

Open **Settings > Web search** in the desktop App.
Select a search mode. Enable **Enable web search**.

## Search modes

- **Auto · Native first**: preserve native search when the selected provider route supports its protocol.
  Use the selected managed backend when the route cannot represent hosted search or explicitly rejects the hosted tool type.
- **Native only**: use upstream search. Do not invoke a managed backend.
- **Managed search only**: execute supported hosted search with the selected browser or Tavily backend.

Native paths cover Responses to Responses, Messages to Messages, and supported direct search bridges in both directions.
Requests outside a bridge contract can use managed search in Auto mode, subject to its own constraints.
Auto is the default, including settings saved by older versions.
The master switch is off by default. Disabled gateways keep their existing native search path.
Managed Codex configurations use live web search in all enabled modes and disable it when the switch is off.

Auto does not route native authentication, balance, quota, network, or ambiguous server failures to a managed backend.
An empty native result does not trigger fallback.
No managed retry starts after successful client output or cancellation.
Fallback stays on the selected model offering. It does not switch to another provider.
Local execution rejects unsupported constraints instead of silently removing them.

Explicit native tool refusals are cached in memory for ten minutes.
The cache separates model offerings, client protocols, and tool declarations.
Saving settings or selecting **Verify again** clears observations. Reconstructing the gateway also clears them.
No model-name or provider-brand blacklist decides search support.

## Managed backends

Select **Browser** or **Tavily API** under **Managed search backend**.
Existing preferences keep Browser as the default.

For Tavily, enter an API key and select **Save key**.
The private local secrets file stores the credential under `search-tavily/api_key`.
Settings and status contain no key value. Saving a key does not enable search.
Select **Remove key** to delete the stored credential.

Tavily receives the query and domain filters, without conversation history.
The fixed endpoint is `https://api.tavily.com/search`.
Requests use basic depth and at most five results.
Automatic parameter upgrades, generated answers, and raw page content are disabled.
API redirects are not followed. Returned sources are independently filtered and deduplicated.

Browser fallback is off by default.
When enabled, a Tavily timeout, rate limit, or temporary service/network failure permits one browser attempt.
Authentication, quota, invalid parameters, empty results, malformed responses, and cancellation do not permit fallback.
Each API call has a maximum 15-second I/O timeout.
Admission and fallback share a 35-second search window, with bounded browser I/O cleanup.
Cancellation is checked before and after blocking API work; that work can remain active until its timeout.
Each backend has its own bounded admission queue.

## Native search endpoints

Native search can use a separate endpoint without changing ordinary Chat Completions requests.
Official API roots automatically select these transports:

| Ordinary API origin | Native search transport | Credential header |
| --- | --- | --- |
| `https://api.deepseek.com` | `/anthropic/v1/messages` | `Authorization: Bearer` |
| `https://api.anthropic.com` | `/v1/messages` | `x-api-key` |
| `https://api.openai.com` | `/v1/responses` | `Authorization: Bearer` |

Defaults apply only to the origin root or `/v1`. Custom paths require an explicit profile.
DeepSeek's Anthropic endpoint supports Claude Code WebSearch. Its Responses endpoint does not provide the same search capability.
Support still depends on the selected model and account. A reseller does not inherit native search from its model names.

For another compatible provider, add `native_search` to its upstream configuration:

```json
"native_search": {
  "api_dialect": "anthropic-native",
  "base_url": "https://provider.example/anthropic/v1",
  "auth": "bearer"
}
```

Use `responses-native` for a Responses endpoint. Use `x-api-key` when its Messages endpoint requires that header.
The endpoint must share the ordinary upstream's origin. The profile reuses the same credential slot and selected model.
Explicit profiles override defaults. Existing explicit `api_dialect` configurations remain supported and disable automatic endpoint selection.
Changing search transport does not change ordinary inference, provider selection, or pricing configuration.

## Activation and verification

Enabling or changing an enabled mode runs a real search through the current Codex model route.
Verification uses model tokens and can take up to 120 seconds. Keep the proxy running.
Failed verification restores the previous settings.
Native mode does not require Chrome. Auto can verify native search without Chrome.
Install Google Chrome to use the browser backend or browser fallback. Tavily does not require Chrome.

The panel reports whether the last successful check used native search or managed search.
A successful check applies only to that request. Verify again after changing the route or network.
Claude Code and full-page fetching require separate verification.

The update uses existing ownership, drift detection, snapshots, and disconnect restoration.
Unmanaged configurations remain unchanged. Connect Codex on the Agents page, then start a new Codex session.
When the proxy is stopped, disabling saves the preference. Client synchronization waits for the next proxy start.
Preferences are stored in search-settings.json under the Token Station data directory.
Model routes and provider credentials do not change.

## Backend diagnostics and limits

Select a backend. For Browser, select Bing or DuckDuckGo.
Enter a query. Select **Test search**.
The test works while search is disabled. It does not call a model or certify model compatibility.
Tavily tests use API credits. Results show the actual backend and provider-reported credits when available.
A browser fallback also shows its reason.

Managed search supports:

- Anthropic Messages direct hosted search and OpenAI Responses live web search.
- Function-capable models on the selected provider route.
- Up to three combined searches or page reads per request and five snippets per search.
- Internal short aliases for incompatible client function names. Returned calls retain their original names.
- Parallel search calls within the request budget. Excess calls receive tool errors.
- Mixed search and client calls. Client tools remain client-owned.
- A temporary isolated Chrome profile, without personal cookies, tabs, or extensions.
- One active browser operation per controller, with bounded FIFO waiting and cancellation.
- Bounded execution, cancellation, and owned process cleanup.

Search returns titles, source links, and snippets. The shared reader can open or find literal text in previously returned source URLs.
The reader supports bounded static HTML and UTF-8 text. It rejects private addresses, unsafe redirects, credentials, and unsupported content types.
Dates, relevance, and article contents are not verified by a returned snippet.
Search-only requests require evidence unless the client explicitly disables tools.
General Agent requests with multiple tools retain their tool selection.
The gateway does not execute or repair model-generated client code.

The local loop buffers model rounds before returning protocol events.
Domain filters apply to supported managed searches and page redirects.
Location constraints, cached-only search, and dynamic code-execution search remain unsupported by managed search.
Compatible native paths retain their upstream capabilities and streaming behavior.
Local-only routing policy forbids managed search, including manual backend tests.
Chrome uses system network settings, not the model-provider proxy settings.
CAPTCHA and unsupported page layouts produce errors. The preview does not bypass verification.

The request log includes search_execution and content-free browser_searches outcomes.
The legacy browser_searches field now also carries the actual backend, typed error kind, and reported API credits when available.
Execution values distinguish native, forced local, protocol fallback, explicit rejection, and cached rejection.
These fields contain no search query, page content, or raw provider error.

## Cost and rollback

Local browser search adds no search API charge. Model token charges still apply.
Native search can incur the upstream provider's search charges.
Tavily uses search API credits. Reported credits do not establish currency cost; account pricing determines billing.
No MCP installation is added.
Select Managed search only and Browser to use the browser-only path.
Disable the master switch to stop local interception and disable managed Codex search.
