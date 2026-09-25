# Harness-managed Web search: design interview

Date: 2026-09-25
Issue: [#44](https://github.com/kent8192/aidash/issues/44)
Status: Q1-Q14 and the consolidated specification were confirmed on 2026-09-25. Implementation is underway.
Inspected development baseline: `e70758ba6a48fc18c07e535f60182a70c8b6f29b`.

Review document: [Harness-managed Web research specification](2026-09-25-web-search-specification.md).

## Established requirements

Issue #44 requires a Harness-managed Web-search capability independent of Registry tool records and `plugin_N` aliases. It requires a defined input/output contract, policy-scoped availability, protected credentials, bounded results, source metadata, explicit failure behavior, tests, and documentation. Provider evaluation and a recorded decision must precede final integration.

[#43](https://github.com/kent8192/aidash/issues/43) covers the other core capabilities. Its [design specification](https://github.com/kent8192/aidash/blob/05d8a62589b5929630aefaca2237c97cdae9154d/docs/superpowers/specs/2026-09-25-harness-capabilities-design.md) provides related policy, credential-isolation, and durable-execution constraints. Its documentation PR, [#45](https://github.com/kent8192/aidash/pull/45), was open when inspected; the specification is not evidence of implemented capabilities.

The [glossary](../../CONTEXT.md) records established terms, aligned with #43's vocabulary. The development baseline has no existing root glossary; #45 separately proposes one. Reconcile overlapping terms when the branches are integrated rather than replacing #45's additional vocabulary.

## Verified code boundaries

- `src/harness.rs` combines built-in tools with Registry tools exposed as `plugin_N` aliases, dispatches invocations through the Run machinery, and applies authorization guards.
- `src/tool.rs` provides the `Tool` interface and a Registry-backed native `http_get` operation restricted to configured hosts. That operation is not a Harness-managed Web-search capability.
- Existing HTTP-fetch handling and durable Run recovery are relevant integration boundaries. Neither proves that general search, page extraction, or a search-provider billing policy already exists.

## Settled interview decisions

The user accepted all four first-round recommendations with "推奨値" on 2026-09-25.

| Decision | Accepted requirement |
| --- | --- |
| Q1 | Include search, reading the source page, and answering with source citations. Page reading is also Harness-managed. |
| Q2 | Support Japanese and English general and technical research, with emphasis on finding primary/official sources and searching for current information when needed. |
| Q3 | An external paid search API is acceptable. The node operator owns the service account, manages its credentials, and pays its costs. Q7 defines initial quotas and attribution. |
| Q4 | Limit search-service submissions to search terms and search conditions. Do not automatically forward conversations or files. Disclosure of nonpublic information is not allowed by default. |

Q8-Q9 refine Q4's disclosure and provider-data requirements. No perfect confidential-information detector is assumed. A paid API being acceptable does not authorize purchasing a subscription or running an unbounded paid benchmark.

The architectural boundary is recorded in [ADR-0001](../adr/0001-harness-managed-web-research.md), disclosure/evidence in [ADR-0002](../adr/0002-web-evidence-and-query-disclosure.md), and the conditional provider selection in [ADR-0003](../adr/0003-brave-search-and-direct-source-reading.md).

## Second-round decisions

The user accepted all five second-round recommendations with "全て推奨" on 2026-09-25.

The [provider comparison](2026-09-25-web-search-providers.md) records current official documentation and unresolved eligibility questions. Published prices and features do not establish measured Japanese/English search quality.

| Decision | Accepted requirement | Decisions it unlocks |
| --- | --- | --- |
| Q5: Page-reading scope | Read public HTML, plain text and text-based PDFs, including user-supplied public URLs without first searching. Authenticated browsing, JavaScript execution and OCR are outside the initial scope. | Retrieval adapter, supported media, URL admission, extraction bounds and failure cases. |
| Q6: Evidence and citation history | Retain source URL, title, retrieval time and bounded passages actually used as evidence under the Run's access and retention policy. Cite the relevant statement; distinguish unread results from examined content. Do not permanently archive entire sites or silently replace prior evidence with a later fetch. | Required provider storage rights, stable source references, citation rendering, replay and deletion. |
| Q7: Initial usage budget | Initial limits are 10 external search attempts and 20 page-retrieval attempts per Run, including retries, and a configurable $20/month node-wide search/content-service budget. Attribute usage to Workspace and Agent; operators can change limits. Reaching a limit prevents new paid requests and preserves completed work. Model, compute and network charges are separate. | Reservations, billing units, concurrency, cancellation accounting, user-visible limit behavior. |
| Q8: Mixed public/nonpublic input | Automatic search is limited to public or explicitly disclosure-authorized context. If the Run contains nonpublic or unclassified material, show the exact outbound query to an eligible approver before sending. Approval covers that query, not arbitrary future queries; credentials are never eligible query material. Do not rely on the model perfectly detecting or removing secrets. | Trusted classification/provenance, approval routing, external-URL disclosure and authorization tests. |
| Q9: Provider-side data conditions | Permit bounded retention of public queries for billing and abuse prevention, but require an applicable commitment against using submitted queries for model training. Do not require Japan-only processing or zero retention for the initial public-search use case. Ordinary operation continues to exclude unauthorized nonpublic disclosure. | Provider/plan eligibility, contract verification, deployment configuration and fallback eligibility. |

Q5-Q9 are accepted design requirements, not permission to buy a service. Actual provider retention durations must be recorded before final selection; "bounded retention" does not authorize indefinite storage. The evidence/disclosure boundary is recorded in [ADR-0002](../adr/0002-web-evidence-and-query-disclosure.md).

## Third-round decisions

The user accepted Q10-Q14 with "全て推奨" on 2026-09-25.

| Decision | Accepted requirement | Decisions it settles |
| --- | --- | --- |
| Q10: Provider and retrieval architecture | Select Brave Web Search as the first integration, with Aidash-controlled page retrieval/extraction. Enable it only after the actual plan satisfies durable storage rights, query no-training, bounded retention and the configured budget; unresolved entitlement is an unavailable capability, not a reason to weaken requirements. | Provider rationale, account eligibility, direct retrieval and no implicit provider/model substitution. |
| Q11: Agent operations and result bounds | Expose separate `web_search`, `web_open` and `web_find` operations. Search returns 5 results by default, at most 10, with language/country/date/domain conditions. Open reads a bounded window of an identified page; find searches retained page text without another external request. Cap each model-visible result at 32 KiB and explicitly report truncation/continuation. | Tool boundaries, output size, source references and local follow-up operations. |
| Q12: Failure, retries and recovery | Return explicit tool outcomes for empty results, inaccessible pages, rate limits, provider errors and exhausted quotas so the Agent can explain or use another permitted source. Retry transient HTTP failures at most once within the same 15-second search or 20-second page-operation deadline and existing budgets. Do not automatically switch providers, reissue completed calls, or blindly replay a call whose completion/billing outcome is unknown. | Failure contract, bounded retry, interruption/restart semantics and fallback policy. |
| Q13: Exact outbound approval | Apply Q8 to outgoing page URLs as well as search queries. Approval binds the final outbound request and destination, within current policy; edits require new approval. An eligible user's explicit request to open an exact public URL already authorizes that URL within the request's scope, rather than requiring duplicate confirmation. | URL disclosure, approval scope, edits, user intent and revocation boundaries. |
| Q14: Acceptance before rollout | Require deterministic authorization, credential, network-boundary, quota, failure and crash-recovery tests plus a recorded live pilot with 10 Japanese and 10 English public queries and supported source formats. Record actual relevance/freshness, source fidelity, latency and cost; unavailable credentials or unverified contract conditions leave live evidence pending, never passed. | Delivery evidence and honest distinction between implemented, configured, verified and enabled. |

Q10 is an accepted conditional provider decision, not evidence that an eligible subscription exists or can be purchased within the initial budget. Q14 describes rollout evidence, not authorization to purchase a subscription in this interview.

The consolidated specification records engineering defaults for typed schemas, source/document identities, timestamps, bounded processing, authorization, disclosure approvals, credentials, budget reservations, uncertain charges and citations. They are implementation proposals for final document review, not additional answers attributed to the user.

## Decision-tree closeout

All product branches raised in the interview are covered by Q1-Q14. The user confirmed the consolidated specification before requesting implementation. Operational evidence remains separate: the actual provider agreement/price and credentials, integrated implementation tests, live query evaluation, and deployment verification have not been supplied by this interview.

If those operational checks cannot satisfy an accepted requirement, reopen the affected decision rather than silently relaxing data conditions, evidence retention, quality or budget.

## Session boundary

The design interview is complete. Implementation work and the separately requested pull request proceed against the confirmed specification; operational acceptance remains unverified.
