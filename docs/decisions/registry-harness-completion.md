# Complete the Registry-centric Harness in one PR

Status: the owner's first-round decisions are recorded; independent builder review found no important unresolved decisions. The complete written design awaits the owner's final confirmation before implementation. This is not an implementation-completion claim. It lives in `docs/decisions/` because this repository explicitly ignores generated `docs/superpowers/` and `docs/plans/` paths.

## Intent and scope

Make the Harness operate on Registry declarations rather than a growing collection of source-specific branches. Complete #108, #109 and #110 in one implementation PR, preserving the already delivered #104–#107 and #111 behavior and reconciling the stale parent checklist and superseded terminology. Explicit model selection remains unchanged.

The owner also included #331's separated research-retention remainder. Its implementation already exists in PR #481, merged into the still-open PR #478 branch, rather than into `develop/0.1.0`. Reuse it and ADR 0083; do not rebuild that design or modify another owner's worktree. Verify its integration and acceptance as a dependency of this delivery. Opening a second replacement PR for #331 is outside this plan.

Investigation baseline: `origin/develop/0.1.0@37d427e09b519fbede493d01997e2447472126dd`. PR #478 was open at `d6ee1e85d2d8bf47ff53fd0dd7091483ce401e84`; PR #481 was merged there as `573e8f1cb6e10583b6ee702786930722bbbb1705`. Refresh these before implementation integration.

## Accepted approach

- Complete the shared decision and source contracts, native adapters, UI projections, verification and documentation together in one coherent PR.
- Aidash is undeployed. Use the new current schema with an explicit development reset, rather than migrating old Agent versions, inline journals or Run snapshots (ADR 0084).
- Reuse the existing compaction gate, routes, approval judge, memory safety and research contracts. Do not replace their authority, recovery or cost accounting with a second pipeline.
- Preserve current default source rendering and retrieval semantics. New Scored behavior is selected explicitly and must not silently change a default before the outcome evaluation owned by #122.

Alternatives considered were a compatibility bridge for immutable normalization, an Auto-only observation report, metadata-only source classification, and an Enforce fallback to unscored inclusion. The owner accepted the alternatives recorded in ADR 0084–0086. Separate implementation PRs for the remaining steps were ruled out by the owner's one-PR instruction.

## Built-in compaction Decider (#108)

Add a system-origin exact-version declaration alongside the existing approval Decider and materialize it in each Tenant's Registry. Use the Jev and Clef Routes and their approved host/state-budget/threshold contracts from ADR 0042 and ADR 0066; do not invent a new provider, change the Route policy or reuse approval thresholds for compaction.

Trusted registration normalization adds the compaction dependency for ordinary Agents, with an explicit removal through the default-removal mechanism and a replacement by an explicitly bound compaction Decider. Record the full graph before hashing; each Run pins it. Generated chains keep their explicit policy-only exception and ancestor budgets. Catalog Approval of the exact version, current credentials, call budgets and live authority remain execution checks, never implications of seeding.

Registration, generation-policy editing, Run details and approval prompts must present the same exact definition. Unapproved, exhausted and unavailable compaction retain their distinct typed reasons. The guided prompt for connecting OpenRouter was explicitly a follow-up in #108 and is not required to invent a new connection flow here.

## Independent Shadow approval observer (#109)

A Tenant setting selects an approved, exact Shadow approval Decider separately from the enforcement choice. The disabled default performs no additional provider calls. Updating the observer changes future observations, never a stored observation's configuration or verdict.

Observe human approval requests across Tool call, Outbound, Web search, Page reading and Generation, including Manual and explicit-ask paths. Keep the existing state-disclosure boundary: authorized human input, Agent instructions, prior calls without results and the authoritative pending action. Generation currently has a separate approval lifecycle and no judge contract; add a typed pending-generation action for observation, not Auto enforcement or a fabricated Tool call. Observation never changes eligibility, Tenant ceilings, approval status or the exact call that approval resumes. Completion Reviews, reconciliation and human questions are not approval kinds and remain outside this observer.

