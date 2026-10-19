# A Decider version tries its Decider Routes in order, for availability only

Decided on 2026-10-19 for #153, in the #156 grilling. Jev is hosted by TypeSafe alone on OpenRouter, and a bound Decider that fails pauses the Run at once with `decider_unavailable`, so one maker's outage stopped every Run whose context outgrew its model. A Decider version now lists Decider Routes in order. The first Route is Jev through OpenRouter's System One API, as today. Later Routes reach other decision models through OpenRouter's Decisions API (`POST /api/alpha/decisions`), which asks the same `noul` question of every model, with the Tenant's same OpenRouter Provider Credential, `provider.zdr = true` and an explicit host list (`provider.only`). The first such Route is `cloudflare/clef` on Cloudflare, DeepInfra and PrimeIntellect, with a state budget that fits the smallest of them (16k tokens) and no long-context mode.

A Decision Attempt moves to the next Route on a connection failure, a timeout, 429, 5xx (including 529) or an answer that is malformed or fails validation. It never moves on 400, 401, 402, 403, 413, a missing Provider Credential Binding or a revoked credential: those fail closed (#136), and 402 is the Tenant's OpenRouter account, which every Route shares. When every Route has failed, the whole list is tried again after 2, 4, 8, 16 and 32 seconds, and then the Run pauses with `decider_unavailable` until a manual resume. Every dispatched Decision Attempt, on any Route and in any retry, counts against the version's `max_calls_per_run`. A charged attempt recovered with an unknown result still pauses the Run, as before.

## Considered Options

- **A bounded retry with no second Route.** Rejected: since October 2026 OpenRouter lists more than twenty decision models from other makers that answer the same question types, several of them on ZDR endpoints.
- **OpenAI's Decisions API, called directly.** Rejected: it needs an OpenAI API key, the Provider Catalog admits only credentials issued through OAuth (ADR 0006), and a Plan Connection reaches only the Responses API (ADR 0007).
- **`openai/gpt-6-luna-decisions` through OpenRouter.** Rejected for now: it is not on OpenRouter's ZDR endpoint list and its input is moderated.
- **Jev through the Decisions API as well, so one adapter serves every Route.** Deferred: the Decisions API is an alpha endpoint, and keeping the first Route on another API keeps a Decisions API change or outage from taking down every Route at once. Revisit when it leaves alpha.
- **Resuming automatically after the pause.** Rejected: semantic outages already end in a pause and a manual resume (ADR 0057, ADR 0059).
- **The Summary Stage as a fallback.** Still rejected, for the reason in ADR 0010.

## Consequences

- Routes remove the dependency on one maker, not on OpenRouter: an OpenRouter outage still pauses compaction, as #156 accepts.
- A Route is chosen only because an earlier one is unavailable. Aidash never compares answers between Routes or picks a more favorable one.
- Models calibrate their probabilities differently, so each Route has its own thresholds and state budget, chosen by an evaluation against Jev on fixed compaction fixtures and recorded in the repository. Changing them makes a new Decider version, which needs a new Catalog Approval.
- Approving a version approves every Route and every host it names, and the approval shows them. A host that OpenRouter adds later never receives state until a new version names it.
- Decision Evidence records which Route answered each decision and which Routes failed before it.
