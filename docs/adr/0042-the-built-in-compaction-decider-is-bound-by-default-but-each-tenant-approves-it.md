# The built-in compaction Decider is bound by default, but each Tenant approves it

Decided on 2026-10-13 for #108. Every Run is pruned by Jev today, through a key supplied in the Deployment's environment, so whoever runs the Deployment, not the Tenant, decided to send Run content to Jev's upstream. With environment keys removed (ADR 0006) and Jev reached through the Tenant's own OpenRouter Provider Credential, that decision moves to the Tenant. Aidash provides a built-in compaction Decider that every Agent is bound to unless it removes that Binding. Like a Sandboxed tool (ADR 0034), it needs a Catalog Approval for its exact version on each Node, because it sends Run content to a third party the Tenant never chose as a model. Until it is approved, a Run whose context outgrows its model pauses with a typed reason.

## Considered Options

- **Bound by default, with the Tenant's OpenRouter connection as consent.** No extra step, but the disclosure to Jev's upstream would be implied by connecting a different service. Rejected.
- **Only explicit Bindings.** Every existing Agent would stop at its first oversized context, breaking ADR 0010's promise that Agents without a Context Policy keep prune-only behavior. Rejected.
- **A Tenant-wide default Decider per lifecycle boundary.** It adds a Tenant setting beside the Binding and the Catalog Approval for no gain over a default Binding. Rejected.

## Consequences

- ADR 0010's promise that Agents without a Context Policy keep prune-only behavior now holds only on Nodes where the built-in Decider is approved; elsewhere such a Run pauses instead of being pruned.
- An Agent that removes the Binding has no pruning stage; if its Context Policy enables the Summary Stage, that stage runs alone, because this is explicit configuration. A Decider that is bound but unavailable (unapproved, no Provider Credential Binding, call limit reached or provider outage) pauses the Run and never falls back to the Summary Stage, as ADR 0010 requires.
- Generated chains do not inherit the default Binding. They use a Decider only when their generation policy names its exact version and every generated ancestor agrees, as compaction required before.
- A new Jev model adds a new version of the built-in Decider, which each Node approves again. Runs keep the version they pinned; once OpenRouter retires that model, those Runs pause.
- A Decider version declares a per-Run call limit that a Binding can only lower, in addition to a generated chain's call budget. Each Decider call is a Decision Attempt paid from the Tenant's OpenRouter account (ADR 0041); its provider-reported cost is kept in the Decision Evidence, never in a Usage Record.
- The `compactor` Registry kind is removed rather than read as a Decider, since Aidash has no deployment to keep compatible.
- (2026-10-17) A Decider version also fixes, in bytes, how much state it sends to Jev and the stages of abridging it (full, inputs ≤ 200 characters, inputs ≤ 60 characters). The budget keeps requests within Jev's default mode (about 32,000 bytes; 32k tokens for the state plus one question). Only a state that still does not fit after the last stage is sent in OpenRouter's long-context mode (up to 262,144 tokens), which TypeSafe describes as slower and less accurate. A larger state pauses the Run with `decider_unavailable`. The Catalog Approval of a version also allows the state retention that version declares. The first built-in version pins `typesafe/jev-1.14`.
- (2026-10-19, #153) The built-in version lists two Decider Routes (ADR 0066): `typesafe/jev-1.14` through the System One API, then `cloudflare/clef` through OpenRouter's Decisions API on Cloudflare, DeepInfra and PrimeIntellect with ZDR. Approving it approves sending Run state to each of those hosts, and the approval shows them. A Tenant that wants Jev alone defines its own Decider version with one Route and binds it instead.
