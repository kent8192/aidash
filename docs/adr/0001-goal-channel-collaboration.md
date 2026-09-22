# Goal-centered collaboration with contextual graph operations

Aidash's primary user experience consists of a Slack-style collaboration space and Graph View. Each channel corresponds to one goal; threads organize discussion independently of executable tasks, so a discussion can produce no tasks or several tasks. Necessary configuration belongs under Settings, while everyday work and intervention remain available from the primary views.

## Channel persistence

Reuse the existing Workspace as the persistent identity of a channel, preserving its ID and the ownership of its tasks and shared work. Present it as a channel in the user interface rather than introducing a new Workspace-to-Channel container hierarchy. Add goal revisions, threads, and participation to that existing collaboration boundary.

Keep the record revision used for optimistic concurrency separate from the goal revision that identifies the objective of a task or run. A thread remains a conversation structure rather than an executable task.

## Registry storage responsibility

The Registry stores Aidash information, including component definitions, interactions, channel messages, task and run states, artifacts, and their histories. It is not merely a component catalog or a policy directory pointing to an unrelated interaction-history store. Graph reconstruction uses the information retained by the Registry and remains available for the period that the Registry supports; there is no product-wide fixed retention duration.

This logical storage responsibility does not collapse the established Home/executing-node ownership or imply a single global Registry instance. Storage and exposure remain separate: discovery and federation must not expose owner-private references, credentials, or other information outside the caller's authorization. Grouping information under the Registry does not require placing every record in the component-metadata table.

## Storage pressure and content erasure

Preserve already retained information by default when Registry capacity becomes insufficient. Notify operators and stop admitting new executions requiring writes to the affected Registry before their records can no longer be stored safely. Keep headroom for in-flight outcomes and safe-stop records. Do not silently discard old history merely to admit new work; resume after capacity is increased or an administrator-authorized retention cleanup restores safe capacity. Protect the records needed for execution recovery, unsettled cost accounting, and duplicate-effect prevention from capacity-driven cleanup.

Ordinary message withdrawal removes content from the current conversation display while retaining the original in authorized history. Provide a separate administrative content-erasure operation for removing specified message content, attachments, and historical reconstruction copies from Aidash-managed storage. Historical conversations and graphs must not restore erased content by selecting an earlier time; show a content-free erasure marker instead. Do not reconstruct the removed payload from historical copies or pretend the missing content is available.

Erasure of managed content does not promise recall of copies already received by people, external nodes, or services outside the operation's control. Retention-preserving admission control does not remove the separate ability to perform authorized content erasure.

## Goal revisions and completion

Updating a goal adds a new revision within the same channel instead of overwriting the objective used by previous work. Tasks, runs, and completion records identify the goal revision they concern. Earlier work and results remain available as history; a delayed completion for an earlier revision cannot complete the current revision.

An update stops new work from starting against the earlier revision and stops its active runs at safe, persisted boundaries before switching their work to the new objective. In-flight calls may finish and their outcomes must be recorded; already performed external effects are not undone. Earlier results remain available for planning the new revision. An unresolved external outcome blocks conflicting new-revision operations until reconciliation, rather than authorizing repetition of the uncertain effect.

Any participating agent with completion authority may submit a goal-completion decision. The system accepts it only when common completion conditions hold, including no unresolved required tasks or pending approvals. Human sign-off and agreement from every participating agent are not required. Completion records identify the deciding agent, reason, and supporting results. A human who finds the result insufficient can update the goal and continue the work; agent completion and human satisfaction are distinct concepts.

Tasks are required by default. A human or agent with separately granted exclusion authority may make a task optional or exclude it from the required work, recording the reason. Completion authority alone does not grant exclusion authority. Exclusion does not rewrite failure as success, satisfy an unmet dependency, or bypass pending approvals or uncertain external-effect reconciliation.

A task block stops that task and work dependent on its unresolved result; independent work may continue. A required task becoming blocked does not automatically stop the entire goal, but an unresolved required task still prevents goal completion.

