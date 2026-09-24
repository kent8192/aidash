# Collaboration and graph interface

## Available interface

The dashboard opens into Collaboration. Its channel list uses authorized Workspace records: select a channel to read its shared messages, inspect tasks and artifacts, answer human requests, or open local and remote execution controls. Thread replies use persisted thread-aware history. Creating a prepared channel creates only the Workspace; the separate create-and-run action uses the existing conversation API.

Graph View is a primary destination. Its agent relationship view uses Cytoscape.js with explicit, version-pinned relationships and an accessible relationship table. Select a recorded entity to open its details or choose an accessible related channel. The current channel is preferred among multiple related channels. Operator-only node topology and communication inspection remains available as a graph mode.

Settings contains agent and component registration, generation policies, authorization, semantic memory, transactions, deployment, marketplace, and node connections. Existing top-level management URLs redirect into Settings. Conversation, workspace, task, and overview entry points redirect into Collaboration. These redirects do not create a second primary interface.

The narrow-screen layout provides channel navigation, conversation, task details, and graph inspection without compressing all desktop panels into one screen. Existing server authorization applies to every request; hiding a button is not an authorization boundary.

## Message delivery and updates

Messages are submitted through the generated HTTP API client. A failed submission preserves its text and idempotency key for an unchanged retry. The same key is not reused for changed text. New messages do not force a user reading older messages to scroll to the bottom. A button returns to the latest message.

Authenticated SSE invalidates the affected read queries, with polling as a reconnecting fallback. A failed or unauthorized read hides the stale conversation or detail instead of retaining the last successful payload. Explicitly requested unavailable channels do not silently select another channel. Graph projection does not use unscoped discovery metadata or private reference documents.

The channel API accepts bounded attachment uploads, associates them with a message, returns attachment metadata in history, and requires current message access for downloads. The browser composer uploads up to eight non-empty files of at most 1 MiB each. It retains file upload identities and successful uploads when a later upload or message submission fails. Changed attachment membership creates a new message identity. Attachments can be downloaded with the current tab authority; text and raster image previews are available without rendering uploaded HTML.

## Workspace layout

The workspace uses a compact application rail, channel navigation, conversation area, and contextual right panel. The initial light theme follows the provided workspace reference; an account-menu switch persists a dark theme in this browser. Channel names and goals can be filtered from the top search field (Cmd/Ctrl+K). The account menu retains language, authority selection, and logout controls.

The right panel shows actual task counts, observed execution agents, outstanding requests, and a miniature graph built only from recorded runs and task dependencies. It does not infer membership, presence, or federation health. Opening a thread replaces that panel while keeping the channel conversation and its unsent draft available. On narrow screens, channel navigation, status, and threads open as panels; the composer stays within the viewport.

The Messages, Tasks, Artifacts, and Events tabs retain existing task details, execution controls, and event inspection. Artifacts also lists attachments from the loaded channel history, with older-page loading; attachments posted as replies remain in their respective threads. The thread list likewise describes loaded channel history, rather than claiming to be an exhaustive cross-channel search.

Approval cards send an explicit `{ "approved": true }` or `{ "approved": false }` response through the existing human-request endpoint. Other answers use the existing dialog. The notification menu lists pending requests from the authorized local/remote snapshot. Channel pause/resume affects only currently observed nonterminal runs for that local home workspace, forwards remote controls through their node, refreshes state, and reports partial failures. This is not a persisted goal-level pause or a block on future task creation.

The sidebar's agent-conversation entries link to existing authorized Conversation workspaces. Starting a conversation continues to use the explicit create-and-run form. Task assignment uses the existing Task form; the interface does not present these actions as membership invitations. Composer agent-name insertion only adds text and does not notify or launch an agent. Enter sends, Shift+Enter inserts a newline, and IME composition does not send.

## Supported scope and remaining server contracts

This interface is an initial vertical slice and does not complete the broader collaboration architecture described in the architecture decision records under `docs/adr/`. It uses the existing persisted thread history, channel attachment upload/download, Workspace, task, human-request and run endpoints. The following are not implemented by this interface change:

- Channel memberships; channel/account eligibility lists and generation-policy opt-in; preparation-to-start transition for an existing channel.
- Revisioned goals, automatic message-intent routing and batched agent response evaluation, goal-level completion/block/resumption, and post-completion result discussion.
- Monetary reservations and per-operation Home admission; delegated-history lifecycle and revocation enforcement.
- Authorized server-side graph neighborhood paging, multi-node historical reconstruction, observation-time coverage, Registry pressure admission, and dependency-aware content erasure.
- Notification delivery while the browser is closed and performance acceptance for large federated histories.

The current relationship graph explicitly identifies its node-local State snapshot coverage. It does not display a time selector or imply that current state reconstructs historical state. Task progress is not a goal-completion declaration. The existing create-and-run API is retained for compatibility and does not implement the planned monetary or eligibility contracts; this change must not be advertised as enforcing those new controls.

## Verification

Run `npm run test:graph --prefix web` and `npm run test:collaboration --prefix web` for pure projections and real headless Cytoscape tests. After `npm run build --prefix web`, run `npm run test:ui --prefix web` for the isolated browser suite. It serves the built application and mocks only the API boundary; it does not prove database or worker behavior.

Run `scripts/test-acceptance.sh` for the existing real PostgreSQL, NATS, federation and browser integration checks. Their assertions continue to exercise message failures, scoped access, human answers, registry/marketplace operations and live task assignment through the new navigation. All existing CI gates remain required; the isolated collaboration browser job is an additional gate.
