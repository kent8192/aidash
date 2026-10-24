# Retained usage facts

Cloud and Dedicated retain complete Pod allocation receipts independently of Run,
operation, Node and Tenant authority. An allocation UUID and its canonical receipt
digest identify one immutable Tenant/start/end payload. An identical retry reuses
the committed facts; a conflicting payload fails and is not acknowledged to the
runner. Entirely exempt and uncovered allocations still retain their raw receipt.

Authoritative boundaries and durations are nonnegative signed 64-bit Unix epoch
nanoseconds. Inputs outside that exact range, leap-second timestamps, nonpositive
intervals, future evidence and invalid digests are rejected rather than rounded.
The database wall clock is read after lifecycle and capacity locks. SQL timestamp
columns are explicit microsecond projections; they do not determine attribution.

Each allocation is frozen as a complete half-open partition. Recorded Tenant
Deletion spans are unioned before exemption is subtracted. Remaining time is
split by durable account coverage, retaining both the coverage UUID and account.
Missing coverage within known history is explicit `uncovered` time. Overlapping
or contradictory history rejects intake; current attachments never supply a
historical account. A Node soft deletion alone creates no exemption.

Storage retains the original point and its frozen covered, uncovered or exempt
attribution at `sampled_at`. The first accepted sample for a Tenant's UTC day is
immutable across replicas and retries, including zero bytes. A point exactly at
an account move belongs to the successor coverage interval. Exempt source samples
are retained but excluded from the existing billable usage projection.

## Account-period queries

Cloud account owners can use their existing authenticated account boundary:

```text
GET /api/billing/accounts/<uuid>/usage?starts_at=2026-10-01T00:00:00Z&ends_at=2026-11-01T00:00:00Z
```

Both RFC3339 endpoints are required; the period is positive and half-open. The
response contains historical covered segments clipped exactly to the period and
samples whose original point falls inside it. `pod_nanos` and segment `nanos` are
decimal strings so JSON clients preserve integers beyond JavaScript's exact
numeric range. Segment and sample timestamps preserve nanoseconds. Another
account's owner receives no access to these facts.

Dedicated keeps its existing Deployment Administrator usage JSON/CSV without
Cloud account routes. Those reports retain the stopped-UTC-day contract: precise
Tenant totals are projected to milliseconds at the query edge and then to the
existing seconds representation. Account-period facts do not infer a currency,
price, subscription anchor, processor export, invoice or credit model.

## Lifecycle and retention

Intake shares the permanent retention boundary, then the Tenant policy row when
present, then the actual Tenant capacity lock. Deleted Tenants remain ingestible.
After physical purge removes policy, permanent Tenant birth and completed purge
history establish the lifecycle instead of a live authorization requirement.
Raw receipts, segments, samples and account coverage survive Tenant purge.

Retention takes the history boundary exclusively before pruning. Dependent
segments expire before their raw parents; referenced coverage and accounts remain
while facts are retained. Open coverage is never closed by an invented purge or
detach timestamp. The retained-history floor itself is permanent and advances in
the same transaction as pruning.

New evidence must begin at or after both permanent Tenant birth and the retained
history floor. An older or straddling receipt fails as unavailable history rather
than partially billing or inventing uncovered time; preserve the provider receipt
for manual reconciliation. An identical retained receipt remains replayable
without recomputing its frozen attribution. Development schemas are recreated;
old rows are not assigned guessed digests or historical coverage.

Stripe Sandbox selection and actual Issue 405 runtime acceptance remain separate
from these model-neutral facts. See [sandbox acceptance](stripe-billing.md).
