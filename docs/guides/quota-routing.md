# Quota-first routing

Quota-first routing uses only the provider and model entries selected in the rotation list.
It chooses available accounts by reset time. Nearby reset times use conversation affinity, rate headroom, and list order.
It skips spent accounts. If every selected account is unavailable, the request fails.
It does not add a paid account outside the rotation list.
An explicit Agent model mapping keeps its existing priority before the quota router.
Use an unmapped automatic route when you want quota rotation.

## Set up the desktop

1. Add an API-key provider.
2. Select **Quota-first** on Home or an Agent route.
3. Add each provider and model to the rotation list.
4. If automatic quota data is unavailable, enter a local quota plan.
5. Add each plan window, its allowance, and the shared counting unit.
6. If known, enter the requests-per-minute limit.
7. Select **Save plan** for each edited provider.
8. Select **Save & apply** to activate the configuration.
9. Open **Live quota** to inspect data sources and account state.

The desktop supports up to 16 windows per edited plan. Every window can constrain the account.
A zero allowance is invalid. Remove every window and select **Clear plan** to remove the plan.
Local token plans count input and output tokens. Local request plans count completed requests, including responses without token usage.

## Automatic data

The gateway reads recognized token-limit response headers. It also queries these exact official API endpoints in the background:

| Provider | Recognized base path | Quota endpoint |
| --- | --- | --- |
| OpenRouter | `https://openrouter.ai/api/v1` | `/api/v1/key` |
| Kimi Code | `https://api.kimi.com/coding` or `/coding/v1` | `/coding/v1/usages` |
| Kimi Code alternate host | `https://api.kimi.ai/coding` or `/coding/v1` | `/coding/v1/usages` |
| MiniMax | `https://api.minimaxi.com/v1`, `/anthropic`, or `/anthropic/v1` | `/v1/token_plan/remains` |
| MiniMax international | Same paths on `https://api.minimax.io` | `/v1/token_plan/remains` |
| Zhipu Coding Plan | `https://open.bigmodel.cn/api/coding/paas/v4` | `/api/monitor/usage/quota/limit` |
| Z.ai Coding Plan | `https://api.z.ai/api/coding/paas/v4` | `/api/monitor/usage/quota/limit` |

Queries use the configured API key. They do not call a model completion.
Custom proxies and lookalike domains are not automatically queried.
MiniMax media allowances and Zhipu MCP limits do not constrain text routing.
Provider plan points are displayed as percentages. They are not local token counts.
OpenRouter credit balances have no scheduled reset.

These adapters have synthetic protocol tests. A real plan account must confirm endpoint access and current response formats.
A successful parse does not verify an account's billing contract.

## Interpret the live view

- **Provider-reported**: A recent response header or quota API reading.
- **Provider reading expired**: The last reading is older than its freshness limit, or a reported window reached its reset.
- **Local estimate**: A plan counted from traffic through this gateway.
- **No window data**: Quota availability is unknown. Rate and cooldown information can still apply.

Omitted headers and failed queries preserve the last valid reading.
Known exhaustion stays closed until the reported reset or a newer valid report.
A stale nonzero reading does not prove current availability.
Provider snapshots are held in memory. Restarting the gateway requires a new provider query.
Local plan counters persist across gateway restarts.

Local windows use a two-bucket estimate. They do not reproduce a provider's calendar or activation-based reset clock.
Traffic outside this gateway is not counted locally. Earlier provider usage can also be unknown.
Do not interpret a new local plan as verified full provider allowance.

Codex and ChatGPT subscription connections do not participate in this rotation.
They use their existing direct connection paths.