An authorized agent may separately block a goal revision when the goal as a whole cannot progress or be achieved under the current conditions. Record the reason, evidence, deciding actor, and conditions needed to resume. A goal-level block stops new execution for that revision and brings active work to safe, persisted boundaries; it does not undo completed external effects. Blocking does not satisfy goal-completion conditions, and required work must not be silently discarded to produce a successful completion.

## Shared history and operation authority

A human participant with channel read access can read the channel's shared history, including material shared before joining. This does not expose an agent's owner-private reference material or grant every operation: posting, execution, control, goal changes, and approval remain subject to separately granted authority. Equivalent operations use the same authorization regardless of whether they originate in collaboration or Graph View.

Use one message composer without requiring a comment-versus-instruction mode. Agents may interpret the intent of an ordinary message, but the server verifies the authenticated sender's authority and the allowed scope when admitting an operation. Posting a message does not grant execution authority. Information supplied during already authorized work does not implicitly expand that work's permissions.

Ambiguous requests require clarification rather than being treated as authorization to act. Goal changes, budget increases, and approvals use explicit confirmations, such as cards identifying the target and proposed change, together with server-side authorization. Natural-language claims such as "already approved" do not substitute for an authorized approval. This does not require human approval for every step of agent work that is already authorized.

## Delegation and channel observation

Delegating a task does not automatically make its executor a continuing channel participant. The delegate receives the task's authorized work and context; continuing participation requires a separate membership operation and authority.

Until the delegated task completes, its executor can read the channel's authorized retained history, including messages from before delegation, and subsequent stream updates to obtain context as needed. Retrieve relevant portions through search or paged reads rather than inserting the entire history into every inference request. This task-bound observation permission does not grant unrelated execution, channel administration, access to owner-private references, access to other channels, or automatic participation in deciding responses to every subsequent channel message. Reads remain subject to current authorization and retained coverage. Completion ends this delegated observation permission; any continuing channel access requires separate authority.

The delegate's work remains visible through recorded delegation and execution relationships in Graph View and through task cards and results in the channel. A delegate can be separately admitted as a continuing participant when that is appropriate.

## Response evaluation and channel presentation

For a given agent, multiple unprocessed messages may be evaluated together rather than requiring a separate inference call for each message. Preserve every message's ID, sender, order, and individual decision; batching must not merge permissions, discard earlier constraints, or initiate duplicate work on replay. Stop, cancel, and authorization-revocation controls do not wait for conversation batching.

The default channel timeline emphasizes human and agent conversation. Agent-to-agent collaborative messages remain readable; low-level tool arguments, results, and retries are grouped into task cards and expandable execution details rather than flooding the conversation. Approval requests, goal blocks, budget exhaustion, and completion remain visible in the channel. Presentation grouping does not remove durable records, historical reconstruction data, or context needed for authorized agent work.

## Shared monetary channel budget

A channel's monetary spending allowance applies across all goal revisions. Updating a goal does not reset its consumption or authorize an increase; increasing the allowance requires separate budget-management authority. Token and call limits may supplement the monetary allowance but do not replace it.

The allowance covers billable response-necessity evaluation, agent work, child tasks, and retries within the supported accounting scope. Admit a billable operation only when its reservation keeps settled consumption plus outstanding reservations within the channel allowance. Reserve before provider or tool I/O and settle against trustworthy usage afterward. Unknown-priced billable operations are not treated as free; unsettled or uncertain charges do not automatically release their reservation. The limit depends on supported providers and tools honoring their declared charging bounds and is not a cap on unrelated hosting or other out-of-scope costs.

Each billable operation, including one performed on a remote node, obtains its reservation from the channel's Home node. Do not preallocate autonomous spending envelopes to remote nodes. If Home cannot be reached to obtain a new reservation, do not start that new billable operation. An already reserved operation may proceed only while its execution authority remains valid; a reservation does not override a goal change or permission revocation.

At the limit, stop admitting new paid work, retain the history, and notify the user. Resumption after a budget change remains subject to the relevant authority and execution conditions.

## Graph exploration and navigation

Graph View supports relationship exploration and authorized operations through contextual details, not editing execution structure by drawing connections. Navigation to collaboration follows the selected entity: a task or run opens its channel; a reusable entity prefers the current channel when related, otherwise offers the viewer's accessible related channels. An entity with no related channel stays in its details rather than creating a channel automatically.

