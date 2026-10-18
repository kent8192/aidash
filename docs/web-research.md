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

| Declared behavior            | Harness effect                                                               |
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
  fetches again. A PDF snapshot's lines carry their physical page, so its
  passages report `pages`. Pages that carry no text are listed under
  `pages_without_text` with `page_images` `no_view_operation` (the original
  is retained, but no research operation shows page images yet) or
  `original_not_retained`; a PDF whose pages all lack text
  delivers no passage (`status: "empty"`).
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

## Reading cited evidence

A Source citation binds to the Run that published it. The Node records that
Run on each message and result Artifact the Run publishes (`run_id` on
`Message` and `Artifact`). People, peers and other Nodes cannot set it, and
the dashboard never infers it from an idempotency key. Text without a
publishing Run, such as a person's message or an answer another Node
delivered, has no citations: its markers stay literal text.

`GET /api/runs/{run}/evidence/{evidence_ref}` reads one Evidence fragment of that
Run, for example `GET /api/runs/<run>/evidence/ev_<uuid>`. It returns:

- the passage exactly as delivered to the Agent, with its lines, pages and the
  snapshot's `text_digest`;
- the Document snapshot's final `url` (and `requested_url` when a redirect
  changed it), title, media type, `fetched_at` and completeness;
- `context`: up to 20 whole snapshot lines and 4 KiB of text on each side of
  the passage. The Agent never received these lines; they let a reader judge
  the passage in its place. `context` is `null` once the snapshot's text is no
  longer retained, while the passage itself stays readable.

The reader needs exactly the authority of `GET /api/runs/{run}`, evaluated at
each read. A Subject needs `workspace.read` and the Run's visibility; an
Deployment Administrator needs the Workspace's Deployment Administrator visibility. Each of these returns the
same `404 {"error":"evidence unavailable"}`, so a response never reveals
whether a Run or its evidence exists:

- a reader who may not read the Run;
- a Run or reference that does not exist, including evidence deleted with its
  Run;
- a reference that belongs to another Run;
- a malformed reference.

The dashboard shows the citations in channel and thread messages and in a
Task's text result as numbered chips. A repeated reference keeps its number.
Opening a chip reads that one fragment. Every 404 renders as "This source is
unavailable", never as an error.

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

The approval record keeps what the approver is shown, as it stood when the
disclosure was requested: the operation and provider, the price of this one
request with the Research account holder's account it is charged to (none for
a provider without a price), the validated search query in a
provider-independent form, and the Run's Working Area so that the approval
appears in its Thread. The account is kept for audit and never shown. A
decision records who decided and when (`decided_by`, `decided_at`). An "allow
for this Run" decision on a search creates a Run grant for the provider's
host, so it covers every later search of that Run through the provider,
including queries the approver has not seen, until new user input or its
expiry. A Page reading grant names the page's host, whichever provider reads
it. A search grant never covers a page reading, nor a search through another
provider. A Run grant lasts at least a minute and at most the Deployment's
`grant_seconds` (an hour by default, and never less than 60); the dashboard
offers 1 to 60 minutes and starts at 15. An approval covers only the charge
its approver was shown: when the Research account or its price changes, the
same request needs a new approval.

A disclosure approval is never revoked: it ends by a decision, by expiry, or
as `closed` with a `closed_reason`: `evidence_revoked` when the Run's research
evidence is revoked, `area_cleaned` when its Working Area is cleaned up, and
`run_ended` when someone decides it after its Run ended. The pending inbox
never lists a disclosure whose Run has ended. One that no subject of the Tenant may
approve is recorded as `blocked` and is never addressed to anyone.

`GET /api/capabilities/approvals` takes two optional filters for an inbox:
`state=pending` lists approvals awaiting a decision and `state=active` lists
Run grants, both without expired ones, and `role=approver` keeps only records
addressed to the reader as approver. Each card says whether its reader is that
approver (`viewer_is_approver`). Only the signed-in Tenant's records are
listed.

## Cost attribution and limits

A search provider may name the Research account holder's account: whoever
runs the Deployment holds it and pays for it. The Brave package reads that
account's profile, which records the agreement,
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
A page read in progress holds a snapshot slot until it settles, so concurrent
reads cannot together exceed the snapshot limit.

`GET /api/runs/{run}/research` returns a Run's usage against these limits, the
recorded prices of its charged attempts (an estimate, not an invoice; attempts
with an unknown outcome and attempts still in flight count), how many attempts
have an unknown outcome or are still sent, and the Run's URL intents. A reader
needs exactly the authority of `GET /api/runs/{run}`.

`GET /api/research/providers` lists the Research providers this Deployment
knows, whether each is set up now (account terms are checked on every
request), and for each operation the reader's Tenant's tool definitions that
have a Catalog Approval; it also returns the per-Run limits. It never says why
a provider is not set up. Only a Tenant's subjects read it.

## In the dashboard

There is no research mode of a Thread or an Agent. When the Agent selected in
a Thread's Agent work panel binds research operations, directly or through a
bundle, the panel lists each operation with its provider and whether it is
ready, not set up in this Deployment, or not approved for use here; the
per-Run limits; and, for the Thread's Run, its usage, estimated research cost,
attempts with an unknown outcome, its URL intents and the disclosures it is
waiting on. A research tool call that failed for a Deployment reason, such as
`budget_exhausted` or `account_unavailable`, is explained in words without
amounts.

