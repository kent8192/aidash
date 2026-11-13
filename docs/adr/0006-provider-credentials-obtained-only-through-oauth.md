# Provider Credentials are obtained only through provider OAuth

Decided on 2026-10-09. A Provider Credential is obtained only through Provider Authorization, the provider's own OAuth consent flow. For OpenRouter, this is the PKCE flow that returns a user-controlled API key. Aidash never accepts Key Material typed or pasted by a user. Model-provider keys supplied in `AIDASH_SECRET_*` environment variables (`credential_env` on model, embedding and compaction configurations) are also removed. Every model-provider call therefore uses a Tenant's Provider Credential, on Cloud and self-hosted alike. Issuing keys by hand was the main source of onboarding friction, and pasted keys could not be attributed to an authorization the user had visibly granted.

## Considered Options

- Keep key pasting beside OAuth. Rejected: two entry paths for Key Material double the surface that must stay write-only, and pasting remains the path users are told to avoid.
- Keep environment-variable keys for self-hosted nodes. Rejected by the user. Self-hosted nodes now require a Provider Credential Store, and the ADR 0002 consequence that nodes without a Store keep `credential_env` no longer holds.

## Consequences

- The Provider Catalog admits only providers that offer an OAuth flow that issues credentials. BYOK with OpenAI Platform API keys is out of scope.
- The OAuth code exchange runs on the application host, which sees the key once and writes it straight to the Provider Credential Store. This matches ADR 0002's assumption that keys entered after a host compromise pass through the compromised host.
- Revocation destroys only Aidash's copy. The OpenRouter key stays valid until the user deletes it. At connection time, Aidash stores the key's unkeyed SHA-256 so it can offer the OpenRouter key-settings deep link. This is an exception to the Fingerprint's no-correlation rule, accepted because each authorization issues a fresh random key.
- Tests and CI register Provider Credentials through an internal write port that is compiled only into test builds. Production binaries have no environment-variable path for model-provider keys.
- Peer Credentials, Subject Credentials and tool or runner secrets are not Provider Credentials and keep their existing configuration.
- Compaction calls Jev through OpenRouter's System One API (`https://openrouter.ai/api/v1/systemone`, model `typesafe/jev-1.13`), decided on 2026-10-09. It therefore uses the Tenant's OpenRouter Provider Credential like every other model-provider call, and TypeSafe needs no Catalog entry of its own. Zero data retention is required on that path; whether `/systemone` accepts `provider.zdr` is verified during #158.

Superseded in part by ADR 0036: Keyless endpoints, which carry no key, remain allowed; only calls that carry a key must use a Tenant's Provider Credential.

## Amendment (2026-11-11, #877)

A Self-hosted Deployment also accepts Key Material that a person supplies, because its host already reads the Secret Store (ADR 0008) and a pasted key trusts nothing more than that. Cloud and Dedicated stay as decided above.

- On a Self-hosted Deployment, a person whom the Tenant's policy allows `provider_credential.create` (or `.rotate` for a new version) may paste Key Material for a Provider Catalog entry in the dashboard. The decision is the same one Provider Authorization uses; no role is added.
- Whoever has Host Access may store Key Material into a named Tenant with the host-side `manage store-provider-credential` command, which reads the key from standard input.
- Each Provider Credential records how its current key arrived: Provider Authorization, pasted, or Host Access. The record and its lifecycle events carry it, so the Tenant's audit history tells the paths apart.
- A supplied key has no key digest and no key-settings deep link. The digest exception above rests on every authorization issuing a fresh key; a supplied key may be stored in several Tenants, so a digest would correlate them.
- Environment-variable model keys (`AIDASH_SECRET_*`, `credential_env`) stay removed on every Offering: such a variable could still be named by Registry configuration and sent to a configured endpoint.
- Adding providers that issue keys only by hand, such as Anthropic or Gemini, to the Provider Catalog is a separate decision.
