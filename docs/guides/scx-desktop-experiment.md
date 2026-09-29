# Local SCX routing

SCX is an optional local classifier for smart tier routing in Token Station.
It selects a low, medium, or high tier. Token Station selects the generation model from your configured pools.
Local SCX preparation is supported on macOS Apple Silicon.

## Set up local smart tiers

1. Install [uv](https://docs.astral.sh/uv/getting-started/installation/).
2. Open Token Station.
3. Open **Global routing** and select **Smart tiers**.
4. Check the low, medium, and high model pools.
5. Enable **Local smart tiers**.
6. Start the proxy if it is stopped.

The standard desktop App includes this control on supported systems.
For a local source installation, use the normal desktop prerequisites and installer:

```sh
scripts/install-local-desktop.sh
```

New installations and settings without a saved choice start with local smart tiers off.
An existing saved choice remains in effect. Installing an update does not enable classification or start a model download without that choice.
The first enable action prepares pinned Python packages and downloads approximately 2.5 GB of model files.
Existing routing stays available during preparation, loading, and classifier failures.
Inference runs locally after preparation. The classifier does not need a model API key or a network inference service.
Generation requests still use the configured providers and their normal billing.

The switch takes effect immediately and remembers the choice across App restarts.
Turning it off stops preparation and inference, releases model memory, and keeps the original routing rules active.
The switch is hidden in fixed-model and quota-first modes. Changing the routing mode keeps its saved preference.
When enabled, each normal App launch loads prepared assets or prepares missing pinned assets in the background.
Each enabled launch makes one automatic startup attempt. After correcting a setup failure, turn the switch off and on to retry.
Quit the App to stop its classifier process and release model memory.
Quitting does not change the saved choice. Recovery safe mode does not start the classifier.

See [runtime setup and protocol](../../scripts/scx-runtime/README.md) for pinned assets and verified local seeding.

## Routing behavior

SCX applies to global and Agent routes that use smart tiers.
Fixed-model routes, quota-first routes, explicit model pins, user rules, and Agent hints keep priority.
SCX uses the existing pools, capability checks, health ranking, recovery, and free-provider fallback restrictions.
Classification does not rewrite the request sent to the selected provider.

When [Jev cloud routing](jev-cloud-routing.md) is enabled, Jev takes priority over SCX.
The local switch keeps its saved choice. A Jev failure uses the original routing decision without calling SCX.

Eligible requests wait at most 400 milliseconds for an SCX suggestion.
Loading, busy, failed, cancelled, or unsupported classifications retain the original routing decision.
The worker accepts one inference at a time. It does not accumulate request text in a queue.
A separate two-second watchdog stops an inference worker that does not respond.
Model loading and preparation have separate startup deadlines.

## Connect an Agent

1. Open **Agent Connections**.
2. Select the Agent and its installation.
3. Select **Preview and connect**.
4. Review the endpoint and configuration changes shown in the preview.
5. Confirm the connection.
6. Restart the Agent if it reads configuration only at startup.

The gateway uses the configured listen address. Local classification does not require a different port.
Preview does not write client settings. Confirmation saves an encrypted snapshot before writing the selected client's settings.
For non-Cursor Agents, normal disconnect restores the managed settings from before connection, including the previous endpoint and key.
Unrelated client settings remain in place. Keep the snapshots while the Agent is connected.
Use one App to manage a selected client's connection at a time.
Cursor keeps its separate connection and configuration restoration workflow.

For a manual client profile, use the App's displayed endpoint and gateway key with model `auto`.
For a configured Agent route, use its scoped endpoint.

## Input limits and request details

SCX classifies visible user and assistant text.
It excludes system scaffolding, tool output, and reasoning traces.
Multimodal requests, unsupported native server-tool requests, and oversized context use existing routing.
Inputs above 16 KiB of projected text or 1,024 formatted SCX tokens are skipped without silent truncation.
Request receipts distinguish the host byte limit from the worker's exact formatted-token limit.
They record only the classifier name, input handling, and a closed limit reason.
Length skips use the existing route and do not truncate the original generation request.
Observe mode does not wait for tokenization and does not add late diagnostics to completed receipts.
An existing prepared runtime refreshes its managed worker script before startup without downloading model assets again.

Recent observations contain only an identifier, mode, tiers, latency, and a closed outcome code.
They remain in memory and are limited to 64 records. The App does not display a comparison panel.
Comparison counts reset when the App restarts.
Normal request receipts can record the `classifier` decision reason.

SCX scores are not calibrated probabilities of answer success.
Tier disagreements are review signals. They do not prove that either model choice is correct.
Evaluate task success, incorrect downgrades, added latency, and total cost with your own workload.
