# Run-scoped Web research

Issue #44 adds three Harness tools, `web_search`, `web_open` and `web_find`, and
their approval, usage and citation controls in an existing conversation Thread.
They are independently enabled on an immutable Agent version; they do not create
Registry tools or require a working area. Existing Agent versions default to
all three flags being false. External Registry integrations keep their existing
identities and authorization.

## Operator configuration

Ship with admission disabled. API and Worker services read `AIDASH_WEB_PROFILE`
as the path to a JSON profile. An absent profile has these defaults:

```json
{
  "admission": false,
  "allowed_domains": [],
  "denied_domains": [],
  "search_attempt_limit": 10,
  "page_attempt_limit": 20
}
```

An empty allowed list permits any otherwise eligible public domain. Entries
match a DNS name and its subdomains at label boundaries; denied entries win.
Each list contains at most five lowercase DNS names. Attempt ceilings can be
tightened to 1–10 searches and 1–20 page requests. Every retry and redirected
request consumes an attempt. Search domains are intersected with caller
restrictions and enforced again on returned candidates. A disjoint intersection
is denied, and a configuration change never silently changes an approved query.

Search also requires `AIDASH_WEB_SEARCH_PROFILE` pointing to the existing
`web_search::AccountProfile` contract. Its required fields are:

| Fields                                                                             | Required operator record                                                                           |
| ---------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `account_id`, `agreement_reference`, `verified_by`                                 | Selected paid account and the reviewed agreement's durable reference.                              |
| `verified_at`, `review_after`                                                      | UTC eligibility window; future verification or an expired review disables search.                  |
| `storage_permitted`, `query_training_prohibited`, `evaluation_permitted`           | All true only after the actual account terms have been checked.                                    |
| `retention_days`, `deletion_after_termination_days`                                | Positive, agreed retention/deletion limits.                                                        |
| `credential_env`                                                                   | Secret-reference environment name, for example `AIDASH_SECRET_BRAVE_SEARCH`; never the key itself. |
| `price_effective_at`, `price_review_after`                                         | UTC validity window for the account's tariff.                                                      |
| `request_price_micro_usd`, `monthly_base_fee_micro_usd`, `monthly_limit_micro_usd` | Verified tariff and budget, in integer millionths of a US dollar.                                  |

An example name or public price is not a verified entitlement. Configure the
referenced secret only in trusted API/Worker services, with the repository's
secret-reference convention. Keep private profiles and keys outside Git and
test artifacts. Missing, invalid or expired account configuration removes search
from the model's tools and denies direct use. It does not disable independently
authorized page reading or local `web_find`. There is no alternate-provider
fallback.

`web_open` requires the isolated controller configured by
`AIDASH_CAPABILITY_PROFILE`. Follow [the runtime setup](core-capabilities.md).
The pinned image and admitted controller must advertise
`aidash-web-extraction/1`. No downloaded document is parsed on the service host.

Publish a new Agent version with only the required `core_capabilities.web_search`,
`web_open`, and `web_find` flags, enable that version in the catalog, and retain
the usual `agent.execute`, Run visibility and `tool.invoke` permissions. Flags
and operator admission do not grant access. Web execution on a remote Home is
explicitly unsupported; credentials and approval grants never transfer to a
peer.

## Disclosure and user interaction

The Thread research form selects the authorized Agent and creates a normal Run.
Unclassified or nonpublic context pauses before an outbound request. The
disclosure panel shows the exact destination and effective provider query/URL,
Run and immutable Agent version, request digest, revision and expiry. Only the
requesting User or a designated `web_approver` User with current Run visibility
and disclosure authority can approve once or deny. An explicit policy deny on
the current authority chain always wins. The approval expires after 15 minutes
and cannot be used by another invocation.

A User may classify **all current** instructions, conversation, references,
Skills, memory, summaries and inherited inputs as public. This is an authenticated
action bound to the current context digest; an Agent cannot label its own inputs
public. New input or changed context invalidates classification and pending
approvals. Compaction preserves the conservative disclosure requirement.

The form's optional direct-URL action supplies the User's exact, one-use intent
for that Run and Agent version, avoiding duplicate confirmation for that URL.
It still checks policy, private-network restrictions and the original message's
visibility. A redirect destination needs a fresh disclosure decision. Run stop
works while approval is pending and while a request is in flight.

Authenticated Run APIs expose usage, classification, exact disclosure decisions,
revocation and `ev_UUID` citation fragments under `/api/runs/{run}/web/…`.
Evidence references are Run-scoped, immutable fragments actually delivered to
the Agent. Search candidates are unread and cannot be cited. Final answers use
`[[web:ev_UUID]]`; one correction is allowed for invalid references, then the
answer is withheld with an explicit verification failure. Reference validity
does not establish factual or semantic correctness. The UI opens the retained
excerpt, source URL/title/host, fetch time, lines and PDF pages as escaped text.

