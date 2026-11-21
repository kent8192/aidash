# Protocol and recovery contracts

This document describes runtime and recovery contracts, including the accepted Node lifecycle and Deployment Key rollover decisions. Cross-Node transaction coordination, semantic vector memory, Kubernetes orchestration and scoped remote execution have implementation paths. Their complete integrated release acceptance remains open; full A2A interoperability is also open. See [transactions](transactions.md), [authorization](authorization.md) and [orchestration](orchestration.md) for their separate contracts and limits.

## Trust and identity

A Deployment publishes `/.well-known/aidash` with its `aidash://` identity, endpoint, protocol version and, when it has a Deployment Key, that key's ID and public key (`key` is `null` otherwise). During a scheduled rollover, optional `key_rollover` publishes `successor` or `retiring` public identities, `published_at`, `activates_at`, `retires_at` (Unix seconds) and the public `transition` proof. The document names no Node and exposes neither private keys nor File/KMS source references. All management routes under `/api` require a bearer access token except the public, credential-free `/api/openapi.json` contract. The dashboard uses authenticated fetch for SSE so credentials do not appear in URLs.

The Deployment Administrator token retains privileged bootstrap, administration and recovery access; it runs no work of its own, because every thread, task and Run belongs to a Tenant (ADR 0043), and it reads or changes no Tenant content or settings (ADR 0102). A Tenant's first Subject Credential in a headless setup comes from `manage issue-subject-credential` on the host. Revocable subject tokens apply the current tenant policy to thread and conversation APIs, human interaction, approved Registry discovery, local execution, state collections and event delivery. Local runs persist the root credential and delegated agent chain, rechecking their intersected authority at each durable boundary. Scoped remote execution uses a durable source grant, receiver admission, explicit activation and scoped Home commands; each Node rechecks its own authority at durable boundaries. Subject bearer tokens are not forwarded. See [authorization](authorization.md) for the exact admission path, actions, catalog approvals, revocation and credential rotation. The full cross-Node acceptance in #38 remains open.

Federation retains the `/federation/v0.1` path namespace and speaks protocol `0.5` only. A request at any other version, including `0.2`, `0.3` and `0.4`, answers 400 before any token is read. A Node of one Deployment authenticates to a Node of another with a short-lived Federation token, a compact JWS signed with the sending Deployment's Deployment Key (`alg` `EdDSA`, `typ` `aidash-federation+jwt`, `kid` the key ID). Each request carries `Authorization: Bearer <Federation token>` and `X-Aidash-Protocol: 0.5`. The token's claims are:

| Claim            | Value                                                                                             |
| ---------------- | ------------------------------------------------------------------------------------------------- |
| `iss`            | the sending Deployment id                                                                         |
| `sub`            | the sending Node address, `aidash://<deployment>/<node name>`                                     |
| `aud`            | the receiving Node address                                                                        |
| `subject`        | the sending subject's key in the sending Tenant's policy; empty when the Node acts                |
| `iat`, `exp`     | `exp - iat` at most 60 seconds; the receiver accepts an `iat` up to 30 seconds ahead of its clock |
| `jti`            | a non-nil UUID                                                                                    |
| `kid`            | the key ID, equal to the header                                                                   |
| `protocol`       | `0.5`                                                                                             |
| `method`, `path` | the request method and its target under `/federation/v0.1`, with the query                        |
| `body`           | base64url without padding of SHA-256 over the request body bytes                                  |
| `transition`     | optional bounded predecessor-signed rollover proof during overlap                                 |

A token is bound to one request, so a captured token reaches no other endpoint. A sender mints a new token, with a new `jti`, for every request it sends. A receiving Node in another Tenant records the sending Deployment and `jti` of every token whose signature verifies until it expires plus 30 seconds; a token it has already seen answers the same `401`, and a request whose `jti` it cannot record is refused (ADR 0139). The receiving Node decides in this order, and every refusal up to and including the signature answers `401` with `{"error":"unauthorized"}`, so a caller cannot tell a wrong Node name from a wrong key. The request names a Node of this Deployment, whose Tenant is not deleted. For a sender in another Deployment, the receiving Deployment needs `[deployment] federation = true` and a Deployment Key of its own, and the receiving Node needs a Node Registration for the sender that pins the sender's key. The signature must verify with its current receiving keyset, including a verified successor transition when needed. A sender in this Deployment is trusted in process and is never accepted over HTTP. A sender of the receiving Node's own Tenant needs no registration and no Peer Mapping: the Tenant's policy must allow `node.use` on the receiving Node for the sender's subject. A sender of another Tenant needs an enabled Node Registration, and a named subject needs an enabled Peer Mapping keyed by `(receiving Node, sending Node, sending subject)`; a missing mapping after a valid signature answers 403. The handler then requires the operation's own action for the acting subject. A token whose `subject` is empty acts as the sending Node itself, and mapped-subject access refuses it with 403. With the switch off or no key, the receiver refuses every token and the sender answers 503 `federation_disabled` before any network call; exchange between Nodes of one Deployment is unaffected. A Node Registration fetches `/.well-known/aidash` from the other Deployment, which must advertise `0.5`. A public endpoint must use HTTPS. A private address works only when the Deployment permits its origin in `[keyless_endpoints] permitted_private_origins`. Every inbound caller is a Node; a Deployment Administrator of another Deployment has no Node and no inbound path. This version is not a hostile multi-tenant sandbox.

