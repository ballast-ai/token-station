# Subscription connections

Select **Models → Add model → Subscriptions**. Choose a channel in the subscription window.
Select **Back to subscriptions** to choose another channel.

| Channel | Local source and transport |
| --- | --- |
| Claude | Reuse the local Claude Code subscription login through the installed Claude Code process. |
| ChatGPT | Reuse the existing local Codex login and its Codex backend. |
| ChatGPT API | Use the existing independent Sign in with ChatGPT registration and public Responses transport. |
| Copilot | Reuse Copilot editor or supported Copilot CLI login. Resolve the account's inference session and model endpoints. |
| Gemini CLI | Reuse the local Gemini CLI Google login and Code Assist project. |
| Antigravity | Reuse supported local Antigravity file, database, or system credential storage and Code Assist transport. |

For a local channel, sign in with its original client first. Then select **Check local login**.
Select **Use local login** or **Use this account**. Token Station checks the account and model catalog before adding the provider.
Select its discovered model in the existing routing page. Save and apply the configuration.

The login reference stays bound to its source and account identity. An account replacement requires an explicit reconnect.
**Disconnect reference** removes the Token Station binding. It does not sign the original client out.
The renderer receives account metadata and opaque reference IDs. It does not receive access tokens or refresh tokens.

Subscription inference uses account allowance. Token Station does not estimate API dollar cost or silently fall back to a paid API.
Model names and supported endpoints come from each account's catalog or the installed client's declared catalog.
Catalog presence does not prove that every model has remaining allowance.

## Current boundaries

- Claude supports text and caller-owned tool rounds. Replies are buffered before the SSE response is returned.
- Claude rejects images, opaque thinking state, unsupported sampling, and forced tool constraints.
- Claude's installed client declares the model catalog. Individual model entitlement still depends on the account.
- Claude requires direct egress. Configured proxy egress is rejected. Ambient proxy variables are ignored.
- Copilot Auto and models with unaccepted policy are excluded. This version does not change model policy for the user.
- Copilot supports fixed public GitHub inference destinations. Custom GitHub Enterprise hosts are not supported.
- Copilot CLI system credentials are supported on macOS. Other CLI keyrings and JSONC configuration are not supported.
- Google account access, project permissions, and model availability are checked by Code Assist.
- Google subscription calls use Host HTTP transport. Receipts record `root_rpc_path` because South cannot represent these root RPC actions.
- Google signed tool state stays bound to its model. Unsupported continuation and foreign opaque state return capability errors.
- Google optional text signatures are retained in buffered response metadata. Stock translated clients and streams do not replay them.
- Windows Antigravity credential support has deterministic parser coverage. Real Windows credential access remains unverified.

These implementations use the channel-specific protocols studied in [Magpie](https://github.com/yetone/magpie).
They do not require a running Magpie process.
