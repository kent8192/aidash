# Web research

Web research lets an Agent find public Web sources, read them and cite what it
read. The Node owns the research contract: its result kinds, the Research
evidence store, Source citation checks, Web disclosure approval and cost
attribution. Providers only search or read pages. They are
[Registry packages](operations/registry-capabilities.md#research-provider-packages)
that install and bind like any other package, so one provider can replace
another without any change to the Harness or the Agent definition.

The Harness has no research tool names and the Agent configuration has no
research flags. It reacts only to declared tool behavior:

| Declared behavior            | Operator effect                                                              |
| ---------------------------- | ---------------------------------------------------------------------------- |
| `yields: [Search]`           | None; Source records are discovery hints and cannot be cited                 |
| `yields: [Fetch, Citations]` | The Run's final answer has its Source citations checked                      |
| `yields: [Citations]`        | The Run's final answer has its Source citations checked                      |
| `disclosure_approval`        | The provider may return `approval_required`; the Run waits like any approval |

## Operations

Every provider package implements one or more of three operations. The host
gives each a stable default Model alias, description and argument schema, which
are identical for every provider.

| Operation | Alias             | Discloses | Yields               | Implemented by                           |
| --------- | ----------------- | --------- | -------------------- | ---------------------------------------- |
| `search`  | `research_search` | yes       | `Search`             | `research.brave@1`, `research.fixture@1` |
| `fetch`   | `research_fetch`  | yes       | `Fetch`, `Citations` | `research.page@1`, `research.fixture@1`  |
| `find`    | `research_find`   | no        | `Citations`          | `research.page@1`, `research.fixture@1`  |

All three are `ReadOnly`, replay-safe and local to the executing Node (they are
not exposed to remote Runs).

- **`search`** takes `query`, optional `language` (`ja`, `en`), `country`,
  `count` (1–10), `page` (0–9), `freshness` (`day`, `week`, `month`, `year` or
  an inclusive date range) and up to five `include_domains` and
  `exclude_domains`. The host validates the request, the provider translates it
  without relaxing it, and the host filters the provider's candidates again:
  only absolute `http(s)` URLs without credentials or fragments, inside the
  domain filters, bounded in size. Each kept candidate becomes a **Source
  record** with a `source_id` and `evidence_state: "unread"`.
- **`fetch`** reads one page as a **Document snapshot**, given a `source_id`
  from this Run or an `http(s)` `url`. It returns the first passage, at most
  `max_bytes` (default 8 KiB, at most 16 KiB) of text, as an **Evidence
  fragment** with an `evidence_ref`; a budget smaller than the next character
  still delivers that one character. Continue with `document_id` and the
  returned `next_cursor`; continuing reads the retained snapshot and never
  fetches again.
- **`find`** returns lines of a retained snapshot that contain `query`. Matching
  folds case (and `ß` to `ss`) unless `case_sensitive` is set. Every match is an
  Evidence fragment. Nothing leaves the Node.

Results share one envelope: `{version, operation, status, limits, data}` with
`status` `ok`, `partial`, `empty`, `error`, `uncertain` or `approval_required`.
A result never exceeds 32 KiB; a passage shrinks rather than being cut silently,
and the cursor resumes exactly after the delivered text. Errors carry a stable
`code` (for example `unknown_source`, `invalid_cursor`, `disclosure_denied`,
`non_public_destination`, `attempt_limit`, `budget_exhausted`, `rate_limited`,
`timeout`) and never contain provider response bodies or credentials.

## Research evidence

The execution app stores evidence per Run:

| Table                | Holds                                                                                  |
| -------------------- | -------------------------------------------------------------------------------------- |
| `research_attempts`  | One row per dispatched `search` or `fetch` invocation key, its outcome and cost        |
| `research_sources`   | Source records                                                                         |
| `research_snapshots` | Document snapshots: retrieved and final URL, title, media type, digests and text lines |
| `research_fragments` | Evidence fragments delivered to the model                                              |

Every row belongs to one Run and is deleted with it. Foreign keys include the
Run, so a fragment can only refer to a snapshot of its own Run. A snapshot is
immutable; its `text_digest` identifies the revision, and cursors and fragments
are bound to it. A later retrieval of the same address creates a new snapshot.

Cursors are stateless tokens bound to the snapshot, its digest, the operation
and, for `find`, the exact query. A cursor for another document, another Run or
another query is rejected as `invalid_cursor` or `unknown_document`; it never
causes a fetch.

An attempt is reserved before the provider is called. Repeating the same
invocation key returns the recorded result, with the same Source record and
fragment identities, instead of dispatching again. An attempt that was
dispatched but never recorded (for example after a crash) returns
`outcome_unknown` and is never repeated.

## Source citations

A final answer cites Evidence fragments as `[[cite:ev_<uuid>]]`. When a Run binds
any operation that declares `yields: Citations`, the Harness checks every marker
of the final answer against the Run's own fragments:

1. Unknown or malformed references add one `citation_correction_required`
   event to the Run's history and ask the model to correct the answer.
2. If the corrected answer still has invalid citations, the answer is withheld
   and the Task completes with: "Research evidence could not be verified. The
   answer was withheld because its citations were unavailable or invalid."

The correction is allowed once per user input: an answer to later user input
gets its own correction. Text published beside tool calls, or while subtasks
are pending, is never corrected, so it is not published when it cites unknown
evidence. When inference progress streaming is enabled, such a Run streams
tool-call progress but not tentative answer text, so unverified citations never
reach the progress stream; the verified final answer is published as usual.

A Run that binds only `search` has no citable evidence, so its answers are not
checked; search snippets are never evidence.

## Web disclosure approval

`search` and `fetch` declare `disclosure_approval`: the exact query or page
address leaves the Node only after a Web disclosure approval. The approval is
an ordinary capability approval record of kind `disclosure` with action
`research.disclose` on resource kind `research`, whose ID is the destination
host. The Run's subject needs `capability.request` on that resource to ask at
all; without it the disclosure is denied.

- If every Run subject's policy allows `research.disclose`, or an active Run
  grant from an earlier "allow for this Run" decision covers the destination,
  the disclosure proceeds.
- Otherwise a designated approver (a `user` subject with the
  `capability_approver` attribute and `capability.approve`) must decide. The
  tool returns `approval_required` with the `approval_id`, and the Run waits as
  it does for any other capability approval. The approver sees the exact
  `disclosure` content in `GET /api/capabilities/approvals` and decides with
  `POST /api/capabilities/approvals/{id}/decide`.
- After approval the Run resumes, and the model repeats the identical call. An
  `allow_once` approval is consumed by that one disclosure, only once its
  attempt is reserved; a call that is not dispatched leaves it usable.
- An "allow for this Run" grant covers later disclosures to the same
  destination only until new user input arrives.
- Denial, expiry or an explicit policy deny returns `disclosure_denied`; the
  provider is never called.
- A Run without local worker authority (for example, a remote Run) has no
  approval flow, so every disclosure is denied, as for `outbound_get`.

The approval is bound to a digest of the Run, operation, provider, destination,
exact content and the Run's observed input sequence. The content is the
complete outbound request: Brave discloses the JSON body it sends, so country,
language, count, page and freshness are approved with the query, and a page
read discloses its URL. New user input therefore
requires a new approval. Approval never overrides destination checks: page
providers refuse non-public names and addresses, including addresses a public
name resolves to at dispatch. A page is read only from the approved destination
host: a redirect to another host returns `cross_host_redirect` instead of being
followed, a redirect from https to http returns `insecure_redirect`, and a
same-host redirect is checked again before any request.

## Cost attribution and limits

A search provider may name the Search service operator's account. The Brave
package reads the operator's account profile, which records the agreement,
retention terms, request price and monthly limit. Before dispatch the host
checks the terms are current and that one more request fits the month's limit
(base fee plus every charged or still-unknown attempt). The reservation records
the account and price. An attempt stays charged once the provider was called,
even if it then failed (for example an HTTP error or an invalid response); only
an attempt stopped before the call, such as one whose one-time approval was
already used, is not charged.
Charges are recorded as signed 64-bit amounts, so a profile whose monthly limit
exceeds 9,223,372,036,854,775,807 micro-USD is unavailable.

Per-Run limits block a request before disclosure or dispatch: 20 searches, 50
page reads, 100 Source records, 50 snapshots and 8 MiB of snapshot text. A page
read by URL creates its Source record when its attempt is reserved, before the
page is requested, so it counts against the Source record limit (and keeps the
record if the read then fails); reading an existing Source record creates none.

## Providers

| Provider             | Tier      | Availability                                                                                                                      |
| -------------------- | --------- | --------------------------------------------------------------------------------------------------------------------------------- |
| `research.brave@1`   | `host`    | `AIDASH_RESEARCH_BRAVE_PROFILE` names a valid account profile and its secret is set                                               |
| `research.page@1`    | `host`    | `AIDASH_RESEARCH_PAGE_READING=enabled`                                                                                            |
| `research.fixture@1` | `fixture` | `AIDASH_RESEARCH_FIXTURE=enabled`; a deterministic in-memory corpus that sends nothing outside the Node, for tests and evaluation |

The Brave key is a **tool secret**: the profile's `credential_env` names an
`AIDASH_SECRET_*` environment variable. It is not a Provider Credential.

```json
{
  "account_id": "brave-account",
  "agreement_reference": "contract-2026-10",
  "verified_by": "operator",
  "verified_at": "2026-10-01T00:00:00Z",
  "review_after": "2027-01-01T00:00:00Z",
  "storage_permitted": true,
  "query_training_prohibited": true,
  "evaluation_permitted": true,
  "retention_days": 90,
  "deletion_after_termination_days": 30,
  "credential_env": "AIDASH_SECRET_BRAVE_SEARCH",
  "price_effective_at": "2026-10-01T00:00:00Z",
  "price_review_after": "2027-01-01T00:00:00Z",
  "request_price_micro_usd": 5000,
  "monthly_base_fee_micro_usd": 0,
  "monthly_limit_micro_usd": 20000000
}
```

Providers are read once at startup; restart the Node after changing them. The
page provider reads `text/html`, `application/xhtml+xml` and `text/plain` only,
up to 4 MiB per response and three same-host redirects. It strips markup, drops script,
style and template content and never executes page content. PDF and other media
return `unsupported_media`.

## Implementing another provider

A provider implements the contract in `aidash-integrations` by implementing one
or both ports of `aidash_application::ports::research`:

- `SearchProvider`: `disclosure(query)` returns the exact destination and
  content it will send (or a contract failure if it cannot send the query
  unchanged), `account()` returns the Search service operator's current terms
  or `None`, and `search(query)` returns raw candidates or a contract
  `Failure`. It must not relax the query's filters; the host filters again.
- `PageProvider`: `admit(url)` performs literal destination checks before any
  disclosure, and `fetch(url)` returns an `ExtractedPage` within the snapshot
  bounds. It must resolve and pin public addresses itself, check every
  redirect and never follow one to another host than the approved destination.

Register the provider and its operations in `RESEARCH_PROVIDERS`
(`aidash-domain`), its package group in `RESEARCH_GROUPS` (`aidash-application`),
its availability and dispatch in the server's research service, and admit its
descriptor in the registry descriptor function with a new migration. Nothing
else changes: the host validates input, waits for disclosure approval, attributes
cost, records evidence and checks citations for every provider alike. The
fixture provider is a complete example; the acceptance tests run the same host
contract against it and a production-shaped provider.

## Not yet covered

- No live Brave request has been made; account eligibility, ranking quality and
  billed-cost reconciliation need live verification with a real account.
- Retained evidence has no separate retention period or revocation endpoint; it
  lives and is deleted with its Run.
- There is no evidence viewer in the dashboard; citations appear as their
  markers in the published answer.
- Third-party providers over the HTTP, MCP or Agent transports cannot declare
  research operations yet; providers are Node implementations.
