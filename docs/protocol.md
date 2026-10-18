# Protocol and recovery contracts

This document describes implemented runtime and recovery contracts. Cross-node transaction coordination, semantic vector memory, Kubernetes orchestration and scoped remote execution have implementation paths. Their complete integrated release acceptance remains open; full A2A interoperability is also open. See [transactions](transactions.md), [authorization](authorization.md) and [orchestration](orchestration.md) for their separate contracts and limits.

## Trust and identity

A node publishes `/.well-known/aidash` with its `aidash://` identity, endpoint, capabilities, clusters and protocol version. All management routes under `/api` require a bearer access token except the public, credential-free `/api/openapi.json` contract. The dashboard uses authenticated fetch for SSE so credentials do not appear in URLs.

The Deployment Administrator token retains privileged bootstrap, administration and recovery access; it runs no work of its own, because every workspace, task and Run belongs to a Tenant (ADR 0043). Revocable subject tokens apply the current tenant policy to workspace and conversation APIs, human interaction, approved Registry discovery, local execution, state collections and event delivery. Local runs persist the root credential and delegated agent chain, rechecking their intersected authority at each durable boundary. Scoped remote execution uses a durable source grant, receiver admission, explicit activation and scoped Home commands; each node rechecks its own authority at durable boundaries. Subject bearer tokens are not forwarded. See [authorization](authorization.md) for the exact admission path, actions, catalog approvals, revocation and credential rotation. The full cross-node acceptance in #38 remains open.

Federation retains the `/federation/v0.1` path namespace. A node advertises protocol `0.3` and accepts peers registered at `0.2` or `0.3`; `0.2` is the minimum for Binding contract 1, so earlier peers cannot admit new execution (drain in-flight work before upgrading). Protocol `0.3` adds one thing: the Home's reply to `scoped/execution/grants/describe` carries the current `corpus_digest` of the grant's Workspace when the grant gives the receiver Home semantic access. A node that sends `X-Aidash-Protocol: 0.2` receives the reply it always did, and a reply without the field is valid. Each request must include `Authorization: Bearer <peer credential>`, `X-Aidash-Node`, and `X-Aidash-Protocol` set to the version the receiving peer was registered with. Register a peer at `0.3` only after it runs `0.3`; its `/.well-known/aidash` must advertise the version you register. A peer must already be enabled in the receiving node's database. Peer credentials must have at least 32 printable ASCII characters and eight distinct characters, checked during registration and use. Generate a random token for each trust relationship. Each node has its own credential references; no central identity or message broker is required across nodes. In deployment, use HTTPS and trusted tools managed by whoever runs the Deployment. This version is not a hostile multi-tenant sandbox.

| Endpoint             | Purpose                                                                           |
| -------------------- | --------------------------------------------------------------------------------- |
| `POST /discover`     | Search locally registered agent metadata; does not recursively broadcast          |
| `GET /observe`       | Return runs, invocations and human requests belonging to the caller's home node   |
| `POST /scoped/graph` | Return a bounded, receiver-authorized Graph View projection for one mapped viewer |