A pasted `http(s)` address in the instruction offers "Read this page" when the
Agent binds a Page reading operation that is ready. Each selected page is sent
as a URL intent in `read_pages` (at most five) with the start, queue or steer
request. The host refuses the whole request with `400 INVALID_URL_INTENT` when
an address could not be read or the Run's Agent binds no Page reading
operation, and otherwise records the pages beside the person's text, on the
Task or the run input, never in it (ADR 0052). Each step whose Task or run
messages carry URL intents renders a fixed instruction into its state
(`url_intents`), and each such run message names its own `read_pages`. A URL
intent only asks the Run to read the page: the Page reading it leads to still
needs its own Web disclosure approval, even when the person who pasted it may
approve. Non-public destinations are refused when the page is read.

The header's decision queue is the approver's inbox. Besides human requests it
lists the capability approvals addressed to the signed-in Designated approver
in the signed-in Tenant and the Run grants they gave, which they can revoke,
and a new approval raises the same in-app notification as a human approval
request. The notification names the operation, destination, Agent and
requester, never the content sent. A Designated approver decides in the inbox
without access to the Thread. The disclosure card shows the operation,
destination, provider, the Deployment account it is charged to, the estimated
price, requester and Agent, a summary of the search query and, collapsed, the
exact content sent. It never shows the Thread, the Run's output, the account
ID or any remaining budget. In the Thread, a denied, expired, closed or
blocked disclosure stays listed as such and is never sent, and the Run's
active grants are listed with their scope for their requester and approver to
revoke.

A Run's details show whether its research evidence was revoked, and when, to
everyone who may read the Run; `GET /api/runs/{id}/research` carries
`revoked_at`, `can_revoke` and `can_read_revocation` so the dashboard offers
only what the reader may do. A subject permitted `research.revoke` revokes a
Run that has done research from its details: the dialog says the revocation is
irreversible and what it closes, and requires a reason that is kept as audit
history. Who revoked a Run's evidence and why is shown only to the audience of
the Tenant's records, through `GET /api/runs/{id}/research-revocation`.

Trust → "Research evidence retention and revocation" (`/trust?view=evidence`)
shows a Tenant's snapshot periods to every subject of the Tenant, beside the
Deployment's ceiling. `GET /api/tenants/{tenant}/retention` says whether the
caller may change them (`can_update`), read their history
(`can_read_history`) and read the Tenant's revocation records
(`can_read_revocations`). A change is reviewed before it is saved, with a
warning when it shortens a period, because it applies to content already kept
at the next hourly sweep; saving makes both periods the Tenant's own, and
clearing returns to the Deployment's. History and records load 50 at a time.
Deployment Administrators choose the Tenant and revoke by Run ID, without
opening the Run; the request names that Tenant (`tenant` in the body), so a
Run of another Tenant answers 404 and is not revoked. A Deployment
Administrator's identity in history and records is shown only to Deployment
Administrators.

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
  "verified_by": "deployment_administrator",
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
up to 4 MiB per response and three same-host redirects, on port 80 for `http`
and 443 for `https`. It decodes a page with its byte order mark, its declared
`charset` or, for HTML, a `<meta>` declaration, and otherwise as UTF-8 or
windows-1252; an encoding the Encoding Standard does not support returns
`unsupported_media`. It strips markup, drops script, style and template content
and never executes page content. Other media return `unsupported_media`; PDFs
are described below.

### PDF pages

The page provider also reads `application/pdf`, and `application/octet-stream`
when the body starts with `%PDF-`, up to 16 MiB. Aidash never parses a PDF in
the server process: it starts the `aidash` binary again as a child
(`aidash __research-pdf extract`) that reads the PDF on standard input and
answers on standard output. The child inherits no environment, so it holds no
Deployment secret, and runs under operating-system limits: 20 s of CPU time, a
1 GiB address space on Linux, a wall-clock deadline shared with the read, and a
bounded reply. Decompression is bounded by that address space rather than by a
per-stream budget, because the parser (`hayro`) has none. A child that fails for
any reason is killed and leaves the page unreadable (`extraction_failed`). The
parser reads page content streams and the fonts they use; it never interprets
annotations, forms, scripts or attachments.

- Lines follow content-stream order, one visual line each, with the page's
  physical 1-based number; a page boundary always ends a line. Page labels are
  not used. The first 500 pages are read.
- A page whose text has fewer than 16 non-whitespace characters carries no
  text and is listed in `pages_without_text`.
- A PDF that needs a password returns `encrypted_document`; one whose
  permissions forbid extracting its text returns `extraction_not_permitted`.
- When some page carries no text, the snapshot keeps the original PDF apart from
  its text, so that the Run's model can view that page as an image (ADR 0053).
  A Run keeps at most 64 MiB of originals; beyond that a snapshot records only
  the original's digest and size. Originals are snapshot contents: the
  Retention Rules purge them and an Evidence Revocation removes them.

On macOS the first PDF a server process reads can take about 30 s while the
system assesses the unsigned binary; Linux has no such delay.

## Implementing another provider

A provider implements the contract in `aidash-integrations` by implementing one
or both ports of `aidash_application::ports::research`:

- `SearchProvider`: `disclosure(query)` returns the exact destination and
  content it will send (or a contract failure if it cannot send the query
  unchanged), `account()` returns the Research account holder's current terms
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
- Citations from one Run cannot be shown in another Run's text; a parent Run
  that relays a child Run's answer does not carry the child's citations.
- Third-party providers over the HTTP, MCP or Agent transports cannot declare
  research operations yet; providers are Node implementations.
