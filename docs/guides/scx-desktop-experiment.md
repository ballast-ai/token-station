# Local SCX desktop experiment

Token Station SCX is a separate macOS Apple Silicon App for testing local three-tier classification.
It preserves the installed `token-station.app` and uses separate configuration, credentials, data, and gateway port.
It does not establish production routing quality or measured cost savings.

## Install

Build from source with the normal desktop prerequisites.
For the first installation, copy the existing routing settings into the experiment:

```sh
scripts/install-local-desktop.sh --scx-experiment --copy-stable-settings
```

The settings copy requires an absent experiment directory. It never merges or replaces existing settings.
For later installations, omit the copy option:

```sh
scripts/install-local-desktop.sh --scx-experiment
```

The App is `/Applications/Token Station SCX.app`.
Its bundle identifier is `com.tokenstation.desktop.scx`.
Its authenticated gateway uses `127.0.0.1:18787`.
The ordinary App and its gateway keep their existing settings.
The experimental App does not receive ordinary automatic updates.

## Use

1. Open **Token Station SCX**.
2. Select smart-tier routing on Home.
3. Check the low, medium, and high model pools.
4. Start the proxy if it is stopped.

Local classification is enabled by default in the experimental App.
Use **Local smart tiers** on the routing page to turn it on or off.
The switch takes effect immediately and remembers the choice across App restarts.
Turning it off stops model preparation and inference, releases model memory, and keeps the original routing rules active.
The switch remains available in fixed-model and quota-first modes.
When enabled, each normal experimental App launch starts SCX routing automatically.
The App loads prepared assets or prepares missing pinned assets in the background.
The App does not display the former experimental panel or comparison records.
Existing routing stays available during preparation, loading, or classifier failures.
Each enabled launch makes one automatic startup attempt. After correcting a setup failure, turn the switch off and on to retry.
Quit the App to stop its classifier process and release model memory.
Quitting does not change the saved switch setting.
SCX applies to global and Agent routes that use smart tiers.
Fixed-model routes, quota-first routes, explicit model pins, user rules, and Agent hints keep priority.
SCX uses the existing pools, capability checks, health ranking, recovery, and free-provider fallback restrictions.
Classification does not rewrite the request sent to the selected provider.

Eligible requests wait at most 400 milliseconds for a suggestion.
Loading, busy, failed, cancelled, or unsupported classifications retain the original routing decision.
The worker accepts one inference at a time. It does not accumulate request text in a queue.
A separate two-second watchdog stops an inference worker that does not respond.
Model loading and preparation have separate startup deadlines.

The first preparation downloads pinned Python packages and approximately 2.5 GB of model files.
Install [uv](https://docs.astral.sh/uv/getting-started/installation/) before preparation.
Inference runs locally after preparation. It does not require a model API key or a classifier network service.
See [runtime setup and protocol](../../scripts/scx-runtime/README.md) for pinned assets and verified local seeding.

## Connect an Agent

1. Open **Agent Connections** in the experimental App.
2. Select the Agent and its installation.
3. Select **Preview and connect**.
4. Check that the scoped endpoint uses `127.0.0.1:18787`.
5. Confirm the connection.
6. Restart the Agent if it reads configuration only at startup.

Preview does not write client settings. Confirmation saves an encrypted snapshot before writing the selected client's settings.
Connecting switches that client from its previous gateway to the experimental gateway.
Normal disconnect restores the previous managed settings, including the previous endpoint and key.
Unrelated client settings remain in place. Keep the experimental snapshots until you disconnect the Agent.
Use one App to manage a selected client's connection at a time.
The ordinary App and its own settings remain available.

Experimental connection records and snapshots use the experimental App's private directory.
The experiment does not automatically refresh client metadata.
Forced ownership removal stays disabled because it cannot restore the previous credentials.
Cursor's separate HTTPS tunnel is not supported in this experiment. Use the ordinary App for Cursor.

For a separate manual client profile, use base URL `http://127.0.0.1:18787/v1` and model `auto`.
Use the experimental App's own gateway key. The ordinary gateway key does not carry over.
For a configured Agent route, use its scoped endpoint under the same experimental origin.

Provider credentials are copied into independent private files only when the settings-copy option is used.
Actual model calls still use the configured providers and their normal billing.
The local classifier itself does not call those providers.

## Limits and observations

The initial worker supports visible user and assistant text.
It excludes system scaffolding, tool output, and reasoning traces.
Multimodal requests, unsupported native server-tool requests, and oversized context use existing routing.
Inputs above 16 KiB of projected text or 1,024 formatted SCX tokens are skipped without silent truncation.

Recent observations contain only an identifier, mode, tiers, latency, and a closed outcome code.
They remain in memory and are limited to 64 records. The App does not display a comparison panel.
Comparison counts reset when the App restarts.
Normal request receipts can record the `classifier` decision reason.
The settings-copy procedure disables request-body capture in the experimental configuration.

SCX scores are not calibrated probabilities of answer success.
Tier disagreements are useful review signals. They do not prove that either model choice is correct.
Keep the ordinary App available while evaluating the experiment on representative tasks.