Create a stable observation identity bound to the approval, originating boundary, pending-action digest and exact observer configuration. At approval creation, freeze an observation frame containing the setting revision, effective restrictions, authorized input cutoff, exact Agent/policy/action references and prior-context sequence references. Reconstruct only the permitted state from those references; do not persist a second full classification-state copy. Generation uses its request/policy/Task frame and real originating inputs rather than a fictitious executing Agent. Add a versioned observation state builder for typed actions instead of pretending every kind is a Tool call or changing an existing immutable builder's question meanings.

Use an observer-owned work lease and journal adapter, reusing portable dispatch, attempt identities, budget accounting and replay rules. `RunDecisionJournal` requires the current Run worker lease/revision and therefore cannot be reused unchanged by a background observer after the Run waits. Publish the approval through the existing path independently of observer completion. Human settlement atomically records its explicit label; a verdict and a label can arrive in either order. Provider results arriving after a human response may finish the observation but cannot settle the approval. Missing state, unapproved definitions, unavailable credentials, exhausted calls and provider failures have explicit observational outcomes; none can become a fabricated verdict or label. Never substitute newer human inputs, future tool history or paid retrospective backfill when the original frame cannot be reconstructed.

Only an explicit human allow or deny is a calibration label. Link by the stable approval identity, and treat duplicate delivery/recovery idempotently. Rule, Auto and Bypass outcomes, expiry, cancellation, revocation, pending records and missing observations remain separate counters. A report's bounded time range and denominator include these exclusions visibly. It never compares one Decider's result with that same result as its human target.

Emit typed labels in the explicit capability-decision and generation Approve/Deny control transactions; do not infer them from `decided_by`, actor strings or prose reasons, since automated advice also fills those fields. An explicit allow-run/allow-always labels that one request, while later requests covered by its grant or rule remain automated outcomes. Generation Stop/Delete and expiration are not human denials.

Reports group by exact Decider version/configuration, Route/model/host, approval kind and operation/effect class where applicable. Show the 2-by-2 verdict/human table, agreement, false allows among human denials and among Shadow allows, false blocks among human approvals, and availability/label coverage. Band by the maximum of the five category probabilities, with explicit intervals and counts, and show the empirical human-denial rate in each band. The probabilities answer independent category questions: neither their maximum nor one minus that maximum is a calibrated probability of the person's decision. Binary labels do not validate individual category probabilities. Expose effective thresholds and observation-setting revisions; no automatic threshold tuning or default rollout is part of reporting. A changed threshold needs a new immutable version and Catalog Approval.

Use existing approval visibility for per-call projections. Tenant aggregates require administrative policy permission and tenant-scoped RLS; no Deployment-wide private-content report. API and UI expose bounded non-content projections only. Classification-state bodies, raw arguments and credentials never enter those projections or unrestricted diagnostics. Empty and insufficient samples are explicit states.

## Registry-declared context sources (#110)

Introduce the common source contract and provider port in domain/application, with adapters in the existing owning layers. Source declarations identify provider operations, read/write/detail tools, pinning, volatility, disclosure and versioned query/refresh policy. Read/write/detail operations reference existing exact qualified Tool declarations rather than copying their schema or permissions. Pin source-to-operation-to-provider edges in the Run snapshot and reject inconsistent or unavailable dependencies at admission. Private contents, attachments and credentials remain in their authorized stores. A declaration or selected candidate grants no authority.

Move current conversation memory, semantic memory, workspace retrieval/observation, references and Skills through providers implementing that contract. The Harness iterates resolved declarations and typed contributions in their recorded order; it no longer calls separate documents/skill/semantic assembly methods. Already removed APIs such as `ExecutionStore::memory` are not reintroduced merely because an old Issue mentions them. Retain the single primary memory provider and Home-issued bank binding; additional context sources do not become competing memory-write targets. Memory mutations still use the bound memory provider and #125's revision/idempotency rules. No full-bank replacement API is introduced.

Each bounded candidate identifies its source, revision/digest, provenance, completeness, omitted-detail route and durable read dependencies. Outcomes distinguish empty, excluded, truncated, deferred/no-space, unavailable and delivered content. Deduplication keys identity plus revision/range and preserves all authority dependencies; equal text from different sources is not equal authority.