## Network, extraction and recovery limits

Public HTTP/HTTPS on ports 80/443 only; reject embedded credentials, signed/key
query parameters, fragments and nonpublic addresses. All DNS answers are checked
and pinned per HTTP attempt. Fetching has no proxy, cookies, authentication,
Referer, subresources or JavaScript execution. Redirects are explicit, limited
to three, reauthorized and metered; HTTPS cannot downgrade to HTTP.

Downloads and decompressed input are capped at 10 MiB; extraction at 1 MiB and
200 PDF pages. HTML, plain text and text PDFs run in a fixed offline gVisor
parser with 1 CPU, 256 MiB and a five-second extraction deadline. Its own seccomp
filter prohibits child creation before parsing; it verifies this with a fork
probe. The trusted result collector retains its separately admitted process
budget. Scanned/encrypted/malformed PDFs and size limits produce explicit
outcomes. No OCR, authenticated browser or host parser fallback is provided.

Model-visible JSON is capped at 32 KiB, including escaping. `web_open` defaults
to 16 KiB of text, at most 24 KiB; continuation uses an opaque cursor against the
fixed cached document. `web_find` is literal, supports full Unicode case folding,
defaults to ten matches and permits at most twenty. Local reads take no network
attempt or provider charge.

Search/page deadlines are 15/20 seconds, including bounded waits, fetch and
extraction. At most one retry is allowed for 408/429/500/502/503/504 inside the
original deadline and approval. Long `Retry-After`, transport ambiguity and
permanent failures are not resent. The node admits one search per second, at
most two active searches, four active page requests and one request per host.

Run/node/month ledgers reserve attempts and estimated cost before dispatch,
including the account's recurring fee. All accounts share the node's monthly
$20 ceiling, further tightened by the configured account limit. Charges remain
reserved after uncertain outcomes. The usage UI reports estimates rather than
reconciled invoices, attributed to Run, Workspace and Agent. Stored response
receipts finalize locally after recovery; completed and uncertain dispatches
never cause automatic repeated HTTP requests. Current Worker lease, Run state,
policy and evidence visibility are rechecked before publication.

Snapshots use at most 20 MiB per Run, expire after 24 hours idle and are purged
within an hour of Run termination. Delivered observations use at most 200 records
and 8 MiB; they survive snapshot expiry under Run access and retention. Retention
starts at 90 days and is shortened by the search account's configured limit.
Revocation, retention expiry and Thread deletion deny subsequent reads and
derived outputs through the dependency ledger. Cleanup removes cached text,
operation/result copies and Run output copies. Its minute interval runs even
with Web admission disabled.

## Verification and remaining rollout gate

Run the PostgreSQL/Harness authorization, evidence and recovery tests:

```sh
cargo test --locked --test web_research
```

The ordinary isolated-runtime CI wrapper now also runs nine controlled HTML,
Unicode/gzip, text-PDF, scanned/encrypted/malformed, page-limit, decompression and
text-limit fixtures through the real reader. With an owned admitted installation:

```sh
python3 scripts/test-web-extractor.py /tmp/aidash-core-demo
AIDASH_CAPABILITY_PROFILE=/tmp/aidash-core-demo/profile.json \
AIDASH_RUNNER_TOKEN_FILE=/tmp/aidash-core-demo/token \
  bash scripts/test-web-research-runtime.sh -- --show-output
```

The second command uses a real public RFC text source through the production
HTTP path, isolated extraction, Harness find and retained citation API. It makes
no Brave request. Browser verification includes the research form, approval,
usage/stop controls and inert citation rendering in `web-research.spec.ts`.

The [recorded local evidence](evidence/2026-10-01-web-research/README.md) is separate
from hosted CI and from the required [20-query live search pilot](../design/2026-09-25-web-search-evaluation.md).
The Brave account is unconfigured, so that paid pilot remains **NOT RUN**. Freeze
its approved queries before measuring, for example:

```sh
python3 scripts/freeze-web-search-pilot.py \
  --date 2026-10-01 --operator OPERATOR_REFERENCE \
  --account-profile /private/operator/verified-brave.json \
  --output /private/evaluation/manifest.json
```

This helper performs no network request, records only account references and
source/profile digests, and refuses to overwrite an earlier manifest. Without
`--account-profile`, it creates an offline rehearsal only. Execute and grade the
frozen queries through authorized product Runs with all controls intact, record
every failure and actual citation, and apply the protocol's language/freshness
thresholds. No fixture is substituted for search ranking or billed-cost evidence.

For rollback, restart API and Workers with Web `admission: false`, preserve the
database/controller journals, and stop/drain affected Runs through their normal
controls. Cleanup and visibility checks continue. Do not down-migrate retained
evidence or enable an alternate provider to bypass admission. Issue #44 stays
open until its account eligibility and recorded live-pilot gate are satisfied.
