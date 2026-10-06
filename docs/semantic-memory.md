# Native Hindsight memory

Aidash stores memory and its search projections in PostgreSQL 17, using pgvector
for semantic ranking and PGroonga for English/Japanese keyword retrieval. There
is no Qdrant process or Python Hindsight sidecar. The Rust implementation adapts
[Hindsight](https://github.com/vectorize-io/hindsight) at the revision recorded in
[`memory/UPSTREAM.md`](../crates/aidash-domain/src/memory/UPSTREAM.md), retaining
its MIT attribution. Accepted requirements and outstanding release gates are in
[the design contract](design/2026-10-06-hindsight-memory-requirements.md).

## Canonical memory and identity

A bank belongs to a Home Node, tenant and Workspace. A private bank also belongs
to a Home-issued logical Agent participant. Definition versions, worker
processes and execution Nodes do not own private memory. Explicitly assigning a
participant to a Task preserves that identity across Runs. Without an assignment,
admission creates a fresh participant. Clones and generated Agents start empty;
explicit same-definition version upgrades retain the participant and its bank.

Canonical units have stable UUIDs, observed revisions, relational text, kind
(world, experience, observation or mental model), learning classification,
verification, occurrence times, entities, exact evidence revisions and graph
links. Typed metadata uses PostgreSQL JSONB; there is no arbitrary JSON-object
Agent memory store. Existing legacy JSON memory is discarded. Message/Artifact
source records and execution evidence remain available. There is no importer,
compatibility reader, legacy cutover, or rollback workflow.

Admission truncates learning and occurrence timestamps to PostgreSQL microsecond
precision before hashing or saving a revision. Nanosecond host clocks and input
dates therefore preserve the same body identity across replay and recovery.

Add, Correct and Delete operate on selected units. Correct/Delete require the
revision actually observed by the caller. Exact-request operation receipts make
unchanged retries idempotent; changing a request under the same key conflicts.
Conflicts require refreshing the displayed evidence before choosing a new
operation. There is no whole-bank replacement.

Selected private units can be published through an explicitly authorized Home
operation into the shared Workspace bank. Publication retains the source's exact
revision and the publisher's current authority. Reading a publication rechecks
that authority, source lineage and the reader's shared-bank disclosure permission;
it does not grant access to the private bank. Source revision changes invalidate
all dependent memory and consumed Run context, including the writer's own Run.
Index-only generation changes preserve canonical revisions and read dependencies.

## Registry and model work

An immutable `memory` Registry definition selects `hindsight_rust` and exact,
versioned extraction, derivation, reflection, embedding, reranker and tokenizer
roles. Reranking can explicitly select a registered RRF passthrough or a model.
UTF-8 upper-bound token accounting is an explicit registered tokenizer. Roles
never silently fall back after a failure. Registry entries contain references
and policy, while bank bodies live in Workspace persistence.

Every policy declares finite source, unit, graph, candidate, result, context,
model-call, token, cost, timeout and retry limits, exact role prices and retention
limits. `semantic_link_min_similarity_millionths` pins the minimum cosine
similarity for semantic graph neighbors (1 through 1,000,000). The Registry form
exposes this cutoff with the six role versions and prices. The independently
committed model ledger reserves the maximum before
I/O, conservatively charges unknown outcomes and memoizes completed exact
requests. Run learning and maintenance triggered by Run writes also resolve the
current execution authority and reserve the generated origin's ancestor token
allowances before model HTTP. Expiry and current policy are checked before the
call and again before admitting its result; an expired origin grants no fallback
to an operator or maintenance allowance.
A private bank pins its accepted Agent policy; an explicit Agent upgrade changes
that policy. A shared bank requires `memory.configure` and an observed settings
revision and an exact-request receipt to change its policy. A caller cannot select a more permissive policy
for an existing bank.

## Retain, recall, derived memory and reflection

Retain extracts independent world facts and experiences from exact, current
source evidence. Model-generated content stays unverified; admission and
verification are distinct. Run learning is opt-in (`learn_from_runs`).
Natural retirement drains opted-in generated Run extraction, human candidate
review, and the resulting initial index jobs under the original active origin.
The completed Run is not runnable again. Its request may remain `ACTIVE` while
this finite completion work waits; the original expiry, explicit stop/delete,
live authority checks, and reserved ancestor allowances still apply. No new
budget is allocated and no lifetime is extended. Disabled learning and terminal
engine failures permit immediate retirement. Later work cannot use a retired or
expired origin's model allowance.
Home maintenance discovers completed bound Runs and schedules durable candidate jobs.
A Run must still be current, completed without a Run error and free of incomplete
or uncertain invocations. Extraction reads canonical Run, accepted input, human-interaction and full
invocation records, not caller-supplied prose or the bounded journal previews
shown by the UI. Discovery uses durable keyset cycles so blocked early Runs
do not starve later participants. Oversized work fails its finite
input bound instead of silently taking a prefix. Candidates enter a separate
human review queue; completion is never a truth label.

`maintain_observations` enables automatic jobs over newly admitted or corrected
world/experience units. Bounded connected groups use exact normalized entities
and case-preserving whitespace duplicate matching, keeping world/experience and
learning categories separate. Each group uses its earliest admitted seed for a
stable observation identity. The model must cite every grouped source revision,
including conflicts; excess evidence fails rather than silently truncating.
Source changes queue idempotent repairs over surviving admitted support, while
superseded automatic observations are retired in the publication transaction.
`refresh_mental_models` enables selective refresh of
recurring questions whose admitted sources changed. A mental model preserves its
question and per-model `automatic_refresh` choice separately from its generated
answer. Derived claims remain unverified and cite exact admitted units. Deleted,
revoked or otherwise unavailable sources cannot be recreated by refresh. Durable
jobs record pending/running/complete/blocked/failed state, bounded attempts and
redacted failure codes. Claim and publication fences prevent expired workers
from publishing; successful publication and job completion commit together.

Recall combines semantic, keyword, graph and temporal rankings with reciprocal
rank fusion, then the registered reranker. Graph traversal is bounded and follows
entity, semantic, temporal and causal links. Current embeddings produce disposable
semantic edges; changed unit/model/index generations cannot reuse old edges.
Explicit retention resolves causal fact indexes to host-issued IDs in one atomic
batch. Run learning leaves these unadmitted references out of review candidates.
Automatic observations combine complete entity/duplicate support with bounded
semantic retrieval and a pinned model verdict. Extending a live observation
retains every earlier source and conflict; retrieval, selection and synthesis
share the operation allowance. Returned units include their complete
provenance, verification and revisions. The entire serialized context envelope
counts toward the token allowance. Disabled, Empty and NoSpace are distinct;
enabled search/index failures stop the inference path instead of yielding an
empty success. Reflection performs bounded recall and exact-revision reads and
returns only supported citations. Reflection is explicit, rather than occurring
before every inference.

Home inference refreshes native Recall from the current Task and latest accepted
Run input at every inference boundary. Exact consumed unit/support revisions are
committed before model disclosure and checked by Run visibility, output delivery
and SSE. Native tools are `memory_mutate`, `memory_recall` and `memory_reflect`.

## Search, cleanup and recovery

The PostgreSQL search projection uses immutable point/model/index generations,
scoped to Home, tenant, Workspace, collection and an authorized point allowlist.
Search retrieves canonical current text rather than trusting vector payloads.
Provider dimension changes require a new generation. Reindex operations use an
observed index revision and preserve canonical memory revisions.

Admitted units persist until explicit deletion or a configured unit age limit.
Candidate bodies, revision bodies and model results have finite retention;
negative identities and receipts have finite admission caps and never reopen a
deleted identity. Deletion excludes immediately and invalidates dependent use.
Durable purge jobs separately track physical history, model-result and vector
cleanup. Cleanup follows retained source lineage into selected shared copies
and old quoted revisions, while rechecking independently corrected current bodies. Their `backup_until` value is a horizon, not proof of backup erasure.
Each failed job records a redacted error and a finite retry count; a failed
deletion does not block unrelated due jobs. Exhausted jobs remain visible.

The PostgreSQL image pins pgvector/PGroonga alongside the existing required
extensions. The supported single-primary fixture enables `pgroonga_crash_safer`
and `pgroonga.enable_crash_safe`. PGroonga recovery and any replica/failover
profile require their own evidence; a single-primary restart is not HA evidence.
The [managed new-format recovery path](memory-recovery.md) retains an external
epoch/revision/deletion ledger and closes memory serving persistently during
reconciliation. It restores memory into live authority/control/evidence records,
rechecks origins and source revisions, and validates or rebuilds disposable
search generations. Recovery acceptance remains pending in the
[verification ledger](design/2026-10-06-hindsight-memory-verification.md).
Legacy JSON backups are unsupported inputs.

## Workspace UI and remote release gates

Workspace memory exposes private/shared scope, participants, observed-revision
unit editing/deletion, exact selected publication, candidate review/edit/reject,
recurring questions, provenance, verification, freshness, history, impact counts,
job attempts and cleanup status in English/Japanese. Unknown network outcomes
retain the exact request key/body for retry; CAS conflicts do not automatically
substitute a new revision.

Required-Home retrieval now carries explicit native participant/provider pins
and complete native Recall envelopes alongside authorized Workspace search.
Both Nodes validate current policy/catalog and exact origin-owned model
reservations before Home provider HTTP. The receiver cannot add its private or
legacy memory. Generated remote Agents receive a new Home participant with an
empty private bank; the selected participant is a policy template, not a body
copy. Disposable index rebuilds preserve canonical consumed-unit proof. Source
changes invalidate canonical remote reads, and Home purge removes cached remote
receipt bodies. Receiver copies also have their own durable expiry and cleanup
queue, bounded by the Home policy. Expiry or withdrawal commits invalidation
before erasure; a storage failure cannot make the old quotation readable again.
The content-free Run management response exposes scheduled, removed or failed
cleanup even when current source authority prevents reading the Run body.
Remote memory writes remain restricted. Kubernetes/k3s acceptance for
ordinary/generated Agents and labeled retrieval/learning quality and cost
assessment are separate gates. Issue #73 numeric service targets and Issue #122
quality targets remain declared release inputs.

## Verification

Use `cargo test -p aidash-domain -p aidash-application -p aidash-harness --lib` for
pure contracts and workflows. `cargo test -p aidash-server --test
reinhardt_persistence native_memory:: -- --test-threads=1` exercises real
PostgreSQL extension/search/unit CAS, participant scope, inference read fences
and durable derived work. The serial setting keeps the multi-database fixture
inside its connection/extension-worker profile. Provider fixtures establish
protocol behavior, rather than live model usefulness. Full Registry-to-UI,
backup/restore/crash, and two-Node acceptance must be recorded against the exact
delivered SHA before claiming overall completion.

Canonical units retain a normalized set of contributing Run origins. Human
candidate admission and later corrections preserve that lineage. Observation
selection, synthesis and disposable vector reconstruction validate every
contributing origin's current execution authority and Registry roles. A single
provider call reserves the union of generated ancestors in one transaction;
shared ancestors are charged once. Cached results are also subject to current
origin checks, and checks repeat after provider I/O. This implementation remains
under acceptance validation; consult the verification ledger for current results.

An explicitly configured unit TTL excludes content at delivery, receipt replay and
transitive source checks before asynchronous physical cleanup. Candidate expiry
also prevents late admission and body disclosure. Recovery fences include the
learning timestamp at PostgreSQL microsecond precision so a snapshot cannot reset
the retention clock. Publication preserves the selected private unit's Run origins;
sharing never grants an operator-funded model allowance.