| Endpoint                   | Purpose                                                                            |
| -------------------------- | ---------------------------------------------------------------------------------- |
| `POST /discover`           | Search locally registered agent metadata; does not recursively broadcast           |
| `POST /scoped/graph`       | Return a bounded, receiver-authorized Graph View projection for one mapped Subject |
| `POST /scoped/duty-events` | Send an event to a `node_event` Duty Trigger of the receiving Node (#1013)         |

The scoped graph endpoint authenticates the source Node before resolving its
exact Subject mapping. The request body names that `subject`, which the receiving Node resolves through its Peer Mapping; protocol `0.4` bodies, which chose a viewer, are refused. It returns display metadata and
evidence-backed edges for B-owned resources after B's current read decisions;
it never returns raw State. Requests bound the mode, kinds, relations, activity
window, page size and traversal depth. Depth is exactly one direct Node hop; a
larger value is rejected. Each opaque cursor is tied to that source, viewer, scope
and projection generation, and the receiver rechecks authority per page. A can
expand C only through its own separately authorized direct Node connection;
B's connection to C is not included in B's graph projection. See the
[authorization contract](authorization.md#federated-graph-view-authority) and
[Graph View controls](agent-relationship-graph.md#thread-graph-view).

### Remote Grants that sleep and hand work on

A Remote Grant's lifetime (1 to 3600 seconds) is its admission deadline. Once this grant's Home binding records the receiver's activation, the grant lives while the original Task still awaits its first claim or while the receiver holds the unfinished Task. Before the first claim, the Task must still be `OPEN`, unowned and at the revision pinned by both the grant and its binding. This lets a Run queue for its Working Area before its first Stint. The grant ends when the Task is Completed, Failed, Cancelled or Abandoned, and at once when the Home revokes it. A grant whose activation the Home never recorded, whose Task someone else claimed, or whose Task a retry reopened still expires with its lifetime. The Home re-evaluates the whole delegation chain every time it describes the grant or runs a scoped command, so a broken chain refuses every use. A grant whose activation the Home never recorded, or whose Task its receiver does not hold (claimed by someone else, or reopened by a retry), still expires with its lifetime; the Task's status never stands in for the activation. Every Home check uses this one rule, the Home's tool asks (ADR 0139) and its hourly renewal of `expires_at` (ADR 0109 amendment of 2026-11-17) included: the renewal extends a grant by another hour only while its activation is recorded, the original Task is awaiting its first claim or the receiver holds the unfinished Task at the bound revision, and its chain still holds. A grant whose activation the Home never recorded is therefore never renewed, and a started grant the renewal can no longer extend (its receiver unreachable, say) stays live under the rule. The receiver never pins `expires_at`, so a renewal ends no admission. A receiver that has started the Run leaves liveness to the Home's describe instead of its pinned copy of `expires_at`; for a Home's request about one of its Runs (Live preview) it still requires the admission to be open and the Run not ended, and holds a Run that never started to the pinned lifetime. The receiver may still pause or cancel the Run, and its administrator may disable the Peer Mapping or the Node Registration, at any time.

A Run that a Remote Grant started may call `agent_discover` and `task_delegate` (#654; ADR 0109 amendment). Their Tool Reach is the Home's Thread, so across Tenants both the Home's Federation Tool Request and the receiving Node's Allowance must name them (ADR 0139); they are not among the default tools, and the Home checks its own Request again before it serves `discover` or `delegate`. Every Task stays at its Home Node, so every hop is a Remote Grant from that Home:

- `agent_discover` is the scoped Home command `discover` (`{"search":{…}}`). The Home's Node of the Task discovers as the grant's root subject under the grant's chain, exactly as a person's discovery does, and returns only the cards a model sees (`{agents:[{node_id, entity}], errors}`). The cards are not recorded as Read dependencies of the receiver's Run.
- `task_delegate` sends the scoped command `delegate` for a child the grant created. For the Home's own Node the Home admits the child locally, as before. For any other Node, the receiver's own Node included, the Home checks the child under the command's lease, releases it, and then prepares and activates a Remote Grant of its own for the child under the grant's whole chain, never as its root person alone: every subject of the chain, the receiver's executor included, must be allowed `task.delegate` on the child and `federation.execute` on the target Node, the new grant carries that chain, and a prepared grant is reused only for exactly that chain. A retry reuses the prepared grant.
- The receiving Node of the Run consents through its policy's `tool.invoke` on the tool, the Agent's `allow_task_delegation`, and `task.delegate` on the Home's Task as the foreign resource `{home}/tasks/{id}`. Only `task_delegate` names a Task other than the Run's own, and only one the Home records as this grant's output (it asks with the scoped command `created_task` before taking its lease); that Task is judged with its own attributes (`source_thread_id`, `source_task_id`, `source_parent_task_id`), never the admitted Task's. Any other id is answered like a Task that does not exist. The Home judges the chain by its own policy: the root person, every Agent of the chain its policy knows, and `federation.execute` on the target Node; it strips only the new receiver's executor when it does not know it. An earlier receiver's executor therefore must be an Agent subject of the Home's policy, as it already must for the Home to admit a Task that executor created; otherwise the hop is refused. Each target decides by its own Node Registration, Peer Mapping, policy and Catalog Approval.
- There is no depth limit (ADR 0143). A Run admitted from another Node, or working a Task another Node's Run created, already runs under the strictest Data Protection policy (ADR 0104). Results reach the Home directly from every hop; a Run fetches Thread Attachments only from its direct Home.

### `run_queued`

When a remote Run first enters its worker, the receiving Node joins the Working Area queue for its Home Node, Home Thread, Agent and Home person. The queue follows receiver entry order. If awake Runs are ahead, the receiver commits the queue position and releases the worker lease with reason `execution.queued`. The queued Run keeps its phase and spends no attempt or budget. It has no Stint, backoff or wait deadline. An eligible Run takes its turn through the existing recovery scan. A Run that falls asleep gives up its turn and rejoins at the tail when it wakes (ADR 0140). The receiver admits a new Run only when fewer than 100 awake Runs occupy the Area queue. Overflow fails the Home Task with reason `QUEUE_LIMIT`, recorded on the Home Thread as `remote_run.failed`.

The receiving Node makes one informational request on first queueing. It signs `POST /federation/v0.1/scoped/execution/commands` as the Node, with `grant_id`, `admission_id`, `operation: "run_queued"` and `data: {"task_id","run_id","runs_ahead","queued_at"}`. Both IDs must match the live admission, `runs_ahead` is an integer from 1 to 99, and `queued_at` is an RFC 3339 timestamp. The Home rechecks the grant's authority, uses the bound Task to find its Thread, and writes a single `remote_run.queued` event with `task_id`, `remote_run_id`, `runs_ahead` and `queued_at`. The reply is `{"recorded":true}`. A replay returns the recorded reply and a changed payload conflicts. A lost notice does not affect Grant renewal or the Task. The receiver sends no retry and sends no notice when a Run rejoins after sleeping.

The queue does not detect cross-Node wait cycles. The Home can cancel a waiting Run through the existing controls.

### Node events

`POST /federation/v0.1/scoped/duty-events` with `{"trigger_id":"<uuid>","payload":<JSON>}` delivers an event to a `node_event` Duty Trigger of a Duty on the receiving Node (#1013, ADR 0142). The sending Node and subject come from the Federation token, never the body. A sender of the receiving Node's Tenant names its subject, whom the Tenant's policy must allow `node.use` on the receiving Node; a sender of another Tenant needs this Tenant's Node Registration, and, when it names a subject, that subject's Peer Mapping. A sibling Node that names no subject is refused with 403. Per Duty, a trigger that names the sending Node (`from_node`) takes its events. An open trigger takes only a same-Tenant sender whose subject may read the Duty (its Requester or successor, `thread.read` on its Thread, or `duty.administer`), and never another Tenant's Node. An unknown trigger, a trigger of another kind or Node, one that names another sender, and an open trigger the sender may not use answer 404 alike. The payload is at most 256 KiB of JSON and is recorded as `{"from_node": <sender>, "data": <payload>}`. The answer is the event's receipt, `{"event_id","outcome"}`, as for a webhook. The event is input, never a requester: the Duty starts or wakes its Task as its own Requester, and the sender's identity never reaches that Task's authority.

The builtin `duty_notify` (`core.duties@1`) sends one such event from a local Run's Node: to a Node of its own Tenant as the person at the root of the Run's chain, and to a Node of another Tenant as the Node itself. It is a Tool call approval of that person; a Run that arrived through Federation or whose chain starts at no person sends none. The receiver does not deduplicate events.

Remote workers retrieve thread metadata with `snapshot_thread`, then use `snapshot_page` for the `tasks`, `artifacts`, `events` and `messages` collections. Each page has `items` and an optional UUID `next` cursor, supplied as `after` on the next request. Pages contain at most 32 records and 3 MiB of serialized data, below the 4 MiB limit on a Federation response. Tasks and artifacts are paginated to completion; events and messages retain their latest-100 observation window. An individual resource larger than 3 MiB is rejected explicitly. Collection reads observe current home state and do not provide a transaction snapshot across pages.

A Run executing for another Home sends files back to that Home through three admission-bound routes, not through `file-transfer/1`, whose sender must hold the Thread (ADR 0139). Each request is signed by the executing Node, acts as the Node (no `subject`) and carries `grant_id`, `admission_id` and the Run's call `key` (1 to 512 bytes). The Home admits it as it admits a Home command: the Remote Grant must be live, its Binding must name this admission, the Home's own Federation Tool Request for the executing Node (with the grant's Run-only answers) and the Run's Binding must both hold the tool the Run called (`file_share`, or `thread_message` and the key of its files, `thread_message:files`), and the task must be the executing Agent's and unfinished. A tool the Request does not name answers `400` with the words of a tool that does not exist. It then derives everything else from its own records. The target is never in the request: a `file_share` lands in the Working Area of the Run that delegated (the latest Run of the grant task's parent that is the chain's delegating Agent), and a message with files is posted for the executing Agent in the grant's Thread, its files staged in no Area. A `file_share` under a grant that no Run made, or from a delegating Run without a Working Area, answers 404 `DELIVERY_NO_DELEGATING_RUN` or `DELIVERY_NO_DELEGATING_AREA`; a message needs neither.

| Endpoint                                    | Body                                                                | Reply                                        |
| ------------------------------------------- | ------------------------------------------------------------------- | -------------------------------------------- |
| `POST /scoped/execution/deliveries/prepare` | `grant_id`, `admission_id`, `key`, `purpose`, `files`               | the staged delivery, `state` `staging`       |
| `POST /scoped/execution/deliveries/chunks`  | `grant_id`, `admission_id`, `key`, `tool`, `file`, `offset`, `data` | the staged delivery, `state` `staging`       |
| `POST /scoped/execution/deliveries/commit`  | the body of `prepare`                                               | the delivery, `state` `committed`, `receipt` |

`purpose` is `{"kind":"file_share"}` or `{"kind":"message","content","descriptions"}`, where `descriptions` maps an attached path to its Image Description; `files` lists `path`, `digest` (lowercase SHA-256 hex), `size` and `media_type`. A file share keeps the Deployment's share limits (at most 64 files, 100 MiB each, 256 MiB in all); a message keeps a local message's (at most 8 files, 8 MiB in all, every image described). A chunk is at most 4 MiB, a multiple of 4 MiB from the start of its file, base64 encoded in a body of at most 6 MiB. The delivery is named by the admission and the key, so every request may be repeated: `prepare` of a delivery that exists answers from it and refuses a changed manifest or purpose (409), a chunk that is staged again must carry the same bytes, and `commit` of a committed delivery returns its receipt. A file share stores the files as a delivered share in the Working Area as `file-transfer/1` does (paths `<delivery id>/<path>`, a `received_scope` constraint only); a message stores each file as a Thread Attachment under a key derived from the delivery, then posts the message once. Staged bytes reserve quota and expire with the delivery. The receipt lists, in order, each file's `digest` and `size`, which the executing Node checks before it tells the Run its files arrived.

Agent memory is separated by agent ID, version, thread UUID and thread home Node. Existing local memory retains its local namespace; a remote Run cannot read or overwrite it by reusing a thread UUID. A dependent run fails with the dependency ID and terminal status when a prerequisite is failed, cancelled or abandoned.

### Deployment Key rollover

A normal rollover publishes one successor, activates its signing 24 hours later
and ends predecessor verification seven days after activation. Only one normal
rollover may run at a time. During the half-open overlap
`[activates_at, retires_at)`, a successor-signed request carries one compact
`transition` proof signed by the receiver's currently trusted key. The complete
Federation token, including this proof, must fit the existing 8192-byte limit.
The outer token still has its 60-second lifetime and exact method, target, query
and body binding. A proof alone admits no request.

Receiving Node Registrations keep cryptographic trust separate from
authorization. Persist advancement only after the full outer token,
Deployment/Node identity, exact mapping and current authority
checks succeed, with a compare-and-set against both trust and authorization
revisions. A crypto-only update preserves permissions, enabled state,
authorization revision, administration actor/timestamps and Graph cursors.
A conflict requires reload and revalidation; it must not overwrite newer trust.

Request-carried verification needs no public-identity HTTP fetch. Publishing a
public key alone never changes receiving trust. A receiver that misses the overlap
needs independent manual confirmation, as does replacement after a lost or
compromised key. Each record's existing administrator controls explicit quarantine
and manual re-pin. Normal proof cannot clear quarantine or enable a disabled
record, and confirmation does not itself enable one. Independent receivers cannot
be promised immediate incident notification or revocation.

For the exact closed-Node durable recovery described below, learning a successor
requires the participant, immutable manifest and caller to be revalidated in the
same transaction as the trust compare-and-set. It grants no new authority and
retains the normal proof expiry: no historical-key fallback or expiry waiver.
See [ADR 0084](adr/0084-normal-deployment-key-rollover-is-proven-by-the-currently-trusted-key.md)
and [normal rollover operations](decisions/nodes-and-federation.md#16-the-deployment-key)
for the existing File/KMS commands and schedule.

## Node deletion and recovery

A deleted Node refuses ordinary reads, access and work at the next durable
boundary, including new Reserve and Prepare. Deletion snapshots the exact work
identities and durable stop intent in its transaction; mutable Run timestamps
cannot reconstruct that set later. Restore keeps the stop intent, fences old
leases and pending work, and never restarts canceled Runs. Preserved data reopens
only under current Tenant policy and current credentials and registrations.

Only exact, already-admitted immutable durable finish/decision/stop cleanup may
continue through a closed Node. Revalidate its participant, manifest and caller;
this exception cannot create work, reserve new authority or undo a decided atomic
commit. It is not a general access exemption for a deleted Node or disabled
registration. See [lifecycle management](authorization.md#tenancy-rules-tenant-creation-and-invitations)
for default switching, grace, capacity and restore restrictions.

Purge removes Node-owned data and preserves sibling Nodes and Tenant-shared
Registry definitions, Provider Credentials, Tenant Secrets, user subjects and
policy revisions. Retained files remain billable according to the Offering until
physical erasure; deleting the whole Tenant retains its existing exemption.
Execution Pod time ends on actual Pod stop, not the Node deletion request.

Separable audit expires under `audit_retention_days` (365 days by default) after
final purge. Assigned Node names remain permanent. Keep the exact current
registration dependency while a retained durable journal needs it for recovery;
audit expiry does not erase that required trust. Content-free sibling source-read
identities and revisions survive source purge, while the source-deletion owner
invalidates derived receipt bodies. Unknown or contradictory ownership blocks
erasure. See [ADR 0083](adr/0083-node-deletion-is-recoverable-and-releases-capacity-before-purge.md).

## Allocation usage

Pod usage follows one actual provider Pod UID through its complete start-to-stop
lifetime. A recreated Pod has a new allocation identity; retries keep the original
identity, canonical receipt digest and immutable Tenant/start/end facts. Command
or Run completion is not allocation stop. Delayed receipts remain ingestible
after authority withdrawal and replay their frozen facts rather than assigning
historical usage from today's Billing Account attachment.

Account coverage and reporting intervals are half-open. Split complete allocation
time by historical coverage and Tenant-deletion exemptions; Node deletion alone
creates no exemption. Storage sampling includes explicit zero-byte samples after
physical purge so an active Tenant cannot retain stale nonzero usage. See
[retained usage facts](operations/usage-facts.md) for precision, replay, retention
and account-period queries.

The accepted [Live preview contract](adr/0085-live-preview-pods-use-the-execution-pod-time-meter-until-they-stop.md)
uses this same meter until actual stop, including time after Run completion.
Generic allocation-meter evidence does not establish actual preview runtime
acceptance (Issue 405), Kubernetes/gVisor acceptance or live Stripe acceptance.
The [Stripe Sandbox experiment](operations/stripe-billing.md) remains blocked on
Sandbox access and actual Checkout; no billing model, launch price, currency or
processor export model is selected by these usage facts. Full remaining-Issue
acceptance, including the separately owned PR 478 restore/Plan Connection repair,
remains tracked in the [completion plan](decisions/issue-244-completion-plan.md).

## Persistence and delivery

Semantic failures preserve their typed cause in Run recovery and the Home operation journal. `durable_memory_budget` means that a native-memory operation has spent its cumulative call, token or cost budget across Retry Cycles. It is permanent for that operation: the Run pauses, its Task is not failed, and recovery offers only `create_follow_up_task`.

For this cause, an error response carries HTTP 409, `X-Aidash-Semantic-Reason: context_budget` and `X-Aidash-Semantic-Cause: durable_memory_budget`. Updated peers prefer a recognized cause header, then fall back to the reason header. Earlier peers read the established `context_budget` reason, which also offers only a follow-up Task. Other causes retain their existing reason header. An unknown persisted journal error fails closed as `context_budget`, so it neither authorizes an unchanged retry nor discloses a saved receipt.

Remote execution status and control replies use the same projection in JSON: `semantic_reason` retains `context_budget`, and the additional `semantic_cause` carries `durable_memory_budget`. Updated peers restore the exact cause when decoding the reply; earlier peers ignore the additional field. An unknown detailed cause falls back to the compatible reason.

API startup and PostgreSQL worker recovery do not wait for NATS. The event-bus supervisor reconnects independently; federation offer retries also run independently of the broker.

A state mutation and its event are committed together. The event table doubles as an outbox. Its publisher waits for a JetStream acknowledgment, then records publication. A crash in between can replay an event, using its event ID as `Nats-Msg-Id`. Inbox uniqueness and task/run uniqueness remain the permanent deduplication boundary after JetStream's short duplicate window expires.

When a CloudEvent plus its NATS headers exceeds the broker payload limit, the publisher omits `data` and includes a `dataref` URI such as `/api/events?after=123`. Resolve this URI against the source Node using an authorized API credential and match the event ID in the returned replay page. All CloudEvent identity and sequence fields remain intact; the full event stays in PostgreSQL. Small events retain their inline `data`.

Event sequence allocation is serialized through a transaction advisory lock so commit order agrees with the SSE cursor. `/api/events/stream` honors `Last-Event-ID`; `/api/events?after=N` provides JSON replay and answers `{"events": [...], "unconfirmed": [...]}`, with the cursor to continue from in the `x-aidash-event-cursor` header. The stream can skip an event whose Read dependency lies on a Node that did not answer, says nothing about it, and does not send it again while the client stays connected; a reconnect reads forward only, and `GET /api/events?after=` checks the skipped events again (ADR 0133). Holding the cursor until the Node answers would stop every stream for one Node's trouble. A disconnected or slow client can resume from the PostgreSQL event log. SSE is an observation channel, not the worker's durable queue.

The Thread's home Node owns task revisions and artifacts. A remote Node owns its run journal and tool invocations. Ordinary federation commands operate on the home API and use stable keys. When a reply is lost, replay either returns the original result or reports a conflict for mismatched input. These ordinary commands do not form a cross-Node atomic transaction. The separate [distributed transaction protocol](transactions.md) implements durable prepare/commit/abort, participant decisions and visibility barriers for admitted mutations. Its subject-scoped authorization, complete process/network failure matrix and Kubernetes release acceptance remain open in #40. External Tool and nonparticipating A2A effects cannot join an atomic manifest.

### Inference Progress

Every provider call of a Run step is an Inference Attempt. The worker records it under its lease before calling the provider. Starting an attempt first closes any attempt of the Run that is still pending as `interrupted/lease_lost`. Each attempt ends with exactly one outcome that never changes: `accepted` is written in the same transaction as the `model.completed` save; `discarded` when a newer Run input supersedes the request, including a stale save, which is rolled back and then discarded under the same lease; `interrupted` with reason `cancelled`, `lease_lost`, `stall`, `stream_error` or `revoked`. When a Run is cancelled, paused, failed, completed or reconciled without its worker, the server closes its pending attempt in the same transaction, as `cancelled` when control is Cancelled and `lease_lost` otherwise. A management pause lets the in-flight call finish, so it leaves the attempt to its worker; if that worker's lease lapses while the Run is not scheduled, such as while paused, the next maintenance pass closes the attempt the same way.

Models stream inference by default: a model config that omits `streaming`, or sets it to `null`, requests a server-sent event stream, and `streaming: false` opts out to the non-streamed request. A 2xx answer to a streamed request that is `application/json` instead of `text/event-stream` is accepted as a whole non-streamed completion without progress; any other content type fails the attempt.

Progress is tentative and display-only. A streaming adapter offers only text and tool-call status (index, ID, name and argument size), never partial arguments or reasoning, and only a complete, validated response can enter ToolCall state. The worker flushes progress every 250 ms or 4 KiB into `inference_progress`, apart from the event journal and without its advisory lock. Rows carry a per-Run `progress_seq`, which the lease-fenced writer allocates under the Run row lock and never reuses. A row holds at most 16 KiB of item data. Fifteen minutes after an outcome, a bounded maintenance pass deletes the attempt's text and tool-call rows; its `started` and `outcome` rows remain as long as the Run. Only the low-rate markers `inference.started`, `inference.accepted`, `inference.discarded` and `inference.interrupted` enter the event journal, with `{run_id, task_id, thread_id, attempt_id, reason?}` and the same thread rule as the Run's own events. They reach `/api/events`, the event stream and NATS, but the dashboard projections (the `events` of `/api/state` and of thread snapshots) omit them: each would otherwise cost every projection a full Run visibility check, and the Run's own events already carry its state. The dashboard does not refetch on a marker frame. Attempt IDs are public; the worker lease token is never exposed.

`GET /api/executions/{run}/inference/stream` delivers progress over SSE. `Last-Event-ID` or `?after=N` resumes after a `progress_seq`; without a cursor the stream starts at the latest attempt's first row. Each frame has `id: <seq>` and `event: inference.delta` (text), `inference.tool_call` or `inference.outcome`; a `started` row is an `inference.outcome` frame whose item is `{"outcome":"pending"}`. Frame data is a CloudEvents 1.0 envelope with type `aidash.inference.progress.v1`, `subject` set to the Run ID and `data` of `{attempt_id, seq, kind, item}`. When the cursor precedes pruned rows, an `event: gap` frame without an ID carries `{attempt_id, outcome, from, to}` before the attempt's outcome row. The stream polls every 250 ms while an attempt is pending and otherwise waits for marker wakeups or the reconciliation interval. Like the event stream, it is an observation channel; a response is durable only when accepted.

The dashboard's run details subscribe to this stream while a local Run is open and not terminal, and resume from the last delivered `progress_seq` after a close. A pending attempt keeps the subscription after the Run turns terminal until its outcome arrives, because the Run query can observe the transition before the stream delivers the outcome written with it. They show the latest five attempts. A pending attempt shows its streaming text, labelled as tentative, and the name and argument size of each tool call. An accepted attempt's text is hidden, and the Run is refetched to show the durable response. Discarded and interrupted attempts collapse into struck-through text with their reason. A gap shows a one-line notice that some progress expired. Text is kept per attempt up to 1 MiB, and replayed seqs are ignored. A polite live region announces only phase changes. Tentative text never enters forms or artifacts.

## Execution and effects

Workers lease one run at a time with a random lease token and a 30-second expiry. They renew every 10 seconds and fence state and tool-result writes by token and expiry. A stale worker cannot advance a new owner's journal. A process crash leaves the last persisted phase, model response and tool-call cursor available for recovery.

The tool invocation record is committed before sending the request. Each tool call has a key derived from run ID, inference step and call index. Its result is committed before advancing that cursor. Replayed completed invocations return the stored result. Task completion and its final artifact are one transaction with an idempotency key and input consistency checks.

For an external effect, exactly-once behavior requires cooperation from the target. An HTTP tool declared `idempotent` must honor `Idempotency-Key`; MCP tools must honor the configured idempotency argument. Declaring an adapter idempotent does not make an arbitrary server idempotent. For `unsafe` adapters, a missing result after an attempt is treated as uncertain and the effect is not repeated. A Deployment Administrator verifies the external outcome and answers the confirmation with `{"result": ...}`. This restores the journal without executing the effect again.

Controls are cooperative at persisted step boundaries. Pause lets an in-flight call finish and persist before stopping the next step. Cancel cannot undo an already performed effect. Human questions and answers are durable. A normal human answer returns the agent to inference; remaining tool calls from the pre-question response are discarded, so a rejected human approval cannot release a queued effect.

A call of a Tenant's HTTP or MCP tool that needs approval raises a Tool call approval, a capability approval of kind `tool_call` decided by an Eligible approver: the requester alone when the policy lets them approve it, otherwise whichever Eligible approver decides first (ADR 0135). A later decision gets 409 with error code `APPROVAL_ALREADY_DECIDED` and `details` naming `decided_by` and `outcome` (`allowed` or `denied`). The Run waits for it in `WaitingState::Approval`. The approver lists it with `GET /api/capabilities/approvals` and decides it with `POST /api/capabilities/approvals/{id}/decide` and `allow_once`, `allow_run`, `allow_always` or `deny`. `allow_always` also creates an allow Approval Rule for the person the Run's delegation chain starts at. It expires after the Deployment's `approval_seconds` (86400 by default). An `allow_run` grant covers one operation of the tool until `grant_seconds` (3600 by default), and new user input does not end it. A page reading grant is narrower: it covers one host and ends at the Run's next user input. Direct permission, the `tool.call_unapproved` action in the Tenant's policy, lets the call proceed without an approval, and so does a Read-only operation, which the tool's definition version declares with `replay: "read_only"` (ADR 0078). Before the approver is asked, the Approval Rules and Approval Mode of the person the Run's chain starts at may give the approval in advance (see [Approval Rules and Modes](authorization.md#approval-rules-and-modes)). In Auto, the Tenant's approval Decider judges the call instead of a person, as [Approval hook](decision-gate.md#approval-hook) describes. An allowed call runs at once, without another model response. A blocked call gets `{"error": "tool call was blocked by Auto: <reason>"}`, where the reason is `data_exfiltration`, `destructive`, `beyond_request`, `injected`, `boundary` or `unavailable`. Generation requests are not judged, so they still wait for a person. On a Run that executes on its home Node in Auto, the model is also told not to stop to ask clarifying questions unless the person's input relies on one, and to make a reasonable assumption and continue. Every approval kind resumes the exact approved call, so the model never repeats it. A denied, expired, revoked or closed approval refuses the call. The model then sees `{"error": "tool call approval was denied"}`, or the matching text (`tool call approval expired`, `tool call approval was revoked`, `tool call approval was closed`), and the Run continues. A call the Tenant's policy refuses, or that no person may approve, gets `tool call was refused by the Tenant's policy` or `tool call approval was refused: no person is eligible to approve it`. An Approval Rule that denies the call, or the person's Don't ask mode, gets `tool call was refused by an Approval Rule` or `tool call was refused: the person asked not to be asked`. A Run executing for another Node still asks its Home Node through an `APPROVAL_REQUIRED` human request that expires after 15 minutes, because the approver is at that Node (ADR 0075). Approval Rules and Modes never cover such a Run (ADR 0076). Pending Tool call approvals close with `closed_reason: run_ended` when their Run ends.

Transient execution errors have a bounded retry budget per persisted operation. A successful operation resets the counter. Exhausted or permanent failures persist a `terminal_transition` intent in a waiting run. The worker retries home-Node delivery across outages and restarts and only finalizes its local journal after the home acknowledges that outcome. It does not re-execute the failed tool while delivering this intent.

A coordinator's final response remains gated while a child is unresolved. A Tenant's subject can call `POST /api/tasks/{id}/abandon` with the current revision and a nonempty reason for a failed, blocked, or cancelled child. Nested children must be completed or abandoned first. The home records `ABANDONED` with the prior status and reason; a resumed parent can finish with remaining artifacts. Dependencies still require actual completion: abandonment does not satisfy a task dependency.

### Context compaction

Context recovery works on the model-visible Context Projection and keeps the original events in the Context Journal. It does not invoke a compaction Decider (ADR 0125).

The compaction trigger, post-compaction fit check and final preflight use the
same complete-request estimate, the Request Estimate (ADR 0017). Its upper
bound is the byte estimate: the UTF-8 bytes of the serialized model-visible
messages and tool definitions (the byte basis, including the JSON escaping of
context inside message text and the cache-salt reserve), plus the media
allowance, `max_output_tokens` and 1,024 framing tokens. It is not the
provider's actual token count.
Output fits inside the registered context window; it is not extra input space.
Recovery saves offloaded or summarized projections as it progresses. When no
eligible reduction can fit, it prevents inference I/O and pauses the Run with
the typed reason `context_unreducible`.
New model registrations capture OpenRouter's advertised maximum completion
tokens as `max_output_tokens` for ordinary steps; model versions created before
this field existed retain their previous output allowance until re-registered.
An optional `provider_max_output_tokens` declares the provider's independent
completion ceiling. Summary output uses its own policy and one eighth of the
window, capped by that ceiling when present; it does not inherit the ordinary
step budget.

A model definition may set `reasoning_passback: false` to stop a route from
recording and resending the provider's reasoning (#942). Omitting it means
`true`. A Run's model version is immutable, so the choice holds for the whole
Run; only Native Runs record reasoning.

A model definition that sets `request_estimator: calibrated` scales only the
byte basis. The ratio is the nearest-rank 99th percentile of reported
`prompt_tokens` per byte of basis over the newest 200 samples of the same
upstream model slug and Projection Version, plus a 10% margin. The media
allowance, output reservation and framing are added unscaled, and the result
never exceeds the byte estimate. With fewer than 20 samples the estimate stays
the byte estimate. Omitting the field uses `calibrated`. Samples come only from complete, media-free Inference Attempts
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

The model's pinned thread and `workspace_observe` use the same bounded view.
Observations include goal/task previews, artifact IDs and metadata, message
previews, and event IDs/kinds/timestamps. Event payloads are never automatically
embedded, so observing an earlier `tool.completed` event cannot copy its full
observation result into the next request. Durable snapshots, invocation records
and audit APIs retain their original content.

`workspace_observe` accepts optional `offset` (default 0) and `limit` (default 20,
maximum 50). Each collection reports its total and next offset. Events/messages
are newest first; tasks/artifacts retain snapshot order. These are live pages,
so refresh after concurrent changes. `workspace_read` selects an accessible
`thread`, `task`, `artifact`, `message` or `event` by exact ID, through the
same authorization-filtered Home snapshot. It returns JSON text in Unicode
character ranges: `offset`, `max_chars` (default 8,000, maximum 16,000),
`total_chars` and `next_offset`. Concatenate chunks in offset order to recover
the full record. Missing and inaccessible IDs return the same error. Reading
mutable records across concurrent changes requires restarting the read.

On replay, legacy `workspace_observe` results are projected into this view in
the working context. This changes only observation representation; human
records, other tool results and the underlying audit journal are preserved.
The conversion commits only when the complete request fits.

### Context recovery

Every history event a Run records receives a sequence number and is appended to
the `run_context_events` Context Journal no later than the transaction that saves
the Run. Results selected for offloading are journaled before their marker is
saved. The history sent to the model is a Context Projection of that journal:
offloading and summaries change the projection, never the journal. Runs
saved before the journal existed import their current projection, marked
`imported`, on their next save or, at the latest, before their first offload or
summary, because earlier projection changes may already have removed
events. Message-read and inference coverage, semantic and authorization read
journals, and pending execution state are kept outside the projection, so
compaction cannot make an unread source look read or forget a revoked
dependency. The journal is internal; it is deleted with its Run and is not
exposed through the public API. The inspection API returns each history event
with its `seq`, the `journal` cursor and any `execution_summary`.

Every Agent uses Context Recovery. An immutable `context_policy` can override
its defaults; omitted or null settings use the same pipeline on Native, Ordered
and Legacy projections.

| Field                      | Default | Bounds    | Meaning                                                          |
| -------------------------- | ------- | --------- | ---------------------------------------------------------------- |
| `preserve_recent`          | 6       | 6–1000    | Recent events or Model Turns protected when their size permits   |
| `offload_target`           | 0.7     | 0.5–0.9   | Target fraction of the window for a batch of Offloaded Results   |
| `protected_tail_fraction`  | 0.25    | 0.05–0.5  | Maximum window share of the protected recent tail                |
| `reread_immunity_steps`    | 3       | 1–100     | Steps during which a read-back result is protected               |
| `reread_summary_threshold` | 3       | 2–100     | Repeated reads that request a Summary Stage and notify the Agent |
| `overflow_retries`         | 2       | 0–4       | Retries after a proven provider Context Overflow                 |
| `overflow_shrink`          | 0.75    | 0.5–0.9   | Factor applied to the effective window per retry                 |
| `summary.max_tokens`       | 4,096   | 256–4,096 | Summary output limit, capped by the window and provider ceiling  |
| `summary.call_budget`      | 8       | 1–64      | Summary calls per Run, including failed attempts                 |

Large tool results are journaled before their projection becomes a short
Offloaded Result marker. The marker retains the first 300 characters of the
result as data, identifies its Journal sequence, and points to Journal Query
or its fallback; the original call remains paired with it.
If the request still does not fit, older results are offloaded in a batch. The
latest Native Model Turn stays intact, and the protected tail is bounded by
size as well as count. The Context Journal remains the source of the original.

If offloading is insufficient, the Summary Stage uses the Agent's own pinned
model and ordinary inference authority and allowance. Code copies the goal from
the Run's pinned Task and people's words and observed outcomes from the Journal;
the model supplies only
per-call labels and prose. Unsupported labels are dropped, while omitting an
item of a previous summary rejects the merge. Each adopted merge records its
source ranges and dependencies and normally must reduce the projection; several
merges can progressively fit a request. A repeated-read summary may grow a
request that still fits. Summary requests are recorded before provider
I/O. Source authority is checked before dispatch and again before adoption; a
revoked summary is discarded and its Journal originals restored. A request
pauses with `context_unreducible` only when no eligible reduction can fit it.

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
requests from every Agent, including its Summary Stage requests, set
`transforms: []`, so the provider never compresses them on its own.

Metrics: `aidash_context_compaction_total{stage,outcome}` counts offload and
summary outcomes (`applied`, `insufficient`, `unavailable`, `invalid`,
`unauthorized`, `revoked`), `aidash_context_compaction_tokens{stage,phase}`
records estimated sizes before and after, and
`aidash_context_overflow_total{outcome,estimator}` counts `retried` and
`exhausted` overflow recoveries by the estimator (`bytes` or `calibrated`)
whose request overflowed. Labels never contain Run identifiers or content.

## Marketplace and localization

The Deployment is a self-hosted package repository. A manifest includes the versioned entity, author, permissions and dependencies. Publication is immutable. Installation verifies the SHA-256 digest, validates dependencies and atomically registers the entity with local installation configuration. This version distributes declarative manifests; it does not execute downloaded code, operate a public SaaS marketplace or handle payments.

UI strings live in `web/src/locales/en-US.json` and `ja-JP.json`. Entity names and descriptions are localized maps. Agent language capabilities are separate metadata used by local and remote discovery. Message translation is not automatic.

### Bounded inspection and retry identifiers

`GET /api/tasks?offset=N` returns up to 500 authorized tasks, newest first, with
`next_offset` for the next page.
`GET /api/state?node=<name>` returns candidate windows for the selected Node;
without `node`, the windows cover the Tenant. Threads, Tasks, Runs, Artifacts,
pending Human Requests and Conversations each scan at most 50 candidates. Runs
are ordered by `updated_at` descending; the other windows use `created_at`
descending. Ties use descending IDs. The response keeps each collection as an
array and adds `next_cursors` with a cursor or null for each collection. A cursor
names the last scanned candidate, including a candidate hidden by authorization.
Filtering can produce short or empty pages with a cursor; a full page can lead
to an empty final page. Returned Threads include the Thread window, the
Threads of all returned items and the Threads of its `unconfirmed` entries.
`unconfirmed` counts only the candidates these windows scanned.
`GET /api/threads?node=<name>&cursor=<cursor>`
continues the Thread window and returns `{items, next_cursor}`. Deployment
Administrators receive empty state collections and null cursors.
`GET /api/tasks/{id}/executions`, `GET /api/threads/{id}/executions` and `GET /api/human-requests` return `{items, next_cursor, unconfirmed}`
in newest-first order, with up to 50 candidates per page. Supply the opaque `cursor` to
continue; visibility filtering can leave a page short or empty even when it has
a next cursor. A full candidate page can also lead to an empty final page.
The Thread Run list orders by last update and accepts `live=true` to omit terminal Runs; it requires `thread.read` and checks each Run's visibility.
Human Requests accept `run`, `node`, and `state=pending|answered|all`;
the default is `pending`. An unanswered request's `answer_by` comes from the
owning Run's waiting state. Run creation times for records predating the
creation-time migration are approximated by their last update at migration. The
cursor currently encodes the last scanned candidate ID. Each candidate Thread
is checked for read access before the item is disclosed.
Run visibility still checks its dependencies, including every Human Request of
the Run; the Human Request list reuses each Run decision within the page.
`GET /api/executions/{id}?offset=N` returns at most 100 invocation previews; request the
next offset when a page is full. Inputs/results over 1 KiB are represented by an
explicit `truncated` marker and text preview. Durable execution records retain
original values. Credential inventories use `?offset=N` pages of 200 records.
Other scoped collections fill pages after authorization checks.

Scoped federation discovery returns `entries` and `next_offset`, with at most 64
entries and 3 MB of metadata per response. Observation returns at most 100
runs, human requests and invocation previews; run context and pending payloads
remain available through the authorized run detail API.

Run-message and remote message controls accept a UUID `idempotency_key`. Reuse
that key when retrying an ambiguous request. The dashboard keeps it until that
submission succeeds. A stream request with `Last-Event-ID: -1` starts from the
current high-water mark; the dashboard refreshes state after the connection is
established and then resumes from acknowledged event IDs on reconnect.

Optional thread snapshots are reduced to the selected model's available
context budget before history compaction, with a `snapshot_truncated` marker.
The durable thread remains intact and tools can retrieve omitted details.
Automatic semantic retrieval skips budgets too small to contain provenance.

Agent registration rejects instructions, referenced skills and tool definitions
that cannot fit the selected model window with output and context reserves.
Under `deferred@1`, registration instead measures Mandatory exposure, Eager tool
definitions and eager Skill blocks plus the full `metadata_bytes`,
`schema_bytes` and `skill_bytes` budgets. It also rejects any single tool
definition over `schema_bytes`, or any Registry or attached Skill block over
`skill_bytes`, naming the alias and size.

### Unconfirmed items

A list, search or Agent tool result that leaves out an item because a Node holding one of its Read dependencies did not answer says so (ADR 0133). `GET /api/state`, `GET /api/tasks`, `GET /api/tasks/{id}/executions`, `GET /api/threads/{id}/executions`, `GET /api/human-requests`, `GET /api/threads/{id}`, `GET /api/threads/{id}/message-history`, `POST /api/threads/{thread}/memory/units/query`, `POST /api/threads/{thread}/semantic/search`, `GET /api/threads/{thread}/semantic/entries` and `GET /api/events` carry `unconfirmed`, always present and `[]` when empty:

```json
"unconfirmed": [
  {"node": "aidash://local/brave-otter-k3m9q2xa", "kind": "task", "thread_id": "…", "count": 3}
]
```

`kind` is one of `task`, `run`, `artifact`, `event`, `message`, `human_request`, `semantic_entry` and `memory_unit`. `count` is the number of rows this response scanned and could not confirm, so a later page can report more. It is not a total, and it is not "you should see these": the Node can still refuse an item once it answers. A row counts once, under the first silent Node its check reached, only after the reader's own permission to read that kind in that Thread and `federation.discover` on that Node have passed. A real refusal is never counted: where a check that can meet a silent Node is followed by others in a chain (a Run's `run.read`, `memory.read` and Human Requests after its Task, an artifact's provenance after its Task, the other evidence of a memory unit), the rest of the chain is judged too, and a row any of them refuses is not counted. An event that sits in no Thread is never counted. A peer's `503 node_unreachable` for a call a Run made through its Home (`{"code": "node_unreachable", "node", "after_effect"}`) stays typed and is retried or paused like a local one. `/api/events`, `semantic/entries` and `memory/units/query` were bare arrays and are now `{events, unconfirmed}`, `{entries, unconfirmed}` and `{units, unconfirmed}`. `ThreadSnapshot` has the field with a default, because peers read it. The federated graph also returns `unconfirmed`, but attributes every count to the answering Node rather than naming the silent Node. It never forwards that third Node’s address. Counts describe scanned rows, not guaranteed visible items; its warning is separate from the display-limit notice.

`GET /api/tasks/{id}/executions` with a parent Task that cannot be confirmed stays `503 node_unreachable`, and so does a message-history `before` message that cannot be confirmed; both are the read of one thing. Only their rows are counted.

A list, search or event read does not ask a Node again for 30 seconds after it did not answer, per Tenant and Node in each process, so a poll stops spending up to 10 seconds with a database connection held. An answer from the Node, including a refusal, clears it. A single read, a Read Recheck and a Run's inference stream always ask. After a Node returns, items can stay out for up to 30 seconds.

`workspace_observe`, `workspace_search`, `memory_recall` and `memory_reflect` add `unconfirmed` and `unconfirmed_note` to their result only when something was left out, so an ordinary result does not change. A child Task that cannot be confirmed is not "no pending child": `complete` and the parent's child summary answer `node_unreachable` before any effect, which retries up to 5 times and then pauses the Run like a Read Recheck.

### Remote semantic reuse

The executing Node reserves every remote inference and compaction at the Home
through `POST /scoped/usage/reserve`; the Home calls back the executing Node's
`POST /scoped/usage/verify` before it charges anything. Both carry a `boundary`
of `{"step":n}`, and the executing Node's verify requires a READY semantic operation of
its own for that step. An `ordered` Run reuses one retrieval across steps
(ADR 0020), so a reuse step instead sends `{"step":n,"reuses":"<operation ID>"}`,
naming the operation whose receipt it reuses. The boundary is bound in the
executing Node's durable dispatch record, so the Home cannot substitute it in the
callback. The two Nodes check the citation against their own journals:

- the executing Node: the operation is READY for this Run and grant, from a
  step no later than the Run's actual step;
- the Home: the same against its journal for the step the executing Node claims, the
  grant is still live (`403`), no consumed source changed or was revoked
  (`409`, reason `invalidated`, the same conditions under which
  `grants/describe` answers 409), and the Thread corpus digest equals the
  one it recorded before that retrieval (`409`, reason `stale`).

A `stale` answer makes the Run drop its cached retrieval and retrieve again
after one second, up to five times in a row.

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
