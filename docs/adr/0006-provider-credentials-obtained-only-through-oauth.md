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
