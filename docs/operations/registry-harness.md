# Registry Harness operation

The Registry fixes the declarations and dependencies available to an Agent. Execution policy, current credentials and Catalog Approval determine whether a Run can use them. A Run keeps its admitted exact versions when newer versions are registered.

This completes the design recorded in [Registry Harness completion](../decisions/registry-harness-completion.md). The [capability glossary](registry-capability-glossary.md) describes provider origin; built-in capabilities also belong to the Registry. Related operations remain in [Registry capabilities](registry-capabilities.md).

## Compaction

Ordinary Agent registration includes `aidash.compaction-decider@1.0.0`. To remove pruning, set `remove_default: ["compaction"]`; the Context Policy may still use its Summary Stage. To replace pruning, bind an exact Decider whose hook is `compaction`. Registration checks the actual definition before recording the replacement in the normalized graph. An Approval or context-selection Decider does not replace compaction.

Approve the exact compaction version on the executing Node and connect the Tenant's OpenRouter Provider Credential. Seeding a definition does not approve it. Small requests do not invoke the Decider. When compaction is needed, missing approval, spent call budgets and unavailable credentials/providers retain their distinct `decider_unapproved`, `decider_calls_exhausted` and `decider_unavailable` pause reasons. Resume through the existing Run control after addressing the cause.

Generated chains use the exact Decider allowed by their generation policy and every generated ancestor. They receive no ordinary compaction default. Their existing ancestor charges and Required-Home contracts remain authoritative.

## Independent approval observation

The Tenant administrator can select an approved exact Shadow Decider with the `approval_observation` hook. This setting is independent of Auto Approval and initially disabled. Disabled observation makes no additional provider calls.

The dashboard exposes the setting and calibration report. The corresponding APIs are:

| Method    | Path                                                               | Visibility                                            |
| --------- | ------------------------------------------------------------------ | ----------------------------------------------------- |
| GET / PUT | `/api/tenants/{tenant}/approval-observer?node=…`                   | Tenant administration                                 |
| GET       | `/api/tenants/{tenant}/approval-observations/{kind}/{id}`          | Existing capability or Generation approval visibility |
| GET       | `/api/tenants/{tenant}/approval-observations/report?start=…&end=…` | Tenant administration                                 |

The per-approval path uses `kind=capability` or `kind=generation`. The observation itself distinguishes Tool call, Outbound, Web search, Page reading and Generation. Reports use a positive, end-exclusive range of at most 31 days; choose a shorter range if the 10,000-approval bound is exceeded.

Each approval freezes its observer version, configuration, original input cutoff and action references. Changing the setting affects future frames. A separate worker may finish after the person responds; it cannot settle, relabel or change the resumed action. Missing original state is reported as unavailable rather than replaced with newer input or retrospectively sent to a provider.

Observation attempts consume the selected observer version's call budget per originating Run, or per Generation request when there is no Run. They do not spend a generated Agent's compaction allowance. A lost worker's uncertain paid attempt remains charged; recovery does not repeat the call. Provider-reported cost is retained in Decision Evidence, with unknown values distinct from an explicit zero. These attempts create no Usage Record.

Only an enabled User's explicit allow or deny is a calibration label. Rule, Auto, Bypass, expiry, cancellation, revocation, pending and unavailable outcomes have separate counters, including missing observations for each approval kind. The report shows the 2-by-2 table, denominators, coverage and independent category-risk bands. The maximum category probability is a risk score; neither it nor its complement estimates the person's approval probability. Fixture agreement does not establish human calibration. Reports never change thresholds or enable automation.

## Context sources

Source declarations pin their provider operation and exact read, write, detail and selection dependencies. Private content remains in its owning authorized store. Sources support bounded Always contributions, OnDemand access through declared tools, and explicit Scored Shadow or Enforce selection. Shadow preserves the original request when classification is unavailable. Enforce pauses with a typed reason instead of including unscored content.

`context_source` is optional on native source and memory declarations. Leaving it absent preserves the default query and rendering. The opt-in `accepted-inputs-first@1` query places authorized accepted inputs newest first, followed by the Task seed, within a UTF-8 byte bound. Native semantic retrieval shares one query: all participating declarations, including the primary memory provider and declarations using the implicit `task-inputs@1` default, must agree. Admission rejects a mixture; opting one source in does not silently change another source's policy.

`Run` volatility places attributed data in the stable source prefix; `Step` is the conservative default. Scored selection requires both declared preview/query disclosure and current disclosure authority to the exact selector routes. A declaration alone cannot permit disclosure.

Required-Home retrieval and its receipts remain mandatory. Those contributions allow Always or Scored Shadow; OnDemand and Scored Enforce are rejected even when a declaration claims `required_home: false`. A source selection never adds a second memory writer or widens remote write authority.

Fetched candidates, selection proposals, final fitting and accepted inference delivery are separate stages. Dropped content earns no delivery receipt. Candidate completeness describes that exact revision/range, not permission to replace an underlying memory record. Omitted detail uses its exact declared operation and rechecks the current revision and authority.

## Undeployed schema cutover

Aidash is undeployed. Recreate development data that contains the previous normalized Agent graph, incompatible Run snapshots or legacy inline research journals. Do not rewrite immutable versions or replay them through a compatibility bridge. Follow [ADR 0084](../adr/0084-default-compaction-bindings-cut-over-with-the-undeployed-schema.md) and the existing development setup for the current schema.

Research journal retention uses the reference-based implementation from PR #481, delivered through PR #478. Exact fitted variants live in the purgeable Research store; journal and summary commitments keep references. Missing, expired or revoked payloads resolve to unavailable markers/tombstones. See [ADR 0083](../adr/0083-research-journals-retain-content-addressed-references-and-purges-remove-the-owned-bytes.md).
