# Token Station

Token Station connects AI Agents to model delivery channels and routes each request to one exact model offering.

## Model Catalog Language

**Model**:
An AI model identity owned by one model developer, independent of where the model is served.
_Avoid_: Provider model, endpoint model

**Provider Channel**:
A credential, region, plan, and endpoint combination that can deliver one or more models.
_Avoid_: Model vendor, model family

**Model Offering**:
One model as delivered through one provider channel, identified by the exact upstream model ID that requests must use.
_Avoid_: Model-provider pair, catalog model

**Model Developer**:
The organization that creates and maintains a model.
_Avoid_: Provider, channel

**Upstream Model ID**:
The exact model string accepted by a provider channel.
_Avoid_: Canonical model ID, display name

**Catalog Verification**:
Evidence that a model offering appears in an authoritative provider source at a recorded time.
_Avoid_: Availability guarantee, account verification

**Account Verification**:
Evidence that a configured account can currently discover or call a model offering.
_Avoid_: Catalog verification
