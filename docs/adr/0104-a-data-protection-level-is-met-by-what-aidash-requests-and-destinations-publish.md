---
status: accepted
---

# A Data Protection Level is met by what Aidash requests and destinations publish

Decided on 2026-10-27. A Tenant's policy sets a Data Protection Level, Standard, No Training or Zero Data Retention, for what may happen to its content wherever Aidash sends it. A Deployment may set a floor, and a delegation chain is held to the strictest level of any Tenant in it. Aidash cannot verify what a destination does with content, so a level is met only by what Aidash requests of each destination and what that destination publishes about itself; anything that cannot be met is refused, never silently downgraded.

| Destination                                                                | No Training                                                                                           | Zero Data Retention                                                                               |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| OpenRouter model calls (Runs, Decider Routes, compaction, embeddings, Jev) | `provider.zdr: true`, or `provider.data_collection: "deny"` if the Tenant opted out of model-call ZDR | `provider.zdr: true` and the route checked against `/endpoints/zdr` before every call; no opt-out |
| Tenant Keyless endpoint                                                    | refused                                                                                               | refused                                                                                           |
| Deployment-hosted model, Aidash's own page reader                          | allowed                                                                                               | allowed                                                                                           |
| Provider tool                                                              | refused                                                                                               | refused                                                                                           |
| Research provider                                                          | only if its Research account terms record no training                                                 | only if they record no retention                                                                  |
| Tenant HTTP or MCP tools, remote MCP servers of Agent Plugins              | not covered                                                                                           | not covered                                                                                       |

Aidash already requires zero data retention on every OpenRouter call (ADR 0036, ADR 0041), and the Cloud credential broker refuses any body without `provider.zdr: true`. That stays the default at every level: a Tenant's policy keeps **model-call ZDR** on unless a Tenant below Zero Data Retention opts out. At Standard the opt-out sends no data policy at all; at No Training it sends `data_collection: "deny"` instead. It covers every OpenRouter call made for the Tenant, and a delegation chain keeps ZDR while any Tenant in it has not opted out. The Capability Token signs which requirement applies, and the broker checks the body against it rather than demanding `zdr` unconditionally.

OpenRouter offers no per-request filter for "no training, retention allowed": `data_collection: "deny"` also excludes providers that store data non-transiently. An opted-out No Training Tenant therefore asks OpenRouter for more than it promises, which keeps the promise. Settings of the Tenant's own OpenRouter account, such as Input & Output Logging, are the Tenant's responsibility and are disclosed as such.

Plan Connections are the one named exemption. OpenAI's Zero Data Retention is an API organization arrangement that ChatGPT sign-in does not reach, and personal plans may train on content unless the person opts out, which Aidash cannot see. A Tenant's policy exempts Plan Connections from its level and its Data Region restriction by default, Runs that use the exemption record it, and it covers only the Tenant's own content: a delegation chain that also carries a Tenant without the exemption cannot use a Plan Connection above Standard. At Standard a Plan Connection needs no exemption, because model-call ZDR applies only to providers that offer zero data retention.

The policy of a Tenant on another Deployment cannot be read yet (#566). Until it can, such a Tenant counts as the strictest policy — Zero Data Retention, model-call ZDR on, no Plan Connection exemption — and the assumption is logged, so remote work keeps running under the safest requirement instead of being refused.

## Considered Options

- **A single Zero Data Retention requirement.** Rejected as too strict to be useful; a looser level was wanted.
- **A No Collection level matching `data_collection: "deny"` exactly.** Rejected: it would refuse every Plan Connection and offers little over Zero Data Retention.
- **Plan Connections always allowed, outside every level.** Rejected: a stricter Tenant elsewhere in the chain would be bypassed.

## Consequences

- Raising a level applies from the next send; the settings screen previews which Agents, approvals and destinations stop working. Lowering it applies at once.
- The level selector shows each level's restrictions in advance and recommends none.
- Research account terms gain whether the service trains on or retains requests.
- New Tenants start at Standard with model-call ZDR on, which is today's behaviour.
- ADR 0036's "zero data retention stays required on Provider Catalog paths" and ADR 0041's broker rule become the default rather than an absolute; the broker's body policy reads the signed requirement.
