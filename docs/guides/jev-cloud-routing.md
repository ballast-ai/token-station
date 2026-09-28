# Jev cloud routing

Jev is an optional cloud classifier for smart tier routing. It uses your TypeSafe API key to select a low, medium, or high tier.
Token Station still selects the generation model from your configured tiers. You still need credentials for those model providers.

## Set up Jev

1. Open **Global routing** and select **Smart tiers**.
2. Enter your TypeSafe API key in the Jev control.
3. Save the key.
4. Test the connection.
5. Read the data-sharing notice and enable Jev.

The connection test sends synthetic text. It does not send your conversations or enable cloud routing.
Saving a key does not enable Jev. Changes apply to new requests without a gateway restart.
Remove the key to disable Jev and delete its saved credential.

## Routing behavior

Explicit model choices, direct routes, quota routing, matching rules, and explicit harness mappings keep their existing precedence.
Eligible requests use Jev before the local SCX classifier, when that experiment is installed.
If Jev fails, Token Station returns to its existing routing decision. It does not call SCX after a Jev failure.
The generation model must still pass the existing capability, locality, health, and protocol checks.
Native Anthropic and Responses text requests keep their original generation payload and protocol.
Failed native classification preserves the same route as disabled Jev, including existing rule decisions.
Native tool-result or reasoning histories, multimodal blocks, and Responses continuation identifiers retain their existing route.

The initial classification deadline is 1500 milliseconds. Results below 0.70 confidence fall back to existing routing.
Timeouts, authentication errors, rate limits, invalid responses, and unavailable tiers also use the existing route.
The control shows the latest classification outcome without displaying conversation text.

## Data and credentials

Enabled Jev sends bounded user and assistant text to `https://api.typesafe.ai/v1/systemone`.
It uses the `jev-latest` alias. TypeSafe API usage is separate from generation-provider usage.
Local-only requests never use Jev. Unsupported input shapes retain their existing route.
System instructions, tool results, reasoning content, and generation-provider credentials are excluded from the classification payload.

The key uses Token Station's existing local credential store. This store is plaintext with private file permissions, not an OS keychain.
Saved keys are not returned to the interface or written to request logs. Cloud settings are separate from the model provider configuration.
Requests follow the configured outbound proxy policy. Redirects are refused. Ambient proxy environment variables are ignored.

Jev confidence is a decision signal. It does not prove that a tier will solve the task correctly.
Evaluate task success, incorrect downgrades, added latency, and total cost with your own workload.

See the [TypeSafe documentation](https://docs.typesafe.ai/introduction) and [API reference](https://api.typesafe.ai/docs).