The scoped graph endpoint authenticates the source Peer before resolving its
exact Subject mapping or named Deployment Administrator grant. It returns display metadata and
evidence-backed edges for B-owned resources after B's current read decisions;
it never returns raw State. Requests bound the mode, kinds, relations, activity
window, page size and traversal depth. Depth is exactly one direct Peer hop; a
larger value is rejected. Each opaque cursor is tied to that source, viewer, scope
and projection generation, and the receiver rechecks authority per page. A can
expand C only through its own separately authorized direct Peer relationship;
B's connection to C is not included in B's graph projection. See the
[authorization contract](authorization.md#federated-graph-view-authority) and
[Graph View controls](agent-relationship-graph.md#workspace-graph-view).

Remote workers retrieve workspace metadata with `snapshot_workspace`, then use `snapshot_page` for the `tasks`, `artifacts`, `events` and `messages` collections. Each page has `items` and an optional UUID `next` cursor, supplied as `after` on the next request. Pages contain at most 32 records and 3 MiB of serialized data, below the 4 MiB peer decoder limit. Tasks and artifacts are paginated to completion; events and messages retain their latest-100 observation window. An individual resource larger than 3 MiB is rejected explicitly. Collection reads observe current home state and do not provide a transaction snapshot across pages.

Agent memory is separated by agent ID, version, workspace UUID and workspace home node. Existing local memory retains its local namespace; a remote Run cannot read or overwrite it by reusing a workspace UUID. A dependent run fails with the dependency ID and terminal status when a prerequisite is failed, cancelled or abandoned.

## Persistence and delivery

API startup and PostgreSQL worker recovery do not wait for NATS. The event-bus supervisor reconnects independently; federation offer retries also run independently of the broker.

A state mutation and its event are committed together. The event table doubles as an outbox. Its publisher waits for a JetStream acknowledgment, then records publication. A crash in between can replay an event, using its event ID as `Nats-Msg-Id`. Inbox uniqueness and task/run uniqueness remain the permanent deduplication boundary after JetStream's short duplicate window expires.

When a CloudEvent plus its NATS headers exceeds the broker payload limit, the publisher omits `data` and includes a `dataref` URI such as `/api/events?after=123`. Resolve this URI against the source node using an authorized API credential and match the event ID in the returned replay page. All CloudEvent identity and sequence fields remain intact; the full event stays in PostgreSQL. Small events retain their inline `data`.

Event sequence allocation is serialized through a transaction advisory lock so commit order agrees with the SSE cursor. `/api/events/stream` honors `Last-Event-ID`; `/api/events?after=N` provides JSON replay. A disconnected or slow client can resume from the PostgreSQL event log. SSE is an observation channel, not the worker's durable queue.

The workspace's home node owns task revisions and artifacts. A remote node owns its run journal and tool invocations. Ordinary federation commands operate on the home API and use stable keys. When a reply is lost, replay either returns the original result or reports a conflict for mismatched input. These ordinary commands do not form a cross-node atomic transaction. The separate [distributed transaction protocol](transactions.md) implements durable prepare/commit/abort, participant decisions and visibility barriers for admitted mutations. Its subject-scoped authorization, complete process/network failure matrix and Kubernetes release acceptance remain open in #40. External Tool and nonparticipating A2A effects cannot join an atomic manifest.

### Inference Progress

Every provider call of a Run step is an Inference Attempt. The worker records it under its lease before calling the provider. Starting an attempt first closes any attempt of the Run that is still pending as `interrupted/lease_lost`. Each attempt ends with exactly one outcome that never changes: `accepted` is written in the same transaction as the `model.completed` save; `discarded` when a newer Run input supersedes the request, including a stale save, which is rolled back and then discarded under the same lease; `interrupted` with reason `cancelled`, `lease_lost`, `stall`, `stream_error` or `revoked`. When a Run is cancelled, paused, failed, completed or reconciled without its worker, the server closes its pending attempt in the same transaction, as `cancelled` when control is Cancelled and `lease_lost` otherwise. A management pause lets the in-flight call finish, so it leaves the attempt to its worker; if that worker's lease lapses while the Run is not scheduled, such as while paused, the next maintenance pass closes the attempt the same way.

Models stream inference by default: a model config that omits `streaming`, or sets it to `null`, requests a server-sent event stream, and `streaming: false` opts out to the non-streamed request. A 2xx answer to a streamed request that is `application/json` instead of `text/event-stream` is accepted as a whole non-streamed completion without progress; any other content type fails the attempt.

Progress is tentative and display-only. A streaming adapter offers only text and tool-call status (index, ID, name and argument size), never partial arguments or reasoning, and only a complete, validated response can enter ToolCall state. The worker flushes progress every 250 ms or 4 KiB into `inference_progress`, apart from the event journal and without its advisory lock. Rows carry a per-Run `progress_seq`, which the lease-fenced writer allocates under the Run row lock and never reuses. A row holds at most 16 KiB of item data. Fifteen minutes after an outcome, a bounded maintenance pass deletes the attempt's text and tool-call rows; its `started` and `outcome` rows remain as long as the Run. Only the low-rate markers `inference.started`, `inference.accepted`, `inference.discarded` and `inference.interrupted` enter the event journal, with `{run_id, task_id, workspace_id, attempt_id, reason?}` and the same workspace rule as the Run's own events. They reach `/api/events`, the event stream and NATS, but the dashboard projections (the `events` of `/api/state` and of workspace snapshots) omit them: each would otherwise cost every projection a full Run visibility check, and the Run's own events already carry its state. The dashboard does not refetch on a marker frame. Attempt IDs are public; the worker lease token is never exposed.

`GET /api/runs/{run}/inference/stream` delivers progress over SSE. `Last-Event-ID` or `?after=N` resumes after a `progress_seq`; without a cursor the stream starts at the latest attempt's first row. Each frame has `id: <seq>` and `event: inference.delta` (text), `inference.tool_call` or `inference.outcome`; a `started` row is an `inference.outcome` frame whose item is `{"outcome":"pending"}`. Frame data is a CloudEvents 1.0 envelope with type `aidash.inference.progress.v1`, `subject` set to the Run ID and `data` of `{attempt_id, seq, kind, item}`. When the cursor precedes pruned rows, an `event: gap` frame without an ID carries `{attempt_id, outcome, from, to}` before the attempt's outcome row. The stream polls every 250 ms while an attempt is pending and otherwise waits for marker wakeups or the reconciliation interval. Like the event stream, it is an observation channel; a response is durable only when accepted.

The dashboard's run details subscribe to this stream while a local Run is open and not terminal, and resume from the last delivered `progress_seq` after a close. A pending attempt keeps the subscription after the Run turns terminal until its outcome arrives, because the Run query can observe the transition before the stream delivers the outcome written with it. They show the latest five attempts. A pending attempt shows its streaming text, labelled as tentative, and the name and argument size of each tool call. An accepted attempt's text is hidden, and the Run is refetched to show the durable response. Discarded and interrupted attempts collapse into struck-through text with their reason. A gap shows a one-line notice that some progress expired. Text is kept per attempt up to 1 MiB, and replayed seqs are ignored. A polite live region announces only phase changes. Tentative text never enters forms or artifacts.

## Execution and effects

Workers lease one run at a time with a random lease token and a 30-second expiry. They renew every 10 seconds and fence state and tool-result writes by token and expiry. A stale worker cannot advance a new owner's journal. A process crash leaves the last persisted phase, model response and tool-call cursor available for recovery.

The tool invocation record is committed before sending the request. Each tool call has a key derived from run ID, inference step and call index. Its result is committed before advancing that cursor. Replayed completed invocations return the stored result. Task completion and its final artifact are one transaction with an idempotency key and input consistency checks.

For an external effect, exactly-once behavior requires cooperation from the target. An HTTP tool declared `idempotent` must honor `Idempotency-Key`; MCP tools must honor the configured idempotency argument. Declaring an adapter idempotent does not make an arbitrary server idempotent. For `unsafe` adapters, a missing result after an attempt is treated as uncertain and the effect is not repeated. A Deployment Administrator verifies the external outcome and answers the confirmation with `{"result": ...}`. This restores the journal without executing the effect again.

Controls are cooperative at persisted step boundaries. Pause lets an in-flight call finish and persist before stopping the next step. Cancel cannot undo an already performed effect. Human questions and answers are durable. A normal human answer returns the agent to inference; remaining tool calls from the pre-question response are discarded, so a rejected approval cannot release a queued effect.

Transient execution errors have a bounded retry budget per persisted operation. A successful operation resets the counter. Exhausted or permanent failures persist a `terminal_transition` intent in a waiting run. The worker retries home-node delivery across outages and restarts and only finalizes its local journal after the home acknowledges that outcome. It does not re-execute the failed tool while delivering this intent.

A coordinator's final response remains gated while a child is unresolved. A Deployment Administrator can call `POST /api/tasks/{id}/abandon` with the current revision and a nonempty reason for a failed, blocked, or cancelled child. Nested children must be completed or abandoned first. The home records `ABANDONED` with the prior status and reason; a resumed parent can finish with remaining artifacts. Dependencies still require actual completion: abandonment does not satisfy a task dependency.

### Context compaction

The [portable decision gate foundation](decision-gate.md) defines explicit
Decider pins and historical replay contracts for #108. Native execution still
uses the compaction path described below until its separate integration lands.

The compaction trigger, post-compaction fit check and final preflight use the
same complete-request estimate, the Request Estimate (ADR 0017). Its upper
bound is the byte estimate: the UTF-8 bytes of the serialized model-visible
messages and tool definitions (the byte basis, including the JSON escaping of
context inside message text and the cache-salt reserve), plus the media
allowance, `max_output_tokens` and 1,024 framing tokens. It is not the
provider's actual token count.
Output fits inside the registered context window; it is not extra input space.
Failure to fit leaves saved context unchanged, prevents inference I/O and pauses
the Run with the typed reason `context_unreducible`.
New model registrations capture OpenRouter's advertised maximum completion
tokens as `max_output_tokens`; model versions created before this field existed
retain their previous output allowance until re-registered.

A model definition that sets `request_estimator: calibrated` scales only the
byte basis. The ratio is the nearest-rank 99th percentile of reported
`prompt_tokens` per byte of basis over the newest 200 samples of the same
upstream model slug and Projection Version, plus a 10% margin. The media
allowance, output reservation and framing are added unscaled, and the result
never exceeds the byte estimate. With fewer than 20 samples the estimate stays
the byte estimate. Omitting the field means `bytes`; new registrations write
`calibrated`. Samples come only from complete, media-free Inference Attempts
of Run steps. They are kept Deployment-wide in `estimate_calibration_samples`,
which holds no Run, Tenant or attempt identifier, and are written in the same
transaction that completes the Usage Record. Deployment Administrators read per-key aggregates
with `manage diagnostics calibration`.

A model definition that sets `request_estimator: {"tokenized": "<encoding>"}`
counts the byte basis with that Token Encoding instead of scaling it. The list
is closed and defined in code: `o200k_base` (OpenAI GPT-4o through GPT-5,
counted with the gpt-oss tokenizer) and `gemma4` (Gemini 3.1 and later, which
Google's SDK counts with Gemma 4). Registration rejects any other name and
never compares the name with the model slug; the web form offers the encoding
as an optional choice and never preselects one. Every string inside a message
is counted as the text the model receives, with its JSON escaping decoded
once, and each message adds 8 tokens. Each tool definition is counted as its
JSON and adds 32 tokens, because providers render schemas in their own format.
Response formats count as JSON, and the cache-salt reserve is added unchanged.
The media allowance, output reservation and framing follow, and the result
never exceeds the byte estimate. Text longer than 16 KiB is counted in pieces
split where whitespace starts, which bounds counting time and keeps a piece's
count reusable. Only the server binary embeds the tokenizer files; a binary
without them, or a Run step pinned to an encoding version it lacks, estimates
in bytes. A tokenized model still collects calibration samples, and its byte
quotas convert through the key's calibrated ratio when one exists.

Each Run step pins an Estimate Snapshot (estimator, version, Token Encoding,
ratio and margin) in the Run's context before the step relies on it. A resumed
step reuses it, so it makes the same fitting and compaction decisions even after
calibration data changed. A calibrated or tokenized snapshot becomes the byte
estimate for the rest of the Run after a Context Overflow, from the same step,
or when the provider reports more input tokens than the snapshot estimated,
from the next step. Run inspection shows the pinned snapshot without its sample
count. Snapshot quotas are byte budgets that convert through the snapshot's
ratio, while the Ordered Stable Prefix quota and the semantic budget always use
the byte estimate, so calibration never changes the Stable Prefix or the
Retrieval Key. Summary Stage requests, Jev, Registry workbench requests, the
run-message limit and the media headroom keep the byte estimate.
Every Usage Record keeps its Request Estimate with its confidence
(`conservative`, `calibrated` or `tokenized`), the byte basis, the output
reservation and the snapshot ratio, so the estimation error of an attempt is
`estimated_tokens - output_reservation - input_tokens`.

Under a `deferred@1` [Exposure policy](operations/registry-capabilities.md#deferred-capability-exposure),
the model-visible tool definitions are only the request's Exposure set, and the
instructions add the resident Skill blocks and the capability index. The same
estimate covers them; `context.usage.exposure` records their bytes per request.
Legacy requests are measured exactly as before.

The model's pinned workspace and `workspace_observe` use the same bounded view.
Observations include goal/task previews, artifact IDs and metadata, message
previews, and event IDs/kinds/timestamps. Event payloads are never automatically
embedded, so observing an earlier `tool.completed` event cannot copy its full
observation result into the next request. Durable snapshots, invocation records
and audit APIs retain their original content.

`workspace_observe` accepts optional `offset` (default 0) and `limit` (default 20,
maximum 50). Each collection reports its total and next offset. Events/messages
are newest first; tasks/artifacts retain snapshot order. These are live pages,
so refresh after concurrent changes. `workspace_read` selects an accessible
`workspace`, `task`, `artifact`, `message` or `event` by exact ID, through the
same authorization-filtered Home snapshot. It returns JSON text in Unicode
character ranges: `offset`, `max_chars` (default 8,000, maximum 16,000),
`total_chars` and `next_offset`. Concatenate chunks in offset order to recover
the full record. Missing and inaccessible IDs return the same error. Reading
mutable records across concurrent changes requires restarting the read.

On replay, legacy `workspace_observe` results are projected into this view in
the working context. This changes only observation representation; human
records, other tool results and the underlying audit journal are preserved.
The conversion commits only when the complete request fits.

Context compaction adapts [fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction/tree/e3f262a7f4d42bd8dd32ced30d26176f7cb545b0) to Aidash's paired tool events. Jev receives a fitted classification view and returns separate call/result retention probabilities; it does not rewrite human messages or summarize history. The first and recent events, non-tool text, instructions, current task/workspace, memory, and any Execution Summary remain verbatim in the inference context. Unneeded pairs can be dropped and unneeded outputs truncated, while the original events remain in the Context Journal and complete invocation results remain in PostgreSQL. Every batch sees the same fitted state, with bounded request size and four concurrent requests. Invalid answers, a missing OpenRouter binding or insufficient compaction leave the original context unchanged. Only a Run with an approved compactor is pruned; any other Run that outgrows its window stops with `prune_unavailable` (ADR 0041). Compaction uses the compactor's exact Jev release and the Tenant's current OpenRouter Provider Credential, independently of the explicitly selected inference model and of whichever grant serves it. Token estimates are conservative and are not provider tokenization.

The default retention threshold is 0.5: keep the complete pair when the result probability reaches the threshold; otherwise retain the call with the first 300 result characters and a notice when the call probability reaches it; otherwise drop the pair. Results up to 420 characters are unchanged. The first and latest six history events are pinned.

The classifier state is limited to 25,000 estimated tokens. Tool input caps shrink through 1,000/200/60 characters, followed by text abridgment, old-text size notes, compact call lines, omission of old call-less entries and merging of adjacent old call-only entries. These reductions never alter stored human or assistant text. Questions are divided into requests of at most 30,000 estimated tokens, below the 32,000-token context OpenRouter lists for Jev. The Jev estimator counts letter runs, digits and symbols following upstream; inference uses a separate conservative character-based budget.

Requests go to OpenRouter's System One API, `POST {base}/systemone` under the Provider Catalog base URL (or the Credential Broker's on Cloud), and send `{model,state,questions,provider:{zdr:true}}` with the exact pinned model, such as `typesafe/jev-1.13`; aliases are refused. Each request is a Decision Attempt with its own identifier: on Cloud its Capability Token names the Run as subject and carries the `decision` operation, and the broker admits only that body shape. A response must name the pinned release or a dated snapshot of it (`typesafe/jev-1.13-20260917`). The reported `usage.cost` is logged with the Decision Attempt and is not a Usage Record. The upstream license notice is listed in [THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md).

### Context recovery

Every history event a Run records receives a sequence number and is appended to
the `run_context_events` Context Journal in the same transaction that saves the
Run. The history sent to the model is a lossy Context Projection of that
journal: pruning and summaries change the projection, never the journal. Runs
saved before the journal existed import their current projection, marked
`imported`, on their next save or, at the latest, before their first pruning or
summary, because earlier pruning may already have removed
events. Message-read and inference coverage, semantic and authorization read
journals, and pending execution state are kept outside the projection, so
compaction cannot make an unread source look read or forget a revoked
dependency. The journal is internal; it is deleted with its Run and is not
exposed through the public API. The inspection API returns each history event
with its `seq`, the `journal` cursor and any `execution_summary`.

An Agent version selects its recovery behavior with an immutable
`context_policy`. Without one, the Run keeps the prune-only behavior above,
`prune-only/1`, and its provider requests are byte-identical to earlier
versions. The opt-in `context-recovery/1` policy accepts:

| Field                 | Default | Bounds                | Meaning                                                                                           |
| --------------------- | ------- | --------------------- | ------------------------------------------------------------------------------------------------- |
| `preserve_recent`     | 6       | 6–1000                | Latest events (Model Turns on `native`) that are never pruned or summarized                       |
| `overflow_retries`    | 2       | 0–4                   | Compact-and-retry attempts after a provider Context Overflow                                      |
| `overflow_shrink`     | 0.75    | 0.5–0.9               | Factor applied to the effective window on each overflow retry                                     |
| `summary.model`       | none    | exact model reference | Separately approved summarizer; enables the Summary Stage                                         |
| `summary.max_tokens`  | 4,096   | 256–4,096             | Summary output bound, further capped at an eighth of the window and the summarizer's output limit |
| `summary.call_budget` | 8       | 1–64                  | Summary requests per Run, including failed attempts                                               |

The pipeline is fixed: fit check, Jev pruning, the optional Summary Stage, then
complete-request validation. Under an explicit policy an unavailable Jev pauses
the Run with `prune_unavailable`; the Summary Stage never stands in for Jev.
When pruning cannot fit the request, the Summary Stage merges eligible older
events into the structured Execution Summary. It absorbs only `tool` events
that are older than the protected tail and were already part of an accepted
inference request. Human answers, corrections, continuation markers and
`model_media_observation` events, whose source messages a summary could not
recheck, always stay verbatim. So do `workspace_observe` pages and workspace
record reads other than message reads, whose records are visible one by one
while a summary rechecks only message sources. On the `native` Projection
Version the unit is a Model Turn: an `assistant` event and the `tool` events of
its calls are absorbed together or not at all, and Jev elides a dropped result
under its call instead of removing the call. The summarizer receives only the
task, the previous summary and the absorbed events, with no tools and a JSON
schema response format. Each merge
copies the previous goal verbatim and
keeps the previous summary's constraints and unresolved items under their IDs,
in the same list and with unchanged text, unless it lists them as resolved;
a changed or moved item is rejected, and a resolved ID must leave both active
lists. A resolution's `resolved_by` and every new
`verification` reference must be the exact ID of a tool call the merge absorbs,
and a new verification's outcome must equal a complete value of that call's
result, so a claim the result does not state is rejected;
previous decisions, resolved and verification entries must be carried
unchanged, so a retired ID never returns. A new artifact's reference, and its
revision when one is given, must each equal a complete argument or result value
of one absorbed event, so `src/parser.rs.bak` never proves `src/parser.rs`; a
previous artifact is carried unchanged unless the merge's history states a
newer, non-empty revision of the same reference. It records the
exact journal range, the absorbed sequence ranges (entries pruned between them
were never absorbed), and the absorbed entries' digest, and carries the sources
it depends on. A summary is adopted only if it validates, strictly shrinks the
complete request and makes it fit. A `summary` text stored before the journal
existed stays model-visible verbatim.

Every summary request is recorded in `context_compaction_attempts` before any
provider I/O. The summarizer must be the exact model pinned in the Run's Binding
snapshot. Local Runs need its catalog approval. Generated chains need every
ancestor to approve it and are charged per ancestor before I/O. Remote
Runs need a summarizer pin and a summary allowance: the semantic
request's optional `summarizer` must name the Agent's pinned `summary.model`, the
Home discloses it as the binding's `summarizer` provider, and each request is
admitted with usage purpose `summary` against every owner's `summary` (or
`remote.summary`) allowance. The Home accepts the request only when the
executor's own inspected Agent pins that `summary.model`; an executor whose
Agent schema predates `context_policy` cannot resolve such an Agent, so peers
without the field never receive it. An Agent without Home semantic retrieval
sends the pin in a `disabled` semantic request; the Home then authorizes the
remote execution and `semantic.disclose` for the summarizer, never `semantic.search`
or its index and embedding provider, and binds `disabled` with only that pin. The
Home accepts only the approved maximum charge: the pinned summarizer's context
window plus `summary.max_tokens` capped at its output limit.
Disclosure is checked before every request. A missing approval makes the stage unavailable,
with no fallback. Every source the candidate depends on is rechecked before
its reservation, so an already revoked source charges nothing, and again once
the reservation holds and before the request leaves the node; a revoked source
settles the attempt as unauthorized without calling the summarizer, and a held
reservation is released: a local charge is refunded whole, and a dispatch the
Home already admitted settles with unknown usage. Adoption
saves the Run and marks the attempt adopted in one
lease-fenced transaction, after the Run's inference authority, every
summarizer approval above and every source the candidate depends on
(including those carried from the previous summary) are rechecked without
charging another call; a summarizer or source revoked during its call is
never adopted. A Run without an authority guard still rechecks every source
after the call. Malformed, empty,
non-reducing, unauthorized or interrupted summaries leave the saved context
unchanged; an interrupted attempt is marked `abandoned` by the next attempt or
by lease recovery. If a source the summary depends on is no longer readable, the
summary is discarded and the original journal events it absorbed return to the
saved projection, and the step pauses for authority before Jev or the model
receives them. The context keeps the discarded summary's dependencies, so
every later step pauses the same way until each source is readable again.
Message sources are the targets of absorbed message reads.
A local Run rechecks message dependencies under its current authority without
recording a read; a remote Run rechecks them through the Home's filtered record
reads. An authority-bearing Run rechecks them again after pruning, whose Jev
I/O releases its authority, before the summarizer or the model is called, and
after the model call, which releases it as well, before its response is
accepted; a source revoked during that call discards the response.

Provider failures are classified before recovery. An OpenRouter `400` or `413`
whose message reports the maximum context length is a Context Overflow; a `413`
without that proof, such as a media payload limit, is not.
Only a Context Overflow enters compact-and-retry. Each retry shrinks the
effective window for the rest of the Run, re-runs the pipeline, and must send a
smaller request than the previous overflowing one; otherwise, or once the retry
allowance is spent, the Run pauses with `overflow_retries_exhausted`. An
overflow that follows a calibrated Request Estimate is an estimator defect: the
Run is demoted to the byte estimate for its remaining steps, in the same save,
and the overflowing request is recorded by its byte estimate so the next
comparison uses the same unit. `aidash_context_overflow_total` carries the
estimator as its `estimator` label. Output
truncation (`output_truncated`) and refusals (`refused`) pause immediately and
never dispatch the response's tool calls; their reported usage still settles
the call's token reservation. A paused Run's typed reason is reported as
`context_reason` by run management and by remote execution status and control.
Other `4xx` responses keep their
existing handling, and transport errors keep their bounded retry. OpenRouter
requests from an Agent with a Context Policy, including its Summary Stage
requests, set `transforms: []`, so the provider never compresses them on its
own. Prune-only Agents keep their unchanged request body and OpenRouter's
default transforms.

Metrics: `aidash_context_compaction_total{stage,outcome}` counts prune and
summary outcomes (`applied`, `insufficient`, `unavailable`, `invalid`,
`unauthorized`, `revoked`), `aidash_context_compaction_tokens{stage,phase}`
records estimated sizes before and after, and
`aidash_context_overflow_total{outcome,estimator}` counts `retried` and
`exhausted` overflow recoveries by the estimator (`bytes` or `calibrated`)
whose request overflowed. Labels never contain Run identifiers or content.

## Marketplace and localization

The node is a self-hosted package repository. A manifest includes the versioned entity, author, permissions and dependencies. Publication is immutable. Installation verifies the SHA-256 digest, validates dependencies and atomically registers the entity with local installation configuration. This version distributes declarative manifests; it does not execute downloaded code, operate a public SaaS marketplace or handle payments.

UI strings live in `web/src/locales/en-US.json` and `ja-JP.json`. Entity names and descriptions are localized maps. Agent language capabilities are separate metadata used by local and remote discovery. Message translation is not automatic.

### Bounded inspection and retry identifiers

`GET /api/tasks?offset=N` returns up to 500 authorized tasks, newest first, with
`next_offset` for the next page. `/api/state` includes only the first task page.
`GET /api/runs/{id}?offset=N` returns at most 100 invocation previews; request the
next offset when a page is full. Inputs/results over 1 KiB are represented by an
explicit `truncated` marker and text preview. Durable execution records retain
original values. Credential inventories use `?offset=N` pages of 200 records.
Scoped collections fill pages after authorization checks.

Scoped federation discovery returns `entries` and `next_offset`, with at most 64
entries and 3 MB of metadata per response. Observation returns at most 100
runs, human requests and invocation previews; run context and pending payloads
remain available through the authorized run detail API.

Run-message and remote message controls accept a UUID `idempotency_key`. Reuse
that key when retrying an ambiguous request. The dashboard keeps it until that
submission succeeds. A stream request with `Last-Event-ID: -1` starts from the
current high-water mark; the dashboard refreshes state after the connection is
established and then resumes from acknowledged event IDs on reconnect.

Optional workspace snapshots are reduced to the selected model's available
context budget before history compaction, with a `snapshot_truncated` marker.
The durable workspace remains intact and tools can retrieve omitted details.
Automatic semantic retrieval skips budgets too small to contain provenance.

Agent registration rejects instructions, referenced skills and tool definitions
that cannot fit the selected model window with output and context reserves.
Under `deferred@1`, registration instead measures Mandatory exposure, Eager tool
definitions and eager Skill blocks plus the full `metadata_bytes`,
`schema_bytes` and `skill_bytes` budgets. It also rejects any single tool
definition over `schema_bytes`, or any Registry or attached Skill block over
`skill_bytes`, naming the alias and size.

### Remote semantic reuse

The executing peer reserves every remote inference and compaction at the Home
through `POST /scoped/usage/reserve`; the Home calls back the peer's
`POST /scoped/usage/verify` before it charges anything. Both carry a `boundary`
of `{"step":n}`, and the peer's verify requires a READY semantic operation of
its own for that step. An `ordered` Run reuses one retrieval across steps
(ADR 0020), so a reuse step instead sends `{"step":n,"reuses":"<operation ID>"}`,
naming the operation whose receipt it reuses. The boundary is bound in the
peer's durable dispatch record, so the Home cannot substitute it in the
callback. The two nodes check the citation against their own journals:

- the executing peer: the operation is READY for this Run and grant, from a
  step no later than the Run's actual step;
- the Home: the same against its journal for the step the peer claims, the
  grant is still live (`403`), no consumed source changed or was revoked
  (`409`, reason `invalidated`, the same conditions under which
  `grants/describe` answers 409), and the Workspace corpus digest equals the
  one it recorded before that retrieval (`409`, reason `stale`).

A `stale` answer makes the Run drop its cached retrieval and retrieve again
after one second, up to five times in a row. Peers that predate the field
interoperate: an older executing peer sends no citation, so its reuse steps
stay pending as before; an older Home ignores the citation, still checks the
live grant and consumed sources while leasing it, but not the corpus digest.

### Remote run-message recovery

A remote input is reserved at home before admission to the executor ledger.
`run_message_commit` acknowledges that durable admission with a positive
`input_seq`, bound immutably to the task, run, key and content. An exact retry
can promote its existing reservation after an outage, even after reservation
expiry or task termination; it does not create a new reservation or reopen the
task. Legacy commits without a sequence still require an active reservation.
The executor retries promotion from its persisted input before delivery.

Explicit cancellation and failure use `run_message_terminal_transition` with
`through_seq`, the executor's admitted-input high-water mark. The request stays
bounded even for large reference-only histories. The home consumes only fences
with an acknowledged sequence at or below that mark, under the same task lock
and transaction as the terminal transition. Unknown reservations remain active;
if they block termination, consumption rolls back too. Legacy key-based terminal
requests remain supported.

The fence migration backfills well-formed peer-prefixed corrections for active
home tasks without requiring a local run row. Scoped admission fetches home
history after authorization preflight, then imports it before allocating the
new input sequence within the authorization-revalidation transaction.