The graph uses aggregation and progressive exploration: clusters and other groupings provide an overview, users expand the relevant neighborhood, and search locates individual entities. Rendering every registered agent as an individual node at once is not a requirement. Configuration relationships and runtime activity remain distinguishable; authorization boundaries and an accessible list/detail alternative apply to the graph as well.

## Historical graph reconstruction

Graph View must support selecting a past time and reconstructing the recorded relationships and entity states for that time, including the exact entity versions used by historical work. A current graph with an event timeline alone does not satisfy this requirement. Historical reconstruction requires retained relationship and state history, independently of the rendering library and the SSE reconnection mechanism.

Use observation-time semantics: reconstruct information known at the selected time, not a hindsight-corrected account based on reports learned later. Distinguish observation time from source-reported occurrence time; an earlier reported occurrence does not, by itself, establish that the information was already known. Do not silently mix these two meanings of time.

Federated historical reconstruction is required. Retrieve authorized histories held by multiple participating nodes, including remote execution history not already retained by the node serving the dashboard. Present each node's historical observations with their provenance instead of forcing every observation into one Home node's perspective. If an executing node had recorded a start while Home had not yet received the report, preserve both perspectives. Do not imply a globally simultaneous, universally known state merely by combining timestamps.

Display the available portion when some history cannot be obtained, and identify the reconstruction as partial. Distinguish fetching, communication failure, and unavailable retention coverage where the viewer may know those distinctions. Missing history is unknown, not an empty graph or a count of zero. Never fill a historical gap with present state. Coverage and omission indicators must not disclose unauthorized entities, nodes, or counts.

Reconstruction is available over the periods for which the Registry retains the required information. Indicate missing, erased, or unavailable intervals and content rather than fabricating states outside retained coverage. Participating Registries may provide different coverage; that does not prevent displaying the authorized portions that are available.

Historical mode is read-only. Users may inspect details and navigate to related channels while retaining the relevant historical context. Pause, resume, cancel, and other state-changing operations require an explicit switch to the current state and current authorization checks. Viewing history never re-executes model calls or external tools. Historical reads also apply current authorization; past access does not bypass present restrictions.

## Graph rendering

Cytoscape.js is the selected first implementation candidate for the graph renderer. Keep the relationship projection and its tests separate from rendering so entity identity, relationship semantics, and navigation do not depend on the renderer. The selection does not establish a performance guarantee, a final layout extension, or a numeric rendering limit.

## Realtime transport

Retain HTTP for submissions and control operations, and authenticated SSE for browser updates. Preserve durable event-cursor replay for reconnection instead of replacing the existing transport with a WebSocket-centered design. A conversational interface does not itself require rebuilding the transport.

## Consequences

These boundaries favor goal-oriented work without treating every conversation as an executable task, and preserve agent autonomy without making channel membership blanket execution authority. Workspace reuse avoids introducing another ownership boundary solely to match Slack terminology. A single composer keeps conversation natural while authorization and explicit confirmations protect consequential operations. Revision-specific work and completion protect the meaning of historical results. Safe-boundary transitions preserve external-effect evidence; task-level blocks need not halt independent work, while goal-level blocks expose inability to proceed instead of claiming success.

Separate authority for execution, task exclusion, and budget increases prevents one permission from implicitly granting the others. Task-bound history and stream access lets delegates acquire context without becoming permanent participants. Batched evaluation limits repeated inference work without collapsing message identities or authority, while conversation-focused presentation retains inspectable execution records. Home-managed monetary reservations trade admission availability during Home outages for a single budget allocation authority per channel.

The Registry owns interaction and execution history as well as component registrations, so its retained coverage determines historical reconstruction. Capacity pressure stops new work by default rather than silently sacrificing retained history; explicit administrative erasure can nevertheless remove content from historical views. Aggregation and contextual navigation make relationship exploration the primary graph interaction rather than requiring an unbounded all-entities canvas. Federated historical reconstruction preserves node-specific knowledge and remains useful with explicit gaps; observation-time semantics, current authorization, and read-only inspection keep historical knowledge separate from present operations.
