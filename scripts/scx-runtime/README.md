# SCX local classification runtime

This runtime serves one local classifier through private stdin/stdout IPC.
It does not open a network listener or call an upstream inference API.
The desktop controller owns its process, queue, timeouts, and routing fallback.

The standard desktop App supports local classification for smart tiers on macOS Apple Silicon.
New installations and settings without a saved choice default to off.
The **Local smart tiers** switch remembers the choice across App restarts.
Only an explicit enable action or an existing enabled preference starts preparation.
When enabled, it loads a prepared model in the background at each normal launch.
If runtime assets are missing, it prepares the pinned assets and then starts classification.
Preparation, loading, and classifier failures keep existing routing rules available.
Each enabled launch makes one automatic startup attempt. After correcting a setup failure, turn the switch off and on to retry.
Turning the switch off stops preparation and the worker to release model memory.
The preference is stored in `semantic-settings.json` beside the runtime directory.
Normal shutdown stops the worker without changing this preference.
Unsupported systems and recovery safe mode do not start the classifier.

The supported setup target is macOS on Apple Silicon with Python 3.11.15.
The model is `scx-admin/scx-router-v0.1` at revision `b45625de43a3bac2861d3f11b96c15a93f4a026e`.
Model SHA256 hashes, dependency versions, and seed-package checksums are in `assets.json`.
`requirements-lock.txt` pins all 44 packages and permitted wheel hashes.
These pins reproduce the verified local runtime. They are not rolling dependency ranges.

## Prepare

Install [uv](https://docs.astral.sh/uv/getting-started/installation/) before preparation.
Setup searches PATH, `~/.local/bin/uv`, `~/.cargo/bin/uv`, and `/opt/homebrew/bin/uv`.
Setup does not install uv through a shell script.

```sh
python3 scripts/scx-runtime/setup.py --runtime-dir /private/token-station-data/semantic-runtime
```

Preparation can download the pinned Python runtime, package wheels, and model files.
It verifies wheel hashes and every model file before reporting success.
The initial model download is approximately 2.5 GB.

Reuse an explicitly selected, verified local installation without downloading those assets:

```sh
python3 scripts/scx-runtime/setup.py \
  --runtime-dir /private/token-station-data/semantic-runtime \
  --seed-python-env /local/verified/.venv \
  --seed-model /local/verified/models/scx
```

Seed packages must match the pinned versions, RECORD checksums, and recorded content hashes.
Setup copies verified package files into a new virtual environment.
It does not reuse the seed environment's console scripts or bytecode caches.
Model seeds must match all pinned model hashes.
The copied runtime has independent package and model files.
Its Python interpreter uses uv's persistent managed Python installation.
Keep that managed Python installation available while using local SCX classification.

Run preparation again to verify and reuse an existing runtime.
A file lock permits only one preparation process.
After cancellation, preparation rebuilds the known incomplete stage directories.
It rejects symbolic links in those stage locations.
The host must terminate the setup process group when cancelling preparation.

The installed layout is:

```text
semantic-runtime/
  .venv/bin/python
  models/scx/
  worker.py
  setup.py
  assets.json
  requirements-lock.txt
  prepared.json
```

The desktop embeds the four source assets listed above outside `.venv/` and `models/`.
No private repository path is required at runtime.
Before each worker launch, the host atomically refreshes the managed `worker.py` from the App bundle.
This protocol update does not download or replace the prepared environment or model files.

## Worker protocol

```sh
/private/token-station-data/semantic-runtime/.venv/bin/python -I \
  /private/token-station-data/semantic-runtime/worker.py \
  --model /private/token-station-data/semantic-runtime/models/scx
```

The worker verifies model hashes, loads locally, and warms inference before readiness:

```json
{"event":"ready"}
```

A startup failure emits `{"event":"error","code":"initialization_failed"}` and exits nonzero.
Each request contains exactly a bounded `id` and a visible text projection:

```json
{"id":1,"text":"Translate hello into French."}
```

Each reply contains the same identifier and a closed status:

```json
{"id":1,"status":"ok","tier":"low"}
{"id":2,"status":"unsupported"}
{"id":3,"status":"error"}
{"id":4,"status":"unsupported","reason":"token_limit"}
```

IDs are unsigned 64-bit integers or 1–128 ASCII letters, digits, underscores, periods, or hyphens.
Invalid protocol IDs produce `id:null` and `status:error`.
The worker never returns prompt text, exception text, raw model scores, or arbitrary model labels.
Library stdout and stderr are suppressed at the file descriptor level.

Five fixed labels map to three tiers:

| SCX label | Tier |
| --- | --- |
| very easy | low |
| easy | low |
| medium | medium |
| hard | high |
| extra hard | high |

The worker uses two CPU threads and one interop thread.
It selects an explicit `torch.device("mps")` when available, otherwise CPU.
It disables remote model code, hub access, telemetry, and progress output.
Inference uses local safetensors and `torch.inference_mode()`.

The complete SCX context and label segments are tokenized before inference.
More than 1,024 combined tokens returns `unsupported` with `reason:token_limit`.
The worker does not classify a truncated request.
Empty text and reserved GLiClass delimiter tokens also return `unsupported`.
Input is bounded to 256 KiB per protocol line and 64 KiB per text field.
The host also preserves its 16 KiB admission limit for the complete visible conversation.
The host must reject multimodal, tool-only, or unsupported native projections before sending text.
Request receipts distinguish host byte rejection from worker token rejection without storing prompt content.
Observe mode does not wait for token validation or update a completed receipt.
Its receipt can report synchronous byte rejection, but later token handling remains unknown.

## Tests

Run dependency-free tests:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 scripts/scx-runtime/test_runtime.py
```

The tests cover protocol recovery, bounded replies, exception redaction, input rejection, complete token counting, checksums, and cancelled-stage recovery.
Real inference validation must use the pinned runtime and model assets.
The smoke result does not establish production routing quality or cost savings.
