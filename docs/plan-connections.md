# ChatGPT Plan Connections

A Plan Connection lets a subject on a Self-hosted Deployment run agents on
their own ChatGPT plan. It belongs to that subject in that Tenant. It does not
provide a Tenant-wide credential, an Aidash sign-in method or a fallback for
OpenRouter. Cloud and Dedicated Plan Connections remain disabled pending
[OpenAI's hosted-app approval](https://github.com/kent8192/aidash/issues/248).

## Enable and connect

Configure the [PostgreSQL Provider Credential Store](provider-credentials.md)
with a Master Key and independent fingerprint key, then opt in:

```toml
[plan_connections]
enabled = true
```

The default is `false`. A Secret Manager Store cannot enable this Self-hosted
path. Whoever runs the Deployment's host can technically read the Plan Tokens
it holds. PostgreSQL encrypts them at rest; that does not protect them from the
host that has the Master Key. Enabling this setting accepts that boundary.

Sign in as the subject that will own the connection and open personal ChatGPT
plan settings in the current Tenant. Connect opens OpenAI's consent screen.
Choose whether to grant ChatGPT plan usage; identity-only consent does not
authorize model calls. Aidash never asks for an OpenAI API key or Plan Tokens.

The server keeps the PKCE verifier, nonce and pending authorization. OpenAI
returns the browser to an HTTP callback on `127.0.0.1`:

- **Desktop:** the native client opens the browser, receives the callback on a
  temporary loopback port and relays its URL over the same Aidash session.
- **Browser:** copy the failed loopback URL from the browser's address bar and
  paste it into the pending connection screen. This URL contains a single-use
  authorization code. Do not paste it into chat, tickets or logs.

The callback must match the initiating session and pending authorization.
Changing Tenant, Mapping, connection or session cancels the local flow. The
server alone exchanges the code and writes Plan Tokens into the encrypted
Store. Callback URLs are transient and are not saved in browser storage.

The connecting External Identity must be the subject's only active mapped
identity. Adding another identity to a subject with a connection is refused;
changing the effective identity never gives that identity the old owner's
plan. Deployment Administrators have no separate execution or connection path.

## Configure an agent's model

Choose ChatGPT in the Registry model form and select a model available to your
connection. Plan model availability depends on that connection and the
provider. The model source is `plan_connection: "chatgpt"`; do not combine it
with `provider_credential` or an environment key. The Responses endpoint is
`https://api.openai.com/v1`.

A model's context window, maximum output tokens, modalities and projection
versions remain explicit Registry metadata. Review any manually entered
defaults before approving a model. The provider model slug must be one the
owner's connection can use. Registering or approving a model never shares a
Plan Connection with another subject.

Legacy, Ordered and Native projection are supported. Native maps messages,
function calls and matching function outputs to Responses items. Every call
supplies its projected history with `store: false` and `stream: true`; it does
not depend on provider-stored history or `previous_response_id`. Encrypted
reasoning is not retained or replayed (ADR 0083). Reasoning continuity and cache
benefits from encrypted-reasoning replay are not promised.

Plan usage accepts function tools, text and supported image input. Audio and
OpenRouter-specific media routes or explicit cache breakpoints are unavailable.
A configured output bound is enforced locally even where the plan endpoint
does not enforce a requested maximum.

Only Runs whose delegation chain starts at the owner may use the connection,
including their scheduled, event-triggered and local child Runs. It serves the
owner's Tenant across Nodes of this Deployment. It does not serve federated
Runs, Tenant maintenance or delegation rooted at a service subject.

Context compaction remains separate: a Run that compacts sends the relevant
history to OpenRouter using the Tenant's OpenRouter binding. Connecting a
ChatGPT plan does not supply that binding or change its data handling.

## Usage, failures and lifecycle

The dashboard identifies applicable calls with **Using ChatGPT plan** and links
to [ChatGPT usage settings](https://chatgpt.com/settings/usage). The owner can
review their allowance and per-app limits there. An exhausted allowance or
app limit fails the Run; Aidash never switches to OpenRouter or another
subject's connection. Repeated sign-in does not restore an exhausted allowance.

Connection settings and a persistent dashboard notice distinguish a connection
that needs reconnection from an ineligible account. Refresh occurs only when a
call needs it and is serialized per connection. An unused connection needs
reconnection after its refresh lifetime; Aidash does not keep it alive with
background refreshes.

If a rotating refresh may have succeeded but its replacement cannot be
recovered safely, the connection needs reconnection. Aidash does not replay an
old grant indefinitely. Interrupted locally durable replacements must be
recovered before another refresh or disconnect completes.

Disabling plan usage stops new authorizations, model discovery, admission and
calls. Existing owners can still inspect and disconnect their connections.
Identity disablement pauses eligibility. Removing the owner's Mapping, leaving
the Tenant or deleting the Tenant retires its connection. Restoring a deleted
Tenant requires a new connection. A Run admitted before disconnection fails at
its next provider call.

Disconnect removes Aidash's local tokens and attempts to revoke the upstream
grant. If revocation is unconfirmed, the screen directs the owner to remove
the connection in ChatGPT too. Local deletion is not proof of provider-side
revocation. See [Store recovery](provider-credentials.md) for Master Key loss.

## Acceptance evidence

The [credential-security completion record](operations/provider-credential-security-completion.md)
tracks local regression checks separately from real provider, IAM and
protected-workflow acceptance. Fake OAuth and Responses fixtures do not prove a
real connection. A real authorization requires the owner's interactive account
selection, login/MFA where needed and consent. Record only sanitized result
metadata; no callback URLs, tokens or provider account identifiers belong in
acceptance artifacts.
