# Harness-managed core capabilities

Date: 2026-09-25
Specification for: [Aidash #43](https://github.com/kent8192/aidash/issues/43)
Status: Written design for review; not an implementation or release claim.

## 1. Outcome and scope

An authorized Agent can search and read its files, run Shell commands and stateful Python, apply structured patches, and discover and load Skills without creating Registry capability records or using `plugin_N` aliases. It can continue work in the same thread, explicitly share stable inputs with another authorized Agent, and transfer those inputs between authorized nodes.

Codex is the behavioral reference, not a dependency on its executable, private Cloud infrastructure, model provider, or complete wire protocol. Aidash retains its durable Runs, current authorization checks, independent node ownership, and third-party Registry integrations. Web-search providers remain in [#44](https://github.com/kent8192/aidash/issues/44).

Section 2 records settled product requirements. Sections 4–12 are the concrete implementation proposal; their component names, endpoint shapes, runtime choice, and numeric defaults are engineering choices for review, not additional decisions attributed to the user. They must not weaken Section 2. Acceptance cases in Section 13 bind the two together.

## 2. Settled requirements

| ID | Required behavior | Acceptance |
| --- | --- | --- |
| R01 | File search, Code interpreter, Shell, Apply patch, and Skills are Harness-managed. Only enabled capabilities appear to the model, and dispatch rejects disabled capabilities even if called by name. | AT01, AT02 |
| R02 | Working files belong to a thread and Agent, survive successive Runs in that scope, and are not implicitly shared with other Agents or threads. Current authorization still applies. | AT03, AT04 |
| R03 | The dedicated Interpreter uses Python. Variables and imports survive while its environment is alive; idle environments stop automatically. Recreation explicitly resets memory without deleting saved files or replaying historical code. | AT05, AT06 |
| R04 | At goal completion and during thread deletion, offer an explicit cleanup choice. No answer, goal completion, or idle suspension is not consent to delete files. | AT07 |
| R05 | Separate “整理する（復元可能）” from “作業ファイルを削除する（復元不可）”. The former verifies a time-limited recovery snapshot before removing active files; the latter additionally confirms deletion of active files and Harness-managed recovery copies. Neither silently deletes originals, published artifacts, or independently shared copies. | AT08, AT09 |
| R06 | Route additional-permission requests to the original requester if eligible, otherwise to a designated eligible approver. Delegation preserves requester identity. An upper-policy denial cannot be approved away. | AT10 |
| R07 | Default approval to one invocation. An approver may explicitly grant a bounded, expiring scope for the same Run and Agent. Grants do not pass to delegated Agents or subsequent Runs; expiry and revocation apply to subsequent actions. | AT11, AT12 |
| R08 | Discover directory-based Skills from authorized locations using progressive disclosure. Loading a Skill does not execute scripts or grant permissions. Registry Skill records are unnecessary for new attachments. | AT13, AT14 |
| R09 | Search and patch stay within authorized file boundaries. Search is bounded; conflicting patches do not silently overwrite files. | AT15, AT16 |
| R10 | Shell, Python, and dependency installation have enforced filesystem, process, network, time, and resource boundaries. Implementing a capability never enables it for all Agents. | AT02, AT17, AT18 |
| R11 | An Agent may explicitly share selected files with an identified recipient without per-transfer human approval when both disclosure and receipt are permitted. Shared contents are fixed at sharing time; editing a recipient copy cannot modify the source. | AT19, AT20 |
| R12 | Include authorized cross-node file transfer. Verify identity, both nodes' policy, integrity, and durable receipt; recover retries without presenting partial files or silently substituting newer source contents. | AT21, AT22 |
| R13 | New reference uploads retain original bytes and extracted text. Authorized execution uses working copies, not an editable stored original. Existing text-only references require re-upload to obtain missing originals. | AT23, AT24 |
| R14 | Agents may install additional Python libraries into their own environment from permitted sources. Apply ordinary sandbox and approval rules; do not alter the host or another Agent's environment. | AT25 |
| R15 | User service credentials stay in dedicated integrations, outside Agent-controlled Shell/Python and dependency-build processes. Do not inherit infrastructure credentials or expose secret values in model context, results, or logs. | AT18, AT26 |
| R16 | Preserve immutable Agent versions, existing references and third-party integrations. Migration must not silently remove a Skill, rewrite a published Agent, or grant new capabilities. | AT24, AT27 |
| R17 | Keep durable invocation history and recovery. Completed effects return stored results on retry; an attempted unsafe effect with unknown outcome must not be blindly repeated. | AT06, AT22, AT28 |
| R18 | Follow Codex-style queue/steer behavior for a thread's ordinary Agent execution; retain Run leases as Worker coordination. Do not add automatic concurrent branches and merge UI merely to answer same-thread follow-up messages. | AT03, AT29 |

The [capability baseline ADR](../../adr/2026-09-25-codex-capability-baseline.md), [file-sharing ADR](../../adr/2026-09-25-agent-initiated-file-sharing.md), and [domain vocabulary](../../../CONTEXT.md) provide the durable decisions and terminology. An Agent working area, a shared file snapshot, and a cleanup recovery snapshot are different resources.

## 3. Existing integration boundaries

The inspected development baseline is `e70758ba6a48fc18c07e535f60182a70c8b6f29b`. Existing code already provides a Harness, the `Tool` interface, Run journals and leases, authorization guards, federation, thread messages, Skill import, and personal-reference processing. The new capability implementation should extend these boundaries rather than replace the execution engine.

| Existing boundary | Intended integration |
| --- | --- |
| `src/harness.rs`, `src/tool.rs` | Resolve permitted core tools beside existing built-ins and Registry adapters; keep durable invocation dispatch. |
| `src/context.rs`, `src/provider.rs` | Budget Skill metadata, bounded file results, and execution previews using the same complete-request accounting. |
| `src/authorization/`, `src/collaboration/` | Apply current resource authority to session input, working areas, approvals, and cleanup controls. |
| `src/store.rs`, `migration/` | Persist execution bindings, operation records, resource ownership, revisions, and outbox events. Use SeaORM/SeaQuery conventions. |
| `src/federation.rs` | Add a separately authorized file-transfer protocol without granting remote database or host-filesystem access. |
| `src/skill_import.rs`, `src/knowledge.rs`, `docs/features/skills-first-agents.md` | Reuse import/extraction behavior, add managed original storage and direct Skill attachments, and retain legacy reads. |
| `tests/common.rs` and integration tests | Reuse authenticated Router requests, real PostgreSQL/NATS fixtures, and scripted provider responses as the main behavior-testing boundary. |

These are extension points, not claims that the new filesystem sandbox or cross-node file-transfer contract already exists. A stored Run lease prevents duplicate ownership of that Run; it does not by itself serialize separate Runs that use one working area. Codex's public `turn/steer` behavior provides the input-routing reference [S1].

## 4. Proposed architecture

Keep orchestration and untrusted execution separate.

```text
Authenticated API / thread controls
  -> Session admission and queue/steer routing
  -> Harness + existing Run lease + current authorization
  -> Core capability dispatcher + durable invocation record
       -> managed file and Skill services
       -> sandbox runner control service -> isolated execution environment
       -> authorized file-transfer service -> recipient node API
  -> bounded results / artifacts / durable events / UI
```

The sandbox runner is a service boundary, not a second planner or autonomous Agent. It executes identified operations and reports their state. It never chooses models, approves actions, or grants itself resources. Control-service credentials remain outside the execution environment; the sandbox receives no generic broker or infrastructure credential.

### 4.1 Session identity and input ordering

Identify an execution session by `(tenant, owner_subject, workspace_home_node, workspace_id, thread_id, agent_node, agent_id)`, with a version-independent node-and-Agent identity. The authorization principal still includes the exact version; Agent version is pinned on every Run rather than silently changing inside a running operation.

The initial controller admits one ordinary active Run in that session. Further independent user requests enter a durable ordered queue. An explicit steer carries the expected active Run ID and enters that Run's ordered input stream; it does not create a second ordinary Run. A stale steer returns a conflict instead of being attached to an unrelated Run. Stop, cancellation, and approval responses are control operations and must not wait behind the work they control.

Steer must reauthorize its sender against the active Run and all existing input-disclosure constraints. The implemented initial policy requires the same working-area owner and current Run interaction authority. Thread membership alone never lets a less-authorized participant inject instructions into another requester's privileged Run.

Do not replace current run-message fencing or delivery deduplication. Carry client idempotency keys through queue admission and preserve the original requester across delegation. A waiting Run releases Worker capacity, not session ownership. Detect self-delegation cycles that would queue a dependency behind its waiting parent and report `SESSION_DEPENDENCY_CYCLE` instead of deadlocking. Different Agents and threads remain independently schedulable.

Background Shell processes are not additional ordinary Runs. They retain their operation identity, limits, authorization scope, and working-area mutation ownership until they stop. Completing a model turn does not prove all its processes have stopped.

### 4.2 Working-area and authority binding

Associate one active working area with the session, with an immutable `area_id`, a monotonically increasing `generation`, and a content `revision`. Keep node-local durable storage separate from ephemeral processes. A Worker change does not create a new area; loss of the node's storage is not advertised as recoverable unless an actual recovery copy exists. Route work to the area-owning node; otherwise report storage unavailable rather than silently starting empty elsewhere.

Every Run must independently pass authorization for the files and inputs it can observe. Session identity does not create a union of all previous requesters' permissions. Reuse a live Interpreter only when Agent version, requester/delegation authority, policy revision, and authorized mount set are compatible; otherwise terminate the old environment and start a clean session. A lesser-authorized requester must not inherit prior variables, open handles, mounts, or model history containing inaccessible inputs.

For initial enforcement, bind an area to the intersection of the disclosure constraints of all inputs mounted into it. New outputs inherit that conservative restriction, even when a model claims to have anonymized them. Broader disclosure needs an explicit authorized declassification mechanism; none is introduced here. When the current requester cannot access the existing area's required inputs, block its reuse with an actionable access error. Do not silently delete the area or expose it to make the conversation proceed. This is a conservative implementation rule, not a claim of comprehensive data-loss prevention for arbitrary code.

Existing Runs finish on their pinned Agent version. An explicitly selected newer version may use the same authorized working files after previous execution ends, but gets a new Interpreter and freshly resolved Skills and policy. No automatic Agent-version upgrades are part of this feature.

### 4.3 Mutations and stale execution

Use a working-area execution epoch in addition to the existing Run lease. The runner accepts commands only for the current epoch and reports a unique operation ID. On ownership loss, it must fence new commands and stop old process groups before a replacement writer is admitted. A database journal fence alone is insufficient because an old process might still write files.

In the initial implementation, potentially mutating Shell/Python operations, patch commits, snapshot creation, dependency changes, and cleanup are exclusive for an area. Model-facing reads see a stable area revision; they wait or return `AREA_BUSY` while a writer changes that revision. Log polling, steering, and cancellation remain available. The runner, not the LLM, determines when a process has actually terminated. Failure to prove a writer has stopped blocks takeover and destructive cleanup.

### 4.4 Operation reconciliation

Before an effect starts, persist the invocation identity and immutable input digest, then submit that same identity to the runner. The runner durably distinguishes accepted, running and terminal operations. Repeated submission of an identical ID observes or attaches to the existing operation; different input under the same ID conflicts. Results and output references are persisted outside the untrusted process before the Harness advances its cursor.

Recovery queries the runner and reconciles the recorded operation; it does not call the command again merely because the Worker lost its reply. If the runner cannot prove whether the effect happened, record `uncertain` and use existing human reconciliation. Do not mark Shell/Python replay-safe simply because a local function can be called twice. Integrate operation reattachment into the existing recovery path before granting any replay-safe classification. Separate structured runner-control messages from untrusted stdout so printed text cannot forge a completion or approval event.

## 5. Proposed capability contracts

### 5.1 Common rules

Core names are stable and never generated as `plugin_N`. A capability may own several tools. `file_read`, process-control tools, and explicit sharing complete the five capabilities; they are not additional Registry entities.

Publish machine-readable JSON Schemas through the existing API schema mechanism. Inputs reject unknown fields. Strings, arrays, offsets, timeouts and output budgets have validated bounds. Run, requester, Agent identity, policy and working-area binding come from trusted `ToolContext`, never from model-supplied claims. A model cannot select another tenant by supplying an ID.

Each result includes `operation_id`, `status`, and `policy_revision`; include `area_id`, `generation` and `revision` when relevant. Valid statuses are `completed`, `running`, `approval_required`, `blocked`, `failed` and `uncertain`. Errors include a stable `code`, safe `message`, and `retryable` boolean. Partial model-visible results include `truncated: true` and an authorized cursor or output resource; never imply that an incomplete listing is exhaustive. Inaccessible and unknown resources have the same externally visible lookup error.

### 5.2 Tool schema inventory

Fields marked `?` are optional. Integers are non-negative unless noted. UUIDs and digests use validated formats. The following definitions specify the initial API shape; generating and testing executable schemas is implementation work.

| Tool | Input fields | Result fields beyond common envelope |
| --- | --- | --- |
| `file_search` | `query: string`, `mode: literal\|regex\|path`, `scope: working\|references\|received`, `path?: string`, `cursor?: string`, `limit?: integer` | `matches: [{file_id, path, digest, location, snippet}]`, `next_cursor?: string`, `truncated: boolean` |
| `file_read` | `file_id: UUID`, `representation: text\|metadata`, `offset?: integer`, `max_bytes?: integer`, `expected_digest?: string` | `content?: string`, `metadata`, `digest`, `next_offset?: integer`, `encoding`, `truncated` |
| `shell` | `command: string`, `cwd?: string`, `timeout_ms?: integer`, `yield_ms?: integer` | `process_id?: UUID`, `stdout`, `stderr`, `exit_code?: integer`, `output_cursor?: string`, `truncated`, `effects_may_have_occurred: boolean` |
| `shell_poll` | `process_id: UUID`, `cursor?: string`, `max_bytes?: integer` | Process state, bounded output delta, exit code when terminal, next cursor |
| `shell_cancel` | `process_id: UUID` | Confirmed process-group state, `effects_may_have_occurred` |
| `code_interpreter` | `code: string`, `expected_session_id?: UUID`, `timeout_ms?: integer` | `session_id`, `session_reset: boolean`, `reset_reason?: string`, `stdout`, `stderr`, `displays: [{mime, file_id}]`, `error?: {type, message}`, `truncated` |
| `apply_patch` | `patch: string`, `expected_revision: integer`, `expected_files: [{path, digest_or_absent}]` | New revision, `changes: [{path, action, old_digest?, new_digest?}]`; structured conflicts on failure |
| `skill_list` | `cursor?: string`, `limit?: integer` | `skills: [{skill_id, name, description, origin, digest}]`, next cursor |
| `skill_load` | `skill_id: UUID`, `expected_digest: string` | Manifest text, immutable content identity, complete supporting-file inventory within the admitted package limit (at most 64 files); no unreachable inventory cursor |
| `skill_read` | `skill_id: UUID`, `digest: string`, `path: string`, `offset?: integer`, `max_bytes?: integer` | Bounded supporting-file text or metadata, next offset, content digest |
| `file_share` | `files: [{file_id, expected_digest}]`, `recipient: {node_id, agent_id, agent_version, thread_id}` | `snapshot_id`, `transfer_id`, fixed manifest digest, durable recipient receipt when completed |

A stale `expected_session_id` returns `SESSION_RESET` before running code and supplies the new identity and reason. First use without an expected ID may initialize an empty session with `session_reset: false`. A reset is also emitted to the human-facing event stream. File reads preserve UTF-8 boundaries; `offset`/`next_offset` are byte positions, and the reader advances only at valid boundaries. Search locations identify line ranges or extracted PDF page / spreadsheet sheet-and-cell locations.

The initial `file_search` is bounded path and text search over accessible working files and extracted reference text. It does not depend on a hosted vector store or silently expand to web search. Invalid or excessive regex work fails explicitly within its time budget. Binary files yield metadata or an available text representation, not fabricated extracted content.

### 5.3 Patch safety

Use the Codex-style add/update/delete patch grammar as the syntax reference [S4], wrapped in the explicit revision/digest preconditions above. Paths are relative to authorized writable roots; reject escape, forbidden roots, symlink traversal, special devices and unsafe hard-link aliases. Do not rely on string-prefix checks or one-time canonicalization while another process can change path components.

Validate all hunks and preconditions before publication. Stage changed contents in a private area. Under the area mutation gate, record a durable patch intent and preimages, apply the staged changes, and publish a new revision only after the complete operation succeeds. If a process stops mid-commit, block readers and writers until recovery finishes the recorded patch or restores its preimages. Do not claim that several filesystem renames and a database transaction are intrinsically atomic.

Conflicts preserve the original visible revision and return affected paths and actual digests. Recovery uses the recorded bytes, not a newly generated model patch. Temporary preimages are restricted execution-recovery data; destructive area deletion includes them. Externally managed checkouts must remain untouched by working-area cleanup.

### 5.4 Skill resolution and loading

Resolve explicitly attached packages and authorized project/user-equivalent `.agents/skills` roots when a Run starts. Those roots are inside its permitted mounts, not the Worker's home. Normalize and validate `SKILL.md` metadata, record origin and a content manifest, and expose bounded metadata first [S2]. Fully qualify colliding names with origin/ID; never let directory ordering silently replace a selected Skill.

Pin the discovered content identity for the Run. `skill_load` and `skill_read` verify that identity and return a conflict if mutable source bytes changed before they were captured. A later Run can resolve an updated directory without rewriting an immutable Agent's source binding. Explicit versioned attachments change through a new Agent version. Preserve imported license metadata and bounded supporting assets. Over-budget instructions fail explicitly rather than silently dropping safety-relevant sections.

A selected script remains data until an Agent explicitly invokes an enabled Shell or Python capability. `allowed-tools` metadata cannot raise effective authority. Read-only Skill mounts prevent arbitrary code from editing a currently pinned Skill; an Agent may create a separately authorized working copy without changing the original attachment.

## 6. Proposed files, storage and ownership

Store binary data outside Run context and ordinary event payloads. The initial storage adapter is node-managed durable filesystem/volume storage with immutable objects, staged writes, integrity digests and tenant-scoped authorization metadata in PostgreSQL. The abstraction can admit object storage later without changing tool contracts. Content hashes are integrity identifiers, not access tokens; do not expose global deduplication existence across tenants.

| Proposed record | Required identity and durable data |
| --- | --- |
| `agent_execution_sessions` | Session key; original requester routing metadata; active Run; next input sequence; revision. Unique session key and one active ordinary Run. |
| `agent_working_areas` | Tenant, owning node, workspace/thread/Agent binding; storage locator; generation/revision; execution epoch; owner and disclosure constraints; state; charged bytes. |
| `capability_operations` | Existing invocation linkage; immutable request digest; Run/Agent/session; epoch; state; runner operation ID; result/output resource; timestamps. Unique invocation identity. |
| `file_objects` / `file_bindings` | Tenant-scoped object identity, digest, length, media type, storage state, owner, provenance, classification and permitted bindings. |
| `reference_documents` | Exact Agent version; original object and extraction object/status; extraction locations; deletion state. Existing text-only entries may have no original. |
| `skill_attachments` | Agent configuration or authorized discovery root; normalized manifest; source and content identity; bounded file manifest. No Registry Skill dependency for new attachments. |
| `file_snapshots` / `file_transfers` | Fixed manifest, sender and recipient identities, authorization evidence, state, retry cursor, received digest and durable receipt. Cleanup snapshots have an explicit separate kind and expiry. |
| `capability_approval_requests` / `capability_grants` | Exact invocation or bounded Run scope; requester and approver; operation/targets; policy revision; expiry/revocation; request digest. |

Reuse an existing table where its contract already fits; these names are proposed logical records, not a requirement to duplicate existing invocations, approvals or artifacts. Mutations and their audit/outbox event are transactional. Files are staged and verified before a visible binding is committed. Unreferenced staging objects are garbage-collected only after their operation is terminal or its bounded staging lifetime expires. Garbage collection does not delete a user's active working area or valid recovery snapshot to conceal quota pressure.

A file copied for analysis is distinct from its original. A recipient's shared copy and a published artifact have their own owner and retention. Workspace-level file management must remain available after thread deletion; keep a minimal tombstone and original ownership/ACL linkage. Removing a thread never broadens access to its retained files. An authorized owner or Workspace administrator can list, restore, or delete them. Restoration after thread deletion creates a new authorized thread/Agent binding and does not resurrect deleted messages.

## 7. Proposed sandbox and environment

### 7.1 Execution boundary

Initial deployment target: Linux Kubernetes/k3s, with a dedicated runner service and a sandboxed runtime profile such as gVisor `runsc`. The exact tested runtime/image versions are locked in the deployment manifest. Local development uses the same runner contract in the dedicated local cluster. When the required runtime, storage quota enforcement or network isolation is missing, mark code execution unavailable; never fall back to running model-generated commands inside the API/Worker host.

Apply Restricted Pod controls, non-root execution, dropped capabilities, a read-only base filesystem, separate writable work/temp volumes, no host namespace sharing, no host paths or container-engine sockets, and no automatically mounted service-account token [S5]. gVisor is an additional boundary, not a proof that workload network access, exposed files or resource abuse are safe [S6]. Runtime selection remains an engineering proposal, not an additional product decision.

Enforce CPU, memory, process-count, disk and wall-time limits in the runner and underlying runtime. Kubernetes resource fields alone do not establish every limit: admission must verify the chosen node/runtime actually enforces process and writable-volume quotas. Perform direct runtime tests for these limits rather than asserting them from a manifest.

Start from an allowlisted environment, not the Worker's environment with a few variables removed. No database, model-provider, federation, cluster or user service credentials enter Shell, Python, package installers, mounted config, process arguments or inherited handles. Isolate the privileged runner control plane from its payload.

### 7.2 Network policy and credentials

Default execution egress is denied. An operator-configured, policy-approved broker mediates allowed outbound destinations. Pod/network isolation must block direct bypass, private infrastructure, metadata addresses, unrestricted DNS and arbitrary proxy tunnels. Revalidate redirects and resolved destinations; preserve destination and Run binding. Test the actual network plugin because creating a NetworkPolicy object alone does not enforce it [S7].

Current policy may allow bounded access without asking every time. Additional permission uses Section 8; prohibited destinations stay prohibited. Expired/revoked grants stop new requests and revoke relevant broker sessions. Stop affected processes when an already established channel or mount cannot be safely withdrawn.

Authenticated service operations remain explicit dedicated integration calls through the Harness. Their private credentials are applied outside code execution and never returned. An integration that returns a short-lived bearer secret to Shell would violate this contract. Reference uploads containing user-written secrets cannot be guaranteed secret-free merely by filtering environment variables; upload handling must warn and restrict such inputs rather than claim automatic removal of arbitrary embedded secrets.

### 7.3 Python and dependencies

Proposed baseline is CPython 3.13 with `ipykernel`, `numpy`, `pandas`, `matplotlib`, `openpyxl` and a PDF text reader in a version-locked image. A maintained patch image and dependency lock are selected and tested during implementation; “latest” is not an execution identity. This set is an initial environment proposal, not a guarantee of arbitrary document fidelity or a ban on other permitted Shell runtimes.

A session uses its own writable environment overlay. Package installation is an ordinary potentially effectful operation: serialize it with execution, restrict sources and network access, record resolved versions and hashes, and enforce quotas. Default public Python sources are an operator allowlist, not an automatic network grant. Build scripts execute within the same restrictions. Authenticated package sources require a dedicated integration; they may not inject credentials into pip or build subprocesses.

Keep a dependency manifest in the working area. Do not silently replay installation scripts or past user code after a reset. A new session reports its available packages; missing dependencies can be explicitly reinstalled as new authorized operations. Saved outputs survive under the file policy, but an arbitrary package overlay is not promised to survive process/environment replacement.

## 8. Proposed approvals and revocation

A permission request binds the exact pending invocation digest, Run, Agent, requester, operation and targets. Show what is requested, why it is outside present authority, and the maximum scope the eligible approver may grant. Validate approver eligibility at decision time and action authority again immediately before execution.

The default control is “Allow once”; “Allow within this Run” explicitly selects actions, destinations or file targets and an expiry. Reusing a grant means checking a structured scope, not trusting a Shell command prefix or a model's explanation. A single invocation grant is consumed by its invocation ID; retries of that identical invocation do not create another allowance. Run scope expires at the earlier of Run termination and its expiry.

Denied, expired and revoked requests remain visible with safe reasons. Requests with no eligible approver block without holding a Worker. Revocation denies new calls even when a grant is cached. An authorization-fingerprint change forces a clean Interpreter before any later user gains access. Cancellation never claims to undo an already performed external effect.

## 9. Proposed cross-node transfer protocol

Capability negotiation advertises an explicit `file-transfer/1` contract independently of task delegation. A missing scoped identity/authorization implementation is a blocked dependency, not permission to use legacy broad peer credentials. Both sender disclosure and recipient admission are required.

1. **Prepare:** Under a stable source revision, freeze selected bytes into a manifest containing relative paths, lengths and digests. Persist transfer ID, exact recipient and immutable request digest. Check disclosure and reserve bounded staging quota.
2. **Admit:** The recipient authenticates the sending node and scoped Agent/delegation identity, validates the receiving Agent/thread authority, validates media/path/size bounds, and records an idempotent incoming transfer. Bind any transport authorization to that transfer; do not expose it to Shell/Python.
3. **Transfer:** Upload bounded chunks with sequence/offset and digest. Retries use the same transfer and frozen manifest. Reject changed metadata, unsafe paths, chunk overlaps with different bytes and quota overruns. Partial contents remain inaccessible to Agents.
4. **Commit:** Verify the full content and recheck current policy. Atomically publish recipient object bindings and a durable receipt with manifest digest. Only that receipt permits the sender to report completed delivery.
5. **Reconcile:** After a lost acknowledgment, query receipt/status using the same identity and transfer ID. Do not create duplicate visible inputs or freeze a different source revision. Abort/expiry removes only uncommitted staging. Emit accepted, failed, blocked and uncertain states distinctly.

The proposed peer endpoints are `POST /federation/v0.1/file-transfers`, `PUT /federation/v0.1/file-transfers/{id}/chunks/{index}`, `POST /federation/v0.1/file-transfers/{id}/commit`, and `GET /federation/v0.1/file-transfers/{id}`. No HTTP request supplies an unrestricted remote storage path.

Revocation before commit prevents new visibility. After an acknowledged delivery, deleting the sender's area or withdrawing future sharing permission does not erase recipient-owned copies or knowledge already read. A future recall protocol is not promised. Private reference restrictions also apply to copied and derived outputs through conservative disclosure labels; renaming a file is not declassification.

## 10. Proposed management API and user experience

Management endpoints use the existing authenticated API, current authorization, resource revision checks and UUID idempotency keys. They are not model-controlled administrator tools.

The implementation admits core working areas only for local thread-bound Runs. A remote workspace Agent with core flags is rejected before activation; it cannot create a hidden remote area behind a home-node thread. Independently admitted owner-node threads manage their own areas through the owner node's settings and deletion controls. Scoped file transfer creates a separately owned recipient copy. The home node does not claim deletion of that copy or of an unaddressable remote working area. The [operations guide](../../operations/core-capabilities.md) and generated OpenAPI from typed handlers define the final route names and supported deployment profile.

| Proposed route | Purpose and controls |
| --- | --- |
| `GET /api/working-areas` | Paginated, authorization-filtered area and recovery inventory, including retained entries whose thread was deleted. |
| `POST /api/working-areas/{id}/cleanup` | Body `mode: reversible\|irreversible`, `expected_revision`, and a separately confirmed irreversible intent. Returns an operation, not premature success. |
| `POST /api/working-areas/{id}/restore` | Select recovery snapshot and authorized destination binding; verify expiry and content; never restore grants or Interpreter memory. |
| `POST /api/agents/reference-documents` | Bounded staged binary upload and extraction; publish an exact-version attachment only after validation. |
| `DELETE /api/agents/reference-documents/{id}` | Authorized original/extraction access revocation and managed-object deletion without silently rewriting Agent configuration; later execution reports the missing reference. |
| `POST /api/agents/skill-attachments` | Import or attach permitted Skill directories and content identity without a Registry Skill entity. |
| `POST /api/capability-approvals/{id}/decision` | `deny`, `allow_once`, or bounded `allow_run`, expected revision and eligible approver. |
| `POST /api/capability-grants/{id}/revoke` | Revoke future use and notify the runner/broker. |
| `GET /api/capability-operations/{id}` | Authorized operation state and bounded output/status cursors. |

Reuse existing thread input and Run control endpoints for queue/steer rather than create a second conversation protocol. Exact published route names must be generated from typed handlers and tested against this inventory before use.

In Agent creation, core capabilities appear as explicit policy-constrained switches, Skill attachment and reference-file controls. Existing Registry integrations remain a separate section. A disabled runtime shows setup guidance and an unavailable capability rather than a broken invocation.

The thread view shows queued requests, active work, steering and stop controls, output/artifact links, approval cards and explicit session-reset messages. At goal completion, suggest reversible cleanup and offer retain/no action. In the thread-deletion dialog, resolve retain, reversible cleanup, or irreversible deletion before losing the thread controls. Identify Agent, file count/size, affected recovery copies and recovery expiry. Irreversible deletion requires an extra confirmation and never becomes the default because a timer elapsed.

Quiesce all relevant processes before cleanup. If a snapshot fails, retain active files. If deletion partly fails, mark `cleanup_failed`, keep retry/reconciliation controls and do not show “deleted.” After deletion, fence the old generation so late process output or an old retry cannot recreate it. Storage management belongs under existing settings; it is not a new primary dashboard or Kanban workflow.

New reference uploads preserve the original and text with page/sheet locations. Parsing is sandboxed and bounded; unsupported/encrypted/unextractable input reports extraction status and limits instead of silently producing incomplete “complete” text. No OCR or spreadsheet formula-recalculation guarantee is added. Original bytes can still be used by authorized processing where supported.

## 11. Proposed initial limits

All values below are engineering defaults for review, not claims about Codex or additional user decisions. Operators may lower limits and configure higher tested ceilings. A Run cannot increase them for itself. Resource exhaustion returns an explicit quota/limit result; it does not silently delete working data.

| Parameter | Initial value |
| --- | --- |
| Interpreter idle timeout | 30 minutes without an executing operation; configurable |
| Cleanup recovery retention | 7 days; show exact expiry before confirmation |
| Reusable Run grant | At most 1 hour and never beyond Run termination |
| Pending approval lifetime | 24 hours; expiry leaves the requested action blocked |
| Sandbox CPU / memory / processes | 2 CPUs / 2 GiB / 128 processes per environment |
| Working-area disk / temporary disk | 1 GiB / 256 MiB; separately charged and enforced |
| Tenant retained object quota | 10 GiB initial deployment default, including originals and snapshots |
| Shell/Python wall time | 120 seconds default; policy ceiling 10 minutes per operation |
| Dependency-install wall time | 5 minutes default; same 10-minute ceiling |
| Tool code / patch input | 64 KiB code or command; 256 KiB patch |
| Search results | 50 matches/page, 32 KiB result cap, 5-second processing budget |
| Text read / execution preview | 16 KiB per response, with explicit continuation |
| Captured process output | 8 MiB per operation; reaching capture limit stops capture with an explicit notice, without silently extending execution time |
| Skill import | 64 files and 256 KiB per package initially, preserving current import limits |
| Reference uploads | 8 files, 10 MiB/file, 200 PDF pages, 64 KiB extracted text across an Agent's reference set initially; report extraction limits |
| Shared transfer | 64 files, 100 MiB/file, 256 MiB/transfer; 4 MiB chunks |
| Uncommitted transfer staging | 24 hours, with quota reservation and bounded retries |

The 8 MiB output cap is a storage cap, not a promise the model sees all output. Oversized captured output never inflates provider context; authorized readers can page retained output, and capture truncation remains visible. Fixed limits must be configurable in one execution profile and documented in deployment configuration rather than scattered through handlers.

## 12. Compatibility and rollout

Introduce additive schema and API fields. New `core_capabilities` and direct Skill attachment fields coexist with legacy references. Defaults grant no new execution authority. An old Agent with Registry Skills continues resolving those exact immutable versions through a compatibility adapter. New imports can reuse validation code but create Harness-managed attachments rather than a Registry Skill entry.

Do not rewrite existing Agent versions or fabricate original bytes for text-only references. A missing original is represented explicitly. Changing attachments/configuration creates a new Agent version; deleting sensitive backing data revokes access and makes an old version fail clearly instead of executing with silently omitted input. Third-party `plugin_N` bindings keep their established meaning.

Roll out in a disabled state, validate the runner and network profile, enable for selected test Agents, then enable policy-authorized production scopes. Cross-node transfer stays unavailable until the negotiated scoped protocol passes both-node tests; its completion is required to close #43, not silently deferred out of scope. The existing model provider and credential management remain unchanged.

Disable new capability admission to roll back. Drain or explicitly stop in-flight operations; preserve their journals, user files and recovery controls. Do not perform destructive down-migrations over retained objects. Old application versions that cannot safely read new Agent configuration must reject it instead of ignoring the new permission fields.

## 13. Acceptance and verification

### Testing boundary

The primary seam is the authenticated API -> Harness invocation -> durable store/output boundary, extending `tests/common.rs`. Use scripted model responses, real PostgreSQL/NATS, and two independent node instances for federation. Assert externally visible outcomes and durable records rather than private helper-call order. This seam selection is an engineering proposal for written review.

A second indispensable seam is the real runner boundary: mock tool handlers cannot prove process, filesystem or network isolation. Use the configured sandbox runtime for escape, quota, credential and process-termination checks; the submitted runtime must pass these tests before it is considered supported. Pure parsers/schema validators may have focused unit tests, but those tests are not substitutes for the two acceptance seams. No paid provider calls or production credentials are required.

| Case | Required observable result |
| --- | --- |
| AT01 | An Agent with enabled core capabilities and no Registry tools/Skills can discover, read, run Python, run Shell, patch and load a Skill; core names never become `plugin_N`. |
| AT02 | Disabled tools are absent from the model schema and denied on direct dispatch. Missing runner/isolation support fails closed, with no host-execution fallback. |
| AT03 | Follow-up Runs in the same thread/Agent retain files and execute in admitted order. A steer targets the active Run without starting another. Other Agents/threads still progress concurrently. |
| AT04 | Another Agent, tenant or lesser-authorized requester cannot read prior files, cached context or Interpreter variables. Changed authority/version recreates the environment and keeps denied files inaccessible. |
| AT05 | Python variables/imports persist between calls in a live session. First use, idle reset and environment loss have explicit identity/reset behavior; saved files remain available when authorized. |
| AT06 | Kill the Worker or Interpreter between execution and result persistence. A known finished result is recovered; an unprovable effect becomes uncertain and historical code is not replayed. |
| AT07 | Goal completion and thread deletion offer the correct cleanup choices. Ignoring the prompt and idle shutdown leave working files intact. |
| AT08 | Reversible cleanup verifies the snapshot before removing files. Inject a snapshot failure and observe no data loss. Restore before expiry recovers exact file digests, not memory or approval grants. |
| AT09 | Irreversible deletion requires separate confirmation, removes managed working/recovery copies, fences late writes, and preserves unrelated originals/artifacts/shared copies. Partial deletion is not reported as success. Retained files remain manageable after thread deletion. |
| AT10 | Requester-first routing falls back only to an eligible designated approver. A delegation chain preserves the requester. No approver or an upper-policy denial blocks the action. |
| AT11 | Allow-once binds one invocation. Explicit Run grants are action/target/Agent scoped, expire at Run end or recorded time, and do not carry into the next Run or child Agent. |
| AT12 | Revoke policy/grants before dispatch and while background network work exists. Subsequent activity is denied, relevant channels are withdrawn, and unsafe effects are not retroactively claimed undone. |
| AT13 | Skill discovery supplies bounded metadata; activation supplies the selected manifest and on-demand files. Loading instructions never executes bundled scripts. |
| AT14 | Skill name collisions require unambiguous identities. Changed content cannot substitute bytes behind a pinned digest. Path escape and unauthorized host discovery fail. |
| AT15 | File search/read enforce ACLs, scopes, byte/result budgets, valid continuation and provenance. Pagination detects revision changes. Malformed regex and non-extractable inputs fail explicitly. |
| AT16 | Patches support add/update/delete and reject stale revision/digest or hunk conflicts without overwrite. Inject mid-commit failure and observe reconciliation before any partial revision becomes visible. |
| AT17 | Real sandbox probes cannot access forbidden host paths, sockets, other Agent areas or metadata services. CPU/memory/process/disk/time limits and process-group cancellation are measurably enforced. |
| AT18 | Runner startup, Shell, Python and package-build processes contain no inherited infrastructure/service secrets. Agent code cannot call the privileged runner control API or bypass egress policy. |
| AT19 | A permitted Agent-initiated share completes without unnecessary human approval and records explicit sender, recipient, files and authority. Denied disclosure/receipt is blocked. |
| AT20 | Source edits after sharing do not alter received bytes. Recipient edits do not alter source or shared manifest. Cleanup and shared snapshots cannot be confused. |
| AT21 | Two authorized nodes transfer exact file contents with integrity checks and durable receipt. Wrong node/Agent, missing scoped protocol, denied egress/disclosure and quota excess are rejected. |
| AT22 | Drop a chunk, corrupt bytes, revoke during transfer and lose the final ACK. No partial file becomes visible, retry uses the original snapshot, and a committed receipt prevents duplicate delivery. |
| AT23 | A new reference keeps original bytes and located extracted text. Python edits a working copy; original digest stays unchanged. Limits and unsupported extraction are explicit. |
| AT24 | Existing text-only references and immutable Agent versions remain valid without fabricated originals. Removing reference access prevents further use, including through a previously warm session. |
| AT25 | A permitted package install changes only the Agent environment and records resolved dependencies. Denied sources, insufficient quota, timeout and authenticated-source secret injection fail clearly. |
| AT26 | A dedicated integration can perform an authorized authenticated fixture operation without returning its credential to the model, sandbox, output or journal. |
| AT27 | Legacy Registry Skills and external tools still execute with their previous identities and authority; migration or feature enablement creates no implicit grants. |
| AT28 | Completed invocations return stored outcomes. Request-key reuse with different inputs conflicts. Stale Worker epochs cannot write after takeover; unconfirmed live writers block replacement and cleanup. |
| AT29 | Stop/approval/steer controls remain usable while work is queued. A parent/child session-dependency cycle fails explicitly; background processes cannot survive past their enforced execution/grant boundary unnoticed. |

Required release evidence consists of schema validation results, behavior-test results, real-runner isolation/quota results, two-node transfer fault tests, migration/compatibility results, and a UI walkthrough of approval/reset/cleanup states. Passing parser tests alone does not satisfy #43.

## 14. Explicit non-goals

This specification does not introduce a new planner, automatic model routing, Codex embedding, hosted OpenAI File Search/Code Interpreter dependency, automatic worktree branching/merging for overlapping requests, durable Python heap restoration, implicit Skill-script execution, direct secret injection, automatic file disclosure to thread members, or arbitrary host execution.

It does not promise universal file-format fidelity, OCR, spreadsheet formula recalculation, perfect secret detection, removal of independent backups, recall of already read shared content, or exactly-once effects from uncooperative external services. A new certification/workbench product and web-search implementation remain separate work.

## 15. Sources and implementation handoff

Repository references are the inspected baseline, not an assertion about unreviewed future commits. The proposal must be reviewed as a document before implementation. The next workflow stage is decomposition into implementation tickets, not silently starting product-code changes from this specification.

- [S1: Codex App Server — thread, turn and steering](https://developers.openai.com/codex/app-server), checked 2026-09-25.
- [S2: Codex Skills — directory-based discovery and loading](https://developers.openai.com/codex/skills), checked 2026-09-25.
- [S3: Codex Worktrees — isolated checkouts and cleanup](https://developers.openai.com/codex/app/worktrees), checked 2026-09-25. Aidash's explicit cleanup choice is defined by R04–R05 rather than copied wholesale.
- [S4: Codex apply-patch syntax](https://github.com/openai/codex/tree/main/codex-rs/apply-patch). Freeze a reviewed upstream grammar revision in the implementation dependency/fixture lock; this does not assert full future-upstream compatibility.
- [S5: Kubernetes Pod Security Standards](https://kubernetes.io/docs/concepts/security/pod-security-standards/), checked 2026-09-25.
- [S6: gVisor security model](https://gvisor.dev/docs/architecture_guide/security/), checked 2026-09-25.
- [S7: Kubernetes NetworkPolicy prerequisites](https://kubernetes.io/docs/concepts/services-networking/network-policies/), checked 2026-09-25.
- [Aidash Tool interface](https://github.com/kent8192/aidash/blob/e70758ba6a48fc18c07e535f60182a70c8b6f29b/src/tool.rs).
- [Aidash Harness](https://github.com/kent8192/aidash/blob/e70758ba6a48fc18c07e535f60182a70c8b6f29b/src/harness.rs).
- [Aidash protocol and recovery](https://github.com/kent8192/aidash/blob/e70758ba6a48fc18c07e535f60182a70c8b6f29b/docs/protocol.md).
- [Aidash Skills and personal-reference behavior](https://github.com/kent8192/aidash/blob/e70758ba6a48fc18c07e535f60182a70c8b6f29b/docs/features/skills-first-agents.md).
- [Aidash integration-test boundary](https://github.com/kent8192/aidash/blob/e70758ba6a48fc18c07e535f60182a70c8b6f29b/tests/common.rs).