Always, OnDemand and explicitly selected Scored pin policies share final request budgeting. Always means a bounded automatic contribution at each relevant boundary, not the entire source. OnDemand makes content available through its declared read tools without automatic injection. Declared `Run` versus `Step` volatility controls deterministic placement, with `Step` the conservative default. Contributions remain attributed data envelopes and never become system instructions. Current source policies reproduce default request snapshots; query inputs, accepted corrections and refresh keys are versioned and visible as digests, not unrestricted private query logs. ADR 0020 and ADR 0059 retain their invalidation, incomplete-index, required-Home and narrow degradation rules.

The current query already includes Task title/description and accepted inline inputs; preserve it as the default. As a builder-selected, explicit opt-in policy, add `accepted-inputs-first@1`: authorized accepted inputs in descending sequence followed by the Task seed, bounded at UTF-8 boundaries, with policy identity and exact input/query digests in its retrieval key. This prevents a recent correction disappearing behind an older prefix. Do not add summary material to the first version. Required-Home contributions remain Always because the existing remote receipt contract refuses eliminating every match. Shadow may observe them, but admission rejects a Scored Enforce policy for that mandatory contribution; this preserves the unchanged remote contract.

Scored Shadow and Enforce use bounded batches of candidate-bound Noul keep questions. The state is bounded authorized previews and declared query material, disclosed only under current permission. Code applies version-pinned thresholds, stable order/ties, deduplication and final request fitting. Enforce cannot obtain an answer by weakening authority or falling back to Always; it pauses with a typed reason. Shadow records its proposal/unavailability and preserves the original request. Mandatory source/receipt obligations remain code constraints before selection.

Track retrieval, selection, final fitting and delivery to an accepted inference separately. Only the content actually present in the final accepted request earns a delivery receipt. Dropped, truncated or merely scored content never becomes full-read evidence for replacement. Lossy projection and compaction keep durable source dependencies and references.

## Research-reference integration (#331)

Consume ADR 0083 and PR #481: immutable journal/projection references, exact content-addressed fitted variants in the purgeable Research store, digest verification, tombstones after expiry/revocation and unavailable markers for missing payloads. Journal and summary range commitments remain reference-based and append-only; exports contain references, while the existing evidence viewer resolves retained data through its authorized citation API.

Verify late-worker fences, orphan/shortened variant purging, summaries and every persistence/export path. Existing legacy inline journals require a development reset. The reference contract does not promise to retract already published answers or the model's own text, which remain governed by the existing policy.

## Verification and delivery

- Map every remaining live acceptance item to code and a meaningful verification result. Separate already delivered behavior from new work and pending external acceptance.
- Check normalization, explicit opt-out/override, generated-chain exceptions, Catalog Approval and typed compaction stops.
- Exercise observation across all five kinds, Manual/ask paths, human-response races, worker recovery, quotas, unavailable observations, label exclusion, report boundaries and tenant visibility.
- Compare default assembled requests for the current projection variants; exercise Scored Shadow invariance and Enforce filtering/failure, accepted corrections, revoked/changed/empty sources, bounded detail, deduplication, final fitting and accepted-delivery/full-read distinctions.
- Retain remote memory, semantic provenance, required-Home receipts, revision-bound mutation and research-retention regressions. Verify #331 against the actual integrated parent, not merely an old PR test claim.
- Run proportional pure-crate, native PostgreSQL, migration, HTTP and browser checks plus the documented architecture/format/lint gates. Ordinary CI uses deterministic fixtures without paid calls or private uploads.
- Treat live public fetch, live provider verification and quality/calibration observations as separate evidence. Never call synthetic agreement human calibration or report queued hosted checks as green.
- Commit, normal-push and open one linked PR using live labels. Report SHA parity, local checks, hosted checks and outstanding live acceptance separately. Do not close Issues merely because a draft PR exists.

## Implementation choices left to the builder

Choose module names, bounded page/batch sizes within existing limits, typed DTO names, indexes, route paths, pagination and UI placement consistently with current conventions. Use Reinhardt ORM/migrations and query expressions; any unsupported DDL exception must be documented. Keep changes in their architectural owners and preserve unrelated worktrees. The plan must make the #478/#481 dependency concrete before reporting #331 delivered on the PR's target branch.
