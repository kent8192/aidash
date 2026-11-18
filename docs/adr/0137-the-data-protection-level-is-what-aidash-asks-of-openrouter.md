---
status: accepted
---

# The Data Protection Level is what Aidash asks of OpenRouter

Decided on 2026-11-16 (#961, #962, #963). This supersedes ADR 0104's levels, its model-call ZDR switch and its table of destinations. Its rule that a level is met by what Aidash requests and the destination publishes, and that anything else is refused rather than downgraded, still holds.

A Tenant's Data Protection Level now governs only the OpenRouter model calls made with its content: Runs, Decider Routes, compaction, embeddings and Jev. There are four levels, and new Tenants start at No Training.

| Level               | `provider` fields         | Route check                                                                                      |
| ------------------- | ------------------------- | ------------------------------------------------------------------------------------------------ |
| No Restriction      | none                      | none                                                                                             |
| No Training         | `data_collection: "deny"` | none                                                                                             |
| Safety Retention    | `zdr: true`               | `provider.only` from `/endpoints/zdr` before every call; Safety Retention entries are admitted   |
| Zero Data Retention | `zdr: true`               | `provider.only` from `/endpoints/zdr` before every call; `safety_retention` entries are excluded |

OpenRouter's `/endpoints/zdr` lists endpoints that keep content only to prevent abuse alongside strict ones, marked `safety_retention: true`, and `zdr: true` admits both unless the account says "ZDR only". Zero Data Retention is therefore strict wherever Aidash asks for it:

- An entry is matched per `(model_id, tag)`, since a tag can be Safety Retention for one model and strict for another.
- A missing or non-boolean `safety_retention` counts as Safety Retention.
- A tag above a Safety Retention tag of the same model is dropped, because `provider.only` admits every tag below the ones it lists.
- A pinned Provider Route or Media Route that is only Safety Retention is refused with its own reason, and a model with only Safety Retention routes is refused before anything is sent.
- When the list cannot be read, the call is refused; Aidash never falls back to `zdr: true` alone.
- A strict System One Decider body is pinned to the admitted routes, and every host of a Decisions API route must itself be admitted.

The Capability Token signs the requirement: `zdr` (strict, and the meaning of an omitted claim), `safety_retention`, `no_training` or `none`. For `zdr` the Enterprise broker reads `/endpoints/zdr` itself with the Tenant's key, cached for 30 seconds, and refuses a body whose `provider.only` is missing, empty, or lists a tag the list does not admit. It refuses the call when the list is unavailable. Both sides share one filter in `aidash-domain`, so the server cannot widen what the broker allows.

There is one level and no separate model-call setting. Once the level governs only OpenRouter, a separate model-call level would only repeat it. The level is the floor of every call made for the Tenant; the Deployment floor and the strictest Tenant of a delegation chain can only raise it. A Tenant on another Deployment still counts as Zero Data Retention (#566).

The other destinations leave the level:

- **Plan Connections** (ChatGPT subscriptions) and **Tenant Keyless endpoints** each get a Tenant switch that forbids them, off by default. A switch applies when any Tenant in the delegation chain turns it on, and a Tenant on another Deployment counts as having both on. Turning one on refuses the next call. Deployment-hosted models are never forbidden, since nothing sent to them leaves the Deployment. Every Run a Plan Connection carries records it.
- **Research**, page reading included, and Tenant HTTP/MCP tools are outside the policy.
- **Provider tools** stay inside it, because they run inside an OpenRouter call that no zero data retention promise covers. A call can use them only when its level is No Restriction.

OpenRouter screens all traffic, zero data retention included, for child sexual abuse material and may keep flagged content for review and legal preservation. No level avoids that, and the settings screen says so. At Zero Data Retention it also suggests setting the Tenant's own OpenRouter account to "ZDR only" as a second safeguard. A Tenant with its own Google Cloud ZDR agreement gets no exception: whatever the list marks as Safety Retention is excluded.

## Considered Options

- **A model-call slider above the Tenant level** (#963 as filed). Rejected once #962 left the level governing only OpenRouter: the two settings would always send the same request.
- **Admitting Safety Retention at Zero Data Retention.** Rejected: the user reads "ZDR" as a promise that content is not kept.
- **Filtering on the server only.** Rejected: the broker exists so that a faulty server cannot breach the signed requirement.

## Consequences

- Every Gemini model, `google/gemini-embedding-2` included, and every other model with only Safety Retention routes is unusable at Zero Data Retention. The embedding form prefills `amazon/titan-embed-image-v1` at that level and warns when the chosen model cannot meet the level. The model picker lists such models but does not let them be chosen.
- An embedding call refused by the level never waits. A Workspace retrieval source that may degrade continues at once with a `data_protection` marker; any other stops the Run with that reason.
- Raising the level lists what stops working and asks the person to confirm first.
- Existing Tenants keep their level, with Standard renamed No Restriction. `model_call_zdr` is dropped, so a former Standard Tenant no longer sends `zdr: true`.
