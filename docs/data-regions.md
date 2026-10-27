# Deployment Region and the Data Region restriction

`[deployment] region` declares `eu`, `us` or `jp`. Cloud and Dedicated must
declare one; Self-hosted may omit it, in which case the Tenant restriction is
unavailable. This is an operator declaration, not a location detector. Aidash
cannot verify a Self-hosted declaration. The declaration covers the database,
Secret Store, backups, logs, Execution Pods, Egress gateway and broker.
External Identities, Billing Accounts and Marketplace are shared across
Deployments outside that boundary.

The Tenant's existing Data Protection policy adds `data_region_restricted`,
defaulting to false and displayed as **Any (Recommended)**. Only Tenant
Administrators can change it. A Tenant cannot name a region independently of
its Deployment or supply an OpenRouter host. Plan Connection exemptions cover
both the level and region; they do not count toward a usable remaining model.
The setting refuses an enabled restriction without a Deployment declaration
or without a remaining Deployment-hosted or region-available OpenRouter model.
The impact endpoint uses the same regional discovery as sends. The Cloud
creation picker has no Tenant credential yet, so it discloses the documented
per-region limitations and account prerequisite; authenticated availability is
checked by the Tenant policy preview and each send.

Each send reloads the current chain policy. Restricted Tenants' regions must
overlap. An unknown remote Tenant contributes the strictest Data Protection
Level, but no region restriction, with that assumption logged (#566). Research
account terms must explicitly record a matching `data_region`. Deployment-hosted
origins are allowed only in the matching Deployment; network permission alone
never declares hosting. Tenant Keyless destinations are refused. Tenant HTTP/MCP
tools and Agent Plugin remote MCP servers follow the existing policy boundary.

OpenRouter calls use the fixed EU/US regional base URL and fail closed for JP.
The Capability Token signs the region and the broker chooses the matching
code-defined host. Unrestricted tokens keep global routing. Before content is
sent, the account's regional `/models/user` catalog is checked, and Provider
tools' pinned engines are checked against regional `/tools`. Catalogs have a
30-second cache partitioned by Tenant, credential pin, region and path. A
read-only `provider_discovery` maintenance capability allows authorized policy
previews without creating a Run or granting inference authority. Per-send
discovery first validates the original Run/Decision/approval-observation scope,
then narrows authority. It retains an inference Run's admitted credential pin;
Deciders use the Tenant's current binding as required by their existing contract. A failed or
unknown catalog never falls back to global routing.

## Upstream facts checked on 2026-10-28

- [Sovereign AI](https://openrouter.ai/docs/guides/features/sovereign-ai) and
  [In-Region Routing](https://openrouter.ai/docs/guides/features/in-region-routing):
  regional hosts are `eu.openrouter.ai` and `us.openrouter.ai`; accounts need a
  supporting plan. Europe includes EU/EEA, UK and Switzerland. Account, billing,
  authentication and operational metadata can remain outside that region.
  BYOK deployments and configured observability destinations remain the account
  holder's responsibility. Regional requests do not fall back out of region.
  The regional guide documents `/models`; this implementation uses the Issue's
  stricter account-specific `/models/user` and refuses if it is unavailable.
- [Server tools overview](https://openrouter.ai/docs/guides/features/server-tools)
  and [Tools API](https://openrouter.ai/docs/api/api-reference/tools/list-server-tools):
  entries have canonical `id`, `default_engine`, and engines with `id` and
  `data_regions`. US web search permits Exa, including auto/omitted engine
  resolving to Exa; EU web search, regional web fetch and Fusion are unavailable.
  Datetime runs in-process in both regions. The implementation checks both the
  live catalog and a conservative code-defined allowlist; unrecognized tools
  or engines are refused. These rules are independent of inference ZDR.

An unauthenticated catalog probe returned HTTP 401; authenticated regional
account acceptance has not been claimed. The runtime reports catalog/auth/plan
failures and refuses the affected destination.

## Current Cloud infrastructure

The managed Helm values declare US. Runtime and TLS secrets use user-managed
replication in `us-central1`; Provider Credential secrets already do. Existing
backup locations remain US. Namespace logs (including sandbox, trusted and
installer Pods) and broker logs get regional buckets and sinks, with matching
exclusions from `_Default` so it does not retain another copy globally. This
does not relocate old logs or secret versions. Secret Manager
[replication locations cannot be changed after creation](https://docs.cloud.google.com/secret-manager/docs/choosing-replication),
so existing automatic secrets need an explicit replacement and payload migration;
TLS/ACME secrets retain `prevent_destroy`, which intentionally blocks an
unplanned replacement. Operators must resolve that migration, review retention
and confirm the actual deployment boundary before enabling the declaration. No
infrastructure has been applied by this change. Regional Cloud provisioning is
#547.

The log configuration follows the
[bucket](https://registry.terraform.io/providers/hashicorp/google/latest/docs/resources/logging_project_bucket_config)
and [sink](https://registry.terraform.io/providers/hashicorp/google/latest/docs/resources/logging_project_sink)
contracts; [project exclusions](https://docs.cloud.google.com/logging/docs/reference/v2/rest/v2/projects.exclusions/create)
apply specifically to `_Default`. Operator-defined additional sinks must also
stay in region. Same-project log buckets are
[automatically authorized as sink destinations](https://docs.cloud.google.com/logging/docs/export/configure_export_v2),
so these sinks do not require a cross-project writer grant.
