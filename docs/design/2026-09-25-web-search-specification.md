# Harness-managed Web research

Date: 2026-09-25
Issue: [#44](https://github.com/kent8192/aidash/issues/44)
Status: Confirmed specification. Q1-Q14 and the engineering defaults were accepted for implementation review on 2026-09-25. The implementation and operational evidence status below must be checked separately.
Inspected baseline: `e70758ba6a48fc18c07e535f60182a70c8b6f29b` on `develop/0.1.0`.

## 1. Outcome and decisions

An authorized Agent can discover public Web sources, read their contents, find passages in a retrieved document, and answer with citations. All three operations are supplied by the Harness independently of Registry tool records and `plugin_N` aliases. Aidash's existing model performs the research reasoning and writes the answer.

The [interview record](2026-09-25-web-search.md) preserves the user's answers. The [glossary](../../CONTEXT.md), [architecture ADR](../adr/0001-harness-managed-web-research.md), [evidence/disclosure ADR](../adr/0002-web-evidence-and-query-disclosure.md), [provider ADR](../adr/0003-brave-search-and-direct-source-reading.md), and [provider comparison](2026-09-25-web-search-providers.md) provide the vocabulary and rationale.

| Decision | Settled requirement | Acceptance |
| --- | --- | --- |
| Q1 | Search, source reading and cited answers are Harness-managed. | AT01, AT14 |
| Q2 | Support Japanese/English general and technical research, primary sources and current information. | AT02, AT15, AT20 |
| Q3 | The node operator owns the paid search account, credentials and costs. | AT07, AT09, AT17 |
| Q4 | Send search terms/conditions rather than entire conversations or files; nonpublic disclosure is denied by default. | AT04, AT05, AT07 |
| Q5 | Read public HTML, plain text and text-based PDFs, including user-supplied URLs. No authenticated browser, JavaScript execution or OCR in this scope. | AT06, AT08 |
| Q6 | Retain bounded evidence with URL, title and retrieval time under Run access/retention rules; cite the relevant statement. | AT13, AT14, AT16 |
| Q7 | Initial limits: 10 search attempts and 20 page-retrieval attempts per Run, including retries; $20/month of node-wide search/content-service spending. Operators may configure limits; attribute usage to Workspace and Agent. | AT09, AT11, AT12 |
| Q8 | Nonpublic or unclassified Run context requires approval of the exact outgoing query. Public-only research can run automatically. | AT04, AT05 |
| Q9 | Bounded provider operational retention is acceptable; model-training reuse of submitted queries is not. No initial Japan-only or zero-retention mandate. | AT17 |
| Q10 | Select Brave Web Search conditionally, paired with Aidash-controlled source reading. Verify actual account eligibility and price before enabling search. | AT01, AT17 |
| Q11 | Provide `web_search`, `web_open` and `web_find`; search defaults to 5 results, at most 10; every model-visible response is at most 32 KiB with explicit continuation/truncation. | AT02, AT08, AT13 |
| Q12 | Explicit failures permit recovery; search/page deadlines are 15/20 seconds, with at most one eligible transient retry. No automatic alternate provider or replay of completed/uncertain external calls. | AT10, AT11, AT12 |
| Q13 | Exact-request disclosure approval also covers page URLs. An eligible user's explicit request to open a URL supplies that scoped intent without duplicate confirmation. | AT05, AT06 |
| Q14 | Require deterministic behavioral tests and a recorded live pilot of 10 Japanese and 10 English queries before rollout. Missing evidence remains pending. | AT01-AT21 |

The rest of this document makes those decisions reviewable as one design. Numeric limits not established by Q7/Q11/Q12 are engineering defaults, configurable within tested policy ceilings. They are not claims about already implemented behavior.

## 2. Selected provider and operating prerequisites

Use Brave's Web Search endpoint for discovery, with a fixed operator-configured HTTPS endpoint and server-side token header. The adapter normalizes candidate sources; it does not request a provider-generated answer. An independent reader obtains source bytes directly and extracts text. No automatic Tavily, Exa, Perplexity, browser, or model-provider substitution is allowed.

Before search becomes available, the operator must record the applicable account/plan and contract reference, verified storage rights for the required history/evaluation, a no-training commitment covering submitted queries, a bounded retention period, permitted integration evaluation, applicable termination/deletion terms, and the current price schedule. Record verifier, verification date and expiry/review date; credentials remain a separate secret reference. These records are evidence-backed operator assertions, not model-set booleans or conclusions the application can infer from possessing a key.

Brave's public FAQ requires an explicit storage-rights plan for retained API results. Its current privacy notice describes query-log retention up to 90 days for operational purposes, subject to its stated exceptions. Neither establishes that a particular deployment already has an eligible agreement or that the ordinary public Search price is the price of such an agreement. Sources: [Brave API FAQ](https://brave.com/search/api/), [API privacy notice](https://api-dashboard.search.brave.com/privacy-policy), [API terms](https://api-dashboard.search.brave.com/terms-of-service).

Unknown, expired, incompatible or unaffordable account conditions keep `web_search` unavailable. They do not waive Q6/Q9 or automatically increase the budget. If eligibility cannot be met, reopen provider selection. A provider agreement does not grant rights to all publisher content.

Independently authorized `web_open` and `web_find` can work on directly supplied public sources without a functioning search account. Disabling search does not discard existing evidence; subsequent access must still satisfy current policy and applicable retention rights.

## 3. Existing code and integration shape

| Existing boundary | Required extension |
| --- | --- |
| `src/registry.rs::AgentConfig` | Add explicit core-capability declarations using the shared #43 shape, with absent fields granting no new capability. Preserve immutable Agent versions. |
| `src/harness.rs`, `src/tool.rs` | Resolve permitted Web tools alongside built-ins; validate typed inputs, dispatch through durable invocations, and fit complete serialized results within context headroom. |
| `src/authorization/execution.rs::Guard::tool` | Add explicit Web-tool cases and resource checks. The inspected dispatcher denies unknown built-ins, so schema exposure alone cannot enable them. |
| `src/store.rs` | Extend existing invocation keys, revision/lease fencing and journals with attempts, approvals, source dependencies and atomic budget reservations. Use SeaORM/SeaQuery. |
| `src/config.rs` | Reuse `AIDASH_SECRET_*` references; resolve values only inside the provider client. |
| `src/provider.rs`, `src/context.rs` | Budget schemas, observations and citation references in complete model requests; preserve evidence identity across compaction. |
| `src/knowledge.rs` | Its inspected PDF path accepts browser-extracted reference text. It is not an existing server-side PDF reader; implement and validate bounded Web PDF extraction separately. |
| Existing authenticated API, Run controls and dashboard | Add capability states, exact disclosure requests, evidence reads and citations without a second conversation protocol. |

Pipeline: authenticated Run authority -> capability/input checks -> disclosure approval where needed -> budget admission -> durable attempt -> provider or isolated reader -> bounded durable result/evidence -> existing Agent context and answer rendering.

Reuse #43's core-capability and approval abstractions where available. #44 does not depend on all Shell/Python/file-transfer slices being complete and does not import their broad network permissions. If a required common authorization seam is absent, implement the necessary narrow seam or record that dependency; never route around it through a Registry alias or legacy unscoped execution.

## 4. Agent-facing contract, version 1

All argument objects and nested objects reject unknown properties. All integers are nonnegative unless their specified range is narrower. Validate the whole request before any external effect. Input validation never silently clips queries, URLs, dates or domain restrictions. Strings must be valid UTF-8 without NUL/control characters where those are not meaningful. Limits are checked again after provider mapping.

### `web_search`

| Argument | Contract |
| --- | --- |
| `query` | Required nonblank string; at most 600 Unicode characters and 75 whitespace-separated words, and at most 4 KiB for the serialized input object. The final provider query, including generated domain operators, must also fit. |
| `language` | Optional `ja` or `en`; otherwise use the Agent's supported preferred language, falling back to `en`. Translate to the provider's documented vocabulary; do not assume every provider uses identical codes. |
| `country` | Optional supported two-letter country code. Omission uses the configured provider default, currently `US`, and returns the effective setting. Do not infer or send a person's physical location. |
| `count` | Integer 1-10, default 5. |
| `page` | Integer 0-9, default 0. A later page is a new external search attempt, subject to all budgets and exact-request approval. |
| `freshness` | Optional one of `day`, `week`, `month`, `year`, or an object with inclusive ISO dates `from` and `to`; reject reversed ranges. Omission means no date filter. |
| `include_domains`, `exclude_domains` | Arrays of normalized DNS hostnames, at most 5 in each; reject URLs, ports, wildcards and overlaps. Matching includes the named domain and its subdomains, with DNS-label boundaries. |

The adapter constructs the final query deterministically using documented domain operators, disables spelling rewrites, requests only Web results, and disables generated answers/rich callbacks and unnecessary decoration. It maps freshness to the provider contract, validates returned URLs, and filters results against requested domains again. Operator-defined domain restrictions also apply independently. Fewer results are acceptable; do not fan out or relax filters to fill the requested count. A timeout, page change, secondary-language query or extra result page cannot become a hidden additional search.

Return a bounded array of source records: `source_id`, original `url`, `title`, plain-text `snippet`, rank and `evidence_state: unread`, plus provider name, search time, effective query/filters, returned/requested counts, filtering/truncation notices and whether more results may be available. Provider dates are optional, carry their supplied meaning, and must not be relabeled as verified publication dates. Source IDs are generated by the Harness, immutable and scoped to the Run; a later discovery does not silently overwrite earlier metadata.

Relevant provider mapping references: [Web Search API](https://api-dashboard.search.brave.com/api-reference/web/search/post), [search operators](https://api-dashboard.search.brave.com/documentation/resources/search-operators). Pin adapter fixtures to a reviewed contract; send a version header only for a verified supported version. The live pilot must verify Japanese/English mappings, domain matching and date behavior.

### `web_open`

Exactly one mode is accepted:

- `url`: an absolute public HTTP(S) URL, at most 4 KiB. Create a source record and make a new policy-authorized retrieval.
- `source_id`: a visible source from this Run. Resolve its immutable URL, then make a new retrieval.
- `document_id` with optional `cursor`: read another window of an existing document snapshot. This mode makes no external request.

All modes accept `max_bytes`, 1-24,576, default 16,384, for the text portion. The complete JSON response remains at most 32 KiB; metadata and JSON escaping count toward that cap. A cursor is opaque, bound to the document/scope and validated before reading. It cannot select an arbitrary byte range, path or foreign document. At least one UTF-8 character and a valid metadata envelope must fit or the request fails clearly.

A successful fetch returns `document_id`, `source_id`, requested and final URL, bounded title, media type, actual `fetched_at`, immutable content/extraction digests, extraction-version identity, optional publisher metadata with provenance, and a line-numbered window. PDF lines also identify their source page. The window supplies an `evidence_ref`, covered line range, `next_cursor`, and separate completeness/truncation flags. A new URL/source invocation means a fresh retrieval; continuations use the fixed snapshot. Completed invocation replay returns its stored observation.

### `web_find`

Arguments: required `document_id`, nonblank literal `text` of at most 256 characters/1 KiB, optional `case_sensitive` (default false), `max_matches` (1-20, default 10), and an opaque continuation cursor. No regular expressions or remote search are supported. Match against the fixed normalized extracted text; case-insensitive matching uses a pinned Unicode case-folding behavior.

Return bounded matching passages with document/page/line positions and evidence references, plus continuation and completeness indicators. The operation requires current document-read authority, even when the document is cached. It consumes local processing/context quota but no search/page attempt or provider charge. A missing/expired document reports that condition without fetching its URL automatically.

### Result envelope

Every operation returns `version: 1`, its `operation`, `status`, `data` or `error`, and `limits` describing truncation/continuation. Status is one of `ok`, `empty`, `partial`, `error`, `approval_required`, `cancelled`, or `uncertain`. An error contains a stable `code`, a sanitized explanation and `retryable`; `retry_after_seconds` is optional. It never contains raw provider error bodies, headers, credentials or debug dumps. Result/state identifiers must remain opaque to callers outside their authorized scope.

`empty` means a valid search or local find returned no matches. It is not interchangeable with denial, a failed provider or a page containing no extractable text. `partial` always identifies what is incomplete. Never cut JSON or UTF-8 bytes to enforce limits: build a valid bounded result and account for the surrounding model-message encoding as well.

## 5. Source fetching, bounds and freshness

The direct reader uses an isolated extraction process with no credentials, host mounts, process-spawn facility, or network access. A separate restricted fetch component performs permitted network requests; the parser receives only bounded bytes. JavaScript, embedded PDF actions, external entities and subresource loads are not executed. Strip executable HTML and render excerpts as data; content instructions cannot change authority or operate tools.

| Engineering default | Limit/behavior |
| --- | --- |
| Search response download | 1 MiB after decompression; excess returns `response_too_large`. |
| Result fields | Title 512 UTF-8 bytes; snippet 1 KiB; URL 4 KiB. Mark text truncation; reject overlong/unsafe URLs rather than modifying their destination. |
| Page download | 10 MiB compressed and 10 MiB decompressed, streamed with enforced caps. An incomplete download is a failed read, not a complete PDF/HTML document. |
| Extracted text | 1 MiB/document; maximum 200 PDF pages; explicit extraction-limit metadata. No OCR; encrypted, scanned-only and unsupported documents have distinct outcomes. |
| Extraction resources | 1 CPU, 256 MiB RAM, 5 seconds, with a process-tree kill on limit/cancellation. These fit within the total page deadline. |
| Document working cache | 20 MiB/Run, durable across Worker restarts, idle expiry after 24 hours and removal within 1 hour after Run termination; explicit expired state. Quota exhaustion does not silently evict active data. |
| Retained observations | At most 200 distinct Web observations and 8 MiB serialized Web observations/Run initially, in addition to the per-response cap; repeated paging cannot accumulate an unlimited archive. |
| Local open/find work | 2 seconds per operation; local reads remain subject to normal Run step/context limits. |
| Redirects | At most 3. Each request counts toward page-attempt quota and undergoes network/disclosure checks. |

Readers accept HTML, plain text and text-based PDF according to validated content/type checks. Treat an HTML access-denied page as HTML, not a PDF because its URL ends in `.pdf`. Titles and dates are untrusted metadata; absence stays unknown. Preserve PDF page identities and clearly describe normalized/wrapped line numbering as positions in this snapshot, not the publisher's visual line layout.

No shared cross-Run query or page-content cache is introduced. Another Run cannot use a predictable URL/digest to read an earlier Run's evidence. A new explicit retrieval gets a new document identity; old evidence remains unchanged. Provider index freshness, a provider-reported page date, the actual fetch time, and a publisher's date are separate fields. Date filtering and a no-cache request are not proof that the underlying information is current. An Agent must disclose stale/incomplete evidence when it cannot establish a current answer.

## 6. Authorization and disclosure

Expose each tool only when node capability configuration, exact Agent version, applicable Run policy and current authorization allow it. Search additionally requires a valid provider configuration/account gate. Dispatch, replay and evidence APIs independently enforce the same relevant authority; knowing a tool name, source ID, cursor or old successful result grants nothing. A legacy unscoped Run without a trusted execution authority cannot use these new capabilities.

Extend the existing `tool.invoke` resources `builtin:web_search`, `builtin:web_open` and `builtin:web_find` with trusted resource attributes and source-read checks. Preserve the originating requester and full Agent authorization chain. An approval never overrides an upper policy denial, expired credential, domain restriction, service-spend limit or revoked source access.

### Trusted context classification

Every input capable of influencing a query has trusted provenance: user/steer messages, task/workspace data, Agent instructions, Skills, reference files, tool output, memory and inherited/delegated context. Classifications are `public`, `nonpublic` and `unclassified`; absent or unverifiable labels mean `unclassified`. Classification changes require an authorized owner/operator action with an audit record, never a model-supplied claim or a sentence in a document.

A Run's disclosure state conservatively joins all information it has observed, including material removed by compaction and summaries/derived outputs. Reading restricted material makes subsequent outgoing requests approval-required. Source-specific disclosure approval does not relabel the entire Run as public. Source revocation still fences retained context under existing source-dependency rules. The baseline's authorization-read ledger is a useful dependency record, not an existing semantic information-flow classifier; add and test the trusted classification layer required here.

Ordinary chat text is unclassified unless the authenticated user explicitly designates the relevant content for public search. A source's being hosted at a public URL does not make a restricted query or its association with the requester public. Public-only automated research requires an explicit trusted public context, not a heuristic that guesses whether a string looks confidential.

### Exact disclosure requests

For a restricted Run, first validate and normalize the request, then present its final nonsecret external destination, method and query/URL/conditions to an eligible approver. Bind the approval to the Run, exact Agent version, invocation, request digest and provider account identity. A proposed 15-minute expiry and Run termination bound its lifetime. One approval covers that invocation's permitted bounded retry of the same request; it does not cover another invocation, changed pagination/filters, a second provider, a child Agent or a subsequent Run.

Route to the original requester when they have both current context-read and disclosure-approval authority; otherwise use the configured eligible approver. Neither a notification nor expiry constitutes approval. No eligible approver produces `approval_unavailable`. Denial/expiry returns a clear tool outcome and permits the Agent to continue without that external effect. Approval storage and waiting use existing durable Run/human-request control patterns, with a typed request digest and dedicated authorization check; natural-language model statements are not grants.

An authenticated eligible user's explicit instruction to open an exact URL supplies an intent grant for that normalized URL in that Run. It still undergoes policy/network checks. Recognize it through a trusted request/URL action bound to the user's message, not by accepting an Agent's claim that the user approved a URL. Reusing a searched URL from a restricted Run is not automatically approved merely because its host is public.

Newly received context, revocation and edits are checked again at dispatch. Do not leave a stale approved request running after replacement/cancellation. A newly encountered redirect that changes the outgoing URL requires matching disclosure authority; a restricted Run may need a new exact-target approval. Never hold a worker/network connection while waiting for human approval; continuation is durable and rechecks source freshness/authority before following the saved target.

### Network and secret boundaries

Permit only public HTTP(S) on approved ports (80/443 initially), without URL credentials or authenticated browser state. Reject known credential/signed-access URLs. Validate the normalized hostname and every resolved address and pin the approved resolution to the connection; repeat this for redirects and reconnects. Deny loopback, private, link-local, metadata, multicast, reserved and other nonpublic destinations, including alternative encodings and IPv4-mapped IPv6. Mixed safe/unsafe DNS answers fail closed. Enforce the same restrictions at the actual network boundary, not only URL parsing.

Disable automatic redirects, environment-inherited proxies, cookies, authorization forwarding, client certificates, Referer propagation and page subresource requests. An HTTPS-to-HTTP redirect is rejected. A redirect never forwards a search-service token. Publisher denial, paywalls and CAPTCHA remain access failures; there is no alternate identity/browser bypass. Operator domain restrictions can further narrow access.

The Brave token resolves from a reference such as `AIDASH_SECRET_BRAVE_SEARCH` only in the fixed provider client. It never enters tool arguments, approval cards, prompts, page requests, extractors, logs, traces or results. Strip raw errors before observability; audit identifiers, decisions, counts, durations and digests. Exact queries and evidence are confined to protected Run/approval records. Reject known secrets even if a caller asks to approve them; this is not a promise to recognize every unknown secret in arbitrary prose.

## 7. Budgets, failure and recovery

### Admission and accounting

Use one durable ledger across all Workers on the node. Before sending, atomically reserve the attempt and its conservative maximum charge against the Run limits and the node's provider-account/month budget. Identify the owning node/account, Workspace, Run, Agent version, invocation, attempt number and pricing revision. Represent money as integer micro-USD with rounding upward. Requests without a current bounded price cannot be admitted.

The $20 initial budget covers attributable provider base fees plus API usage, exclusive of tax; model, compute and network charges are separate. Reserve known recurring fees at the start of the UTC calendar-month bucket. An eligible agreement with a base fee above the limit is not usable under that configuration. Free credits or an unverified discount do not silently expand the allowed spending. Usage outside Aidash through the same account is not governed by this ledger; use a dedicated key/account and provider-side limits where available. The local estimate is not a reconciled provider invoice.

Count every outgoing search attempt, including retries and page requests for later search-result pages, against the limit of 10. Count every direct page HTTP request, including redirect hops and retries, against the limit of 20. Local `web_open(document_id)` and `web_find` do not count as network attempts. A single invocation can therefore consume multiple attempts; report that usage accurately.

Reservations are durable before dispatch. Only proof that a request was not dispatched permits release without an attempt charge. After possible dispatch, preserve the conservative debit until reliable reconciliation; timeout, Worker death, cancellation, and receipt of a nonbillable-looking HTTP status are not proof of zero provider cost. Store actual metering when supplied, otherwise label amounts as estimates. Month rollover uses the dispatch bucket and cannot erase prior uncertain charges. Concurrent requests cannot each spend the same remaining balance.

Initial additional load controls are one search request/second across the account's node Workers, at most two in-flight searches, and four in-flight page requests/node with one per host. The actual provider entitlement can impose lower limits. Waiting for a rate slot is included in the operation deadline; waiting for human approval is not, and does not hold an API charge reservation.

### Failures and retry policy

| Condition | Observable result and handling |
| --- | --- |
| Valid search/find with no matches | `empty`, with effective filters and a zero count. The Agent may formulate a new permitted request. |
| Malformed input, unsupported filters or conflicting selector | `invalid_input`, no network call or charge. |
| Disabled/missing capability or provider configuration | `capability_unavailable`, no network call; show setup detail only to eligible operators. |
| Policy/source denial or revocation | Deny before the effect/read. Preserve existing Run pause behavior where continuing inference would reuse unauthorized context; never downgrade denial to successful empty results. |
| Disclosure decision absent/denied/expired | Durable `approval_required`, then `approval_denied`, `approval_expired` or `approval_unavailable` as appropriate. No outbound request occurs. |
| Unsafe destination or forbidden redirect | `unsafe_url`; no connection to that destination, no internal address in an unprivileged error. |
| Publisher 403/404/410, auth wall or CAPTCHA | `source_unavailable`, with a bounded status reason; do not retry the same denied page automatically. |
| Provider 401/403 | `provider_auth`; no retry or alternate credential/provider. Notify eligible operators through existing application state, not a new external messaging channel. |
| Quota/monetary/observation limit | `budget_exceeded` or `limit_exceeded`, preserving completed work and giving a scoped remaining/reset indication. |
| HTTP 408/429/500/502/503/504 | At most one retry of the identical authorized request when the deadline, rate limit and budget permit. Otherwise return `rate_limited` or `provider_unavailable`/`source_unavailable`. |
| No valid response before deadline | `timeout`; dispatch/billing may be uncertain. Do not automatically resend a transport-ambiguous request. |
| Oversized/malformed upstream response | `response_too_large` or `provider_response_invalid`; no raw body is exposed and no automatic retry. |
| Unsupported/encrypted/scanned-only document or parser failure | `unsupported_content`, `no_extractable_text` or `parse_failed`; never manufacture text or claim full-page coverage. |
| Snapshot/cursor unavailable or mismatched | `document_expired`, `invalid_cursor` or a nondisclosing authorization error; no automatic network fetch. |
| Cancellation | `cancelled`; abort network/parsing and fence late results. Do not claim cancellation reverses an already sent request. |
| Recovery cannot establish an external outcome | `uncertain` with `outcome_unknown`; retain the attempt debit and permit a separately admitted future operation. |

`retryable` means a future properly authorized operation may work; it does not promise another automatic attempt. Respect a valid `Retry-After`, including HTTP dates, within the remaining deadline; a longer delay yields a clear rate-limit result. Without it, use a short bounded backoff. Disable hidden client-library retries. The 15-second search and 20-second page deadlines include admission/rate waiting, DNS/connect/TLS, response streaming, redirects and extraction, after disclosure approval. A redirected request is separately approved when necessary; while waiting, terminate the active external operation and resume through a durable bounded continuation rather than holding a connection. Persist the remaining active-work deadline: approval pauses do not consume it, but approval, redirect and restart cannot reset it or the retry/attempt counters.

Search provider errors are normalized into tool results, avoiding the inspected generic Worker retry path that would otherwise issue additional calls. Database/lease failures still follow durable Run recovery and cannot be misrepresented as successful searches. There is no automatic fallback to another search provider or to a logged-in browser. Existing knowledge may be used with an explicit statement that fresh verification failed.

### Durable execution

Persist a canonical request digest, account/config/pricing identity and attempt sequence under the existing invocation key. Required stages are `prepared`, `dispatched`, `result_recorded`, and `completed`; terminal non-success results remain durable. Mark dispatch before the actual send, so a crash in the gap is conservatively uncertain.

1. A fenced `prepared` attempt proved never dispatched can be released or resumed under fresh authority.
2. A `dispatched` attempt without a stored outcome becomes `outcome_unknown`; it is not blindly replayed.
3. A stored normalized response/document result can finish journal/accounting publication without another request.
4. A completed invocation returns exactly its stored observation, subject to current read/retention checks; a reused key with different arguments conflicts.
5. Cancellation, stale Worker leases, policy revocation and a superseding Run input fence later sends and writes. Pending operations reload approval, authority and budget state after restart.

External search and initial page reads cannot inherit the unconditional `replay_safe = true` behavior of current built-ins merely because HTTP is read-only: billing and disclosure are external effects. Local snapshot reads/finds can be reconstructed without network effects. Add a typed Web-operation recovery path; the existing generic uncertain-tool request for a person to supply arbitrary JSON must not create fabricated source evidence. Record uncertainty as a tool outcome instead. This does not promise exactly-once billing from a provider without compatible idempotency support.

## 8. Evidence, citations and data lifetime

Distinguish three resources: immutable source records, fixed document snapshots, and evidence fragments actually returned to the Agent. Each fragment binds the document digest/extraction version to its page/line range and retained text. Search snippets remain discovery hints and cannot obtain a page-read evidence reference.

An answer cites an `evidence_ref` from a successfully returned open/find result using a Harness-defined marker, for example `[[web:ev_example]]`. The renderer resolves markers against authorized fragments and shows a numbered link, source title/host and retrieval time; expanding it shows the retained excerpt and PDF page where applicable. Escape titles/text and validate link schemes. Do not let model-provided HTML, URLs or fake reference IDs manufacture a valid citation. Plain links can still be displayed as links, clearly distinct from validated evidence references.

Validate reference existence, scope, covered range and current readability before publishing the final answer. Invalid/unread/foreign references trigger a bounded correction opportunity through normal Run step limits; if unresolved, expose the citation problem and never label it verified. Structural validation proves that a passage was available, not that the passage semantically proves every associated claim. The Agent must assess support and qualify weak or contradictory evidence; the live evaluation examines this behavior.

Retain bounded delivered observations/fragments in protected Run history for replay and audit. Keep the full extracted-text working cache separate and temporary under Section 5; expiring that cache preserves the snapshot identity/metadata and retained fragments, so existing citations remain usable. Context compaction does not invalidate citation identities or remove durable evidence; it also does not declassify observed inputs. Restart and cache expiry must not silently refetch a source to substitute different content for a citation.

The inspected baseline retains complete invocation results in PostgreSQL and does not establish a separate time-based Run-expiry policy. Evidence follows whatever current Run retention/deletion policy applies; this specification does not invent an existing general retention subsystem. Add Web-data purge hooks for Run deletion and contract/policy-required expiry, covering protected events, cached snapshots, fragments and copied observations. At expiry/deletion, retained links resolve to an unavailable/tombstoned state; do not regenerate deleted evidence from the live Web. Block or redact future reads of dependent copies through the source-dependency mechanism. Previously read/exported material is not claimed retractable.

Recheck source authority for API reads, cached tool replay, events/SSE, citations and derived outputs, preserving the baseline's journal-source rules. Public webpage contents can still be associated with a private query, Run or research purpose, so never publish Web evidence into a global cache/catalog by default. A provider token becoming temporarily unavailable does not alone delete evidence; loss of storage entitlement follows the verified agreement's retention obligations.

## 9. Configuration, APIs and user experience

Use additive typed configuration, with new Web capabilities disabled by default. Expose separate capability declarations for the three tools within the shared `core_capabilities` model; published Agent versions remain immutable. Record configuration revisions on operations. Old versions that cannot safely understand new configuration must reject it rather than silently dropping permission fields. Existing Registry integrations and native `http_get` retain their behavior and identities.

| Operator-managed configuration | Required behavior |
| --- | --- |
| Provider/account and fixed endpoint | First adapter is Brave Web Search. Model arguments cannot select endpoints, accounts or providers. |
| Credential reference | `AIDASH_SECRET_*` value supplied through deployment secrets, independently rotatable; no plaintext stored in Agent records. |
| Contract eligibility record | Account/plan, evidence reference, storage/evaluation rights, no-training scope, retention/termination rules, verification and review dates. Unverified/expired conditions disable new search admission. |
| Price and budgets | Versioned conservative request rates, recurring charges and effective dates; monthly node limit, Run attempt limits and optional lower scoped caps. |
| Execution/network profile | Deadlines, response/download/parser/cache/observation caps, account rate limits, publisher restrictions and permitted ports. |
| Disclosure authority | Trusted classification actions, eligible requester/approver policy and expiring exact-request grants. No model-set classification or approval fields. |

Extend existing authenticated Run controls with typed disclosure decisions and authorized evidence/usage reads. Proposed logical endpoints are `GET /api/runs/{run_id}/web/evidence/{evidence_ref}`, `GET /api/runs/{run_id}/web/usage`, and `POST /api/runs/{run_id}/web/disclosures/{request_id}/decision`. Decisions contain the expected revision, request digest and allow/deny outcome; actor/authority comes from authentication. Use the shared #43 approval endpoint instead if it already provides these exact guarantees. Publish actual routes through typed OpenAPI and test them; these routes are proposals, not existing APIs.

Agent configuration shows Web search, page reading and page-text search with policy-constrained switches. Users see actionable states such as unavailable setup, awaiting disclosure approval, searching, partial source, no results or limit reached. Approval cards show the exact outgoing information and destination, with allow/deny controls and expiry; API headers and tokens never appear. A queued approval cannot block stop/steer/cancellation controls. Citation expansion and usage displays follow the same authorization as their Run.

Usage displays identify attempts, retries, local reads, estimated spend, uncertain charges and remaining limits; do not label estimates as invoiced totals. Setup/account detail is restricted to eligible operators. No separate research dashboard or new planner is required.

Delegation preserves context classifications and originating authority but transfers no provider credential or exact-request grant. External effects use the executing node's own configured account and budget. Remote capability use requires compatible scoped authority/evidence handling on both nodes; missing support is an explicit unavailable/denied outcome. No fallback to legacy peer authority, and no completion dependency on #43's unrelated file-transfer features is introduced.

## 10. Acceptance and verification

Use the authenticated API -> Harness -> durable store/evidence boundary with scripted model/provider responses and real PostgreSQL fixtures. Test actual external-effect counts rather than helper-call ordering. For destination restrictions, parsing quotas, cancellation and secret isolation, also exercise the real fetch/extraction boundary in a controlled isolated environment; mocked responses alone do not prove isolation. A test-only private-network fixture must not become a production allow-private-address switch.

| Case | Required observable evidence |
| --- | --- |
| AT01 | An enabled Agent with no Registry tools invokes search, opens a result/direct URL, finds a passage and produces a working citation; names are never `plugin_N`. |
| AT02 | Typed contract rejects malformed/oversized/unknown inputs, invalid dates/domains/selectors and excessive counts before network calls. Japanese multibyte text and JSON escaping stay within limits; mapped filters and returned-source restrictions are preserved. |
| AT03 | Disabled/unconfigured/denied tools are absent from the model schema and denied on direct invocation; cached source IDs and forged cursors do not bypass authority. |
| AT04 | Private references, memory, Skills, inherited context, summaries and newly steered input require approval. Compaction and Agent-provided public labels cannot remove that requirement. Verified public-only context can search automatically. |
| AT05 | Exact query/URL approval permits only the bound operation and eligible bounded retry; edits, account changes, new invocations, another Agent, expiry and revocation do not reuse it. Explicit authenticated URL intent avoids duplicate approval without bypassing policy. |
| AT06 | Real fetch tests reject internal/metadata targets, encoded addresses, mixed DNS answers, rebinding and forbidden redirects before connection. Redirects recheck disclosure and quotas; no auth, cookie, proxy or Referer leakage occurs. |
| AT07 | Sentinel credentials are absent from model requests, parser environment, page servers, approvals, results, events, traces and logs, including malicious provider errors and extraction failures. |
| AT08 | HTML, plain text and text-PDF fixtures produce bounded located text. Oversized/compressed, encrypted, scanned, malformed, slow and script-dependent sources report explicit outcomes; parser process/resource limits are enforced. |
| AT09 | Parallel Workers cannot exceed remaining money/attempt/observation limits; retries/redirects count; local reads do not spend provider quota. Restart, month rollover, tariff expiry, base fees and uncertain charges preserve correct attribution and admission. |
| AT10 | Empty search, publisher denial, auth failure, provider outage and limits are distinguishable. The Agent can use a different permitted source or explain failed verification without fabricated current facts. |
| AT11 | Eligible HTTP failures cause at most one retry within the original deadline, approval and budget. Long `Retry-After`, exhausted limits, transport ambiguity and permanent errors cause no automatic resend or provider switch. |
| AT12 | Kill Workers before/after dispatch, response storage and completion. Recovered completed results make no request; stored outcomes complete without refetch; unknown outcomes stay uncertain and debited. Stale leases and cancelled operations cannot publish late evidence. |
| AT13 | Continuation/find use fixed local snapshots, preserve line/page references and honor scope/caps. Expiry, cursor tampering and denied access never trigger hidden network calls. |
| AT14 | Valid citations open the exact retained fragment. Unread, fabricated, foreign and out-of-range references are rejected; HTML/URL injection is inert. Structural validity is not described as semantic fact verification. |
| AT15 | Freshness filters, search time, provider dates, publisher dates and fetch time remain distinct. Later source edits/refetches cannot change an earlier citation; stale results do not claim current verification. |
| AT16 | Revocation and deletion/expiry affect evidence APIs, journals, SSE, derived outputs and caches consistently. Deleted evidence cannot be recreated by replay or late responses. |
| AT17 | Missing/expired contract evidence, keys, price schedules or incompatible account terms prevent new search calls. Direct page tools remain independently governed. No public list price is treated as proof of a storage-rights entitlement. |
| AT18 | Old Agent configurations gain no capability, published versions are unchanged, third-party Registry tools still work, and rollback disables admission while preserving authorized data/recovery. |
| AT19 | UI walkthrough covers setup/denial, public automatic search, exact approval, cancellation during approval, a partial PDF, citations and budget exhaustion, including authorized and unauthorized viewers. |
| AT20 | Record the live Japanese/English pilot below with actual contract/credential prerequisites met; no fixture result or provider marketing benchmark is substituted. |
| AT21 | Delegation does not transfer keys or disclosure grants. Compatible scoped remote execution respects both authorities and executing-node budgets; unsupported/legacy paths deny access explicitly. |

### Live pilot

Freeze 10 public intents before calls, each expressed once in Japanese and once in English, for 20 queries total. Include four technical-documentation intents, three general-research intents and three requests for recent information. The [prepared pilot protocol](2026-09-25-web-search-evaluation.md) supplies the queries and grading rules. Expected source criteria are primary/official support where available, actual relevance to the question, readable evidence and faithful citation metadata; publish the fully instantiated query list and expected source criteria before measuring.

Proposed acceptance threshold: at least 8/10 queries in each language find a relevant authoritative source in the top five, and all six recent-information queries must establish the correct as-of result using dated primary evidence or a substantiated absence of the requested event. A snippet alone cannot establish freshness. Record source-read success/failure, page format, citation support, each latency, median/maximum observed latency and metered/estimated cost. With this small pilot, do not claim a production latency percentile or universal coverage. Required format fixtures must include HTML, plain text and text PDF. Public queries only; rights to retain/publish the evaluation must be confirmed under the selected plan.

Respect the 10-search Run cap by splitting the pilot across authorized Runs and count it within the configured node budget. Record the account plan, adapter version, query/options, evaluation date, all failures and qualitative judgments. If quality or cost misses the accepted requirements, keep rollout disabled and revisit the adapter/provider deliberately; do not edit the evaluation after seeing results to fabricate a pass.

### Evidence status at document handoff

| Evidence | Status |
| --- | --- |
| Q1-Q14 product decisions and provider rationale | Recorded and confirmed by the user. |
| Official provider-document comparison | Performed; architectural recommendation only. |
| Actual account/contract/storage/no-training/price eligibility | Not verified. |
| Product implementation and executable schema/API | Brave search adapter and input schema have an initial implementation; Harness exposure, reading, evidence, authorization and accounting remain incomplete. |
| Behavior, database, network isolation, parser and UI tests | Adapter unit tests and a local HTTP boundary fixture pass; integrated authorization, database, parser and UI tests have not run. |
| Live 20-query pilot and actual provider latency/cost | Not run. |
| Production capability enablement | Not performed. |

## 11. Delivery, rollout and scope boundaries

Implementation proceeds through vertical increments: capability/config/authorization and normalized search with fixtures; bounded source reading/find/evidence and citations; exact disclosure/accounting/recovery with adversarial cases; then integrated UI and live rollout evidence. Each increment includes its own schema, API, documentation and behavior tests. These are implementation boundaries, not newly created tickets.

Ship disabled, verify the contract/account and isolation environment, run the acceptance suite and pilot, then enable selected policy-authorized Agents. Issue #44 remains open until the required integration and verification evidence exists. A documentation review, a configured secret, or a green mock test alone is insufficient.

Rollback blocks new admissions, aborts/drains identified in-flight operations with conservative accounting, and preserves authorized evidence, approvals, journals and recovery state under retention obligations. Do not destructively down-migrate user evidence or silently fall back to Registry tools. Coordinate the additive capability vocabulary with #43 and reconcile the root glossary with its separate documentation branch.

This scope excludes logged-in browser automation, JavaScript rendering, OCR, image/video search, whole-site crawling, automatic provider switching, a provider-generated research answer, per-user API keys, a universal secret detector, unlimited archives and guarantees of exactly-once provider billing. It does not promise that every public page is retrievable, every cited claim is true, or already disclosed/exported information can be recalled.

These design artifacts were prepared in `docs/issue-44-web-search-design` and copied into the Issue #44 implementation worktree for the requested pull request. The repository intentionally ignores root `CONTEXT.md` and `docs/adr/`; add these specific reviewed files explicitly without changing unrelated ignore rules.
