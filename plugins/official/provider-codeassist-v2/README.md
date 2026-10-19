# Google Code Assist subscription adapter

This South ABI component translates `gemini-cli-plan` and `antigravity-plan` requests.
The Token Station host reads the local application login and supplies the credential.
The component has no network, filesystem, or secret-store access.

The host must provide `google_project` and the `local_subscription_token` slot.
Gemini CLI uses `cloudcode-pa.googleapis.com`. Antigravity uses `daily-cloudcode-pa.googleapis.com`.
Requests and streaming replies use the Code Assist envelope.

Caller tools remain caller-owned. Google function-call signatures travel in opaque call IDs.
Tool history from another model returns a capability error.
Recognized reasoning-preset aliases share their underlying model identity. Version, family, Lite, and image differences remain distinct.
Google thinking parts replay through a bounded opaque carrier bound to the same model and unchanged thinking text.
Local tool identifiers use random nonces. The adapter restores the original Google identifiers and signatures during replay.
Unsupported continuation, hosted tools, foreign opaque thinking, strict schemas, and serial-only tool constraints return explicit errors.
Low and high reasoning effort are supported for Gemini model families.
Generated images are not supported.

Tests use synthetic credentials and deterministic provider fixtures.
Live account access requires a local application login and a successful Code Assist entitlement check.

Ordinary text thought signatures are optional in Google's generate-content contract.
They do not turn visible output into a thinking or redacted-thinking block.
Buffered responses retain their exact values in the canonical message extension `codeassist_optional_text_signatures`.
Stock Agent adapters do not replay that extension. Stock streaming cannot carry this optional text metadata.
The text remains available, but optional reasoning continuity can be reduced in later turns.
Required function-call signatures remain attached to their original calls through opaque call IDs.
Image output and other unsupported required state still return explicit capability errors.
See [Google's non-function-call signature contract](https://ai.google.dev/gemini-api/docs/generate-content/thought-signatures#signatures-in-non-functioncall-parts).
