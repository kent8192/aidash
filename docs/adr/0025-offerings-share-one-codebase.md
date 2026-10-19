# Offerings share one codebase: cloud adapters are build features, the Offering is a validated runtime setting

Aidash ships as three Offerings: Self-hosted, Cloud and Dedicated. Of the 291
HTTP endpoints, 257 behave identically across them. The differences sit in
sign-in, joining a Tenant and Tenant administration, plus the cloud adapters
(GCIP, Secret Manager, Credential Broker).

We keep one codebase and one set of URLs. The cloud adapters are Cargo
features, so a Self-hosted build contains no GCP code. Each deployment selects
its Offering in a runtime `offering` setting. Startup refuses an Offering whose
adapters the binary was not built with. Behavioral differences come from that
Offering's defaults for the tenancy rules, never from the URL.

## Considered Options

- **URL prefixes per Offering** (`/api/cloud/...`, `/api/enterprise/...`). The
  same resource would have several URLs, and their authorization checks could
  drift apart. Offerings are product packaging, so a feature moving between
  Offerings would break URLs. Rejected.
- **One binary per Offering, fixed at compile time.** Cloud and Dedicated use
  the same adapters, so this would build two nearly identical artifacts.
  Rejected.
- **A runtime switch only.** Every Self-hosted binary would still contain GCP
  code and IAM assumptions. Rejected.

## Consequences

- An Offering's endpoints that another Offering lacks are not mounted there, so
  they return 404.
- The legacy Keycloak issuer support is removed. Self-hosted keeps generic OIDC,
  including direct Google.
- The setting is named `offering`, not `edition`, because `edition` is the Rust
  edition.

## Status

Partially superseded by ADR 0032 (2026-10-13): cloud adapters live in the separate `enterprise/` workspace and the Enterprise build, not in Cargo features of the one server.
