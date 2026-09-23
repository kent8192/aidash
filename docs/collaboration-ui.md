# Collaboration and graph interface

## Available interface

The dashboard opens into Collaboration. Its channel list uses authorized Workspace records: select a channel to read its shared messages, inspect tasks and artifacts, answer human requests, or open local and remote execution controls. Thread replies use persisted thread-aware history. Creating a prepared channel creates only the Workspace; the separate create-and-run action uses the existing conversation API.

Graph View is a primary destination. Its agent relationship view uses Cytoscape.js with explicit, version-pinned relationships and an accessible relationship table. Select a recorded entity to open its details or choose an accessible related channel. The current channel is preferred among multiple related channels. Operator-only node topology and communication inspection remains available as a graph mode.

Settings contains agent and component registration, generation policies, authorization, semantic memory, transactions, deployment, marketplace, and node connections. Existing top-level management URLs redirect into Settings. Conversation, workspace, task, and overview entry points redirect into Collaboration. These redirects do not create a second primary interface.

The narrow-screen layout provides channel navigation, conversation, task details, and graph inspection without compressing all desktop panels into one screen. Existing server authorization applies to every request; hiding a button is not an authorization boundary.

## Message delivery and updates

Messages are submitted through the generated HTTP API client. A failed submission preserves its text and idempotency key for an unchanged retry. The same key is not reused for changed text. New messages do not force a user reading older messages to scroll to the bottom. A button returns to the latest message.

Authenticated SSE invalidates the affected read queries, with polling as a reconnecting fallback. A failed or unauthorized read hides the stale conversation or detail instead of retaining the last successful payload. Explicitly requested unavailable channels do not silently select another channel. Graph projection does not use unscoped discovery metadata or private reference documents.

The channel API accepts bounded attachment uploads, associates them with a message, returns attachment metadata in history, and requires current message access for downloads. The browser composer does not yet expose attachment upload controls.

## Supported scope and remaining server contracts

This interface is an initial vertical slice, not completion of the collaboration architecture described in `CONTEXT.md` and `docs/adr/`. It adds persisted thread history and channel attachment upload/download APIs while reusing the existing Workspace, task, human-request and run endpoints. The following are not implemented by this interface change:

- Channel memberships; channel/account eligibility lists and generation-policy opt-in; preparation-to-start transition for an existing channel.
- Revisioned goals, automatic message-intent routing and batched agent response evaluation, goal-level completion/block/resumption, and post-completion result discussion.
- Monetary reservations and per-operation Home admission; delegated-history lifecycle and revocation enforcement.
- Authorized server-side graph neighborhood paging, multi-node historical reconstruction, observation-time coverage, Registry pressure admission, and dependency-aware content erasure.
- Channel attachment controls in the browser composer, notification delivery while the browser is closed, and performance acceptance for large federated histories.

The current relationship graph explicitly identifies its node-local State snapshot coverage. It does not display a time selector or imply that current state reconstructs historical state. Task progress is not a goal-completion declaration. The existing create-and-run API is retained for compatibility and does not implement the planned monetary or eligibility contracts; this change must not be advertised as enforcing those new controls.

## Verification

Run `npm run test:graph --prefix web` and `npm run test:collaboration --prefix web` for pure projections and real headless Cytoscape tests. After `npm run build --prefix web`, run `npm run test:ui --prefix web` for the isolated browser suite. It serves the built application and mocks only the API boundary; it does not prove database or worker behavior.

Run `scripts/test-acceptance.sh` for the existing real PostgreSQL, NATS, federation and browser integration checks. Their assertions continue to exercise message failures, scoped access, human answers, registry/marketplace operations and live task assignment through the new navigation. All existing CI gates remain required; the isolated collaboration browser job is an additional gate.
