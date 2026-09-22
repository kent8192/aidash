# Goal-centered collaboration with contextual graph operations

Aidash's primary user experience consists of a Slack-style collaboration space and Graph View. Each channel corresponds to one goal; threads organize discussion independently of executable tasks, so a discussion can produce no tasks or several tasks. Necessary configuration belongs under Settings, while everyday work and intervention remain available from the primary views.

## Channel persistence

Reuse the existing Workspace as the persistent identity of a channel, preserving its ID and the ownership of its tasks and shared work. Present it as a channel in the user interface rather than introducing a new Workspace-to-Channel container hierarchy. Add goal revisions, threads, and participation to that existing collaboration boundary.

Keep the record revision used for optimistic concurrency separate from the goal revision that identifies the objective of a task or run. A thread remains a conversation structure rather than an executable task.

## Goal revisions and completion

Updating a goal adds a new revision within the same channel instead of overwriting the objective used by previous work. Tasks, runs, and completion records identify the goal revision they concern. Earlier work and results remain available as history; a delayed completion for an earlier revision cannot complete the current revision.

An update stops new work from starting against the earlier revision and stops its active runs at safe, persisted boundaries before switching their work to the new objective. In-flight calls may finish and their outcomes must be recorded; already performed external effects are not undone. Earlier results remain available for planning the new revision. An unresolved external outcome blocks conflicting new-revision operations until reconciliation, rather than authorizing repetition of the uncertain effect.

Any participating agent with completion authority may submit a goal-completion decision. The system accepts it only when common completion conditions hold, including no unresolved required tasks or pending approvals. Human sign-off and agreement from every participating agent are not required. Completion records identify the deciding agent, reason, and supporting results. A human who finds the result insufficient can update the goal and continue the work; agent completion and human satisfaction are distinct concepts.

Tasks are required by default. A human or agent with separately granted exclusion authority may make a task optional or exclude it from the required work, recording the reason. Completion authority alone does not grant exclusion authority. Exclusion does not rewrite failure as success, satisfy an unmet dependency, or bypass pending approvals or uncertain external-effect reconciliation.

A goal revision can itself be blocked with a recorded reason when progress is impossible or its objective cannot be achieved under the current conditions. Goal-level blocking is distinct from an individual task failing and does not satisfy goal-completion conditions. Required work must not be silently discarded to produce a successful completion.

## Shared history and operation authority

A human participant with channel read access can read the channel's shared history, including material shared before joining. This does not expose an agent's owner-private reference material or grant every operation: posting, execution, control, goal changes, and approval remain subject to separately granted authority. Equivalent operations use the same authorization regardless of whether they originate in collaboration or Graph View.

Ordinary comments are not execution instructions or approvals. The server checks the sender's authority for the requested action; natural-language claims such as "already approved" do not substitute for an authorized approval. This separation does not require human approval for every step of agent work that is already authorized.

## Shared channel budget

A channel's spending allowance applies across all goal revisions. Updating a goal does not reset its consumption or authorize an increase; increasing the allowance requires separate budget-management authority.

The allowance covers response-necessity evaluation, agent work, child tasks, and retries. At the limit, stop admitting new paid work, retain the history, and notify the user. Resumption after a budget change remains subject to the relevant authority and execution conditions.

## Graph exploration and navigation

Graph View supports relationship exploration and authorized operations through contextual details, not editing execution structure by drawing connections. Navigation to collaboration follows the selected entity: a task or run opens its channel; a reusable entity prefers the current channel when related, otherwise offers the viewer's accessible related channels. An entity with no related channel stays in its details rather than creating a channel automatically.

The graph uses aggregation and progressive exploration: clusters and other groupings provide an overview, users expand the relevant neighborhood, and search locates individual entities. Rendering every registered agent as an individual node at once is not a requirement. Configuration relationships and runtime activity remain distinguishable; authorization boundaries and an accessible list/detail alternative apply to the graph as well.

## Historical graph reconstruction

Graph View must support selecting a past time and reconstructing the recorded relationships and entity states for that time, including the exact entity versions used by historical work. A current graph with an event timeline alone does not satisfy this requirement. Historical reconstruction requires retained relationship and state history, independently of the rendering library and the SSE reconnection mechanism.

Use observation-time semantics: reconstruct information known at the selected time, not a hindsight-corrected account based on reports learned later. Distinguish observation time from source-reported occurrence time; an earlier reported occurrence does not, by itself, establish that the information was already known. Do not silently mix these two meanings of time.

Federated historical reconstruction is required. Retrieve authorized histories held by multiple participating nodes, including remote execution history not already retained by the node serving the dashboard. The scope is not limited to the receiving node's local history. Historical access must respect the participating nodes' authorization and privacy boundaries.

Historical reconstruction remains available for as long as the Registry permits the required history to be retained. No fixed retention duration is mandated.

Historical mode is read-only. Users may inspect details and navigate to related channels while retaining the relevant historical context. Pause, resume, cancel, and other state-changing operations require an explicit switch to the current state and current authorization checks. Viewing history never re-executes model calls or external tools. Historical reads also apply current authorization; past access does not bypass present restrictions.

## Graph rendering

Cytoscape.js is the selected first implementation candidate for the graph renderer. Keep the relationship projection and its tests separate from rendering so entity identity, relationship semantics, and navigation do not depend on the renderer. The selection does not establish a performance guarantee, a final layout extension, or a numeric rendering limit.

## Realtime transport

Retain HTTP for submissions and control operations, and authenticated SSE for browser updates. Preserve durable event-cursor replay for reconnection instead of replacing the existing transport with a WebSocket-centered design. A conversational interface does not itself require rebuilding the transport.

## Consequences

These boundaries favor goal-oriented work without treating every conversation as an executable task, and preserve agent autonomy without making channel membership blanket execution authority. Workspace reuse avoids introducing another ownership boundary solely to match Slack terminology. Revision-specific work and completion protect the meaning of historical results. Safe-boundary transitions preserve external-effect evidence; goal-level blocking exposes inability to proceed instead of claiming success. Separate authority for execution, task exclusion, and budget increases prevents one permission from implicitly granting the others. Aggregation and contextual navigation make relationship exploration the primary graph interaction rather than requiring an unbounded all-entities canvas. Federated historical reconstruction requires access to retained remote relationships and states, not merely their current values; observation-time semantics and read-only inspection preserve the distinction between historical knowledge and present operations.
