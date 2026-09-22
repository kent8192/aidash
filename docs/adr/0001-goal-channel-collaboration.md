# Goal-centered collaboration with contextual graph operations

Aidash's primary user experience consists of a Slack-style collaboration space and Graph View. Each channel corresponds to one goal; threads organize discussion independently of executable tasks, so a discussion can produce no tasks or several tasks. Necessary configuration belongs under Settings, while everyday work and intervention remain available from the primary views.

## Goal revisions and completion

Updating a goal adds a new revision within the same channel instead of overwriting the objective used by previous work. Tasks, runs, and completion records identify the goal revision they concern. Earlier work and results remain available as history; a delayed completion for an earlier revision cannot complete the current revision.

Any participating agent with completion authority may submit a goal-completion decision. The system accepts it only when common completion conditions hold, including no unresolved required tasks or pending approvals. Human sign-off and agreement from every participating agent are not required. Completion records identify the deciding agent, reason, and supporting results. A human who finds the result insufficient can update the goal and continue the work; agent completion and human satisfaction are distinct concepts.

## Shared history and operation authority

A human participant with channel read access can read the channel's shared history, including material shared before joining. This does not expose an agent's owner-private reference material or grant every operation: posting, execution, control, goal changes, and approval remain subject to separately granted authority. Equivalent operations use the same authorization regardless of whether they originate in collaboration or Graph View.

Ordinary comments are not execution instructions or approvals. The server checks the sender's authority for the requested action; natural-language claims such as "already approved" do not substitute for an authorized approval. This separation does not require human approval for every step of agent work that is already authorized.

## Graph exploration and navigation

Graph View supports relationship exploration and authorized operations through contextual details, not editing execution structure by drawing connections. Navigation to collaboration follows the selected entity: a task or run opens its channel; a reusable entity prefers the current channel when related, otherwise offers the viewer's accessible related channels. An entity with no related channel stays in its details rather than creating a channel automatically.

The graph uses aggregation and progressive exploration: clusters and other groupings provide an overview, users expand the relevant neighborhood, and search locates individual entities. Rendering every registered agent as an individual node at once is not a requirement. Configuration relationships and runtime activity remain distinguishable; authorization boundaries and an accessible list/detail alternative apply to the graph as well.

## Realtime transport

Retain HTTP for submissions and control operations, and authenticated SSE for browser updates. Preserve durable event-cursor replay for reconnection instead of replacing the existing transport with a WebSocket-centered design. A conversational interface does not itself require rebuilding the transport.

## Consequences

These boundaries favor goal-oriented work without treating every conversation as an executable task, and preserve agent autonomy without making channel membership blanket execution authority. Revision-specific work and completion protect the meaning of historical results; separating commands from comments prevents conversation access from becoming implicit operation authority. Aggregation and contextual navigation make relationship exploration the primary graph interaction rather than requiring an unbounded all-entities canvas.
